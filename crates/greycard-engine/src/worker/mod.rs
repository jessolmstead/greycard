//! The engine on its own thread: decodes and develops, one job at a
//! time, the newest develop winning. Thumbnails are made beside it,
//! on a pool of their own (`thumbpool`).

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use greycard_core::CameraProfile;
use greycard_core::color::{as_shot_temp_tint, profile_from_frame};
use greycard_core::develop::dehaze::DehazeStats;
use greycard_core::develop::lens::LensCorrection;
use greycard_core::develop::local_contrast::LocalContrastStats;
use greycard_core::develop::sharpen::SharpenStats;
use greycard_core::develop::{DemosaicMethod, DevelopSettings, Prepared, develop};
use greycard_core::image::WorkingImage;
use greycard_core::picture::{Picture, decode_picture_path, is_picture_path};
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
use self::decode::open;
#[cfg(test)]
use self::develop::develop_job;
use self::{
    base::{FrameKept, KeptMaskPicture, Stages, WorkerModels, timed, window_steps},
    decode::{lens_report, open_told, timed_open},
    develop::{Round, develop_aside, develop_round},
    engine::{left_out_of, left_out_words, run},
    export::{Written, export_open, export_other, seeded, walk_for_export},
    keep::keep_open,
    masks::{MasksMade, learned_masks, masks_for, person_on},
    thumbs::{cached_thumbnail_noting, thumb_lookup},
};

pub(crate) use self::base::{learned_failure, network, run_tiling};
#[cfg(any(test, feature = "testing"))]
pub use self::prefetch::{behind_prefetch, claim};
pub use self::thumbs::{THUMB_RECIPE, THUMB_WIDTH, count_thumb_cache};
pub use self::{
    decode::correct_lens_of,
    handle::order_thumbnails,
    prefetch::{DEEPEST, Depth, Heading, Neighbor, STREAK_FOR_ONE, Side},
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

/// The picture the viewport uploads, the render crate's.
pub use greycard_render::halves::Halves;

use greycard_core::RgbSpace;
use greycard_core::raw::RawFrame;
use greycard_edit::brush::Raster;
use greycard_edit::mask::Shape;
use greycard_edit::retouch::Retouch;
use greycard_edit::{Edit, Noise};

use crate::ai::{Ai, DenoiseOn, Key, NoDenoiser, NoFill};
use crate::version_key::Key as Frame;

/// The frame a develop is for: its file, and its version, none for the
/// file's own, as an open of it names them.
pub type DevelopOf = (PathBuf, Option<std::sync::Arc<str>>);

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
        /// The learned shapes whose raster the window would ask for at
        /// once, each with the model its own choice names: read from
        /// the mask cache and answered ahead of the first picture when
        /// made already. One the window would offer a model for, or
        /// wait on, is not here, and is asked for after the picture.
        learned: Vec<(Key, &'static greycard_ai::Model)>,
    },
    Develop {
        edit: Edit,
        generation: u64,
        turn: u8,
        /// The frame it is for, its file and version, as the window had
        /// it current when it sent it: one sent while an open of that
        /// frame still waits becomes that open (`handle::into_slot`).
        /// None for one the worker makes itself.
        of: Option<DevelopOf>,
    },
    /// Another edit of the open frame developed beside its own, as a
    /// before and after shows it: answered as a develop of
    /// `generation` is, and nothing of the frame's own develop (its
    /// kept points, its base, the develop it last delivered) changes.
    DevelopAside {
        edit: Edit,
        generation: u64,
        turn: u8,
        /// As a develop's.
        of: Option<DevelopOf>,
    },
    /// The next fill of the open frame's develop `generation`, which
    /// landed with fills still to make: queued by the worker itself,
    /// behind any develop asked for, which gives the rest up.
    #[doc(hidden)]
    Fill { generation: u64 },
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
    /// The picture the refined masks of `edit` are made against, for
    /// the open picture's develop `generation` drawn without one: made
    /// from the base in hand, answered with `MaskPicture`.
    MaskPicture { edit: Box<Edit>, generation: u64 },
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
        iso: crate::edited_pictures::Iso,
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
    /// Fetch a model into the store; on its own thread. `warm`: on a
    /// Mac, Core ML's compile of it is made once it lands
    /// (`ai::warm_core_ml`), for a fetch of every model at once.
    Fetch {
        model: &'static greycard_ai::Model,
        warm: bool,
    },
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
    /// Of those, the removals refused for the card's memory: the name,
    /// the need, and the card's free memory, bytes, for the window's
    /// word on them; the reason in `failed` is the log's.
    pub short: Vec<(String, u64, u64)>,
    /// Fill patches left as they were for now, by id: the picture is
    /// delivered first, and the worker makes these after it, one a
    /// develop of the same generation.
    pub pending: Vec<u64>,
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
    /// A picture's own white and the light its white balance showed it
    /// at; none for a raw.
    pub picture: Option<(greycard_core::TempTint, greycard_core::TempTint)>,
}

impl WhiteBase {
    /// No gains and no matrix: a picture already in the working space.
    pub const IDENTITY: Self = Self {
        gains: [1.0; 3],
        matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        clip: f32::INFINITY,
        temp_tint: None,
        picture: None,
    };

    /// The white balance `wb`, with channels clipping at `clip`.
    pub fn from(wb: &greycard_core::WhiteBalance, clip: f32) -> Self {
        let cols = wb.matrix_f32();
        Self {
            gains: wb.coefficients_f32(),
            matrix: std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r])),
            clip,
            temp_tint: Some(wb.temp_tint),
            picture: None,
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
        /// The frame's own white balance as temperature and tint: a
        /// raw's As shot, a picture's own white.
        as_shot: (f64, f64),
        /// The frame and its profile, for the white balance preview;
        /// neither for a picture that is not a raw.
        frame: Option<Arc<RawFrame>>,
        profile: Option<Box<CameraProfile>>,
        /// What the lens database made of it.
        lens: LensReport,
        /// The lenses a picker offers for it.
        lens_options: Vec<crate::lens::LensOption>,
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
        kind: greycard_render::finish::Source,
        /// The tone equalizer's plane for this develop, for the
        /// viewport's own texture.
        guide: Arc<greycard_render::finish::Guide>,
        /// The bands the masks' Detail is put on from, when a mask
        /// asks for one ([`greycard_render::detail::DetailPlanes`]).
        planes: Option<Arc<greycard_render::detail::DetailPlanes>>,
        white: WhiteBase,
        seconds: f64,
        /// What the local contrast did, when it ran, and where: on
        /// the CPU in so many seconds, on the GPU, or kept from the
        /// last develop. For the tests: the log says it otherwise.
        #[cfg(test)]
        detail: Option<(LocalContrastStats, DetailRan)>,
        /// What the sharpen did, when it ran: its measure, for the
        /// sharpen section.
        sharpen: Option<SharpenStats>,
        /// What the dehaze did, when it ran. For the tests, as
        /// `detail` is.
        #[cfg(test)]
        dehaze: Option<DehazeStats>,
        /// The patches that had no source and were given one here,
        /// each whole, as developed, with the source the engine chose.
        sources: Vec<greycard_edit::retouch::Patch>,
        /// What the learned denoiser did.
        learned: LearnedReport,
        /// What the fill model did.
        fills: FillReport,
    },
    /// Whether the merge of ours just opened, one that keeps its recipe,
    /// can be made again with its frames denoised
    /// ([`crate::merge::can_denoise_again`]): said after its develop,
    /// since it reads the frames.
    MergeAgain {
        path: PathBuf,
        can: bool,
    },
    /// The kind of mosaic of a raw being opened (`None`: it is not a
    /// mosaic), read beside its decode so the demosaic menu is the
    /// file's before its pixels are.
    Mosaic {
        path: PathBuf,
        mosaic: Option<greycard_core::raw::Mosaic>,
    },
    /// The fill model is at work on the patch named, for a develop
    /// still on its way: `made` of the `of` fills the develop had to
    /// make are done.
    Filling {
        generation: u64,
        name: String,
        made: usize,
        of: usize,
    },
    /// The fill at work for develop `generation` has made `step` of
    /// its `of` steps: the generative remove's UNet calls.
    FillStep {
        generation: u64,
        step: usize,
        of: usize,
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
    /// A learned mask of `file`, the picture open, runs on the CPU, the
    /// card short of it once the engine let go of what it could: the
    /// mask at `key`, or the People or Landscape model (none). `status`
    /// is the line the parts' runs on the CPU say
    /// ([`crate::ai::part_on_cpu`]), said before the run.
    MaskOnCpu {
        key: Option<Key>,
        file: Option<PathBuf>,
        status: &'static str,
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
    /// The picture the refined masks are made against, for the develop
    /// `generation` asked with (`Job::MaskPicture`); none with no base
    /// in hand.
    MaskPicture {
        generation: u64,
        picture: Option<Arc<greycard_render::refine::MaskPicture>>,
    },
    /// A file of a model (or of the runtime) coming: its bytes so far
    /// and in all, zero when the server did not say.
    Fetching {
        name: &'static str,
        file: &'static str,
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

impl Queue {
    /// `generation`, an open's as the window sent it, as the worker has
    /// it now: the develop's it was folded into, if it was.
    fn folded_to(&self, generation: u64) -> u64 {
        match self.folded {
            Some((from, to)) if from == generation => to,
            _ => generation,
        }
    }
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
    /// A context built by whoever holds the device (the command line's
    /// export, on a device of its own), taken as it is.
    context: Option<greycard_gpu::Context>,
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
    /// The masks a develop's new base asks for, moved to `masks` once
    /// the develops have been quiet a while (`masks::Settling`).
    settling: Option<masks::Settling>,
    /// The people on the open picture are wanted (`Job::People`).
    people: bool,
    /// What the Landscape button offers is wanted (`Job::Landscape`).
    landscape: bool,
    /// The refined masks' picture is wanted (`Job::MaskPicture`), the
    /// newest ask replacing an older.
    mask_picture: Option<Job>,
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
    /// A develop folded into a waiting open (`handle::into_slot`): the
    /// generation the window sent the open at, and the one it lands at
    /// now. Until the next open, neighbors the window sends under the
    /// first are taken as under the second.
    folded: Option<(u64, u64)>,
    /// Files whose edit changed without an open of them, with the edit
    /// now when the window has it: their shelved bases that no open
    /// would take are let go (`Worker::edit_changed`).
    edits_changed: Vec<(PathBuf, Option<FileNow>)>,
    /// The open frame's fills made with no model, of this value
    /// (`Ai::fill_by_test`): for a test of the rounds.
    #[cfg(test)]
    fill_by_test: Option<f32>,
    /// The patient waits the worker had slept by the end of each export
    /// frame, in order (`crate::ai::waits_by_test`).
    #[cfg(test)]
    waits_by_test: Vec<usize>,
    /// The shelf's budget, as a test has it, for a shelf made anew.
    #[cfg(test)]
    shelf_by_test: Option<shelf::Budget>,
    /// The card's free memory, and what its sessions give back when
    /// dropped, as a test has them (`Ai::free_memory_by_test`).
    #[cfg(test)]
    free_by_test: Option<(u64, u64)>,
    /// What the worker's other models give back each time the card is
    /// given back across them, as a test has it.
    #[cfg(test)]
    elsewhere_by_test: u64,
    /// How many times the worker gave the card back, for a test.
    #[cfg(test)]
    gave_back_by_test: usize,
    /// What the open frame's fills let go of the card, as a test has
    /// it for a worker with no device.
    #[cfg(test)]
    fills_release_by_test: Option<engine::FillsReleaseByTest>,
    /// A card release was still set on the open frame's models between
    /// two jobs, for a test.
    #[cfg(test)]
    release_left_by_test: bool,
    /// The models' store, and the mask cache beside it, the open
    /// frame's masks are read from and made with, as a test has them.
    #[cfg(test)]
    models_by_test: Option<greycard_ai::Store>,
}

pub type Deliver = Arc<dyn Fn(Outcome) + Send + Sync>;

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
pub type ThumbCaps = Arc<Mutex<Option<(u64, u64)>>>;

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
    /// The thumbnails' pool and the reduced develops, none for a
    /// worker with no background side (headless export).
    background: Option<Arc<Background>>,
    /// The local previews the pool makes behind the thumbnails, in
    /// the same cache.
    previews: Arc<crate::previews::Previews>,
    /// Which frames show their edit, and the pictures made from it, in
    /// the same cache (`edited`).
    edited: Arc<crate::edited_pictures::Edited>,
}

/// The worker's background side: the thumbnails' own threads, shared
/// with the worker's thread, which holds them back while a develop
/// runs, and the reduced develops of the edit's pictures they run
/// (`reduced`). A window's worker has one; headless export's has none,
/// so it starts no thread it would leave idle.
pub struct Background {
    pool: Arc<crate::thumbpool::Pool>,
    reducer: Arc<crate::reduced::Reducer>,
}

impl crate::walk::Hold for Background {
    /// While a develop runs, the pool begins no thumbnail
    /// ([`engine::during_develop`]); after it, all its threads may.
    fn hold(&self, on: bool) {
        let threads = self.pool.threads();
        self.pool.set_limit(if on {
            engine::during_develop(threads)
        } else {
            threads
        });
    }
}

/// A save of the open frame's edit, waiting on the worker for the
/// develop it is the picture of (`Job::Keep`).
struct Keep {
    frame: Frame,
    edit: Edit,
    turn: u8,
    iso: crate::edited_pictures::Iso,
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
    fn of(edit: &Edit, turn: u8, outcome: &Outcome, base: Option<&FrameKept>) -> Option<Self> {
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
    shown: Option<&'a ShownTexture>,
    base: Option<&'a mut FrameKept>,
    gpu: Option<&'a greycard_gpu::Context>,
    /// The lens database the frame's list reads.
    lenses: Option<&'a greycard_lens::Database>,
}

/// A frame developed once on the CPU, as an export of a frame that is
/// not open develops it, and finished as often as asked: the camera
/// match's develop, which finishes each frame at two exposures.
/// Everything the walk does before the finish (the lens, the retouch,
/// the detail, the sharpen) is in `image`, with what the finish reads
/// beside it made under the develop's edit; the finish under another
/// edit of the same develop is [`FrameDevelop::finish`].
pub struct FrameDevelop {
    image: Arc<WorkingImage>,
    input: crate::finish::FinishInput,
    /// What of the develop's edit was left out, said with each finish.
    left_out: Vec<String>,
}

impl FrameDevelop {
    /// Open the raw at `path` and walk it under `edit` through the
    /// export's list to the finish, with the lens database for the lens
    /// corrections: the export's own develop, with no GPU and no
    /// learned denoiser's cache.
    pub fn develop(
        path: &std::path::Path,
        edit: &Edit,
        lenses: Option<&greycard_lens::Database>,
    ) -> Result<FrameDevelop, String> {
        let (input, _) = timed_open(path).map_err(|e| format!("{e:#}"))?;
        if !matches!(input, Input::Raw(_)) {
            return Err("not a raw".into());
        }
        let mut ai = Ai::new();
        let deliver: Deliver = Arc::new(|_| {});
        let (image, input, left_out) = walk_for_export(
            input.walked(),
            edit,
            0,
            &mut WorkerModels::new(&mut ai, None, &deliver, 0),
            lenses,
            &greycard_render::export::Settings::default(),
            &mut crate::walk::Kept::new(),
            &mut None,
        )?;
        Ok(FrameDevelop {
            image,
            input,
            left_out,
        })
    }

    /// A picture standing in for a develop, with no base: for a test
    /// of the finish.
    #[cfg(any(test, feature = "testing"))]
    pub fn of_image(image: WorkingImage) -> FrameDevelop {
        let source = (image.width as u32, image.height as u32);
        FrameDevelop {
            image: Arc::new(image),
            input: crate::finish::FinishInput {
                source,
                full: source,
                guide: None,
                planes: None,
                rasters: Default::default(),
                white: None,
                clip_level: f32::INFINITY,
                kind: Default::default(),
            },
            left_out: Vec::new(),
        }
    }

    /// What the finish is handed: a raw's develop and its body's
    /// baseline, as the export finishes it.
    pub fn source(&self) -> greycard_render::finish::Source {
        self.input.kind
    }

    /// The developed picture finished under `edit` (whose develop is
    /// the one this was made under) and `settings`, as an export
    /// finishes it.
    pub fn finish(
        &mut self,
        edit: &Edit,
        settings: &greycard_render::export::Settings,
    ) -> greycard_render::export::Rendered {
        for item in &self.left_out {
            tracing::warn!("{item}");
        }
        crate::finish::render(&self.image, edit, &self.input, settings)
    }

    /// What the finish would be handed for `edit` and `settings`,
    /// before any pixel goes through it ([`greycard_render::export::Scene`]).
    pub fn scene(
        &self,
        edit: &Edit,
        settings: &greycard_render::export::Settings,
    ) -> greycard_render::export::Scene {
        let framed;
        let (w, h) = (self.image.width as f32, self.image.height as f32);
        let image: &WorkingImage = if edit.geometry.is_identity_on(w, h) {
            &self.image
        } else {
            framed = greycard_render::geometry::apply(&self.image, &edit.geometry);
            &framed
        };
        greycard_render::export::scene_framed(
            image,
            edit,
            self.input.source,
            settings,
            self.input.guide.as_deref(),
        )
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
            Job::Open { generation, .. }
            | Job::Develop { generation, .. }
            | Job::DevelopAside { generation, .. }
            | Job::Fill { generation } => Blame::Develop(*generation),
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
            | Job::MaskPicture { .. }
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

/// Whether a panic's message is wgpu's for a lost device: what it
/// panics with when a wait, or a buffer it maps for itself, meets one.
pub fn says_lost(message: &str) -> bool {
    message.contains("device is lost")
}

/// A panic's message, the GPU crate's.
pub use greycard_gpu::panic_message;

pub use crate::stages::quarters;

/// The open file: a raw to develop, or a picture already in the
/// working space (its image taken out of its record, so the base
/// shares it rather than copying it).
#[derive(Clone)]
enum Input {
    Raw(Arc<RawFrame>),
    Picture {
        image: Arc<WorkingImage>,
        meta: Arc<Picture>,
    },
}

impl Input {
    /// The file as a walk takes it, sharing its picture.
    fn walked(&self) -> crate::walk::Input {
        match self {
            Input::Raw(f) => crate::walk::Input::Raw(f.clone()),
            Input::Picture { image, meta } => crate::walk::Input::Picture {
                image: image.clone(),
                meta: meta.clone(),
            },
        }
    }

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
pub enum DetailRan {
    Cpu(f64),
    Gpu,
    Kept,
}

#[cfg(test)]
mod reduced_check;
