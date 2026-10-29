//! The engine on its own thread: decodes and develops, one job at a
//! time, the newest develop winning. Thumbnails are made beside it,
//! on a pool of their own (`thumbpool`).

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use greycard_core::CameraProfile;
use greycard_core::color::{as_shot_temp_tint, profile_from_frame};
use greycard_core::develop::ca::{CaOptions, CaStats, correct_ca};
use greycard_core::develop::dehaze::{self, DehazeStats, Reduced};
use greycard_core::develop::lens::LensCorrection;
use greycard_core::develop::local_contrast::{
    LocalContrastOptions, LocalContrastStats, local_contrast,
};
use greycard_core::develop::sharpen::{SharpenStats, sharpen_with_mask};
use greycard_core::develop::{
    CaCorrector, DemosaicMethod, DevelopSettings, demosaic_prepared, develop, finish, prepare_with,
};
use greycard_core::image::WorkingImage;
use greycard_core::picture::{Picture, decode_picture_path, is_picture_path};
use greycard_core::raw::CfaPattern;
use greycard_core::raw::{Shot, ShotSummary};
use greycard_library::thumbs::{Tag, Thumb, Thumbs};

mod base;
mod decode;
mod develop;
mod engine;
mod export;
mod handle;
mod keep;
mod masks;
mod prefetch;
#[cfg(test)]
mod samples;
mod shelf;
#[cfg(test)]
mod tests;
mod thumbs;

#[cfg(test)]
use self::{base::blend, decode::open};
use self::{
    base::{
        Stages, engine_base, engine_base_or_stop, learned_base, timed, turn_base, turns_exactly,
    },
    decode::{lens_corrections, lens_report, open_told, timed_open},
    develop::develop_job,
    engine::{left_out_of, run},
    export::{export_other, finish_export, open_picture, scene_export, seeded, write_export},
    keep::keep_open,
    masks::{MasksMade, learned_masks, masks_for, person_on},
    thumbs::{cached_thumbnail_noting, thumb_lookup},
};

#[cfg(test)]
pub(crate) use self::prefetch::{behind_prefetch, claim};
pub use self::thumbs::{THUMB_RECIPE, THUMB_WIDTH, count_thumb_cache};
pub(crate) use self::{
    decode::correct_lens_of,
    handle::order_thumbnails,
    prefetch::{Neighbor, Side},
    shelf::FileNow,
    thumbs::{file_stat, thumb_key, thumb_tag, thumbnail},
};

/// A developed picture for the viewport: half floats for it to
/// upload, or a texture the engine's GPU op already left on the
/// device, the sharpen's mask in its alpha either way.
pub enum Developed {
    Halves(Arc<Halves>),
    Texture(greycard_gpu::wgpu::Texture),
}

impl Developed {
    pub fn width(&self) -> u32 {
        match self {
            Developed::Halves(h) => h.width,
            Developed::Texture(t) => t.width(),
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            Developed::Halves(h) => h.height,
            Developed::Texture(t) => t.height(),
        }
    }
}

/// A developed image as the GPU takes it: RGBA half floats, alpha one.
pub struct Halves {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<half::f16>,
}

impl Halves {
    /// The picture, with `mask` in the alpha when there is one: the
    /// sharpen's blend, for the viewport to paint where it acted.
    /// Zero alpha otherwise, so nothing is painted.
    pub(crate) fn from_image(image: &WorkingImage, mask: Option<&[f32]>) -> Self {
        use rayon::prelude::*;
        // A row at a time, in parallel: a 24 MP frame is a hundred
        // million conversions.
        let (w, h) = (image.width, image.height);
        let mut pixels = vec![half::f16::ZERO; w * h * 4];
        pixels
            .par_chunks_mut(w * 4)
            .zip(image.data.par_chunks(w * 3))
            .enumerate()
            .for_each(|(y, (out, row))| {
                let mask = mask.map(|m| &m[y * w..(y + 1) * w]);
                let (out, _) = out.as_chunks_mut::<4>();
                let (row, _) = row.as_chunks::<3>();
                for (x, (o, px)) in out.iter_mut().zip(row).enumerate() {
                    o[0] = half::f16::from_f32(px[0]);
                    o[1] = half::f16::from_f32(px[1]);
                    o[2] = half::f16::from_f32(px[2]);
                    o[3] = half::f16::from_f32(mask.map_or(0.0, |m| m[x]));
                }
            });
        Self {
            width: w as u32,
            height: h as u32,
            pixels,
        }
    }
}
use greycard_core::RgbSpace;
use greycard_core::raw::RawFrame;
use greycard_edit::brush::Raster;
use greycard_edit::mask::Shape;
use greycard_edit::retouch::Retouch;
use greycard_edit::{Edit, Noise};

use crate::ai::{Ai, Key, NoDenoiser, NoFill};
use crate::versions::Key as Frame;

pub enum Job {
    Open {
        path: PathBuf,
        /// The version of the file whose line `edit` is, none for the
        /// file's own: the frame the editor has open, which the pool
        /// leaves to the worker's own develop.
        version: Option<std::sync::Arc<str>>,
        edit: Edit,
        generation: u64,
        /// A raw with no sidecar yet: its learned-denoiser blend is
        /// seeded from the frame's ISO here, where the frame is
        /// decoded anyway, and reported back in `Opened`.
        seed_blend: bool,
        /// The frame's quarter turns clockwise on top of the camera's
        /// orientation tag (`Sidecar::turn`). The worker keeps it
        /// with the open file, so an export does not have to carry
        /// it as well.
        turn: u8,
        /// The window knows the file's mosaic already (its library row,
        /// an open before): no stance is read ahead of the decode.
        stance_known: bool,
    },
    Develop {
        edit: Edit,
        generation: u64,
        turn: u8,
    },
    /// The picture of a frame (a file, and the version of it) for the
    /// list's cell `index`: the picture kept of its edit when one is,
    /// else the camera's, made and kept if need be.
    Thumbnail { index: usize, frame: Frame },
    /// A frame's thumbnail from the cache alone, under the key its row
    /// in the index gives (the content hash and the mtime stamp): for
    /// a frame whose file cannot be read, its root being offline. A
    /// miss is a `NoThumbnail`, and nothing is made.
    CachedThumbnail {
        index: usize,
        frame: Frame,
        hash: String,
        stamp: u64,
    },
    /// Write a set of frames, each under its own edit and all under
    /// the set's settings: queued as one export a frame, so a develop
    /// asked for meanwhile goes first between any two of them.
    ExportSet {
        set: Arc<crate::queue::Set>,
        frames: Vec<crate::queue::Frame>,
    },
    /// Frame `index` of a set, as the queue holds it.
    ExportFrame {
        set: Arc<crate::queue::Set>,
        index: usize,
        frame: crate::queue::Frame,
    },
    /// A learned mask's raster for `shape`, the component `key`. For a
    /// shape whose model can be more than one file (Subject), `model`
    /// is the one the panel's `step` already chose — with the
    /// session's declines and the providers record folded in, which
    /// the worker has no way to re-derive on its own; `None` for a
    /// shape whose model is not in question.
    Mask {
        key: Key,
        shape: Shape,
        model: Option<&'static greycard_ai::Model>,
    },
    /// The people on the open picture, as the People model finds them,
    /// for a Part chosen to be picked among: answered with `People`.
    People,
    /// What the Landscape button offers on the open picture, from the
    /// sky's model run once on it: answered with `Landscape`.
    Landscape,
    /// `frame` was saved under `edit` at `turn`: when it is the open
    /// frame and its develop is in hand, the edit's pictures are made
    /// from it and kept (`edited`); when the develop is still to land,
    /// they are made when it does, or when the frame is left with it in
    /// hand. Answered with `EditedKept` once the frame's pictures may
    /// have changed.
    Keep {
        frame: Frame,
        edit: Edit,
        turn: u8,
        /// The frame's ISO from its row, which the learned blend is
        /// judged against in saying whether this is an edit at all.
        iso: crate::edited::Iso,
    },
    /// Make the learned masks' rasters of a frame under its line's edit
    /// and put them in the mask cache, so the reduced develop of its
    /// pictures (`reduced`) can read them: the lowest thing the worker
    /// does, taken only when nothing else is queued. Answered with
    /// `EditedKept` so the frame is asked for again.
    MasksFor(Frame),
    /// A person picked on one frame looked for on another the same
    /// People shape reached ("use this person on all"): answered with
    /// `PersonOn`. One a frame, so a develop asked for goes first
    /// between any two.
    PersonOn(Box<PersonJob>),
    /// Fetch a model into the store; on its own thread.
    Fetch { model: &'static greycard_ai::Model },
    /// Fetch Microsoft's CPU-only ONNX Runtime into the store, and load
    /// it; on its own thread.
    FetchRuntime {
        runtime: &'static greycard_ai::Runtime,
    },
    /// Fetch the lens database into its store; on its own thread.
    FetchLenses,
    /// The worker's own: a device arrived and nothing else is queued.
    Gpu,
}

/// A frame to look for a person on ([`Job::PersonOn`]).
#[derive(Debug, Clone)]
pub struct PersonJob {
    /// Which run of "use this person on all" it is of.
    pub batch: u64,
    pub frame: Frame,
    /// The frame's line's edit, and its quarter turns.
    pub edit: Edit,
    pub turn: u8,
    /// The person the shape came to the frame of, which is left where
    /// it is decided alone; none for everyone's.
    pub carried: Option<greycard_edit::mask::Person>,
    /// The person picked on the other frame, and how.
    pub wanted: crate::ai::Picked,
}

/// What the learned denoiser did for a develop.
#[derive(Debug, Clone)]
pub enum LearnedReport {
    /// The edit did not ask for it.
    Off,
    Ran {
        version: String,
        provider: &'static str,
        seconds: f64,
    },
    /// Its answer read back from the disk cache.
    Cached { seconds: f64 },
    /// Its answer from an earlier develop served, blended anew.
    Kept,
    /// The tier is not in the store; the engine's own path stood in.
    Missing(&'static greycard_ai::Model),
    /// It could not run; the engine's own path stood in.
    Failed(String),
    /// The learned denoiser does not take this frame's mosaic (X-Trans):
    /// the edit asked for it, and the develop ran without it. Says why.
    Unavailable(&'static str),
}

/// Why the learned denoiser does not run on `frame`, when the edit asks
/// for it: its network takes a packed 2x2 Bayer mosaic, and has no
/// X-Trans version. Read from the layout before anything is prepared
/// or loaded. The profiled denoise runs on X-Trans as on Bayer.
fn denoise_unavailable(frame: &RawFrame, learned: bool) -> Option<&'static str> {
    let xtrans = matches!(&frame.layout, greycard_core::raw::SensorLayout::Cfa(p) if p.is_xtrans());
    (xtrans && learned).then_some("the learned denoiser has no X-Trans version yet")
}

/// What the fill model did for a develop's Fill patches, by name.
/// Nothing at all when every fill was kept from an earlier develop.
#[derive(Debug, Clone, Default)]
pub struct FillReport {
    /// Patches made up this develop, and the seconds they took together.
    pub made: Vec<String>,
    pub seconds: f64,
    /// Patches left as they were, and the model each waits on: it is
    /// not in the store.
    pub missing: Vec<(String, &'static greycard_ai::Model)>,
    /// Patches the generative remove made, left as they were: this
    /// machine has no WebGPU.
    pub no_gpu: Vec<String>,
    /// The engine each Fill patch applied this develop was made by,
    /// kept or made, by patch id: none when the retouch was kept whole.
    pub engines: Vec<(u64, crate::ai::Engine)>,
    /// Patches left as they were, and why: the model could not run.
    pub failed: Vec<(String, String)>,
}

/// The white balance a develop used: camera gains and the camera to
/// working matrix, rows; and the working-space value its channels
/// clip at, for the auto white balance to leave clipped pixels out.
#[derive(Debug, Clone, Copy)]
pub struct WhiteBase {
    pub gains: [f32; 3],
    pub matrix: [[f32; 3]; 3],
    pub clip: f32,
    /// The same white as a temperature and tint, what a mask's offset
    /// is from; none for a picture already in the working space.
    pub temp_tint: Option<greycard_core::TempTint>,
}

impl WhiteBase {
    /// No gains and no matrix: a picture already in the working space.
    pub const IDENTITY: Self = Self {
        gains: [1.0; 3],
        matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        clip: f32::INFINITY,
        temp_tint: None,
    };

    /// The white balance `wb`, with channels clipping at `clip`.
    pub fn from(wb: &greycard_core::WhiteBalance, clip: f32) -> Self {
        let cols = wb.matrix_f32();
        Self {
            gains: wb.coefficients_f32(),
            matrix: std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r])),
            clip,
            temp_tint: Some(wb.temp_tint),
        }
    }
}

/// What the lens database made of a file.
#[derive(Debug, Clone, PartialEq)]
pub enum LensReport {
    /// No database on this machine.
    NoDatabase,
    /// The file names no lens.
    NoLensName,
    /// The lens is not in the database.
    Unknown(String),
    /// The lens found, whether the body was, and whether the profile
    /// was measured on a smaller sensor than the body's.
    Found {
        lens: String,
        camera: bool,
        smaller_sensor: bool,
    },
}

/// What kind of file is open.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceKind {
    Raw,
    /// A picture that is not a raw: what its samples were taken to be
    /// in, and their depth.
    Picture {
        space: String,
        bits: u8,
    },
}

// `Developed` carries the develop's picture, its guide plane and every
// stat the panel shows, 400 bytes against the next variant's 160; an
// outcome is made a few times a second at most, so the size is not
// worth an indirection.
#[allow(clippy::large_enum_variant)]
pub enum Outcome {
    Opened {
        generation: u64,
        /// The frame's own white balance as temperature and tint.
        as_shot: (f64, f64),
        /// The frame and its profile, for the white balance preview;
        /// neither for a picture that is not a raw.
        frame: Option<Arc<RawFrame>>,
        profile: Option<Box<CameraProfile>>,
        /// What the lens database made of it.
        lens: LensReport,
        kind: SourceKind,
        /// What the file says the frame was shot at, for the panel.
        shot: ShotSummary,
        /// When the camera's clock says it was taken, the index's form
        /// (`greycard_library::normalize_taken`): the panel's capture
        /// time line for a frame the library index does not hold.
        taken: Option<String>,
        /// The learned-denoiser blend seeded from the ISO, when the
        /// job asked for one and the file was a raw.
        blend: Option<f32>,
    },
    Developed {
        generation: u64,
        /// The frame's quarter turns it was developed at, which a
        /// turn pressed since has moved on from until its own lands.
        turn: u8,
        image: Developed,
        /// What the picture is, a raw's scene or a picture already
        /// rendered, which says how the viewport finishes it: the
        /// panel's open frame may be another kind by the time this
        /// is drawn, and the picture keeps its own.
        kind: crate::finish::Source,
        /// The tone equalizer's plane for this develop, for the
        /// viewport's own texture.
        guide: Arc<crate::finish::Guide>,
        /// The bands the masks' Detail is put on from, when a mask
        /// asks for one ([`crate::detail::DetailPlanes`]).
        planes: Option<Arc<crate::detail::DetailPlanes>>,
        white: WhiteBase,
        seconds: f64,
        /// What the local contrast did, when it ran, and where: on
        /// the CPU in so many seconds, on the GPU, or kept from the
        /// last develop.
        detail: Option<(LocalContrastStats, DetailRan)>,
        /// What the sharpen did, when it ran.
        sharpen: Option<SharpenStats>,
        /// What the dehaze did, when it ran.
        dehaze: Option<DehazeStats>,
        /// The patches that had no source and were given one here,
        /// each whole, as developed, with the source the engine chose.
        sources: Vec<greycard_edit::retouch::Patch>,
        /// What the learned denoiser did.
        learned: LearnedReport,
        /// What the fill model did.
        fills: FillReport,
    },
    /// The kind of mosaic of a raw being opened (`None`: it is not a
    /// mosaic), read beside its decode so the demosaic menu is the
    /// file's before its pixels are.
    Mosaic {
        path: PathBuf,
        mosaic: Option<greycard_core::raw::Mosaic>,
    },
    /// The fill model is at work on the patch named, for a develop
    /// still on its way.
    Filling {
        generation: u64,
        name: String,
    },
    /// A frame's thumbnail, with the frame it was made for: the list
    /// may have changed under it, so the index alone is not enough.
    /// `size` is the long edge it was asked for, which the grid uses
    /// to tell a picture it has outgrown from one it has not.
    /// `cached` when it came from the thumbnail cache, and `seconds`
    /// what the worker spent on it, for the folder's account in the
    /// log.
    Thumbnail {
        index: usize,
        frame: Frame,
        size: u32,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        cached: bool,
        seconds: f64,
    },
    /// A frame no thumbnail could be made of, so the folder's count
    /// still closes.
    NoThumbnail {
        index: usize,
        frame: Frame,
    },
    /// Thumbnails and `NoThumbnail`s waiting in the pool's outbox, for
    /// a worker made with [`Worker::batched`]: one of these for as
    /// many as the threads finish before the window takes them in.
    Thumbnails(crate::thumbpool::Batch),
    Failed {
        generation: u64,
        message: String,
    },
    /// Frame `index` of a set is begun; `name` is the name it is
    /// asked to be written under.
    SetFrameStarted {
        set: Arc<crate::queue::Set>,
        index: usize,
        name: String,
    },
    /// What came of frame `index` of a set.
    SetFrameDone {
        set: Arc<crate::queue::Set>,
        index: usize,
        source: PathBuf,
        /// The edit the frame was written under: the one asked for,
        /// its learned blend seeded from the ISO when the frame's was
        /// still to be seeded.
        edit: Edit,
        /// The id of the version the frame was, none for the file's
        /// own line: the export is recorded on that line.
        version: Option<String>,
        done: crate::queue::Done,
    },
    /// The set's last frame is done with, whichever way.
    SetDone {
        set: Arc<crate::queue::Set>,
        tally: crate::queue::Tally,
    },
    /// A learned mask made (or found made) for `shape` at `key`, on
    /// `file`, the picture open when it was made.
    Mask {
        key: Key,
        shape: Shape,
        file: Option<PathBuf>,
        raster: Arc<Raster>,
        /// The provider that ran it; none when it was cached.
        provider: Option<&'static str>,
        seconds: f64,
        /// What the status line says instead: a Sky shape on a frame
        /// with no sky; a Part whose person is not settled.
        note: Option<String>,
        /// A Part made empty, its person not settled on this picture.
        part: Option<crate::ai::Unresolved>,
    },
    MaskFailed {
        key: Key,
        message: String,
    },
    /// The people on the open picture, `file`, for a Part to be
    /// picked among; or why they could not be found.
    People {
        file: Option<PathBuf>,
        people: Result<(Vec<crate::ai::Candidate>, &'static str, f64), String>,
    },
    /// What came of a person looked for on `frame`, for run `batch` of
    /// "use this person on all"; or why they could not be looked for.
    PersonOn {
        batch: u64,
        frame: Frame,
        found: Result<crate::ai::PersonOn, String>,
    },
    /// What the Landscape button offers on the open picture, `file`,
    /// the largest first, with the provider and the seconds; or why it
    /// could not be found.
    Landscape {
        file: Option<PathBuf>,
        offers: Result<(Vec<greycard_ai::landscape::Offer>, &'static str, f64), String>,
    },
    Fetching {
        name: &'static str,
        done: u64,
        total: u64,
    },
    Fetched {
        model: &'static greycard_ai::Model,
    },
    /// Microsoft's CPU-only ONNX Runtime arrived and is loaded: the AI
    /// features are on, on the CPU.
    RuntimeFetched {
        runtime: &'static greycard_ai::Runtime,
    },
    /// Microsoft's CPU-only ONNX Runtime arrived whole and cannot be
    /// used: it would not load here. The line says so, not that the
    /// download failed.
    RuntimeUnusable {
        /// The runtime's registry id.
        id: &'static str,
        message: String,
    },
    FetchFailed {
        /// The model's registry id.
        id: &'static str,
        name: &'static str,
        message: String,
    },
    LensesFetching {
        done: u64,
        total: u64,
    },
    /// The lens database arrived and the worker has it.
    LensesFetched,
    LensesFetchFailed {
        message: String,
    },
    /// The pictures `frame` shows in the strip, the grid and the loupe
    /// may have changed: its edit's were kept, or its edit was reset or
    /// left without them. Its cell asks for its picture again.
    EditedKept {
        frame: Frame,
    },
    /// The thumbnails' pool is making pictures the cache did not have
    /// (camera thumbnails, local previews, the edit's pictures): how
    /// many are left, or that it has nothing left in hand.
    Making(crate::thumbpool::Making),
}

#[derive(Default)]
struct Queue {
    /// A lens database fetched since the worker read its own, for it
    /// to take up before its next develop.
    lenses: Option<Arc<greycard_lens::Database>>,
    /// The editor's device and queue, handed over once the window has
    /// them, for the engine's GPU ops to run on: the picture they
    /// leave is then the viewport's without a copy.
    gpu: Option<(greycard_gpu::wgpu::Device, greycard_gpu::wgpu::Queue)>,
    /// The window's device was lost (`Worker::gpu_lost_teller`): the engine's
    /// ops go to the CPU, and with `Some(true)` the develop on screen is
    /// made again there; `Some(false)` when the editor is about to
    /// start again, which develops it anew.
    gpu_lost: Option<bool>,
    /// The newest open or develop; an older one still queued is dropped.
    develop: Option<Job>,
    /// Masks wanted, the newest shape for a key replacing an older,
    /// with the model chosen for a learned shape that needs one
    /// picked (Subject; `None` for a shape whose model is not in
    /// question, Object among them, which always wants SAM).
    masks: std::collections::VecDeque<(Key, Shape, Option<&'static greycard_ai::Model>)>,
    /// The people on the open picture are wanted (`Job::People`).
    people: bool,
    /// What the Landscape button offers is wanted (`Job::Landscape`).
    landscape: bool,
    /// The saves of the open frame (`Job::Keep`), the newest a frame,
    /// taken ahead of a develop or an open queued after them, so a frame
    /// left at once still has its picture in hand when its save is seen.
    keeps: std::collections::VecDeque<Job>,
    exports: std::collections::VecDeque<Job>,
    /// Frames whose learned masks the reduced develop found missing
    /// from the mask cache (`Job::MasksFor`): made here, behind
    /// everything else, since each is a decode and a model run.
    masks_for: std::collections::VecDeque<Frame>,
    /// Frames to look for a person picked on another on
    /// (`Job::PersonOn`), behind the open frame's own masks and the
    /// exports.
    people_on: std::collections::VecDeque<Job>,
    /// A Subject model arrived since the one loaded, if any: drop it,
    /// so the next Subject mask picks up the new file rather than the
    /// one already running.
    forget_subject: bool,
    /// A model arrived in the store: the learned masks whose second run
    /// failed are asked for again (`Ai::retry_fallbacks`).
    models_arrived: bool,
    /// The editor is leaving: finish what is in hand and stop, so the
    /// engine's GPU context is dropped on this thread rather than
    /// under a process that is already on its way out.
    stopping: bool,
    /// The frames beside the one opened last, whose bases are made
    /// onto the shelf when nothing else is asked for (`prefetch`).
    neighbors: Option<prefetch::Neighbors>,
    /// The generation and the file of the last open whose develop
    /// landed, as the worker has it: what neighbors sent again for the
    /// frame open (`Worker::neighbors_again`) are for.
    landed: Option<(u64, PathBuf)>,
    /// Files whose edit changed without an open of them, with the edit
    /// now when the window has it: their shelved bases that no open
    /// would take are let go (`Worker::edit_changed`).
    edits_changed: Vec<(PathBuf, Option<FileNow>)>,
}

pub(crate) type Deliver = Arc<dyn Fn(Outcome) + Send + Sync>;

/// How long the editor waits on its way out for the worker to finish
/// what it is in the middle of. A develop is a second or two; a job
/// that outlasts this is left where it is, which is what leaving did
/// before there was any waiting at all.
pub const LEAVING: std::time::Duration = std::time::Duration::from_secs(5);

/// The thumbnail cache on disk, shared between the worker, which
/// reads and writes it, and the settings sheet, which counts and
/// clears it. `None` until the editor hands one over, and in tests.
pub type ThumbCache = Arc<Mutex<Option<Thumbs>>>;

/// The thumbnail cache's caps as last set, shared with the threads
/// that apply them.
pub(crate) type ThumbCaps = Arc<Mutex<Option<(u64, u64)>>>;

pub struct Worker {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    deliver: Deliver,
    /// The worker's thread, until it is joined on the way out.
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    thumbs: ThumbCache,
    /// The cache's two caps in bytes, thumbnails' then previews', as
    /// last handed over or set: what the settings sheet shows without
    /// waiting on the cache's lock, which a count or an eviction holds
    /// for seconds. `None` while there is no cache.
    caps: ThumbCaps,
    /// The thumbnails' own threads, shared with the worker's thread,
    /// which holds them back while the first develop runs.
    pool: Arc<crate::thumbpool::Pool>,
    /// The local previews the pool makes behind the thumbnails, in
    /// the same cache.
    previews: Arc<crate::previews::Previews>,
    /// Which frames show their edit, and the pictures made from it, in
    /// the same cache (`edited`).
    edited: Arc<crate::edited::Edited>,
    /// The reduced develops of the edit's pictures in the pool
    /// (`reduced`).
    reducer: Arc<crate::reduced::Reducer>,
}

/// The last develop, kept for an export under the same develop and the
/// same orientation.
struct Last {
    edit: Edit,
    turn: u8,
    image: Arc<WorkingImage>,
    /// Whether the base this picture came from had its CA or its lens
    /// corrected on the GPU. The export's picture must be from the CPU.
    on_gpu: bool,
    /// What the develop left out of what the edit asked for
    /// ([`left_out_of`]), for an export that takes this picture.
    left_out: Vec<String>,
}

impl Last {
    /// The develop that just finished, from the base it ran on, and
    /// its outcome.
    fn made(edit: Edit, base: &Base, image: Arc<WorkingImage>, outcome: &Outcome) -> Self {
        Self {
            edit,
            turn: base.turn,
            image,
            on_gpu: base.on_gpu,
            left_out: left_out_of(outcome, Some(base)),
        }
    }
}

/// A save of the open frame's edit, waiting on the worker for the
/// develop it is the picture of (`Job::Keep`).
struct Keep {
    frame: Frame,
    edit: Edit,
    turn: u8,
    iso: crate::edited::Iso,
}

/// The develop the viewport shows, as a texture the engine's GPU ops
/// left on the device, with the edit and the turn it was made under:
/// held until the next develop begins, so a save of the open frame can
/// read a reduced copy of it back for the pictures kept of its edit.
struct ShownTexture {
    edit: Edit,
    turn: u8,
    /// The develop's generation, for a develop again of the same.
    generation: u64,
    texture: greycard_gpu::wgpu::Texture,
    /// What the develop left out of what the edit asked for
    /// ([`left_out_of`]): a picture without it is not kept.
    left_out: Vec<String>,
}

impl ShownTexture {
    /// The texture a develop's outcome carries, for `edit` at `turn`,
    /// from `base`.
    fn of(edit: &Edit, turn: u8, outcome: &Outcome, base: Option<&Base>) -> Option<Self> {
        match outcome {
            Outcome::Developed {
                image: Developed::Texture(texture),
                generation,
                ..
            } => Some(Self {
                edit: edit.clone(),
                turn,
                generation: *generation,
                texture: texture.clone(),
                left_out: left_out_of(outcome, base),
            }),
            _ => None,
        }
    }
}

/// What the worker holds of the open frame that the edit's pictures
/// can be made from.
struct InHand<'a> {
    opened: Option<&'a std::path::Path>,
    last: Option<&'a Last>,
    shown: Option<&'a ShownTexture>,
    base: Option<&'a Base>,
    gpu: Option<&'a greycard_gpu::Context>,
}

/// A frame developed once on the CPU, as an export of a frame that is
/// not open develops it, and finished as often as asked: the camera
/// match's develop, which finishes each frame at two exposures.
/// Everything the frame's own develop does after the base (the lens,
/// the retouch, the detail, the sharpen) is in `image`; the finish
/// under `edit` is [`FrameDevelop::finish`].
pub(crate) struct FrameDevelop {
    image: Arc<WorkingImage>,
    base: Option<Base>,
    ai: Ai,
}

impl FrameDevelop {
    /// Open the raw at `path` and develop it under `edit`, with the
    /// lens database for the lens corrections. The export's own
    /// develop, with no GPU and no learned denoiser's cache.
    pub(crate) fn develop(
        path: &std::path::Path,
        edit: &Edit,
        lenses: Option<&greycard_lens::Database>,
    ) -> Result<FrameDevelop, String> {
        let (input, _) = timed_open(path).map_err(|e| format!("{e:#}"))?;
        if !matches!(input, Input::Raw(_)) {
            return Err("not a raw".into());
        }
        let mut ai = Ai::new();
        let mut base = None;
        let deliver: Deliver = Arc::new(|_| {});
        match develop_job(
            &input, edit, 0, 0, &mut base, &mut None, &mut ai, None, lenses, &mut None, &deliver,
        ) {
            (_, Some(image)) => Ok(FrameDevelop { image, base, ai }),
            (Outcome::Failed { message, .. }, None) => Err(message),
            _ => Err("the develop made no picture".into()),
        }
    }

    /// A picture standing in for a develop, with no base: for a test
    /// of the finish.
    #[cfg(test)]
    pub(crate) fn of_image(image: WorkingImage) -> FrameDevelop {
        FrameDevelop {
            image: Arc::new(image),
            base: None,
            ai: Ai::new(),
        }
    }

    /// What the finish is handed: a raw's develop and its body's
    /// baseline, as the export finishes it.
    pub(crate) fn source(&self) -> crate::finish::Source {
        self.base.as_ref().map(|b| b.source).unwrap_or_default()
    }

    /// The developed picture finished under `edit` (whose develop is
    /// the one this was made under) and `settings`, as an export
    /// finishes it.
    pub(crate) fn finish(
        &mut self,
        edit: &Edit,
        settings: &crate::export::Settings,
    ) -> crate::export::Rendered {
        let (rendered, left_out) = finish_export(
            self.image.clone(),
            edit,
            self.base.as_ref(),
            &mut self.ai,
            settings,
        );
        for item in left_out {
            tracing::warn!("{item}");
        }
        rendered
    }

    /// What the finish would be handed for `edit` and `settings`,
    /// before any pixel goes through it ([`crate::export::Scene`]).
    pub(crate) fn scene(
        &self,
        edit: &Edit,
        settings: &crate::export::Settings,
    ) -> crate::export::Scene {
        scene_export(&self.image, edit, self.base.as_ref(), settings)
    }
}

/// Who a job's panic is reported to: the outcome the UI is waiting
/// on for that job, when it is waiting on one.
#[derive(Debug, Clone, PartialEq)]
enum Blame {
    Develop(u64),
    Mask(Key),
    People,
    Landscape,
    PersonOn(u64, Frame),
    Nobody,
}

impl Blame {
    fn of(job: &Job) -> Self {
        match job {
            Job::Open { generation, .. } | Job::Develop { generation, .. } => {
                Blame::Develop(*generation)
            }
            // A frame of a set is caught where it runs, and counted
            // there as the set's failure.
            Job::ExportSet { .. } | Job::ExportFrame { .. } => Blame::Nobody,
            Job::Mask { key, .. } => Blame::Mask(*key),
            Job::People => Blame::People,
            Job::Landscape => Blame::Landscape,
            Job::PersonOn(job) => Blame::PersonOn(job.batch, job.frame.clone()),
            Job::Thumbnail { .. }
            | Job::CachedThumbnail { .. }
            | Job::Fetch { .. }
            | Job::FetchRuntime { .. }
            | Job::FetchLenses
            | Job::Gpu
            | Job::MasksFor(_)
            | Job::Keep { .. } => Blame::Nobody,
        }
    }

    fn outcome(self, message: String) -> Option<Outcome> {
        let message = format!("the worker panicked: {message}");
        Some(match self {
            Blame::Develop(generation) => Outcome::Failed {
                generation,
                message,
            },
            Blame::Mask(key) => Outcome::MaskFailed { key, message },
            Blame::People => Outcome::People {
                file: None,
                people: Err(message),
            },
            Blame::Landscape => Outcome::Landscape {
                file: None,
                offers: Err(message),
            },
            Blame::PersonOn(batch, frame) => Outcome::PersonOn {
                batch,
                frame,
                found: Err(message),
            },
            Blame::Nobody => return None,
        })
    }
}

/// A panic's message, which is a `&str` or a `String` for every
/// `panic!` with a text, and nothing readable otherwise.
/// Whether a panic's message is wgpu's for a lost device: what it
/// panics with when a wait, or a buffer it maps for itself, meets one.
pub(crate) fn says_lost(message: &str) -> bool {
    message.contains("device is lost")
}

pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into())
}

/// A whole number of quarter turns clockwise as an orientation, for
/// a picture that was already turned by its own tag at decode; None
/// for no turn at all.
pub(crate) fn quarters(turn: u8) -> Option<greycard_core::raw::Orientation> {
    use greycard_core::raw::Orientation;
    match turn % 4 {
        1 => Some(Orientation::Rotate90),
        2 => Some(Orientation::Rotate180),
        3 => Some(Orientation::Rotate270),
        _ => None,
    }
}

/// The open file: a raw to develop, or a picture already in the
/// working space (its image taken out of its record, so the base
/// shares it rather than copying it).
enum Input {
    Raw(Arc<RawFrame>),
    Picture {
        image: Arc<WorkingImage>,
        meta: Arc<Picture>,
    },
}

impl Input {
    /// Whether the file is the sensor's data rather than a rendered
    /// picture.
    fn is_raw(&self) -> bool {
        matches!(self, Input::Raw(_))
    }

    /// What the file says of its body and its lens.
    fn identity(&self) -> (&str, &str, &Shot) {
        match self {
            Input::Raw(f) => (&f.make, &f.model, &f.shot),
            Input::Picture { meta, .. } => (&meta.make, &meta.model, &meta.shot),
        }
    }
}

/// A develop before its dehaze and sharpen, and what the sharpen
/// needs to know.
struct Base {
    edit: Edit,
    /// The frame's quarter turns this base was made at: a turn is
    /// not part of the edit, so `same_base` cannot see it.
    turn: u8,
    image: Arc<WorkingImage>,
    /// The tone equalizer's read of the scene, of the patched picture
    /// when there is one and of the base otherwise, made once with
    /// each: the Light sliders never re-develop, so a slider move
    /// must not pay for it, and neither must a sharpen change. Every
    /// way of making a base leaves [`crate::finish::Guide::NONE`] here
    /// and `develop_job` fills it in after the lens, and again with
    /// each fresh retouch or a retouch cleared.
    guide: Arc<crate::finish::Guide>,
    white: WhiteBase,
    radius: Option<f32>,
    clip_level: f32,
    /// A raw's scene or a picture already rendered, for the finish.
    source: crate::finish::Source,
    /// Counts the bases made, for the learned masks to know theirs.
    stamp: u64,
    /// The base with its retouch applied, for the retouch it was,
    /// whether every fill in it was made, and the Fill patches left as
    /// they were, by id.
    patched: Option<(Retouch, Arc<WorkingImage>, bool, Vec<u64>)>,
    /// The picture before its sharpen, on the GPU, for the sharpen's
    /// slider: made once, re-read on every move.
    pre: Option<PreSharpen>,
    /// The patched picture on the GPU, for the Detail section there
    /// on every Detail move, held with the `Arc` it was made from so
    /// its identity cannot be reused. Kept while the GPU path is the
    /// one taken; let go when a picture the ops decline takes the
    /// develop to the CPU, or when neither the Detail section nor the
    /// sharpen is on.
    uploaded: Option<(Arc<WorkingImage>, Arc<greycard_gpu::Image>)>,
    /// The dehaze's reduced copy of the picture it read on the GPU,
    /// for a Dehaze move to fit again from without summing the blocks
    /// and reading them back again.
    haze: Option<KeptHaze>,
    /// Whether the CA or the lens correction in this base ran on the GPU.
    /// The export's picture is the reference's, and both are in the base
    /// (unlike the sharpen, which the export re-runs), so an export
    /// makes a fresh base on the CPU when this is set.
    on_gpu: bool,
    /// The learned denoiser's report when this base is the engine's
    /// stand-in for it (its model missing, or failing): a develop that
    /// keeps the base reports `Kept`, and an export of it must still
    /// say what it went without (`left_out_of`).
    stand_in: Option<LearnedReport>,
    /// Why the demosaic the edit chose did not run, when it was a
    /// method of the other mosaic and the frame's default ran in its
    /// place: said with what an export left out (`left_out_of`). Not
    /// for the engine's own default, which an edit stores when nobody
    /// chose.
    gave_way: Option<String>,
    /// The camera profile a raw's white was resolved through, which a
    /// mask's own white balance is resolved through too
    /// ([`Base::white_shift`]); `None` for a picture that is not a raw.
    profile: Option<Arc<CameraProfile>>,
    /// The bands the masks' Detail reads, with what they were made
    /// from, kept for the next develop: a mask's slider moves no
    /// develop, and a move elsewhere that leaves these alone should
    /// not make them again.
    planes: Option<KeptPlanes>,
    /// Taken back from the shelf of frames left behind
    /// ([`shelf::Shelf::take`]) and not developed from since: the next
    /// develop's log line says where its base came from, and clears
    /// this.
    from_shelf: bool,
}

/// The masks' bands ([`crate::detail::DetailPlanes`]) with what they
/// were made from: the patched picture, the bands asked for, and the
/// global's local contrast the haze was read after (none without a
/// haze, or with the global's Texture and Clarity at rest).
struct KeptPlanes {
    patched: Arc<WorkingImage>,
    wants: greycard_edit::DetailWants,
    haze_after: Option<LocalContrastOptions>,
    planes: Arc<crate::detail::DetailPlanes>,
}

impl KeptPlanes {
    /// Whether these serve a develop of `patched` asking for `wants`
    /// with the global's local contrast `global`: the same picture,
    /// every band asked for, and a haze read after that contrast.
    fn serve(
        &self,
        patched: &Arc<WorkingImage>,
        wants: &greycard_edit::DetailWants,
        global: Option<LocalContrastOptions>,
    ) -> bool {
        Arc::ptr_eq(&self.patched, patched)
            && self.wants.covers(wants)
            && (!wants.haze || self.haze_after == global)
    }

    /// Whether these hold what `edit`'s masks read, for a finish of
    /// the picture they were made beside.
    fn serves_edit(&self, edit: &Edit) -> bool {
        let wants = edit.detail_wants();
        self.wants.covers(&wants) && (!wants.haze || self.haze_after == edit.detail.options())
    }
}

/// The masks' bands for a develop: none asked for, kept from the last,
/// or being made on a thread of their own beside the develop.
enum PlanesJob {
    Nothing,
    Kept(Arc<crate::detail::DetailPlanes>),
    Making {
        made: std::sync::mpsc::Receiver<Option<crate::detail::DetailPlanes>>,
        // Held for its drop alone.
        _cancel: CancelOnDrop,
        patched: Arc<WorkingImage>,
        wants: greycard_edit::DetailWants,
        haze_after: Option<LocalContrastOptions>,
        started: Instant,
    },
}

impl PlanesJob {
    /// Start making the planes of `patched` on a thread of their own:
    /// on the GPU's path the CPU is otherwise idle while the device
    /// works, so they cost the develop nothing they do not overlap. A
    /// thread and not the pool's `spawn`, whose panic would end the
    /// process; a panic here drops the sender, and the develop goes on
    /// without the planes. The bands `reuse` holds (planes of the same
    /// picture whose haze was read after another global local
    /// contrast) are taken from it, so a move of the global's Texture
    /// or Clarity makes the haze alone again. A develop that ends
    /// without taking the planes up cancels them (`Drop`).
    fn start(
        patched: Arc<WorkingImage>,
        global: Option<LocalContrastOptions>,
        clip_level: f32,
        wants: greycard_edit::DetailWants,
        reuse: Option<Arc<crate::detail::DetailPlanes>>,
    ) -> Self {
        let (send, made) = std::sync::mpsc::channel();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let haze_after = global.filter(|_| wants.haze);
        let (picture, stop) = (patched.clone(), cancel.clone());
        let spawned = std::thread::Builder::new()
            .name("detail planes".into())
            .spawn(move || {
                let planes = crate::detail::DetailPlanes::make_with(
                    &picture,
                    haze_after,
                    clip_level,
                    wants,
                    reuse.as_deref(),
                    &stop,
                );
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    tracing::debug!("the masks' detail bands: the develop ended; dropped");
                } else if send.send(planes).is_err() {
                    tracing::debug!("the masks' detail bands: nobody waiting; dropped");
                }
            });
        match spawned {
            Ok(_) => Self::Making {
                made,
                _cancel: CancelOnDrop(cancel),
                patched,
                wants,
                haze_after,
                started: Instant::now(),
            },
            Err(e) => {
                tracing::warn!("the masks' detail bands: no thread to make them on: {e}");
                Self::Nothing
            }
        }
    }

    /// The planes for the develop being delivered, waited for if they
    /// are still being made, and kept in `b` for the next.
    fn land(&mut self, b: &mut Base) -> Option<Arc<crate::detail::DetailPlanes>> {
        match std::mem::replace(self, Self::Nothing) {
            Self::Nothing => None,
            Self::Kept(planes) => Some(planes),
            Self::Making {
                made,
                patched,
                wants,
                haze_after,
                started,
                ..
            } => {
                let waited = Instant::now();

                let planes = match made.recv() {
                    Ok(Some(planes)) => Some(Arc::new(planes)),
                    Ok(None) => None,
                    Err(_) => {
                        tracing::warn!(
                            "the masks' detail bands: the thread making them died; developing without them"
                        );
                        None
                    }
                };
                tracing::info!(
                    "the masks' detail bands in {:.3} s, {:.3} s of it waited for",
                    started.elapsed().as_secs_f64(),
                    waited.elapsed().as_secs_f64()
                );
                b.planes = planes.clone().map(|planes| KeptPlanes {
                    patched,
                    wants,
                    haze_after,
                    planes,
                });
                planes
            }
        }
    }
}

/// The planes' thread's stop, set when the job that started it goes:
/// a develop that fails or is given up before its delivery stops the
/// planes it started, and the thread gives up between the bands and
/// the haze and sends nothing. Set after a landing too, when the
/// thread is done and it stops nothing.
struct CancelOnDrop(Arc<std::sync::atomic::AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Base {
    /// What the masks' white balances are made matrices from: this
    /// develop's white and the profile it was resolved through.
    fn white_shift(&self) -> Option<greycard_edit::WhiteShift> {
        let profile = self.profile.as_ref()?;
        Some(greycard_edit::WhiteShift::from_parts(
            (**profile).clone(),
            self.white.gains,
            self.white.matrix,
            self.white.temp_tint?,
        ))
    }
}

/// The picture the sharpen reads, on the GPU, with what it was made
/// from and what the steps before the sharpen reported. The image is
/// the Detail section's output on the device (the dehaze's, or the
/// local contrast's when the dehaze is off), the patched picture's
/// own upload when nothing before the sharpen is set, or the CPU's
/// picture uploaded when the ops declined the picture.
struct PreSharpen {
    patched: Arc<WorkingImage>,
    detail: greycard_edit::Detail,
    image: Arc<greycard_gpu::Image>,
    detail_stats: Option<LocalContrastStats>,
    dehaze_stats: Option<DehazeStats>,
}

/// The dehaze's reduced copy, with what it was made from: the patched
/// picture and the local contrast the GPU ran on it. The GPU's local
/// contrast gives the same picture from the same inputs, run to run,
/// so a move of the Dehaze alone runs the local contrast again (a few
/// ms, where keeping its output would hold another picture on the
/// device, 716 MB at 45 MP) and fits from this copy.
struct KeptHaze {
    patched: Arc<WorkingImage>,
    detail: Option<LocalContrastOptions>,
    reduced: Arc<Reduced>,
}

/// Where a develop's dehaze ran, as [`DetailRan`] says it of the
/// local contrast, for the log.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DehazeRan {
    Cpu,
    Gpu,
    Kept,
}

/// Where a develop's local contrast ran: on the CPU, in so many
/// seconds; on the GPU, where the time to submit is not the time it
/// takes, so none is given (as for the sharpen); or not in this
/// develop, the picture after it kept from the last.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DetailRan {
    Cpu(f64),
    Gpu,
    Kept,
}

/// The learned denoiser's answer and the plain demosaic of the same
/// mosaic, both finished, for the edit they were made for; a strength
/// is a blend of the two.
struct LearnedBase {
    edit: Edit,
    /// The frame's quarter turns this pair was made at. `Edit` has
    /// no turn in it, so `same_learned` cannot see one move; without
    /// this the network's picture and the plain one beside it would
    /// both stay the way up they were and the viewport would show an
    /// unturned frame while every other part of the window said
    /// turned.
    turn: u8,
    model: Arc<WorkingImage>,
    plain: Arc<WorkingImage>,
    white: WhiteBase,
    radius: Option<f32>,
    clip_level: f32,
    /// Whether the CA correction in this pair ran on the GPU. The
    /// correction is applied to the mosaic before the network sees
    /// it, so it is in both pictures and in every base blended from
    /// them; a develop without a GPU (the export) makes the pair
    /// again on the CPU rather than serve it, as the base's own check
    /// does for the base.
    ca_on_gpu: bool,
}

/// Whether a kept learned pair can serve a develop at `turn` under
/// `edit`: the same turn and the same learned settings, and, without a
/// GPU, only a pair whose CA correction was not the GPU's.
fn pair_serves(l: &LearnedBase, turn: u8, edit: &Edit, has_gpu: bool) -> bool {
    l.turn == turn && l.edit.same_learned(edit) && (has_gpu || !l.ca_on_gpu)
}

#[cfg(test)]
mod reduced_check;
