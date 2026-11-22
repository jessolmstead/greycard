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
//! that the mosaic is demosaiced as in a full develop ([`crate::stages::Facts::bin_under`]), so
//! the 2048 picture is never brought up from fewer pixels. An X-Trans
//! mosaic is binned 3x3 to a third, under the same rule at three times
//! 2048. Then the
//! pipeline as it is on that picture: the learned highlight fill where
//! the edit names one, look-scaled to the bin and on the full mosaic's
//! cells, each binned pixel one Bayer cell (notes §719 §3), and not a
//! skip, so such a picture stands for the edit's; the lens, the defringe, the retouch, the local
//! contrast, the dehaze, the guide plane; then the finish exactly as the
//! open frame's pictures are finished (`edited_pictures::make`: the geometry, a
//! resize to 2048, `export::render_framed` with no output sharpening,
//! sRGB), and the made sizes, kept under the edit's key like the open
//! frame's.
//!
//! Left out, since they are measured on the mosaic's own pixels and mean
//! nothing at half size: the capture sharpen, the learned denoiser and
//! the profiled one. So these pictures are kept under a recipe of their
//! own (`edited_pictures::REDUCED_RECIPE`), and the open frame's develop makes
//! its own in their place when the frame is opened. The grain stays: it
//! is laid on by the finish at 2048, placed by the output's pixels, on
//! both develops alike. Every op measured in pixels is
//! measured on the half-size picture: Texture's, Clarity's, and
//! Structure's radii and the dehaze's window are fractions of the long
//! edge already, the
//! guide plane is made at its fixed 1024 texels, the masks and the
//! vignette are placed by the frame in `render_framed`, the retouch's
//! patches are in units of the picture's width, and the defringe's
//! radius, the one in pixels, is halved. The CA correction runs on the
//! mosaic in full before the bin, where its radius means what it did.
//!
//! A picture made without something its edit asks for is not kept, as
//! the open frame's is not: a Fill patch whose fill is not on disk (its
//! model runs on the open frame alone; one kept at the full size is
//! read and taken down by the bin), a look not in the look directory,
//! and a learned mask whose raster is not in the mask cache. The look and the masks are
//! asked after before anything is decoded. A mask whose model is not in
//! the store waits for a fetch of one; one whose model is there is asked
//! of the worker, at the lowest priority it has (`Job::MasksFor`), and
//! the frame is queued again when every raster is made. A frame tried
//! and found wanting is not tried again under the same key until then,
//! so nothing is decoded over and over for a picture that cannot be
//! kept; until then the frame shows the camera's JPEG.
//!
//! The rasters the worker makes for this go into the mask cache under
//! the reduced base's print (`mask_base`), where this develop reads
//! them: made from the reduced develop's picture, they are a proxy's,
//! and the open frame and an export, whose base is the full develop's,
//! make their own rather than read them.
//!
//! The develop is a walk of the stage list under `Purpose::Reduced`
//! (`stages`), where its bin and its skips are stated, not a second
//! pipeline; the models' picture is the same walk under
//! `Purpose::ModelsSee`, the retouch left out.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use greycard_core::image::WorkingImage;
use greycard_edit::Edit;
use greycard_edit::brush::Raster;

use crate::ai::{Ai, FillReader};
use crate::edited_pictures::{Edited, Shows};
use crate::stages::{Purpose, Stage, Step, stages};
use crate::version_key::Key;
use crate::walk::{Buf, Input, Kept, Models, NoHold, NoModels, plan, walk};

/// How many reduced develops the pool runs at once: one while a frame
/// is open in the editor, whose develop is what the user is waiting on
/// (two in hand cost it about a third again, one about a twentieth);
/// two while culling or browsing on a machine with four or more
/// thumbnail threads, else one. Each holds a decoded frame (a 45 MP
/// mosaic is 180 MB as floats, and its prepared copy as much again)
/// and runs on the [`Reducer::threads`] the develops share.
pub fn at_once(threads: usize, editing: bool) -> usize {
    if editing || threads < 4 { 1 } else { 2 }
}

/// How long the editor is left alone (no develop asked for or under
/// way, so no slider in motion either) before a reduced develop begins
/// while a frame is open in it.
pub const IDLE: std::time::Duration = std::time::Duration::from_millis(1500);

/// How many reduced develops may be in hand now, and when to look again
/// if that is none only for want of quiet: while a frame is open in the
/// editor, none until the window has been idle for [`IDLE`] (`idle`, the
/// time since its last develop was asked for or done), then one;
/// otherwise as [`at_once`] says.
pub fn begin(
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
pub enum Why {
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
pub struct Developed {
    /// The picture after everything up to the finish, at half the
    /// frame's size (or in full for a frame too small to bin).
    pub image: WorkingImage,
    /// The full develop's size, oriented: what the learned masks'
    /// rasters are checked by.
    pub full: (u32, u32),
    pub bin: usize,
    pub clip_level: f32,
    pub guide: Arc<greycard_render::finish::Guide>,
    pub kind: greycard_render::finish::Source,
    pub white: Option<greycard_edit::WhiteShift>,
    /// The masks' bands, of `image`.
    pub detail: Option<Arc<greycard_render::detail::DetailPlanes>>,
    /// The camera's frame on the picture (`Facts::camera`).
    pub camera: Option<greycard_edit::geometry::Crop>,
    /// How long each step took, seconds: the decode, the develop to
    /// the binned picture, the rest to the finish.
    pub decode: f64,
    pub develop: f64,
    pub rest: f64,
}

/// The learned masks' rasters by adjustment id and component.
pub type Rasters = std::collections::HashMap<(u64, usize), Arc<Raster>>;

/// The frame at `path` opened for a walk: a raw decoded whole, or a
/// picture file with its image taken out of its record.
fn open(path: &Path) -> anyhow::Result<Input> {
    if greycard_core::picture::is_picture_path(path) {
        let mut picture = greycard_core::picture::decode_picture_path(path)?;
        let image = std::mem::replace(&mut picture.image, WorkingImage::new(0, 0));
        return Ok(Input::Picture {
            image: Arc::new(image),
            meta: Arc::new(picture),
        });
    }
    // Read whole, never mapped: the thumbnails' pool develops every
    // edited frame of a folder, a share's among them, and a mapped file
    // whose volume goes away is a SIGBUS.
    let frame = greycard_core::decode::decode_path_unmapped(path)?;
    // The decode's camera frame is the last word for the keys
    // (`edited_pictures::note_decoded`).
    crate::edited_pictures::note_decoded(path, frame.shot.camera_frame);
    Ok(Input::Raw(Arc::new(frame)))
}

/// The frame at `path` developed reduced under `edit` at `turn`, up to
/// the finish; the learned masks are the caller's ([`rasters`]).
pub fn develop(
    path: &Path,
    edit: &Edit,
    turn: u8,
    lenses: Option<&greycard_lens::Database>,
) -> Result<Developed, Why> {
    develop_as(path, edit, turn, lenses, &Purpose::Reduced)
}

/// [`develop`], with the fills kept on disk read through `fills`.
pub fn develop_reading(
    path: &Path,
    edit: &Edit,
    turn: u8,
    lenses: Option<&greycard_lens::Database>,
    fills: Option<&FillReader>,
) -> Result<Developed, Why> {
    walk_reduced(path, edit, turn, lenses, &Purpose::Reduced, fills)
}

/// The reduced develop's models: none runs; a fill kept on disk is
/// read where there is a reader.
struct ReadFills<'a>(Option<&'a FillReader>);

impl Models for ReadFills<'_> {
    fn can_denoise(
        &mut self,
        model: &'static greycard_ai::Model,
    ) -> Result<(), crate::walk::NoLearned> {
        NoModels.can_denoise(model)
    }

    fn denoise(
        &mut self,
        model: &'static greycard_ai::Model,
        frame: &greycard_core::raw::RawFrame,
        prepared: &greycard_core::develop::Prepared,
    ) -> Result<Vec<f32>, crate::walk::NoLearned> {
        NoModels.denoise(model, frame, prepared)
    }

    fn fill(
        &mut self,
        patch: &greycard_edit::retouch::Patch,
        region: &greycard_core::develop::retouch::Region,
        picture: &WorkingImage,
    ) -> Option<Vec<f32>> {
        NoModels.fill(patch, region, picture)
    }

    fn fill_read(
        &mut self,
        patch: &greycard_edit::retouch::Patch,
        region: &greycard_core::develop::retouch::Region,
        picture: &WorkingImage,
        factor: usize,
        full: (usize, usize),
        shown: Option<&crate::shown::Shown>,
    ) -> Option<Vec<f32>> {
        self.0?
            .fill_read_only(patch, region, picture, factor, full, shown)
    }

    fn could_fill_more(
        &mut self,
        retouch: &greycard_edit::retouch::Retouch,
        picture: &WorkingImage,
    ) -> bool {
        NoModels.could_fill_more(retouch, picture)
    }

    fn rasters(
        &mut self,
        how: crate::stages::Masks,
        base: &Arc<WorkingImage>,
        patched: Option<crate::walk::Patched<'_>>,
        meta: &crate::walk::Meta,
    ) -> crate::walk::Rasters {
        NoModels.rasters(how, base, patched, meta)
    }
}

/// The frame at `path` walked under `purpose`, the reduced develop or
/// what the models see, on the CPU and from nothing kept: the list's
/// bin and skips are the reduced develop's, and a fill not kept leaves
/// the picture without the edit.
pub fn develop_as(
    path: &Path,
    edit: &Edit,
    turn: u8,
    lenses: Option<&greycard_lens::Database>,
    purpose: &Purpose,
) -> Result<Developed, Why> {
    walk_reduced(path, edit, turn, lenses, purpose, None)
}

/// [`develop_as`], the fills read through `fills`.
fn walk_reduced(
    path: &Path,
    edit: &Edit,
    turn: u8,
    lenses: Option<&greycard_lens::Database>,
    purpose: &Purpose,
    fills: Option<&FillReader>,
) -> Result<Developed, Why> {
    debug_assert!(
        purpose.is_reduced(),
        "the reduced develop walks Reduced or ModelsSee, whose fills are read or left out"
    );
    let started = Instant::now();
    let input = open(path).map_err(|e| Why::Failed(format!("{e:#}")))?;
    let decode = started.elapsed().as_secs_f64();
    let facts = input.facts(turn);
    let mut steps = stages(edit, purpose, &facts, lenses);
    // Nothing is kept for a walk after this one: a kept point holds its
    // picture, and the next stage would copy it rather than change it.
    steps.steps.retain(|s| !matches!(s, Step::Keep(_)));
    let plan = plan(&steps, purpose, false);
    let walked = walk(
        &steps,
        &plan,
        &input,
        &mut Kept::new(),
        &mut ReadFills(fills),
        &mut None,
        &NoHold,
    )
    .map_err(|e| Why::Failed(e.to_string()))?;
    if !walked.without.is_empty() {
        return Err(Why::Without(walked.without));
    }
    // The develop to the binned picture, and the rest after it, for
    // the log line.
    let to_base = steps
        .ops()
        .take_while(|s| {
            matches!(
                s,
                Stage::Prepare(_)
                    | Stage::Ca(_)
                    | Stage::Bin(_)
                    | Stage::Demosaic(..)
                    | Stage::Denoise(_)
                    | Stage::HighlightFill(_)
                    | Stage::Warp(_)
                    | Stage::IntoWorking { .. }
                    | Stage::White { .. }
                    | Stage::Orient(_)
                    | Stage::Reduce(_)
            )
        })
        .count();
    let develop = walked.seconds.iter().take(to_base).map(|(_, s)| s).sum();
    let rest = walked.seconds.iter().skip(to_base).map(|(_, s)| s).sum();
    let white = walked.meta.white_shift();
    // Let go of every other hold on the picture, so it is not copied.
    drop(input);
    let mut walked = walked;
    walked.taps.masks = None;
    let Buf::Working(image) = walked.buf else {
        unreachable!("a walk on the CPU ends at a picture in memory")
    };
    Ok(Developed {
        image: Arc::try_unwrap(image).unwrap_or_else(|a| (*a).clone()),
        full: walked.meta.full,
        bin: walked.meta.bin,
        clip_level: walked.meta.clip_level,
        guide: walked
            .taps
            .guide_for_finish()
            .unwrap_or_else(|| Arc::new(greycard_render::finish::Guide::NONE)),
        kind: walked.meta.source,
        white,
        detail: walked.taps.planes,
        camera: facts.camera(),
        decode,
        develop,
        rest,
    })
}

/// Whether the look the edit names is in the look directory: asked
/// before anything is decoded, since a picture without it is not kept.
pub fn look_ready(edit: &Edit) -> Result<(), Why> {
    if let greycard_edit::look::LutChoice::Named(name) = &edit.look_lut.lut
        && look_missing(edit, &greycard_edit_io::Stores::user())
    {
        return Err(Why::Without(vec![format!("the look {name}")]));
    }
    Ok(())
}

/// Whether the look the edit names is wanted and cannot be had from
/// `stores`: no table for the picture's display curve, or not in the
/// directory at all. Under a camera profile's own tone curve
/// (`ProfileTone::of`) a look that is there is never missing: a table
/// for the curve is laid on or set aside by the render, and a look with
/// none has only tables made for other curves of ours, which the
/// profile's curve sets aside too (`look::over_profile_curve`). Only a
/// look not in the directory is missing then.
pub(crate) fn look_missing(edit: &Edit, stores: &greycard_edit_io::Stores) -> bool {
    if !matches!(edit.look_lut.lut, greycard_edit::look::LutChoice::Named(_))
        || edit.look_lut.strength <= 0.0
        || edit
            .look_lut
            .look_under(stores, edit.display_curve)
            .is_some()
    {
        return false;
    }
    let there = || {
        stores
            .looks
            .as_deref()
            .is_some_and(|dir| greycard_edit_io::looks::is_there_in(dir, &edit.look_lut))
    };
    !(greycard_render::profile_tone::ProfileTone::of(edit, stores).is_some() && there())
}

/// Whether every learned mask `edit` draws on the frame at `path` can be
/// had, asked before anything is decoded: `Why::NoModel` names the ones
/// whose model is not in the store, `Why::Masks` the ones the worker is
/// to make first.
pub fn masks_ready(ai: &mut Ai, path: &Path, edit: &Edit, turn: u8) -> Result<(), Why> {
    if !draws_learned(edit) {
        return Ok(());
    }
    let mut to_make = Vec::new();
    let mut no_model = Vec::new();
    // The frame's size as its file says it stands, read only for a Sky,
    // the one shape whose raster's width follows it: a frame the rule
    // gives the wide raster is not ready on its old narrow one.
    let mut frame = None;
    let print = crate::mask_base::print(Some(path), edit, turn, crate::mask_base::Size::Reduced);
    for a in &edit.adjustments {
        if !a.enabled || a.mask.is_empty() {
            continue;
        }
        for (_, c) in a.mask.live() {
            if !c.shape.is_learned() || !crate::ai::prompted(&c.shape) {
                continue;
            }
            let size = match &c.shape {
                greycard_edit::mask::Shape::Sky { .. } => *frame.get_or_insert_with(|| {
                    greycard_core::decode::stance_path_unmapped(path)
                        .ok()
                        .and_then(|s| s.shown_size(turn))
                        .map(|(w, h)| (w as usize, h as usize))
                }),
                _ => None,
            };
            match ai.raster_ready(path, &c.shape, turn, size, print) {
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
pub fn rasters(
    ai: &mut Ai,
    path: &Path,
    edit: &Edit,
    turn: u8,
    full: (u32, u32),
) -> Result<Rasters, Why> {
    let mut rasters = Rasters::new();
    if !draws_learned(edit) {
        return Ok(rasters);
    }
    let mut missing = Vec::new();
    let print = crate::mask_base::print(Some(path), edit, turn, crate::mask_base::Size::Reduced);
    for a in &edit.adjustments {
        if !a.enabled || a.mask.is_empty() {
            continue;
        }
        for (i, c) in a.mask.live() {
            if !c.shape.is_learned() || !crate::ai::prompted(&c.shape) {
                continue;
            }
            match ai.cached_raster(path, &c.shape, turn, full, print) {
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
pub fn draws_learned(edit: &Edit) -> bool {
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
pub struct Reducer {
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
    pub fn threads() -> usize {
        std::env::var("GREYCARD_REDUCED_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or_else(|| {
                (std::thread::available_parallelism().map_or(2, |n| n.get()) / 4).clamp(2, 8)
            })
    }

    // Not `Default`: its models read the store, which a default should
    // not go to disk for.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self::with_ai(Ai::new())
    }

    /// A reducer reading the mask cache and the store through `ai`.
    pub fn with_ai(ai: Ai) -> Self {
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
    pub fn wants(&self, edited: &Edited, frame: &Key) -> bool {
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
    pub fn lenses(&self) -> Option<Arc<greycard_lens::Database>> {
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
    pub fn set_lenses(&self, lenses: Option<Arc<greycard_lens::Database>>) {
        *self.lenses.lock().expect("reduced lenses") = Some(lenses);
    }

    /// The roots on a network mount, as the window has them now.
    pub fn set_network(&self, roots: Vec<PathBuf>) {
        *self.network.lock().expect("network roots") = roots;
    }

    /// Whether `frame` is developed alone: its file is under a root on
    /// a network mount.
    pub fn alone(&self, frame: &Key) -> bool {
        self.network
            .lock()
            .expect("network roots")
            .iter()
            .any(|root| frame.path.starts_with(root))
    }

    /// How many reduced develops were begun.
    #[cfg(any(test, feature = "testing"))]
    pub fn develops(&self) -> usize {
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
    pub fn masks_done(&self, frame: &Key, all: bool) {
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
    pub fn models_arrived(&self) -> Vec<Key> {
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
    pub fn make(&self, edited: &Edited, frame: &Key) -> Result<bool, Why> {
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
        let (mut sidecar, trouble) = greycard_edit_io::on_disk::read_sidecar_said(path);
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
        let key = edited.key_of(path, &line.current);
        let mut moved = false;
        if key != held {
            if !crate::edited_pictures::is_edited(path, &line.current, iso) {
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
        let Some(marked) = edited.mark(&hash, key, crate::edited_pictures::REDUCED_RECIPE) else {
            // Being made by another of these threads (another link to
            // the same content).
            return Err(Why::NotWanted);
        };
        let lenses = self.lenses();
        let fills = self.ai.lock().expect("reduced ai").fill_reader(path);
        let started = Instant::now();
        self.develops.fetch_add(1, Ordering::SeqCst);
        let developed = self
            .pool()
            .install(|| develop_reading(path, &edit, turn, lenses.as_deref(), fills.as_ref()))?;
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
        let finish = crate::edited_pictures::Finish {
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
            camera: developed.camera,
        };
        let made = self
            .pool()
            .install(|| crate::edited_pictures::make(&developed.image, &finish))
            .map_err(|e| Why::Failed(e.to_string()))?;
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
    /// A look whose tables are all made for another display curve of
    /// ours is set aside under a camera profile's own tone curve, not
    /// missing: the export does not report it and the reduced develop
    /// keeps its pictures. Without the switch it is missing, and a look
    /// not in the directory is missing either way.
    #[test]
    fn a_look_set_aside_under_the_profiles_curve_is_not_missing() {
        use greycard_edit::camera::ProfileChoice;
        use greycard_edit::look::{LookLut, LutChoice};
        let dir = std::env::temp_dir().join(format!(
            "greycard-look-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (profiles, looks) = (dir.join("profiles"), dir.join("looks"));
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::create_dir_all(&looks).unwrap();
        std::fs::write(
            profiles.join("Curved.dcp"),
            greycard_core::testing::dcp::synthetic(),
        )
        .unwrap();
        // Fitted under AgX only: no table for per channel.
        std::fs::write(
            looks.join("Fitted.agx.cube"),
            "# display_curve: agx\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
        )
        .unwrap();
        let stores = greycard_edit_io::Stores {
            profiles: Some(profiles),
            looks: Some(looks),
        };
        let mut edit = Edit::default();
        edit.camera.choose(ProfileChoice::Named("Curved".into()));
        edit.look_lut = LookLut {
            lut: LutChoice::Named("Fitted".into()),
            strength: 1.0,
        };
        assert!(look_missing(&edit, &stores), "no table for per channel");
        edit.camera.tone_curve = true;
        assert!(!look_missing(&edit, &stores), "set aside");
        edit.look_lut.lut = LutChoice::Named("Gone".into());
        assert!(look_missing(&edit, &stores), "not in the directory");
        greycard_core::testing::remove_dir_retry(&dir);
    }

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

    /// The kept pictures of an edited frame are finished at its body's
    /// baseline, as the open frame and its export are.
    #[test]
    fn the_reduced_develop_carries_the_body_s_baseline() {
        let dir = scratch("baseline");
        let at = |name: &str, (make, model): (&str, &str)| {
            let path = dir.join(name);
            crate::testing::linear_dng(&path, 96, 64, make, model);
            develop(&path, &Edit::default(), 0, None)
                .expect("it develops")
                .kind
        };
        let b = greycard_edit::baseline::for_body(Some("Fujifilm GFX 100S II"));
        assert_eq!(
            at("gfx.dng", crate::testing::GFX),
            greycard_render::finish::Source::Scene(greycard_render::finish::Baseline::of_stops(b))
        );
        assert_eq!(
            at("r5.dng", ("Canon", "Canon EOS R5 Mark II")),
            greycard_render::finish::Source::SCENE
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// A Fill patch refuses the reduced develop, whose fills its model
    /// does not make, and the picture is not kept; what the models see
    /// leaves the retouch out, so the same edit develops for them.
    #[test]
    fn a_fill_refuses_the_reduced_develop_but_not_the_models_picture() {
        let dir = scratch("fill-refused");
        let path = dir.join("frame.dng");
        crate::testing::linear_dng(&path, 96, 64, "Canon", "Canon EOS R5");
        let mut edit = Edit::default();
        edit.retouch.patches.push(greycard_edit::retouch::Patch {
            id: 1,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[0.5, 0.3]],
            radius: 0.05,
            ..Default::default()
        });
        let reduced = develop_as(&path, &edit, 0, None, &crate::stages::Purpose::Reduced);
        assert!(
            matches!(reduced, Err(Why::Without(ref w)) if w.len() == 1),
            "{:?}",
            reduced.err()
        );
        assert!(develop_as(&path, &edit, 0, None, &crate::stages::Purpose::ModelsSee).is_ok());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A fill the full develop kept on disk is read by the reduced
    /// develop, and its picture is the edit's; before it was kept, the
    /// develop is without it.
    #[test]
    fn the_reduced_develop_reads_a_fill_kept() {
        struct Making<'a>(&'a mut Ai);
        impl Models for Making<'_> {
            fn can_denoise(
                &mut self,
                m: &'static greycard_ai::Model,
            ) -> Result<(), crate::walk::NoLearned> {
                NoModels.can_denoise(m)
            }
            fn denoise(
                &mut self,
                m: &'static greycard_ai::Model,
                f: &greycard_core::raw::RawFrame,
                p: &greycard_core::develop::Prepared,
            ) -> Result<Vec<f32>, crate::walk::NoLearned> {
                NoModels.denoise(m, f, p)
            }
            fn fill(
                &mut self,
                patch: &greycard_edit::retouch::Patch,
                region: &greycard_core::develop::retouch::Region,
                picture: &WorkingImage,
            ) -> Option<Vec<f32>> {
                self.0.fill(patch, region, picture).ok().map(|(d, _)| d)
            }
            fn could_fill_more(
                &mut self,
                _: &greycard_edit::retouch::Retouch,
                _: &WorkingImage,
            ) -> bool {
                false
            }
            fn rasters(
                &mut self,
                how: crate::stages::Masks,
                base: &Arc<WorkingImage>,
                patched: Option<crate::walk::Patched<'_>>,
                meta: &crate::walk::Meta,
            ) -> crate::walk::Rasters {
                NoModels.rasters(how, base, patched, meta)
            }
        }
        let dir = scratch("fill-read");
        let path = dir.join("frame.dng");
        crate::testing::linear_dng(&path, 96, 64, "Canon", "Canon EOS R5");
        let mut edit = Edit::default();
        edit.retouch.patches.push(greycard_edit::retouch::Patch {
            id: 1,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[0.5, 0.3]],
            radius: 0.05,
            ..Default::default()
        });
        let mut ai = Ai::for_test_on_disk(greycard_ai::Store::at(dir.join("models")));
        ai.fill_by_test = Some(0.6);
        ai.forget(Some(path.clone()));
        let reader = ai.fill_reader(&path).expect("a reader on disk");
        assert!(matches!(
            develop_reading(&path, &edit, 0, None, Some(&reader)),
            Err(Why::Without(_))
        ));
        // The full develop makes the fill and keeps it.
        let input = open(&path).unwrap();
        let purpose = Purpose::Export(Box::default());
        let steps = stages(&edit, &purpose, &input.facts(0), None);
        let plan = plan(&steps, &purpose, false);
        let full = walk(
            &steps,
            &plan,
            &input,
            &mut Kept::new(),
            &mut Making(&mut ai),
            &mut None,
            &NoHold,
        )
        .expect("the full develop");
        assert!(full.without.is_empty());
        let read = develop_reading(&path, &edit, 0, None, Some(&reader)).expect("the fill read");
        // The hole is the fill's, the patch's point (in units of the
        // width) at its center.
        let w = read.image.width;
        let at = ((w * 3 / 10) * w + w / 2) * 3;
        assert!(
            (read.image.data[at] - 0.6).abs() < 0.05,
            "{}",
            read.image.data[at]
        );
        // With no reader, as before.
        assert!(develop(&path, &edit, 0, None).is_err());
        crate::testing::remove_dir_retry(&dir);
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
        let key = edited.key_of(&version.path, &warm);
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

    /// The pool's develop of an edited raw, and the Sky's read of the
    /// frame's size beside it, map no raw: the pool works through every
    /// edited frame of a folder while the window is idle, a share's
    /// among them, and a mapped raw whose volume drops while the
    /// machine sleeps is a SIGBUS nothing catches. The test DNGs carry
    /// no color matrix, so the develop fails after the decode has read
    /// the file; with no Sky model in the store, the Sky's frame stops
    /// once its size is read.
    #[test]
    fn an_edited_raw_is_developed_without_a_map() {
        let dir = scratch("unmapped");
        let raws = dir.join("raws");
        std::fs::create_dir_all(&raws).unwrap();
        let mut plain = Edit::default();
        plain.light.exposure = 1.0;
        let mut sky = plain.clone();
        sky.adjustments = with_sky().adjustments;
        let cache: crate::worker::ThumbCache =
            Arc::new(Mutex::new(Some(greycard_library::thumbs::Thumbs::at(
                dir.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            ))));
        let edited = Arc::new(Edited::new(cache));
        let mut frames = Vec::new();
        for (name, edit) in [("plain.dng", &plain), ("sky.DNG", &sky)] {
            let path = raws.join(name);
            greycard_library::fixture::write_mosaic_dng(&path, "RGGB");
            greycard_edit::Sidecar {
                line: greycard_edit::Line::of(edit.clone()),
                ..Default::default()
            }
            .save(&path)
            .unwrap();
            frames.push(Key::file(&path));
        }
        edited.note_all(frames.iter().map(|f| (f.clone(), true, None, None)));
        greycard_core::decode::watch_maps_under(&raws);
        // A store of its own, empty, so a Sky model fetched on this
        // machine does not turn the Sky's frame into one asking for
        // masks.
        let reducer = Reducer::with_ai(Ai::for_test_on_disk(greycard_ai::Store::at(
            dir.join("models"),
        )));
        for frame in &frames {
            let made = reducer.make(&edited, frame);
            assert!(
                matches!(made, Err(Why::Failed(_)) | Err(Why::NoModel(_))),
                "{made:?}"
            );
        }
        // Each read its file: the decode, and the Sky's size.
        assert_eq!(greycard_core::decode::maps_under(&raws), 0, "raws mapped");
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
                crate::edited_pictures::BIG,
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
        let key = edited.key_of(&frame.path, &edit);
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
                crate::mask_base::print(
                    Some(&frame.path),
                    &edit,
                    0,
                    crate::mask_base::Size::Reduced,
                ),
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
        assert!(!edited.all_kept_by(&hash, key, crate::edited_pictures::EDITED_RECIPE));
        assert!(!reducer.wants(&edited, &frame), "kept");
        crate::testing::remove_dir_retry(&dir);
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
        let wiped = greycard_edit_io::key::develop_key(&edit, Some("a lens database since wiped"));
        let (frame, edited) = frame(&dir, "stale.png", &edit, Some(wiped));
        let key = edited.key_of(&frame.path, &edit);
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
        let now = edited.key_of(&frame.path, &refit);
        edited.settle_key(&frame, &hash, 77, None);
        assert!(matches!(reducer.make(&edited, &frame), Ok(true)));
        assert_eq!(edited.shows(&frame), Shows::Edit(now));
        assert!(edited.all_kept(&hash, now));
        assert_eq!(reducer.develops(), 2);
        crate::testing::remove_dir_retry(&dir);
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
        let fitted = edited.key_of(&frame.path, &edit);
        assert!(matches!(reducer.make(&edited, &frame), Ok(true)));
        assert!(edited.all_kept(&hash, fitted));
        assert!(!edited.files_moved(), "nothing moved");
        assert!(!reducer.wants(&edited, &frame));
        // The refit: other contents, another length.
        std::fs::write(&table, "LUT_3D_SIZE 2\n0 0 0.5\n").unwrap();
        let refit = edited.key_of(&frame.path, &edit);
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
        crate::testing::remove_dir_retry(&dir);
    }
}
