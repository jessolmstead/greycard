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
use greycard_core::develop::local_contrast::{
    LocalContrastOptions, LocalContrastStats, local_contrast,
};
use greycard_core::develop::sharpen::{SharpenStats, sharpen_with_mask};
use greycard_core::develop::{
    CaCorrector, DemosaicMethod, DevelopSettings, demosaic_prepared, develop, develop_with, finish,
    prepare_with,
};
use greycard_core::image::WorkingImage;
use greycard_core::picture::{Picture, decode_picture_path, is_picture_path};
use greycard_core::raw::CfaPattern;
use greycard_core::raw::{Shot, ShotSummary};
use greycard_library::thumbs::{Tag, Thumb, Thumbs};

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

pub enum Job {
    Open {
        path: PathBuf,
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
    },
    Develop {
        edit: Edit,
        generation: u64,
        turn: u8,
    },
    Thumbnail {
        index: usize,
        path: PathBuf,
    },
    /// A file's thumbnail from the cache alone, under the key its row
    /// in the index gives (the content hash and the mtime stamp): for
    /// a frame whose file cannot be read, its root being offline. A
    /// miss is a `NoThumbnail`, and nothing is made.
    CachedThumbnail {
        index: usize,
        path: PathBuf,
        hash: String,
        stamp: u64,
    },
    /// Write the open frame under `edit` to `path`, doing what
    /// `on_exists` says if a file of that name is there already.
    Export {
        edit: Edit,
        path: PathBuf,
        settings: crate::export::Settings,
        on_exists: crate::export::OnExists,
        /// The frame on screen when it was asked for, and the export
        /// preset the sheet was then: handed back with `Exported`.
        source: PathBuf,
        preset: Option<String>,
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
    /// The frame at `path` was saved under `edit` at `turn`: when it is
    /// the open frame and its develop is in hand, the edit's pictures
    /// are made from it and kept (`edited`); when the develop is still
    /// to land, they are made when it does, or when the frame is left
    /// with it in hand. Answered with `EditedKept` once the frame's
    /// pictures may have changed.
    Keep {
        path: PathBuf,
        edit: Edit,
        turn: u8,
        /// The frame's ISO from its row, which the learned blend is
        /// judged against in saying whether this is an edit at all.
        iso: crate::edited::Iso,
    },
    /// Fetch a model into the store; on its own thread.
    Fetch {
        model: &'static greycard_ai::Model,
    },
    /// Fetch the lens database into its store; on its own thread.
    FetchLenses,
    /// The worker's own: a device arrived and nothing else is queued.
    Gpu,
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
}

/// What the fill model did for a develop's Fill patches, by name.
/// Nothing at all when every fill was kept from an earlier develop.
#[derive(Debug, Clone, Default)]
pub struct FillReport {
    /// Patches made up this develop, and the seconds they took together.
    pub made: Vec<String>,
    pub seconds: f64,
    /// Patches left as they were: the model is not in the store.
    pub missing: Vec<String>,
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
}

impl WhiteBase {
    /// No gains and no matrix: a picture already in the working space.
    pub const IDENTITY: Self = Self {
        gains: [1.0; 3],
        matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        clip: f32::INFINITY,
    };

    /// The white balance `wb`, with channels clipping at `clip`.
    pub fn from(wb: &greycard_core::WhiteBalance, clip: f32) -> Self {
        let cols = wb.matrix_f32();
        Self {
            gains: wb.coefficients_f32(),
            matrix: std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r])),
            clip,
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
        /// The tone equalizer's plane for this develop, for the
        /// viewport's own texture.
        guide: Arc<crate::finish::Guide>,
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
    /// The fill model is at work on the patch named, for a develop
    /// still on its way.
    Filling {
        generation: u64,
        name: String,
    },
    /// A file's thumbnail, with the path it was made from: the list
    /// may have changed under it, so the index alone is not enough.
    /// `size` is the long edge it was asked for, which the grid uses
    /// to tell a picture it has outgrown from one it has not.
    /// `cached` when it came from the thumbnail cache, and `seconds`
    /// what the worker spent on it, for the folder's account in the
    /// log.
    Thumbnail {
        index: usize,
        path: PathBuf,
        size: u32,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        cached: bool,
        seconds: f64,
    },
    /// A file no thumbnail could be made of, so the folder's count
    /// still closes.
    NoThumbnail {
        index: usize,
        path: PathBuf,
    },
    /// Thumbnails and `NoThumbnail`s waiting in the pool's outbox, for
    /// a worker made with [`Worker::batched`]: one of these for as
    /// many as the threads finish before the window takes them in.
    Thumbnails(crate::thumbpool::Batch),
    Failed {
        generation: u64,
        message: String,
    },
    Exported {
        path: PathBuf,
        seconds: f64,
        /// What the edit names and the file was written without
        /// (`left_out_of`, `finish_export`).
        left_out: Vec<String>,
        /// What the policy made of a file of that name being there
        /// already; none when nothing was.
        note: Option<String>,
        /// The frame, the edit the file was written under and the
        /// export preset, as the job had them: what the frame's
        /// history records.
        source: PathBuf,
        edit: Edit,
        preset: Option<String>,
    },
    /// Nothing written: a file of that name was there and the policy
    /// says to leave it.
    ExportSkipped {
        path: PathBuf,
    },
    ExportFailed {
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
    Fetching {
        name: &'static str,
        done: u64,
        total: u64,
    },
    Fetched {
        model: &'static greycard_ai::Model,
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
    /// The pictures `path` shows in the strip, the grid and the loupe
    /// may have changed: its edit's were kept, or its edit was reset or
    /// left without them. Its cells ask for their pictures again.
    EditedKept {
        path: PathBuf,
    },
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
    /// The newest open or develop; an older one still queued is dropped.
    develop: Option<Job>,
    /// Masks wanted, the newest shape for a key replacing an older,
    /// with the model chosen for a learned shape that needs one
    /// picked (Subject; `None` for a shape whose model is not in
    /// question, Object among them, which always wants SAM).
    masks: std::collections::VecDeque<(Key, Shape, Option<&'static greycard_ai::Model>)>,
    /// The people on the open picture are wanted (`Job::People`).
    people: bool,
    /// The saves of the open frame (`Job::Keep`), the newest a frame,
    /// taken ahead of a develop or an open queued after them, so a frame
    /// left at once still has its picture in hand when its save is seen.
    keeps: std::collections::VecDeque<Job>,
    exports: std::collections::VecDeque<Job>,
    /// A Subject model arrived since the one loaded, if any: drop it,
    /// so the next Subject mask picks up the new file rather than the
    /// one already running.
    forget_subject: bool,
    /// The editor is leaving: finish what is in hand and stop, so the
    /// engine's GPU context is dropped on this thread rather than
    /// under a process that is already on its way out.
    stopping: bool,
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
}

impl Worker {
    /// A worker whose thumbnails come to `deliver` one call each: the
    /// tests', which read them off a channel.
    #[cfg(test)]
    pub fn new(deliver: impl Fn(Outcome) + Send + Sync + 'static) -> Self {
        Self::build(Arc::new(deliver), false)
    }

    /// A worker whose thumbnails come to `deliver` in batches, as
    /// [`Outcome::Thumbnails`]: the window's, where a call is a turn
    /// of the event loop and a warm folder of thousands would take
    /// every one before drawing.
    pub fn batched(deliver: impl Fn(Outcome) + Send + Sync + 'static) -> Self {
        Self::build(Arc::new(deliver), true)
    }

    fn build(deliver: Deliver, batched: bool) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let thumbs: ThumbCache = Arc::new(Mutex::new(None));
        let edited = Arc::new(crate::edited::Edited::new(thumbs.clone()));
        let previews =
            Arc::new(crate::previews::Previews::new(thumbs.clone()).with_edited(edited.clone()));
        let make: crate::thumbpool::Make = {
            let (thumbs, previews, edited) = (thumbs.clone(), previews.clone(), edited.clone());
            Arc::new(move |path, size| {
                cached_thumbnail_noting(
                    &thumbs,
                    Some(&previews),
                    Some(&edited),
                    path,
                    size,
                    thumbnail,
                )
            })
        };
        let lookup: crate::thumbpool::Lookup = {
            let (thumbs, previews, edited) = (thumbs.clone(), previews.clone(), edited.clone());
            Arc::new(move |path, size| {
                thumb_lookup(&thumbs, path, size, Some(&previews), Some(&edited)).2
            })
        };
        let hooks = crate::thumbpool::PreviewHooks {
            owes: {
                let previews = previews.clone();
                Arc::new(move |path| previews.owes(path))
            },
            make: {
                let previews = previews.clone();
                Arc::new(move |path| previews.make(path).map(drop))
            },
        };
        let pool = crate::thumbpool::Pool::with_lookup(
            crate::thumbpool::default_threads(),
            lookup,
            make,
            deliver.clone(),
        )
        .with_previews(hooks);
        let pool = Arc::new(if batched { pool.batched() } else { pool });
        let (q, d, p, e) = (queue.clone(), deliver.clone(), pool.clone(), edited.clone());
        let thread = std::thread::Builder::new()
            .name("greycard worker".into())
            .spawn(move || run(q, d, p, e))
            .expect("spawning the worker");
        Self {
            queue,
            deliver,
            thread: Mutex::new(Some(thread)),
            thumbs,
            caps: Arc::new(Mutex::new(None)),
            pool,
            previews,
            edited,
        }
    }

    /// Which frames show their edit: the window says which it holds as
    /// edited, and the culling loupe looks their pictures up.
    pub(crate) fn edited(&self) -> Arc<crate::edited::Edited> {
        self.edited.clone()
    }

    /// The local previews, for the culling loupe's decode threads,
    /// which read them for a frame out of reach and make them from a
    /// picture they decoded anyway.
    pub(crate) fn previews(&self) -> Arc<crate::previews::Previews> {
        self.previews.clone()
    }

    /// The roots whose frames get a local preview from now on.
    pub(crate) fn set_preview_roots(&self, roots: Vec<PathBuf>) {
        self.previews.set_roots(roots);
    }

    /// Look thumbnails up in `cache`, and keep the ones made, from
    /// the next thumbnail on.
    /// The cache's one walk to count what it holds starts at once, on
    /// a thread of its own, so neither the window nor a thumbnail waits
    /// on it.
    pub fn set_thumb_cache(&self, cache: Option<Thumbs>) {
        *self.caps.lock().expect("thumbnail caps") =
            cache.as_ref().map(|c| (c.cap(), c.preview_cap()));
        *self.thumbs.lock().expect("thumbnail cache") = cache;
        count_thumb_cache(&self.thumbs, |_| {});
    }

    /// The thumbnail cache, for the settings sheet.
    pub fn thumb_cache(&self) -> ThumbCache {
        self.thumbs.clone()
    }

    /// The cache's caps in bytes, thumbnails' then previews', as last
    /// set, or `None` with no cache. Never waits on the cache.
    pub(crate) fn thumb_caps(&self) -> Option<(u64, u64)> {
        *self.caps.lock().expect("thumbnail caps")
    }

    /// The caps as last set, for a thread that applies them: one
    /// that waited on the cache's lock applies the newest, not the one
    /// it was started for.
    pub(crate) fn thumb_caps_shared(&self) -> ThumbCaps {
        self.caps.clone()
    }

    /// Record a new thumbnails' cap for the sheet to show, ahead of the
    /// cache taking it on a thread of its own.
    pub(crate) fn note_thumbs_cap(&self, bytes: u64) {
        if let Some(caps) = self.caps.lock().expect("thumbnail caps").as_mut() {
            caps.0 = bytes;
        }
    }

    /// The same for the previews' cap.
    pub(crate) fn note_previews_cap(&self, bytes: u64) {
        if let Some(caps) = self.caps.lock().expect("thumbnail caps").as_mut() {
            caps.1 = bytes;
        }
    }

    /// Stop the worker and wait for it to put its GPU buffers down.
    ///
    /// A process that exits while the worker is inside the driver
    /// dies there rather than at its own hand: the buffers of a
    /// develop half way through go with a device the window has
    /// already torn down. So the editor asks the worker to stop as
    /// its window closes and waits [`LEAVING`] for it to say it has.
    ///
    /// True when it has, its thread joined. False when it is still in
    /// a job after that: its thread is kept, so [`Worker::running`]
    /// and [`Worker::finished`] still say so, and the caller must not
    /// let the process go through the C library's exit with it there.
    /// The exit handlers tear the GPU driver down under a worker that
    /// may be waiting on a fence in it, and the process dies there
    /// (`startup::main` leaves by `_exit` instead).
    pub fn stop(&self) -> bool {
        self.stop_within(LEAVING)
    }

    /// [`Worker::stop`], waiting `limit` rather than [`LEAVING`].
    fn stop_within(&self, limit: std::time::Duration) -> bool {
        self.pool.stop();
        {
            let (lock, cv) = &*self.queue;
            lock.lock().expect("worker queue").stopping = true;
            cv.notify_all();
        }
        let Some(thread) = self.thread.lock().expect("worker thread").take() else {
            return true;
        };
        let asked = std::time::Instant::now();
        while !thread.is_finished() && asked.elapsed() < limit {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if thread.is_finished() {
            let _ = thread.join();
            tracing::debug!("worker: stopped in {:.0} ms", asked.elapsed().as_millis());
            true
        } else {
            tracing::warn!(
                "worker: still busy after {:.1} s; leaving it",
                limit.as_secs_f64()
            );
            *self.thread.lock().expect("worker thread") = Some(thread);
            false
        }
    }

    /// Whether the worker's thread has ended: joined by
    /// [`Worker::stop`], or ended since and joined here. False while
    /// it is still in the job a stop left it in, or has not been asked
    /// to stop.
    pub fn finished(&self) -> bool {
        let mut held = self.thread.lock().expect("worker thread");
        match held.take_if(|t| t.is_finished()) {
            Some(thread) => {
                let _ = thread.join();
                true
            }
            None => held.is_none(),
        }
    }

    /// Whether the worker's thread is still there to take a job: a
    /// panic outside any job's guard ends it, and whoever waits on an
    /// outcome would otherwise wait for ever.
    pub fn running(&self) -> bool {
        self.thread
            .lock()
            .expect("worker thread")
            .as_ref()
            .is_some_and(|t| !t.is_finished())
    }

    pub fn send(&self, job: Job) {
        if let Job::Fetch { model } = job {
            let deliver = self.deliver.clone();
            std::thread::Builder::new()
                .name("greycard fetch".into())
                .spawn(move || {
                    let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        fetch(model, &*deliver)
                    }));
                    if let Err(payload) = held {
                        deliver(Outcome::FetchFailed {
                            id: model.id,
                            name: model.name,
                            message: format!(
                                "the fetch panicked: {}",
                                panic_message(payload.as_ref())
                            ),
                        });
                    }
                })
                .expect("spawning the fetch");
            return;
        }
        if let Job::FetchLenses = job {
            let (deliver, queue) = (self.deliver.clone(), self.queue.clone());
            std::thread::Builder::new()
                .name("greycard lens fetch".into())
                .spawn(move || {
                    let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        fetch_lenses(&queue, &*deliver)
                    }));
                    if let Err(payload) = held {
                        deliver(Outcome::LensesFetchFailed {
                            message: format!(
                                "the fetch panicked: {}",
                                panic_message(payload.as_ref())
                            ),
                        });
                    }
                })
                .expect("spawning the lens fetch");
            return;
        }
        if let Job::Thumbnail { index, path } = job {
            self.pool.push(index, path);
            return;
        }
        if let Job::CachedThumbnail {
            index,
            path,
            hash,
            stamp,
        } = job
        {
            // Off the window's thread, since a hit reads the entry's
            // file; not through the pool, whose lookup and making both
            // start from the file, which this frame's cannot be. At
            // the size the pool makes and keeps pictures at, or the
            // entry it kept is never found.
            let (thumbs, deliver, size, edited) = (
                self.thumbs.clone(),
                self.deliver.clone(),
                crate::grid::made_size(self.pool.size()),
                self.edited.clone(),
            );
            rayon::spawn(move || {
                let started = Instant::now();
                // Caught here: a panic on rayon's pool with no handler
                // takes the editor down with it. A frame held as edited
                // shows the last picture of its edit kept, its sidecar
                // being out of reach with its file.
                let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    edited
                        .lookup(&hash, size, edited.shows_offline(&path))
                        .or_else(|| {
                            thumbs
                                .lock()
                                .expect("thumbnail cache")
                                .as_mut()
                                .and_then(|c| {
                                    c.get(
                                        &hash,
                                        size,
                                        Tag {
                                            recipe: THUMB_RECIPE,
                                            stamp,
                                        },
                                    )
                                })
                        })
                }))
                .unwrap_or_else(|payload| {
                    tracing::warn!(
                        "thumbnail {}: the cache's lookup panicked: {}",
                        path.display(),
                        panic_message(payload.as_ref())
                    );
                    None
                });
                deliver(match hit {
                    Some(thumb) => Outcome::Thumbnail {
                        index,
                        path,
                        size,
                        width: thumb.width,
                        height: thumb.height,
                        rgb: thumb.rgb,
                        cached: true,
                        seconds: started.elapsed().as_secs_f64(),
                    },
                    None => Outcome::NoThumbnail { index, path },
                });
            });
            return;
        }
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("worker queue");
        match job {
            Job::Thumbnail { .. } | Job::CachedThumbnail { .. } => {
                unreachable!("thumbnails go to the pool above")
            }
            job @ (Job::Export { .. } | Job::ExportFrame { .. }) => q.exports.push_back(job),
            Job::ExportSet { set, frames } => {
                for (index, frame) in frames.into_iter().enumerate() {
                    q.exports.push_back(Job::ExportFrame {
                        set: set.clone(),
                        index,
                        frame,
                    });
                }
            }
            Job::Mask { key, shape, model } => {
                q.masks.retain(|(k, _, _)| *k != key);
                q.masks.push_back((key, shape, model));
            }
            Job::People => q.people = true,
            Job::Keep {
                path,
                edit,
                turn,
                iso,
            } => {
                q.keeps
                    .retain(|k| !matches!(k, Job::Keep { path: p, .. } if *p == path));
                q.keeps.push_back(Job::Keep {
                    path,
                    edit,
                    turn,
                    iso,
                });
            }
            job => q.develop = Some(job),
        }
        cv.notify_one();
    }

    /// Run the engine's GPU ops on this device from now on. The
    /// context is built on the worker's thread, so the shaders compile
    /// there and not in the window's setup.
    pub fn set_gpu(&self, device: &greycard_gpu::wgpu::Device, queue: &greycard_gpu::wgpu::Queue) {
        let (lock, cv) = &*self.queue;
        lock.lock().expect("worker queue").gpu = Some((device.clone(), queue.clone()));
        cv.notify_one();
    }

    /// A Subject model just arrived in the store: the worker's loaded
    /// one, if any, is dropped, so the next Subject mask loads
    /// whichever file `step` now picks rather than the one already
    /// running.
    pub fn forget_subject(&self) {
        let (lock, cv) = &*self.queue;
        lock.lock().expect("worker queue").forget_subject = true;
        cv.notify_one();
    }

    /// Make the thumbnails for `first..=last` before the folder's
    /// others, the rest outward from that range. The strip says what
    /// it shows; a folder of hundreds is otherwise decoded in file
    /// order, and the frames on screen wait for every earlier one.
    pub fn want_thumbnails(&self, first: usize, last: usize) {
        self.pool.want(first, last);
    }

    /// Another folder's list: the thumbnails still waiting are
    /// dropped, and one in hand is not delivered. Its numbering is
    /// the old list's, and a late picture must not land on the new.
    pub fn forget_thumbnails(&self) {
        self.pool.forget();
    }

    /// How many threads make thumbnails.
    pub fn thumb_threads(&self) -> usize {
        self.pool.threads()
    }

    /// Hold the thumbnails' threads for a develop the window is
    /// about to ask for, or let them go: see `thumbpool::Pool::hold`.
    pub fn hold_thumbnails(&self, on: bool) {
        self.pool.hold(on);
    }

    /// Put these thumbnails in the queue in place of the ones there:
    /// a new list, or the same list renumbered by a merge, whose
    /// queued jobs name files by numbers that now mean others. Those
    /// would be made and thrown away on arrival (a delivery checks the
    /// path), and a frame whose job was among them would never get its
    /// picture, which is how a merge blanked the grid.
    /// `wanted`, the files on screen, are made first, as the grid's
    /// range would have them.
    pub fn replace_thumbnails(&self, list: Vec<(usize, PathBuf)>, wanted: Option<(usize, usize)>) {
        self.pool.forget();
        self.pool.push_all(list);
        if let Some((first, last)) = wanted {
            self.pool.want(first, last);
        }
    }

    /// Make thumbnails at this long edge from now on. The grid's
    /// cells grow past the strip's 178, and a picture made for the
    /// strip is mush in a large one; the size follows the cell both
    /// ways, so a cell that has shrunk stops paying for the one
    /// before it. Whether a picture already made is made again is
    /// the caller's business, and that only ever steps up.
    pub fn set_thumb_size(&self, size: u32) {
        self.pool.set_size(size);
    }
}

/// The frames on the strip first, in file order, then the rest
/// outward from the middle of that range: what a scroll asks for
/// next is whichever end it is heading towards.
pub(crate) fn order_thumbnails(pending: &mut [(usize, PathBuf)], first: usize, last: usize) {
    // Ties broken by the frame's number, so two the same distance
    // from the center come in the list's order whichever thread
    // queued them first.
    let center = (first + last) / 2;
    pending.sort_by_key(|(i, _)| {
        let away = if (first..=last).contains(i) {
            0
        } else {
            i.abs_diff(center)
        };
        (away, *i)
    });
}

/// Fetch a model, reporting as it comes.
fn fetch(model: &'static greycard_ai::Model, deliver: &dyn Fn(Outcome)) {
    let name = model.name;
    let store = match greycard_ai::Store::user() {
        Ok(s) => s,
        Err(e) => {
            deliver(Outcome::FetchFailed {
                id: model.id,
                name,
                message: e.to_string(),
            });
            return;
        }
    };
    let mut last = Instant::now();
    let result = store.fetch(model, |p| {
        // A report every so often, and the first and the last.
        if p.done == 0 || p.done == p.total || last.elapsed().as_millis() > 200 {
            last = Instant::now();
            deliver(Outcome::Fetching {
                name,
                done: p.done,
                total: p.total,
            });
        }
    });
    deliver(match result {
        Ok(()) => Outcome::Fetched { model },
        Err(e) => Outcome::FetchFailed {
            id: model.id,
            name,
            message: e.to_string(),
        },
    });
}

/// Fetch the lens database, reporting as it comes, and hand it to the
/// worker.
fn fetch_lenses(queue: &Arc<(Mutex<Queue>, Condvar)>, deliver: &dyn Fn(Outcome)) {
    let store = match greycard_lens::Store::user() {
        Ok(s) => s,
        Err(e) => {
            deliver(Outcome::LensesFetchFailed {
                message: e.to_string(),
            });
            return;
        }
    };
    let mut last = Instant::now();
    let result = store.fetch(|p| {
        if p.done == 0 || p.done == p.total || last.elapsed().as_millis() > 200 {
            last = Instant::now();
            deliver(Outcome::LensesFetching {
                done: p.done,
                total: p.total,
            });
        }
    });
    let loaded =
        result.and_then(|()| greycard_lens::Database::load_dir(&store.dir()).map(Arc::new));
    match loaded {
        Ok(db) => {
            let (lock, cv) = &**queue;
            lock.lock().expect("worker queue").lenses = Some(db);
            cv.notify_one();
            deliver(Outcome::LensesFetched);
        }
        Err(e) => deliver(Outcome::LensesFetchFailed {
            message: e.to_string(),
        }),
    }
}

/// The last develop, kept for an export under the same develop and the
/// same orientation.
struct Last {
    edit: Edit,
    turn: u8,
    image: Arc<WorkingImage>,
    /// Whether the base this picture came from had its CA corrected on the GPU.
    /// The export's picture must be from the CPU, not the GPU.
    ca_on_gpu: bool,
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
            ca_on_gpu: base.ca_on_gpu,
            left_out: left_out_of(outcome, Some(base)),
        }
    }
}

/// What a develop left out of what its edit asked for, in words for a
/// log line: a learned denoiser or a fill whose model is not in the
/// store, or that could not run. An export writes the picture without
/// them, as the window shows it, and says so. `base` is the base the
/// develop ran on: one kept from an earlier develop reports `Kept`,
/// and the stand-in it may be is said from what it carries.
fn left_out_of(outcome: &Outcome, base: Option<&Base>) -> Vec<String> {
    let Outcome::Developed { learned, fills, .. } = outcome else {
        return Vec::new();
    };
    let learned = match (learned, base.and_then(|b| b.stand_in.as_ref())) {
        (LearnedReport::Kept, Some(stood_in)) => stood_in,
        (report, _) => report,
    };
    let mut out = Vec::new();
    match learned {
        LearnedReport::Missing(model) => out.push(format!(
            "the learned denoiser: {} is not downloaded; the engine's denoise instead",
            model.name
        )),
        LearnedReport::Failed(why) => out.push(format!(
            "the learned denoiser failed ({why}); the engine's denoise instead"
        )),
        _ => {}
    }
    for name in &fills.missing {
        out.push(format!(
            "fill {name}: {} is not downloaded; left as it was",
            greycard_ai::FILL.name
        ));
    }
    for (name, why) in &fills.failed {
        out.push(format!(
            "fill {name}: the fill model failed ({why}); left as it was"
        ));
    }
    out
}

fn run(
    queue: Arc<(Mutex<Queue>, Condvar)>,
    deliver: Deliver,
    pool: Arc<crate::thumbpool::Pool>,
    edited: Arc<crate::edited::Edited>,
) {
    let (lock, cv) = &*queue;
    let mut ai = Ai::new();
    match greycard_ai::Store::user() {
        Ok(store) => tracing::info!("model store {}", store.root().display()),
        Err(e) => tracing::warn!("no model store: {e}"),
    }
    let mut lenses: Option<Arc<greycard_lens::Database>> = greycard_lens::Store::user()
        .ok()
        .and_then(|s| s.load())
        .map(|(db, _)| Arc::new(db));
    let cache = greycard_ai::DenoiseCache::user()
        .inspect_err(|e| tracing::warn!("no denoise cache: {e}"))
        .ok();
    let mut input: Option<Input> = None;
    // The open file's path, for the export's account of its source.
    let mut opened_path: Option<PathBuf> = None;
    // What the file said about itself, for the export's EXIF.
    let mut metadata: Option<Arc<greycard_core::decode::RawMetadata>> = None;
    // The open file's quarter turns, from the job that opened it and
    // from every develop after.
    let mut turn: u8 = 0;
    // The last develop, kept for an export under the same settings
    // and the same turn.
    let mut last: Option<Last> = None;
    // The last develop before its dehaze and sharpen, so a change to
    // either costs only those.
    let mut base: Option<Base> = None;
    // The learned denoiser's answer, kept beside the base so its
    // strength is a blend and not another run of the network.
    let mut learned: Option<LearnedBase> = None;
    // The engine's GPU ops, once the window has a device to give.
    let mut gpu: Option<greycard_gpu::Context> = None;
    // The learned models for a set's frames other than the open one,
    // made on the first such frame and dropped with the set: what the
    // open file's own `ai` holds (its masks, its fills) is for it, and
    // a set run past it must not take that away.
    let mut set_ai: Option<Ai> = None;
    // A part's people were found and the part is not made yet: a
    // person is being picked, and the People model is wanted on the
    // click, whatever the edit says meanwhile.
    let mut picking = false;
    // The viewport's develop as a texture on the device, for the edit's
    // pictures to be read back from; none while a develop runs.
    let mut shown: Option<ShownTexture> = None;
    // A save of the open frame whose develop is still to land.
    let mut pending: Option<Keep> = None;
    loop {
        let mut device = None;
        let job = {
            let mut q = lock.lock().expect("worker queue");
            loop {
                // The editor is leaving: the engine's GPU context and
                // everything it holds are dropped here, on the thread
                // that made them, before the process goes.
                if q.stopping {
                    return;
                }
                if let Some(db) = q.lenses.take() {
                    lenses = Some(db);
                    edited.set_lenses(crate::edited::lens_database());
                    discard_base(&mut base, gpu.as_ref());
                    learned = None;
                }
                if let Some(d) = q.gpu.take() {
                    device = Some(d);
                }
                if q.forget_subject {
                    q.forget_subject = false;
                    ai.forget_subject();
                }
                if let Some(job) = q.keeps.pop_front() {
                    break job;
                }
                if let Some(job) = q.develop.take() {
                    break job;
                }
                if q.people {
                    q.people = false;
                    break Job::People;
                }
                if let Some((key, shape, model)) = q.masks.pop_front() {
                    break Job::Mask { key, shape, model };
                }
                if let Some(job) = q.exports.pop_front() {
                    break job;
                }
                if device.is_some() {
                    // Nothing else to do: build the context now, off
                    // the lock, and wait again.
                    break Job::Gpu;
                }
                q = cv.wait(q).expect("worker queue");
            }
        };
        if let Some((d, q)) = device.take() {
            match greycard_gpu::Context::from_device(&d, &q) {
                Ok(ctx) => {
                    let planes = if ctx.keeps_local_contrast_planes() {
                        "kept"
                    } else {
                        "made per run"
                    };
                    tracing::info!(
                        "engine ops on the GPU: {}, the local contrast's planes {planes}",
                        ctx.name()
                    );
                    gpu = Some(ctx);
                }
                Err(e) => tracing::warn!("engine ops stay on the CPU: {e}"),
            }
        }
        if matches!(job, Job::Gpu) {
            continue;
        }
        // A save of the open frame: its edit's pictures from the develop
        // in hand, or held for the develop still to land.
        if let Job::Keep {
            path,
            edit,
            turn,
            iso,
        } = job
        {
            let hand = InHand {
                opened: opened_path.as_deref(),
                last: last.as_ref(),
                shown: shown.as_ref(),
                base: base.as_ref(),
                gpu: gpu.as_ref(),
            };
            let keep = Keep {
                path,
                edit,
                turn,
                iso,
            };
            let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                keep_open(keep, false, hand, &mut ai, &edited, &deliver)
            }));
            pending = held.unwrap_or_else(|payload| {
                tracing::warn!(
                    "the edit's pictures: the worker panicked: {}",
                    panic_message(payload.as_ref())
                );
                None
            });
            continue;
        }
        // The frame is about to be left, or its picture replaced: a save
        // still waiting on it is made from what is in hand now, or let go.
        if let Some(keep) = pending.take()
            && matches!(job, Job::Open { .. } | Job::Develop { .. })
        {
            let leaving = match &job {
                Job::Open { .. } => true,
                Job::Develop { edit, turn, .. } => {
                    !(edit.same_develop(&keep.edit) && *turn % 4 == keep.turn)
                }
                _ => false,
            };
            let hand = InHand {
                opened: opened_path.as_deref(),
                last: last.as_ref(),
                shown: shown.as_ref(),
                base: base.as_ref(),
                gpu: gpu.as_ref(),
            };
            pending = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                keep_open(keep, leaving, hand, &mut ai, &edited, &deliver)
            }))
            .unwrap_or(None);
        }
        // The texture goes before a develop makes the next, so the two are
        // never held at once.
        if matches!(job, Job::Open { .. } | Job::Develop { .. }) {
            shown = None;
        }
        // A frame of a set: written, counted, and the set said to be
        // done after its last; a panic in it is that frame's failure.
        if let Job::ExportFrame { set, index, frame } = job {
            let mut panicked = false;
            // The edit the file was written under: the frame's, its
            // learned blend seeded from the ISO when it was still to
            // be, as a first open would have seeded it.
            let mut rendered = frame.edit.clone();
            let (done, finished) = crate::queue::step(&set, index, &frame.source, || {
                // The name the policy chose, which the status line says:
                // the one written, not the one asked for.
                let resolved = crate::queue::resolve(&set, &frame);
                deliver(Outcome::SetFrameStarted {
                    set: set.clone(),
                    index,
                    name: file_label(resolved.path().unwrap_or(&frame.out)),
                });
                let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let note = resolved.note();
                    let Some(path) = resolved.path().map(std::path::Path::to_path_buf) else {
                        return crate::queue::Done::Skipped {
                            path: frame.out.clone(),
                        };
                    };
                    // A subfolder is made as its first frame goes in; a
                    // chosen folder is there, and one that is not (a
                    // drive gone) is never made on the disk under it.
                    if set.sub.is_some()
                        && let Some(dir) = path.parent()
                        && let Err(e) = std::fs::create_dir_all(dir)
                    {
                        return crate::queue::Done::Failed {
                            message: format!("making {}: {e}", dir.display()),
                        };
                    }
                    let start = Instant::now();
                    // The frame on screen is the open file: its picture
                    // is the last develop's when that was of this edit.
                    // Any other is opened and developed on the side,
                    // and the open file's state is left as it was.
                    let written = if opened_path.as_deref() == Some(frame.source.as_path()) {
                        let edit = seeded(&frame.edit, frame.seed_blend, input.as_ref());
                        rendered = edit.clone();
                        open_picture(
                            &edit,
                            frame.turn % 4,
                            input.as_ref(),
                            &mut last,
                            &mut base,
                            &mut learned,
                            &mut ai,
                            cache.as_ref(),
                            lenses.as_deref(),
                            &deliver,
                        )
                        .and_then(|(image, mut left_out)| {
                            left_out.extend(write_export(
                                image,
                                &edit,
                                base.as_ref(),
                                &mut ai,
                                opened_path.as_deref(),
                                metadata.as_deref(),
                                &set.settings,
                                &path,
                            )?);
                            Ok(left_out)
                        })
                    } else {
                        // The editor's People model, lent for the
                        // frame rather than a second one loaded.
                        let other = set_ai.get_or_insert_with(Ai::new);
                        ai.lend_parts(other);
                        let written = export_other(
                            &frame,
                            other,
                            cache.as_ref(),
                            lenses.as_deref(),
                            &deliver,
                            &set.settings,
                            &path,
                        );
                        other.lend_parts(&mut ai);
                        written.map(|(edit, left_out)| {
                            rendered = edit;
                            left_out
                        })
                    };
                    match written {
                        Ok(left_out) => crate::queue::Done::Exported {
                            path,
                            seconds: start.elapsed().as_secs_f64(),
                            note,
                            left_out,
                        },
                        Err(message) => crate::queue::Done::Failed { message },
                    }
                }));
                held.unwrap_or_else(|payload| {
                    panicked = true;
                    crate::queue::Done::Failed {
                        message: format!(
                            "the worker panicked: {}",
                            panic_message(payload.as_ref())
                        ),
                    }
                })
            });
            if panicked {
                discard_base(&mut base, gpu.as_ref());
                learned = None;
                last = None;
                ai.forget(opened_path.clone());
                set_ai = None;
            }
            deliver(Outcome::SetFrameDone {
                set: set.clone(),
                index,
                source: frame.source,
                edit: rendered,
                done,
            });
            if finished {
                // The set's own models go with it.
                set_ai = None;
                let tally = set.tally();
                deliver(Outcome::SetDone { set, tally });
            }
            continue;
        }
        // A develop has the machine: the thumbnails' threads are held
        // to what `during_develop` allows until it is done.
        let developing = matches!(job, Job::Open { .. } | Job::Develop { .. });
        // A picture with no Part on it lets the People model go: it
        // holds about a gigabyte, and two with a picture's crops.
        if matches!(job, Job::Open { .. }) {
            picking = false;
        }
        if let Job::Open { edit, .. } | Job::Develop { edit, .. } = &job
            && !has_live_part(edit)
            && !picking
        {
            ai.release_parts();
        }
        if developing {
            pool.set_limit(during_develop(pool.threads()));
        }
        let blame = Blame::of(&job);
        // A panic in a job is the hook's to write, with its
        // backtrace; here it becomes the failure the UI is waiting
        // on, and the develop state is dropped rather than trusted.
        let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
            Job::Open {
                path,
                edit,
                generation,
                seed_blend,
                turn: wanted,
            } => match timed_open(&path) {
                Ok((opened, m)) => {
                    metadata = Some(Arc::new(m));
                    let blend = match &opened {
                        Input::Raw(f) if seed_blend => Some(Noise::blend_for_iso(f.shot.iso)),
                        _ => None,
                    };
                    let edit = {
                        let mut edit = edit;
                        if let Some(b) = blend {
                            edit.noise.learned_strength = b;
                        }
                        edit
                    };
                    let (as_shot, frame, profile, kind) = match &opened {
                        Input::Raw(f) => {
                            let profile = profile_from_frame(f).ok().map(Box::new);
                            let as_shot = profile
                                .as_ref()
                                .and_then(|p| as_shot_temp_tint(f, p).ok())
                                .map(|tt| (tt.cct, tt.duv))
                                .unwrap_or((5500.0, 0.0));
                            (as_shot, Some(f.clone()), profile, SourceKind::Raw)
                        }
                        Input::Picture { meta, .. } => (
                            (5500.0, 0.0),
                            None,
                            None,
                            SourceKind::Picture {
                                space: meta.space.clone(),
                                bits: meta.bits,
                            },
                        ),
                    };
                    deliver(Outcome::Opened {
                        generation,
                        as_shot,
                        frame,
                        profile,
                        lens: lens_report(lenses.as_deref(), &opened),
                        kind,
                        shot: {
                            let (make, model, shot) = opened.identity();
                            shot.summary(make, model)
                        },
                        blend,
                    });
                    discard_base(&mut base, gpu.as_ref());
                    learned = None;
                    ai.forget(Some(path.clone()));
                    // The file is the worker's before the develop, so
                    // a panic in the develop leaves the worker and the
                    // UI, which has its Opened, on the same file.
                    input = Some(opened);
                    opened_path = Some(path);
                    let opened = input.as_ref().expect("just set");
                    turn = wanted % 4;
                    let (outcome, image) = develop_job(
                        opened,
                        &edit,
                        turn,
                        generation,
                        &mut base,
                        &mut learned,
                        &mut ai,
                        cache.as_ref(),
                        lenses.as_deref(),
                        &mut gpu,
                        &deliver,
                    );
                    shown = ShownTexture::of(&edit, turn, &outcome, base.as_ref());
                    last =
                        image.and_then(|i| base.as_ref().map(|b| Last::made(edit, b, i, &outcome)));
                    deliver(outcome);
                }
                Err(e) => deliver(Outcome::Failed {
                    generation,
                    message: e.to_string(),
                }),
            },
            Job::Develop {
                edit,
                generation,
                turn: wanted,
            } => {
                if let Some(f) = &input {
                    turn = wanted % 4;
                    let (outcome, image) = develop_job(
                        f,
                        &edit,
                        turn,
                        generation,
                        &mut base,
                        &mut learned,
                        &mut ai,
                        cache.as_ref(),
                        lenses.as_deref(),
                        &mut gpu,
                        &deliver,
                    );
                    shown = ShownTexture::of(&edit, turn, &outcome, base.as_ref());
                    last =
                        image.and_then(|i| base.as_ref().map(|b| Last::made(edit, b, i, &outcome)));
                    deliver(outcome);
                }
            }
            Job::Export {
                edit,
                path,
                settings,
                on_exists,
                source,
                preset,
            } => {
                let start = Instant::now();
                // What is already there decides the name before the
                // work is done, not after it.
                let resolved = on_exists.resolve(&path);
                let note = resolved.note();
                let Some(path) = resolved.path().map(std::path::Path::to_path_buf) else {
                    deliver(Outcome::ExportSkipped { path });
                    return;
                };
                // The turn is the open file's, which the export
                // never sets: it develops what is on the screen.
                let written = open_picture(
                    &edit,
                    turn,
                    input.as_ref(),
                    &mut last,
                    &mut base,
                    &mut learned,
                    &mut ai,
                    cache.as_ref(),
                    lenses.as_deref(),
                    &deliver,
                )
                .and_then(|(image, mut left_out)| {
                    left_out.extend(write_export(
                        image,
                        &edit,
                        base.as_ref(),
                        &mut ai,
                        opened_path.as_deref(),
                        metadata.as_deref(),
                        &settings,
                        &path,
                    )?);
                    Ok(left_out)
                });
                deliver(match written {
                    Ok(left_out) => Outcome::Exported {
                        path,
                        seconds: start.elapsed().as_secs_f64(),
                        note,
                        source,
                        edit,
                        preset,
                        left_out,
                    },
                    Err(message) => Outcome::ExportFailed { message },
                });
            }
            Job::Mask { key, shape, model } => {
                if matches!(shape, Shape::Part { .. }) {
                    picking = false;
                }
                let outcome = match &base {
                    Some(b) => {
                        match ai.raster(
                            b.stamp, &b.image, &b.edit, b.source, b.turn, key, &shape, model,
                        ) {
                            Ok(made) => Outcome::Mask {
                                key,
                                shape,
                                file: opened_path.clone(),
                                raster: made.raster,
                                provider: made.provider.map(|p| p.name()),
                                seconds: made.seconds,
                                note: made.note,
                                part: made.part,
                            },
                            Err(message) => Outcome::MaskFailed { key, message },
                        }
                    }
                    None => Outcome::MaskFailed {
                        key,
                        message: "nothing developed yet".into(),
                    },
                };
                deliver(outcome);
            }
            Job::People => {
                picking = true;
                let people = match &base {
                    Some(b) => ai
                        .people(b.stamp, &b.image, b.source, b.turn)
                        .map(|(found, provider, seconds)| (found, provider.name(), seconds)),
                    None => Err("nothing developed yet".into()),
                };
                deliver(Outcome::People {
                    file: opened_path.clone(),
                    people,
                });
            }
            Job::Fetch { .. }
            | Job::FetchLenses
            | Job::Gpu
            | Job::Thumbnail { .. }
            | Job::CachedThumbnail { .. }
            | Job::ExportSet { .. }
            | Job::ExportFrame { .. }
            | Job::Keep { .. } => {
                unreachable!(
                    "fetches and thumbnails run on threads of their own; the device, a set's frames and a save are taken above"
                )
            }
        }));
        if let Err(payload) = held {
            discard_base(&mut base, gpu.as_ref());
            learned = None;
            last = None;
            shown = None;
            // The next stamp restarts with the base; what was cached
            // under the old ones goes with them.
            ai.forget(opened_path.clone());
            let message = panic_message(payload.as_ref());
            if let Some(outcome) = blame.outcome(message) {
                deliver(outcome);
            }
        }
        if developing {
            pool.set_limit(pool.threads());
            // A save that was waiting for this develop: its pictures now.
            if let Some(keep) = pending.take() {
                let hand = InHand {
                    opened: opened_path.as_deref(),
                    last: last.as_ref(),
                    shown: shown.as_ref(),
                    base: base.as_ref(),
                    gpu: gpu.as_ref(),
                };
                pending = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    keep_open(keep, false, hand, &mut ai, &edited, &deliver)
                }))
                .unwrap_or(None);
            }
        }
    }
}

/// How many of the pool's `threads` may begin a thumbnail while a
/// develop runs: none. Those in hand finish; the rest wait for the
/// develop to be done. Measured on a cold folder of 300, eight
/// threads left running took the first develop from 1.28 s to 1.61 s
/// and four from 1.28 to 1.49, where holding them all kept it at
/// 1.21 and cost the folder 0.8 s of its 3.6. The develop runs rayon
/// over every core and the thumbnails' decodes take cores it wanted.
/// `GREYCARD_THUMB_DURING_DEVELOP` overrides it, for measuring.
fn during_develop(threads: usize) -> usize {
    std::env::var("GREYCARD_THUMB_DURING_DEVELOP")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(0, |n| n.min(threads))
}

/// A path's file name, for a status line.
fn file_label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The open file's picture under `edit` at `turn`: the last develop's
/// when it was of the same develop and turn, else developed afresh.
/// The export's picture is the reference's, on the CPU, whatever the
/// viewport ran on. If the last develop had CA on the GPU, redevelop.
#[allow(clippy::too_many_arguments)]
fn open_picture(
    edit: &Edit,
    turn: u8,
    input: Option<&Input>,
    last: &mut Option<Last>,
    base: &mut Option<Base>,
    learned: &mut Option<LearnedBase>,
    ai: &mut Ai,
    cache: Option<&greycard_ai::DenoiseCache>,
    lenses: Option<&greycard_lens::Database>,
    deliver: &Deliver,
) -> Result<(Arc<WorkingImage>, Vec<String>), String> {
    if let Some(l) = &*last
        && l.turn == turn
        && l.edit.same_develop(edit)
        && !l.ca_on_gpu
    {
        return Ok((l.image.clone(), l.left_out.clone()));
    }
    let Some(f) = input else {
        return Err("no file is open".into());
    };
    match develop_job(
        f, edit, turn, 0, base, learned, ai, cache, lenses, &mut None, deliver,
    ) {
        (outcome, Some(image)) => {
            *last = base
                .as_ref()
                .map(|b| Last::made(edit.clone(), b, image.clone(), &outcome));
            Ok((image, left_out_of(&outcome, base.as_ref())))
        }
        (Outcome::Failed { message, .. }, None) => Err(message),
        _ => Err("the develop made no picture".into()),
    }
}

/// A set frame's edit with its learned-denoiser blend seeded from the
/// raw's ISO when it has no sidecar yet, as its first open would.
fn seeded(edit: &Edit, seed: bool, input: Option<&Input>) -> Edit {
    let mut edit = edit.clone();
    if seed && let Some(Input::Raw(f)) = input {
        edit.noise.learned_strength = Noise::blend_for_iso(f.shot.iso);
    }
    edit
}

/// A frame of a set that is not the open file: opened and developed
/// here under its own edit, with a base and a learned pair of its
/// own so the open file's are there as they were for the next
/// develop, then written to `path`. The edit it was written under,
/// and what of it was left out ([`left_out_of`]).
fn export_other(
    frame: &crate::queue::Frame,
    ai: &mut Ai,
    cache: Option<&greycard_ai::DenoiseCache>,
    lenses: Option<&greycard_lens::Database>,
    deliver: &Deliver,
    settings: &crate::export::Settings,
    path: &std::path::Path,
) -> Result<(Edit, Vec<String>), String> {
    let (input, metadata) = timed_open(&frame.source).map_err(|e| format!("{e:#}"))?;
    let edit = seeded(&frame.edit, frame.seed_blend, Some(&input));
    ai.forget(Some(frame.source.clone()));
    let (mut base, mut learned) = (None, None);
    let (image, mut left_out) = match develop_job(
        &input,
        &edit,
        frame.turn % 4,
        0,
        &mut base,
        &mut learned,
        ai,
        cache,
        lenses,
        &mut None,
        deliver,
    ) {
        (outcome, Some(image)) => (image, left_out_of(&outcome, base.as_ref())),
        (Outcome::Failed { message, .. }, None) => return Err(message),
        _ => return Err("the develop made no picture".into()),
    };
    left_out.extend(write_export(
        image,
        &edit,
        base.as_ref(),
        ai,
        Some(&frame.source),
        Some(&metadata),
        settings,
        path,
    )?);
    Ok((edit, left_out))
}

/// Finish `image`, the develop `base` was made for, under `edit` and
/// the sheet's `settings`, and write it to `path`: the learned masks
/// made now if they are not yet, the edit's geometry, the finish, the
/// mark, and the file with the source's EXIF. What of the edit's
/// masks could not be made, and so was left out.
#[allow(clippy::too_many_arguments)]
fn write_export(
    image: Arc<WorkingImage>,
    edit: &Edit,
    base: Option<&Base>,
    ai: &mut Ai,
    source_path: Option<&std::path::Path>,
    metadata: Option<&greycard_core::decode::RawMetadata>,
    settings: &crate::export::Settings,
    path: &std::path::Path,
) -> Result<Vec<String>, String> {
    let (mut rendered, left_out) = finish_export(image, edit, base, ai, settings);
    let origin = crate::export::Origin {
        source_name: source_path
            .and_then(|s| s.file_name())
            .map(|n| n.to_string_lossy().into_owned()),
        edit: Some(edit.to_json()),
    };
    // The mark, on the export alone; one that cannot be drawn fails
    // the export rather than let an unmarked picture out.
    crate::export::mark(&mut rendered, settings)
        .and_then(|()| crate::export::write(&rendered, settings, path, metadata, &origin))
        .map_err(|e| format!("{e:#}"))?;
    Ok(left_out)
}

/// `image`, the develop `base` was made for, finished under `edit` and
/// `settings` as an export is: the learned masks made now if they are
/// not yet, the edit's geometry, then the finish. No mark and no file.
/// Beside it, what the edit names that could not be had, which the
/// picture is finished without, as the window draws it without: a
/// look not in the look directory, and each learned mask whose model
/// is not downloaded or failed.
fn finish_export(
    image: Arc<WorkingImage>,
    edit: &Edit,
    base: Option<&Base>,
    ai: &mut Ai,
    settings: &crate::export::Settings,
) -> (crate::export::Rendered, Vec<String>) {
    let source = (image.width as u32, image.height as u32);
    let (rasters, left_out) = learned_masks(edit, base, ai);
    let framed;
    let image: &WorkingImage = if edit.geometry.is_identity() {
        &image
    } else {
        framed = crate::geometry::apply(&image, &edit.geometry);
        &framed
    };
    let clip_level = base.map(|b| b.clip_level).unwrap_or(f32::INFINITY);
    let rendered = crate::export::render(
        image,
        edit,
        source,
        settings,
        &rasters,
        clip_level,
        base.map(|b| &*b.guide),
        base.map(|b| b.source).unwrap_or_default(),
        base.and_then(Base::white_shift).as_ref(),
    );
    (rendered, left_out)
}

/// Learned masks' rasters by adjustment id and component.
type Rasters = std::collections::HashMap<(u64, usize), Arc<Raster>>;

/// The learned masks `edit` draws, made now if they are not yet, for
/// the picture `base` was made for; and what the edit names that could
/// not be had: a look not in the look directory, and each learned mask
/// whose model is not downloaded or failed.
fn learned_masks(edit: &Edit, base: Option<&Base>, ai: &mut Ai) -> (Rasters, Vec<String>) {
    let mut rasters = std::collections::HashMap::new();
    let mut left_out = Vec::new();
    // A look the edit names that the export cannot have, not in the
    // look directory or with no table for the picture's display curve:
    // the picture is finished without it, as the window draws it
    // without, and said in the Look section's own words.
    if let greycard_edit::look::LutChoice::Named(name) = &edit.look_lut.lut
        && edit.look_lut.strength > 0.0
        && edit.look_lut.look_under(edit.display_curve).is_none()
    {
        let why = if edit.look_lut.is_there() {
            greycard_edit::look::mismatch(&greycard_edit::look::list(), name, edit.display_curve)
                .map(|(why, _)| why)
                .unwrap_or_else(|| "its table would not read".into())
        } else {
            "not in the look directory".into()
        };
        left_out.push(format!(
            "the look {name}: {}; written without it",
            why.trim_end_matches(['.', ' '])
        ));
    }
    if let Some(b) = base {
        for a in &edit.adjustments {
            // An adjustment switched off, or with nothing live in its
            // mask, is not drawn (`finish::Local::of`): nothing of it
            // is made, and nothing of it is missed.
            if !a.enabled || a.mask.is_empty() {
                continue;
            }
            for (i, c) in a.mask.live() {
                // An Object with nothing picked yet asks for nothing.
                if !c.shape.is_learned() || !crate::ai::prompted(&c.shape) {
                    continue;
                }
                match ai.raster(
                    b.stamp,
                    &b.image,
                    &b.edit,
                    b.source,
                    b.turn,
                    (a.id, i),
                    &c.shape,
                    None,
                ) {
                    Ok(made) => {
                        // A Part whose person is not settled here goes
                        // out empty, and the log says so.
                        if let Some(part) = &made.part {
                            let why = part.left_out();
                            left_out.push(format!(
                                "{}'s {} shape: {why}; its mask written empty",
                                a.name,
                                c.shape.name()
                            ));
                        }
                        rasters.insert((a.id, i), made.raster);
                    }
                    Err(why) => left_out.push(format!(
                        "{}'s {} shape: {why}; its mask written without it",
                        a.name,
                        c.shape.name()
                    )),
                }
            }
        }
    }
    (rasters, left_out)
}

/// A save of the open frame's edit, waiting on the worker for the
/// develop it is the picture of (`Job::Keep`).
struct Keep {
    path: PathBuf,
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
                ..
            } => Some(Self {
                edit: edit.clone(),
                turn,
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

/// A save of the open frame (`Job::Keep`): its edit's pictures made
/// from the develop the worker holds when that is of this edit, on the
/// keeping thread; a reset edit's taken out of the cache; a frame not
/// open, or `leaving` with no develop of its edit in hand, held as
/// edited and its key read again, so it shows the camera's until a
/// picture of its edit is kept. Answers the save back when its develop
/// is still to land.
fn keep_open(
    keep: Keep,
    leaving: bool,
    hand: InHand<'_>,
    ai: &mut Ai,
    edited: &Arc<crate::edited::Edited>,
    deliver: &Deliver,
) -> Option<Keep> {
    let path = keep.path.clone();
    let name = crate::panel::browser::file_name(&path);
    let said = |path: PathBuf| deliver(Outcome::EditedKept { path });
    let is_edited = crate::edited::is_edited(&path, &keep.edit, keep.iso);
    if !is_edited {
        let hash = greycard_library::hash_file(&path).ok();
        if edited.reset(&path, hash.as_deref()) {
            said(path);
        }
        return None;
    }
    let left = |edited: &crate::edited::Edited| {
        edited.note(&path, true, keep.iso);
        said(path.clone());
    };
    if hand.opened != Some(path.as_path()) || !edited.cache_on() {
        left(edited);
        return None;
    }
    let Ok(hash) = greycard_library::hash_file(&path) else {
        left(edited);
        return None;
    };
    let key = edited.key_of(&keep.edit);
    // Kept already (a save that changed nothing of the picture, or a
    // develop of an edit kept before): the cells are asked again only
    // when they were showing something else.
    if edited.all_kept(&hash, key) {
        if edited.settle_key(&path, &hash, key, keep.iso) {
            said(path);
        }
        return None;
    }
    // Being made already, from a save or a develop just before: that
    // making says when it is kept, and nothing waits here.
    let marked = edited.mark(&hash, key)?;
    // The develop in hand of this edit at this turn: the frame's turn is
    // not in the key, but the picture in hand is turned by it, and the
    // making takes it back out.
    let cpu = hand
        .last
        .filter(|l| l.turn == keep.turn && l.edit.same_develop(&keep.edit));
    let gpu = hand
        .shown
        .filter(|s| s.turn == keep.turn && s.edit.same_develop(&keep.edit));
    let (cpu, gpu, base) = match (cpu, gpu, hand.gpu, hand.base) {
        (Some(l), _, _, Some(b)) => (Some(l), None, b),
        (None, Some(s), Some(ctx), Some(b)) => (None, Some((s, ctx)), b),
        _ => {
            if leaving {
                left(edited);
                return None;
            }
            return Some(keep);
        }
    };
    // A picture without something its edit asks for (a learned model
    // not downloaded or failed, a look not in the directory) is not the
    // edit's, and would be kept under the edit's key for good: none is
    // kept, and the frame shows the camera's until one can be.
    let (rasters, mut left_out) = learned_masks(&keep.edit, Some(base), ai);
    left_out.extend(
        cpu.map(|l| &l.left_out)
            .or(gpu.map(|(s, _)| &s.left_out))
            .into_iter()
            .flatten()
            .cloned(),
    );
    if !left_out.is_empty() {
        for why in &left_out {
            tracing::info!("the edit's pictures of {name}: not kept, {why}");
        }
        left(edited);
        return None;
    }
    let factor = |source: (u32, u32)| crate::edited::factor_for(source, &keep.edit.geometry);
    let (developed, source, factor, read_back) = match (cpu, gpu) {
        (Some(l), _) => {
            let source = (l.image.width as u32, l.image.height as u32);
            (
                crate::edited::Developed::Full(l.image.clone()),
                source,
                factor(source),
                None,
            )
        }
        (None, Some((s, ctx))) => {
            let source = (s.texture.width(), s.texture.height());
            let k = factor(source);
            let started = Instant::now();
            match crate::edited::read_back_reduced(ctx, &s.texture, k) {
                Ok(reduced) => (
                    crate::edited::Developed::Reduced(reduced),
                    source,
                    k,
                    Some(started.elapsed().as_secs_f64()),
                ),
                Err(e) => {
                    tracing::warn!("the edit's pictures of {name}: the read back failed: {e}");
                    left(edited);
                    return None;
                }
            }
        }
        (None, None) => unreachable!("one picture or the other, matched above"),
    };
    let job = crate::edited::Keeping {
        path,
        hash,
        key,
        developed,
        factor,
        read_back,
        iso: keep.iso,
        marked,
        finish: crate::edited::Finish {
            turn: keep.turn,
            source,
            rasters,
            clip_level: base.clip_level,
            guide: Some(base.guide.clone()),
            kind: base.source,
            white: base.white_shift(),
            edit: keep.edit,
        },
    };
    let (on, deliver) = (edited.clone(), deliver.clone());
    edited.keep_later(Box::new(move || {
        crate::edited::keep(&on, job, |path| deliver(Outcome::EditedKept { path }))
    }));
    None
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
}

/// Who a job's panic is reported to: the outcome the UI is waiting
/// on for that job, when it is waiting on one.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Blame {
    Develop(u64),
    Export,
    Mask(Key),
    People,
    Nobody,
}

impl Blame {
    fn of(job: &Job) -> Self {
        match job {
            Job::Open { generation, .. } | Job::Develop { generation, .. } => {
                Blame::Develop(*generation)
            }
            Job::Export { .. } => Blame::Export,
            // A frame of a set is caught where it runs, and counted
            // there as the set's failure.
            Job::ExportSet { .. } | Job::ExportFrame { .. } => Blame::Nobody,
            Job::Mask { key, .. } => Blame::Mask(*key),
            Job::People => Blame::People,
            Job::Thumbnail { .. }
            | Job::CachedThumbnail { .. }
            | Job::Fetch { .. }
            | Job::FetchLenses
            | Job::Gpu
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
            Blame::Export => Outcome::ExportFailed { message },
            Blame::Mask(key) => Outcome::MaskFailed { key, message },
            Blame::People => Outcome::People {
                file: None,
                people: Err(message),
            },
            Blame::Nobody => return None,
        })
    }
}

/// A panic's message, which is a `&str` or a `String` for every
/// `panic!` with a text, and nothing readable otherwise.
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
fn quarters(turn: u8) -> Option<greycard_core::raw::Orientation> {
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
    /// What the file says of its body and its lens.
    fn identity(&self) -> (&str, &str, &Shot) {
        match self {
            Input::Raw(f) => (&f.make, &f.model, &f.shot),
            Input::Picture { meta, .. } => (&meta.make, &meta.model, &meta.shot),
        }
    }
}

/// `open`, with a line in the log for what it was and how long it took.
fn timed_open(
    path: &std::path::Path,
) -> anyhow::Result<(Input, greycard_core::decode::RawMetadata)> {
    let started = Instant::now();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let result = open(path);
    match &result {
        Ok((Input::Raw(f), _)) => tracing::info!(
            "opened {name}: {} {} {}x{} in {:.2} s",
            f.make,
            f.model,
            f.width,
            f.height,
            started.elapsed().as_secs_f64()
        ),
        Ok((Input::Picture { image, .. }, _)) => tracing::info!(
            "opened {name}: picture {}x{} in {:.2} s",
            image.width,
            image.height,
            started.elapsed().as_secs_f64()
        ),
        Err(e) => tracing::error!("open {name}: {e:#}"),
    }
    result
}

/// Read a file: a raw with its metadata, or a picture with its own.
fn open(path: &std::path::Path) -> anyhow::Result<(Input, greycard_core::decode::RawMetadata)> {
    if is_picture_path(path) {
        let mut picture = decode_picture_path(path)?;
        let image = std::mem::replace(&mut picture.image, WorkingImage::new(0, 0));
        let metadata = picture.metadata.clone();
        return Ok((
            Input::Picture {
                image: Arc::new(image),
                meta: Arc::new(picture),
            },
            metadata,
        ));
    }
    let (f, m) = greycard_core::decode::decode_path_with_metadata(path)?;
    Ok((Input::Raw(Arc::new(f)), m))
}

/// Forget the base, and with it the ops' working textures on the GPU:
/// a new file or a new lens database develops afresh.
fn discard_base(base: &mut Option<Base>, gpu: Option<&greycard_gpu::Context>) {
    if base.take().is_some()
        && let Some(gpu) = gpu
    {
        gpu.release();
    }
}

/// Whether a live Part is in `edit`'s masks.
fn has_live_part(edit: &Edit) -> bool {
    edit.adjustments.iter().any(|a| {
        a.enabled
            && a.mask
                .live()
                .any(|(_, c)| matches!(c.shape, Shape::Part { .. }))
    })
}

/// A develop before its dehaze and sharpen, and what the sharpen
/// needs to know.
struct Base {
    edit: Edit,
    /// The frame's quarter turns this base was made at: a turn is
    /// not part of the edit, so `same_base` cannot see it.
    turn: u8,
    image: Arc<WorkingImage>,
    /// The tone equalizer's read of the scene, made once with
    /// the base: the Light sliders never re-develop, so a slider move
    /// must not pay for it, and neither must a sharpen change. Every
    /// way of making a base leaves [`crate::finish::Guide::NONE`] here
    /// and `develop_job` fills it in the one place, after the lens.
    guide: Arc<crate::finish::Guide>,
    white: WhiteBase,
    radius: Option<f32>,
    clip_level: f32,
    /// A raw's scene or a picture already rendered, for the finish.
    source: crate::finish::Source,
    /// Counts the bases made, for the learned masks to know theirs.
    stamp: u64,
    /// The base with its retouch applied, for the retouch it was,
    /// and whether every fill in it was made.
    patched: Option<(Retouch, Arc<WorkingImage>, bool)>,
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
    /// Whether the CA correction in this base ran on the GPU. The
    /// export's picture is the reference's, and the CA is in the base
    /// (unlike the sharpen, which the export re-runs), so an export
    /// makes a fresh base on the CPU when this is set.
    ca_on_gpu: bool,
    /// The learned denoiser's report when this base is the engine's
    /// stand-in for it (its model missing, or failing): a develop that
    /// keeps the base reports `Kept`, and an export of it must still
    /// say what it went without (`left_out_of`).
    stand_in: Option<LearnedReport>,
    /// The camera profile a raw's white was resolved through, which a
    /// mask's own white balance is resolved through too
    /// ([`Base::white_shift`]); `None` for a picture that is not a raw.
    profile: Option<Arc<CameraProfile>>,
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

/// What the lens database says of a file.
fn lens_report(db: Option<&greycard_lens::Database>, input: &Input) -> LensReport {
    let Some(db) = db else {
        return LensReport::NoDatabase;
    };
    let (make, model, shot) = input.identity();
    let Some(name) = shot.lens_model.as_deref() else {
        return LensReport::NoLensName;
    };
    match db.profile_for(make, model, shot) {
        Some(p) => {
            tracing::info!(
                "lens {name}: {}{}{}",
                p.name(),
                if p.camera.is_some() {
                    ""
                } else {
                    ", body not in the database"
                },
                if p.measured_on_smaller_sensor() {
                    format!(
                        ", measured on a smaller sensor (the corners reach r={:.2}; the vignetting is held at its edge past r=1)",
                        p.reach()
                    )
                } else {
                    String::new()
                }
            );
            LensReport::Found {
                lens: p.name().to_string(),
                camera: p.camera.is_some(),
                smaller_sensor: p.measured_on_smaller_sensor(),
            }
        }
        None => {
            tracing::warn!("lens {name} on {make} {model}: not in the database");
            LensReport::Unknown(name.to_string())
        }
    }
}

/// The developed picture with the edit's lens corrections: the
/// profile's from the database, the manual one, in the order the edit
/// gives them. The picture itself when there is nothing to do.
fn correct_lens(
    image: WorkingImage,
    input: &Input,
    edit: &Edit,
    db: Option<&greycard_lens::Database>,
) -> WorkingImage {
    let (make, model, shot) = input.identity();
    let profile = db
        .and_then(|db| db.profile_for(make, model, shot))
        .map(|p| p.correction(image.width, image.height, shot, greycard_lens::Wanted::ALL));
    let mut image = image;
    for c in edit.lens.corrections(profile) {
        image = greycard_core::develop::lens::correct(&image, &c).0;
    }
    image
}

/// Develop `edit`: the base from the cache when only the local
/// contrast, the dehaze or the sharpen differs, else the engine,
/// then those three on a copy, in that order: the sharpen's
/// contrast threshold is measured on the picture it gets, which
/// should be the one with its haze gone. With the
/// learned denoiser on, its answer is kept and blended by strength;
/// when it is not to be had the engine's own path stands in. A
/// picture that is not a raw is its own base: nothing before the lens
/// applies to it.
///
/// With `gpu`, the Detail section and the sharpen run there: the
/// patched picture is uploaded once and kept with the base, the local
/// contrast and the dehaze make the picture before the sharpen from it
/// on a Detail move and that is kept too, and the result is a texture
/// the viewport draws as it is, so a Detail slider costs the Detail
/// section and the sharpen alone, and a sharpen slider the sharpen
/// alone. The dehaze's fit runs on the CPU between its two GPU halves,
/// from a reduced copy the GPU makes, which is kept for the next
/// Dehaze move. No CPU picture comes back then; the export, which
/// passes no `gpu`, develops its own on the reference path. A GPU
/// error (out of memory, a lost device) is warned once and the context
/// is dropped: the session stays on the CPU from then on.
#[allow(clippy::too_many_arguments)]
fn develop_job(
    input: &Input,
    edit: &Edit,
    turn: u8,
    generation: u64,
    base: &mut Option<Base>,
    learned: &mut Option<LearnedBase>,
    ai: &mut Ai,
    cache: Option<&greycard_ai::DenoiseCache>,
    lenses: Option<&greycard_lens::Database>,
    gpu: &mut Option<greycard_gpu::Context>,
    deliver: &Deliver,
) -> (Outcome, Option<Arc<WorkingImage>>) {
    let start = Instant::now();
    let stamp = base.as_ref().map_or(1, |b| b.stamp + 1);
    let mut report = LearnedReport::Off;
    let mut ca_note = None;
    // Only the frame's turn moved, and nothing in the base cares
    // which way up the picture is: the base is turned where it
    // stands, a permutation of its pixels, and is then kept below.
    if let Some(b) = base.as_mut()
        && b.turn != turn
        && b.edit.same_base(edit)
        && (gpu.is_some() || !b.ca_on_gpu)
        && turns_exactly(edit, lenses)
    {
        let started = Instant::now();
        turn_base(b, learned, turn, stamp, gpu.as_ref());
        tracing::info!(
            "base turned to {turn} quarter turns in {:.2} s",
            started.elapsed().as_secs_f64()
        );
    }
    // A base is kept while its edit's base is the same, and, without
    // a GPU (the export), only if its CA correction was not the GPU's:
    // the export's picture is the reference's, so it makes the base
    // afresh on the CPU, and the session keeps that one.
    let fresh_base = !base.as_ref().is_some_and(|b| {
        b.edit.same_base(edit) && b.turn == turn && (gpu.is_some() || !b.ca_on_gpu)
    });
    if fresh_base {
        let settings = DevelopSettings {
            sharpen: None,
            dehaze: None,
            // The frame's quarter turns, composed onto the camera's
            // tag: the engine's one orientation step does both, so
            // nothing after it can disagree about which way up the
            // picture is. A file that is not a raw was turned at
            // decode, so its turn is applied to the picture below.
            orientation: match input {
                Input::Raw(f) => Some(f.orientation.turned(i32::from(turn))),
                Input::Picture { .. } => None,
            },
            ..edit.settings()
        };
        let tier = edit
            .noise
            .tier()
            .map(|t| greycard_ai::denoiser(t).expect("the edit's tiers are the registry's"));
        // The CA correction on the GPU when there is one, the
        // reference's otherwise and on any error. A picture the device
        // cannot hold is this op's to decline, on the CPU for this
        // develop and with the context kept; a device error is
        // remembered for after the develop. What ran and how long is
        // noted for the log.
        let ca_ran = std::cell::Cell::new(None::<(CaRan, f64)>);
        let ca_failed = std::cell::Cell::new(None::<greycard_gpu::Error>);
        let ctx = gpu.as_ref();
        let run_ca = |samples: &[f32],
                      w: usize,
                      h: usize,
                      pattern: &CfaPattern,
                      options: &CaOptions|
         -> greycard_core::Result<(Vec<f32>, CaStats)> {
            if let Some(ctx) = ctx {
                let started = Instant::now();
                match ctx.correct_ca(samples, w, h, pattern, options) {
                    Ok(out) => {
                        ca_ran.set(Some((CaRan::Gpu, started.elapsed().as_secs_f64())));
                        return Ok(out);
                    }
                    Err(greycard_gpu::Error::Unsupported(why)) => {
                        tracing::info!("the CA correction on the CPU: {why}");
                    }
                    Err(e) => ca_failed.set(Some(e)),
                }
            }
            let started = Instant::now();
            let out = correct_ca(samples, w, h, pattern, options);
            ca_ran.set(Some((CaRan::Cpu, started.elapsed().as_secs_f64())));
            out
        };
        let ca: Option<CaCorrector<'_>> = Some(&run_ca);
        let made = match (input, tier) {
            (Input::Picture { image, meta }, _) => Ok(Base {
                edit: edit.clone(),
                turn,
                image: match quarters(turn) {
                    Some(o) => Arc::new(greycard_core::develop::orient((**image).clone(), o)),
                    None => image.clone(),
                },
                guide: Arc::new(crate::finish::Guide::NONE),
                white: WhiteBase {
                    clip: meta.clip_level(),
                    ..WhiteBase::IDENTITY
                },
                radius: None,
                clip_level: meta.clip_level(),
                source: crate::finish::Source::Display,
                stamp,
                patched: None,
                pre: None,
                uploaded: None,
                haze: None,
                ca_on_gpu: false,
                stand_in: None,
                profile: None,
            }),
            (Input::Raw(frame), Some(model)) => {
                match learned_base(
                    frame,
                    edit,
                    turn,
                    &settings,
                    model,
                    learned,
                    ai,
                    cache,
                    stamp,
                    ca,
                    gpu.is_some(),
                ) {
                    Ok((b, r)) => {
                        report = r;
                        Ok(b)
                    }
                    Err(r) => {
                        report = r;
                        engine_base(frame, edit, turn, &settings, stamp, ca)
                    }
                }
            }
            (Input::Raw(frame), None) => engine_base(frame, edit, turn, &settings, stamp, ca),
        };
        if let Some(e) = ca_failed.take() {
            // Once is enough: the device is not trusted again this
            // session, and every op from here is the CPU's.
            tracing::warn!("the GPU CA correction: {e}; the CPU's from now on");
            if let Some(ctx) = gpu.take() {
                ctx.release();
            }
        }
        ca_note = ca_ran.take();
        let ca_on_gpu = matches!(ca_note, Some((CaRan::Gpu, _)));
        // A learned pair made in this develop carries this develop's
        // CA; a pair kept from an earlier one already says where its
        // own ran, and the base took that from it.
        if ca_on_gpu
            && matches!(
                report,
                LearnedReport::Ran { .. } | LearnedReport::Cached { .. }
            )
            && let Some(l) = learned.as_mut()
        {
            l.ca_on_gpu = true;
        }
        match made {
            Ok(mut b) => {
                b.ca_on_gpu |= ca_on_gpu;
                // The profile the develop resolved its white through,
                // chosen as `white_balance_for` in the engine chooses
                // it: the edit's DCP, else the file's own.
                if let Input::Raw(frame) = input {
                    b.profile = settings
                        .profile
                        .as_ref()
                        .map(|p| p.camera.clone())
                        .or_else(|| profile_from_frame(frame).ok())
                        .map(Arc::new);
                }
                b.stand_in = matches!(report, LearnedReport::Missing(_) | LearnedReport::Failed(_))
                    .then(|| report.clone());
                let corrects = !edit.lens.is_identity(lenses.is_some());
                let defringe = edit.lens.defringe();
                if corrects || defringe.is_some() {
                    let mut image = Arc::try_unwrap(b.image).unwrap_or_else(|a| (*a).clone());
                    if corrects {
                        image = correct_lens(image, input, edit, lenses);
                    }
                    // After the geometry, so the resample does not
                    // smear a chroma just neutralized, and before the
                    // retouch and the sharpen.
                    if let Some(options) = defringe {
                        greycard_core::develop::defringe::defringe(&mut image, &options);
                    }
                    b.image = Arc::new(image);
                }
                // After the lens and the defringe, before the retouch
                // and the sharpen: the geometry has not moved a pixel
                // yet, so the plane is in the developed picture's own
                // coordinates, which is where the viewport and the
                // export both read it. Neither the retouch's patches
                // nor the sharpen moves a region's mean luminance by
                // anything the guide's scale can see.
                b.guide = Arc::new(crate::finish::guide_plane(&b.image));
                *base = Some(b)
            }
            Err(e) => {
                return (
                    Outcome::Failed {
                        generation,
                        message: e.to_string(),
                    },
                    None,
                );
            }
        }
    } else if edit.noise.tier().is_some() {
        report = LearnedReport::Kept;
    }
    let b = base.as_mut().expect("a base was just made");
    // The retouch, its patches given sources where they had none,
    // applied on a copy of the base and kept for the next develop.
    let chosen = edit.retouch.choose_sources(&b.image);
    let retouch = edit.retouch.with_sources(&chosen);
    let sources = retouch.chosen_patches(&chosen);
    let mut fills = FillReport::default();
    // A kept picture with fills left unmade serves while the model
    // is still missing, and no longer once it has been fetched.
    let patched = if retouch.is_empty() {
        b.image.clone()
    } else if let Some((r, image, whole)) = &b.patched
        && *r == retouch
        && (*whole || !ai.fill_available())
    {
        image.clone()
    } else {
        let mut image = (*b.image).clone();
        retouch.apply_with(&mut image, |patch, region, picture| {
            // A fill the model has to make is said so before it
            // starts, since it takes a while.
            let kept = ai.fill_kept(patch);
            if !kept && ai.fill_available() {
                deliver(Outcome::Filling {
                    generation,
                    name: patch.name(),
                });
            }
            let started = Instant::now();
            match ai.fill(patch, region, picture) {
                Ok(data) => {
                    if !kept {
                        fills.made.push(patch.name());
                        fills.seconds += started.elapsed().as_secs_f64();
                    }
                    Some(data)
                }
                Err(NoFill::Missing) => {
                    fills.missing.push(patch.name());
                    None
                }
                Err(NoFill::Failed(why)) => {
                    fills.failed.push((patch.name(), why));
                    None
                }
            }
        });
        let image = Arc::new(image);
        let whole = fills.missing.is_empty() && fills.failed.is_empty();
        b.patched = Some((retouch, image.clone(), whole));
        image
    };
    // The local contrast and the dehaze on one copy of the patched
    // picture, on the CPU; none when neither is asked for.
    let clip_level = b.clip_level;
    let before_sharpen = |copy: &mut Option<WorkingImage>| {
        let detail = edit.detail.options().map(|options| {
            let started = Instant::now();
            let image = copy.get_or_insert_with(|| (*patched).clone());
            let stats = local_contrast(image, &options, clip_level);
            (stats, started.elapsed().as_secs_f64())
        });
        let dehaze_stats = edit.detail.dehaze_options().map(|options| {
            let image = copy.get_or_insert_with(|| (*patched).clone());
            dehaze::dehaze(image, &options)
        });
        (detail, dehaze_stats)
    };
    // The ops on the GPU, when there is one. With the sharpen on, the
    // picture before it is kept on the device from the last develop
    // when nothing before the sharpen changed, and is made there by
    // the Detail section from the patched picture's upload when the
    // section moved. With the sharpen off, the Detail section writes
    // the viewport's texture itself; with neither on, the ops'
    // textures are let go.
    match (gpu.as_ref(), edit.sharpen.options()) {
        (Some(ctx), Some(options)) => {
            let kept = b
                .pre
                .as_ref()
                .is_some_and(|p| Arc::ptr_eq(&p.patched, &patched) && p.detail == edit.detail);
            let mut ran = Ran::default();
            let made = if kept {
                Ok(())
            } else {
                make_pre_sharpen(ctx, b, &patched, edit, &before_sharpen, &mut ran)
            };
            let sharpened = made.and_then(|()| {
                let pre = b.pre.as_ref().expect("made, or kept");
                let texture = ctx.viewport_texture(pre.image.width(), pre.image.height());
                ctx.sharpen(&pre.image, &options, b.radius, b.clip_level, &texture)
                    .map(|stats| (stats, texture))
            });
            match sharpened {
                Ok((stats, texture)) => {
                    let pre = b.pre.as_ref().expect("made, or kept");
                    let seconds = start.elapsed().as_secs_f64();
                    note_developed(
                        pre.image.width(),
                        pre.image.height(),
                        fresh_base,
                        ca_note,
                        &report,
                        &fills,
                        ran.detail,
                        pre.dehaze_stats
                            .map(|_| ran.dehaze.unwrap_or(DehazeRan::Kept)),
                        Some(Sharpened::Gpu),
                        seconds,
                    );
                    return (
                        Outcome::Developed {
                            generation,
                            turn,
                            image: Developed::Texture(texture),
                            guide: b.guide.clone(),
                            white: b.white,
                            seconds,
                            detail: pre
                                .detail_stats
                                .map(|s| (s, ran.detail.unwrap_or(DetailRan::Kept))),
                            sharpen: Some(stats),
                            dehaze: pre.dehaze_stats,
                            sources,
                            learned: report,
                            fills,
                        },
                        None,
                    );
                }
                Err(greycard_gpu::Error::Unsupported(why)) => {
                    // This picture's, not the device's (one it cannot
                    // hold): the CPU for this develop, the context kept.
                    tracing::info!("the ops on the CPU: {why}");
                    b.pre = None;
                    b.uploaded = None;
                    b.haze = None;
                    ctx.release();
                }
                Err(e) => {
                    // Once is enough: the device is not trusted again
                    // this session, and every develop from here is the
                    // CPU's.
                    tracing::warn!("the GPU ops: {e}; the CPU's from now on");
                    b.pre = None;
                    b.uploaded = None;
                    b.haze = None;
                    if let Some(ctx) = gpu.take() {
                        ctx.release();
                    }
                }
            }
        }
        (Some(ctx), None) => {
            // The sharpen is off: its input and its planes go, whichever
            // path the develop takes.
            b.pre = None;
            ctx.release_sharpen();
            if edit.detail.options().is_some() || edit.detail.dehaze_options().is_some() {
                let mut ran = Ran::default();
                match detail_on_gpu(ctx, b, &patched, edit, true, &mut ran) {
                    Ok(Some(made)) => {
                        let texture = made.texture.expect("a viewport texture was asked for");
                        let seconds = start.elapsed().as_secs_f64();
                        note_developed(
                            texture.width(),
                            texture.height(),
                            fresh_base,
                            ca_note,
                            &report,
                            &fills,
                            ran.detail,
                            ran.dehaze,
                            None,
                            seconds,
                        );
                        return (
                            Outcome::Developed {
                                generation,
                                turn,
                                image: Developed::Texture(texture),
                                guide: b.guide.clone(),
                                white: b.white,
                                seconds,
                                detail: made.detail.map(|s| (s, DetailRan::Gpu)),
                                sharpen: None,
                                dehaze: made.dehaze,
                                sources,
                                learned: report,
                                fills,
                            },
                            None,
                        );
                    }
                    Ok(None) => {
                        // This picture's, not the device's: the CPU for
                        // this develop, the context kept.
                        b.uploaded = None;
                        b.haze = None;
                        ctx.release();
                    }
                    Err(e) => {
                        tracing::warn!("the GPU Detail section: {e}; the CPU's from now on");
                        b.uploaded = None;
                        b.haze = None;
                        if let Some(ctx) = gpu.take() {
                            ctx.release();
                        }
                    }
                }
            } else {
                // The CPU's develop: nothing on the device is of use.
                b.uploaded = None;
                b.haze = None;
                ctx.release();
            }
        }
        (None, _) => {}
    }
    let mut copy: Option<WorkingImage> = None;
    let (detail, dehaze_stats) = before_sharpen(&mut copy);
    let (stats, mask) = match edit.sharpen.options() {
        Some(options) => {
            let image = copy.get_or_insert_with(|| (*patched).clone());
            let (stats, mask) = sharpen_with_mask(image, &options, b.radius, b.clip_level);
            (Some(stats), Some(mask))
        }
        None => (None, None),
    };
    let image = match copy {
        Some(image) => Arc::new(image),
        None => patched,
    };
    let halves = Arc::new(Halves::from_image(&image, mask.as_deref()));
    let seconds = start.elapsed().as_secs_f64();
    note_developed(
        image.width as u32,
        image.height as u32,
        fresh_base,
        ca_note,
        &report,
        &fills,
        detail.as_ref().map(|(_, s)| DetailRan::Cpu(*s)),
        dehaze_stats.map(|_| DehazeRan::Cpu),
        stats.map(|_| Sharpened::Cpu),
        seconds,
    );
    (
        Outcome::Developed {
            generation,
            turn,
            image: Developed::Halves(halves),
            guide: b.guide.clone(),
            white: b.white,
            seconds,
            detail: detail.map(|(s, secs)| (s, DetailRan::Cpu(secs))),
            sharpen: stats,
            dehaze: dehaze_stats,
            sources,
            learned: report,
            fills,
        },
        Some(image),
    )
}

/// What the CPU's steps before the sharpen report: the local
/// contrast's stats and seconds, and the dehaze's stats, each when it
/// ran.
type BeforeSharpen = (Option<(LocalContrastStats, f64)>, Option<DehazeStats>);

/// The patched picture on the device: the one kept with the base
/// when it is this picture's, else uploaded now and kept.
fn uploaded_patched(
    ctx: &greycard_gpu::Context,
    b: &mut Base,
    patched: &Arc<WorkingImage>,
) -> greycard_gpu::Result<Arc<greycard_gpu::Image>> {
    if let Some((p, image)) = b.uploaded.as_ref()
        && Arc::ptr_eq(p, patched)
    {
        return Ok(image.clone());
    }
    b.uploaded = None;
    let image = Arc::new(ctx.upload(patched)?);
    b.uploaded = Some((patched.clone(), image.clone()));
    Ok(image)
}

/// Where this develop's local contrast and dehaze ran, for the log;
/// none for an op that did not run in it.
#[derive(Default)]
struct Ran {
    detail: Option<DetailRan>,
    dehaze: Option<DehazeRan>,
}

/// The picture before the sharpen, on the GPU, for `edit`'s Detail
/// section on `patched`, into `b.pre`: [`detail_on_gpu`]'s picture,
/// or, for a picture the ops decline (`Unsupported`), the CPU's local
/// contrast and dehaze (`before_sharpen`), uploaded, with the patched
/// picture's upload, the dehaze's reduced copy and the local
/// contrast's planes let go, since that path has no use for them. The
/// last picture before the sharpen goes first, so that two are never
/// held at once (716 MB at 45 MP). Any other GPU error is the
/// caller's to act on.
fn make_pre_sharpen(
    ctx: &greycard_gpu::Context,
    b: &mut Base,
    patched: &Arc<WorkingImage>,
    edit: &Edit,
    before_sharpen: &dyn Fn(&mut Option<WorkingImage>) -> BeforeSharpen,
    ran: &mut Ran,
) -> greycard_gpu::Result<()> {
    b.pre = None;
    if let Some(made) = detail_on_gpu(ctx, b, patched, edit, false, ran)? {
        b.pre = Some(PreSharpen {
            patched: patched.clone(),
            detail: edit.detail,
            image: made.image.expect("no viewport texture was given"),
            detail_stats: made.detail,
            dehaze_stats: made.dehaze,
        });
        return Ok(());
    }
    b.uploaded = None;
    b.haze = None;
    ctx.release_local_contrast();
    let mut copy = None;
    let (detail, dehaze_stats) = before_sharpen(&mut copy);
    ran.detail = detail.map(|(_, s)| DetailRan::Cpu(s));
    ran.dehaze = dehaze_stats.map(|_| DehazeRan::Cpu);
    let image: &WorkingImage = copy.as_ref().unwrap_or(patched);
    let image = Arc::new(ctx.upload(image)?);
    b.pre = Some(PreSharpen {
        patched: patched.clone(),
        detail: edit.detail,
        image,
        detail_stats: detail.map(|(s, _)| s),
        dehaze_stats,
    });
    Ok(())
}

/// What the Detail section made on the device: the picture after it,
/// or the viewport's texture it went into, and its ops' stats.
struct OnGpu {
    image: Option<Arc<greycard_gpu::Image>>,
    texture: Option<greycard_gpu::wgpu::Texture>,
    detail: Option<LocalContrastStats>,
    dehaze: Option<DehazeStats>,
}

/// `edit`'s Detail section on the device, from the patched picture's
/// upload: the local contrast when a slider of it is set, then the
/// dehaze when it is on, each reading the last one's output, as the
/// reference runs them. The dehaze sums the blocks of its input there
/// and reads them back, fits its model on the CPU from them (the
/// reference's own fit), and applies the model there; the reduced
/// copy is kept, so a move of the Dehaze alone fits again from it. With
/// `viewport`, the last op writes a viewport texture, made once the
/// upload has found the picture fits the device; else the result is a
/// new picture for the sharpen, the upload itself when neither op is
/// set. None for a picture the device or an op declines
/// (`Unsupported`: one larger than the device's textures, or an amount
/// so small its strength is zero, which the CPU's path leaves alone),
/// to take the CPU for this develop.
fn detail_on_gpu(
    ctx: &greycard_gpu::Context,
    b: &mut Base,
    patched: &Arc<WorkingImage>,
    edit: &Edit,
    viewport: bool,
    ran: &mut Ran,
) -> greycard_gpu::Result<Option<OnGpu>> {
    let declined = |op: &str, why: String| {
        tracing::info!("the {op} on the CPU: {why}");
        Ok(None)
    };
    let uploaded = match uploaded_patched(ctx, b, patched) {
        Ok(uploaded) => uploaded,
        Err(greycard_gpu::Error::Unsupported(why)) => return declined("Detail section", why),
        Err(e) => return Err(e),
    };
    let texture = viewport.then(|| ctx.viewport_texture(uploaded.width(), uploaded.height()));
    let viewport = texture.as_ref();
    let local = edit.detail.options();
    let dehaze = edit.detail.dehaze_options();
    let (image, detail) = match (local, dehaze, viewport) {
        (None, _, _) => {
            ctx.release_local_contrast();
            (uploaded, None)
        }
        // The local contrast last, into the viewport's texture.
        (Some(options), None, Some(out)) => {
            return match ctx.local_contrast_to_viewport(&uploaded, &options, b.clip_level, out) {
                Ok(stats) => {
                    ran.detail = Some(DetailRan::Gpu);
                    b.haze = None;
                    Ok(Some(OnGpu {
                        image: None,
                        texture,
                        detail: Some(stats),
                        dehaze: None,
                    }))
                }
                Err(greycard_gpu::Error::Unsupported(why)) => declined("local contrast", why),
                Err(e) => Err(e),
            };
        }
        (Some(options), _, _) => match ctx.local_contrast(&uploaded, &options, b.clip_level) {
            Ok((image, stats)) => {
                ran.detail = Some(DetailRan::Gpu);
                (Arc::new(image), Some(stats))
            }
            Err(greycard_gpu::Error::Unsupported(why)) => return declined("local contrast", why),
            Err(e) => return Err(e),
        },
    };
    let Some(options) = dehaze else {
        b.haze = None;
        return Ok(Some(OnGpu {
            image: Some(image),
            texture,
            detail,
            dehaze: None,
        }));
    };
    // The reduced copy: kept from the last develop when it was made
    // from this picture through the same local contrast, else summed
    // here and read back.
    let reduced = match b.haze.as_ref() {
        Some(k) if Arc::ptr_eq(&k.patched, patched) && k.detail == local => k.reduced.clone(),
        _ => {
            b.haze = None;
            let reduced = Arc::new(ctx.dehaze_reduced(&image)?);
            b.haze = Some(KeptHaze {
                patched: patched.clone(),
                detail: local,
                reduced: reduced.clone(),
            });
            reduced
        }
    };
    let Some(model) = reduced.model(&options) else {
        return declined(
            "dehaze",
            format!("a strength of zero at {}", options.amount),
        );
    };
    let applied = match viewport {
        Some(out) => ctx
            .dehaze_to_viewport(&image, &model, out)
            .map(|stats| (None, stats)),
        None => ctx
            .dehaze(&image, &model)
            .map(|(image, stats)| (Some(Arc::new(image)), stats)),
    };
    match applied {
        Ok((image, stats)) => {
            ran.dehaze = Some(DehazeRan::Gpu);
            Ok(Some(OnGpu {
                image,
                texture,
                detail,
                dehaze: Some(stats),
            }))
        }
        Err(greycard_gpu::Error::Unsupported(why)) => declined("dehaze", why),
        Err(e) => Err(e),
    }
}

/// Where a develop's sharpen ran.
#[derive(Clone, Copy)]
enum Sharpened {
    Cpu,
    Gpu,
}

/// Where a base develop's CA correction ran.
#[derive(Clone, Copy)]
enum CaRan {
    Cpu,
    Gpu,
}

/// One line for a develop: what was made anew and what was kept,
/// and the seconds, the same the outcome carries. A model that is
/// missing is a state, named here and offered by the UI, not warned
/// on every develop; one that would not load is warned where the
/// load fails. A fill that failed is warned here, with its name.
#[allow(clippy::too_many_arguments)]
fn note_developed(
    width: u32,
    height: u32,
    fresh_base: bool,
    ca: Option<(CaRan, f64)>,
    learned: &LearnedReport,
    fills: &FillReport,
    detail: Option<DetailRan>,
    dehazed: Option<DehazeRan>,
    sharpened: Option<Sharpened>,
    total: f64,
) {
    let mut parts: Vec<String> = Vec::new();
    parts.push(if fresh_base {
        "base made".into()
    } else {
        "base kept".into()
    });
    match ca {
        Some((CaRan::Gpu, s)) => parts.push(format!("CA on the GPU {s:.2} s")),
        Some((CaRan::Cpu, s)) => parts.push(format!("CA {s:.2} s")),
        None => {}
    }
    match learned {
        LearnedReport::Off => {}
        LearnedReport::Ran {
            version,
            provider,
            seconds,
        } => parts.push(format!("denoiser {version} on {provider} {seconds:.2} s")),
        LearnedReport::Cached { seconds } => parts.push(format!("denoiser cached {seconds:.2} s")),
        LearnedReport::Kept => parts.push("denoiser kept".into()),
        LearnedReport::Missing(_) => parts.push("denoiser missing".into()),
        LearnedReport::Failed(_) => parts.push("denoiser failed".into()),
    }
    if !fills.made.is_empty() {
        parts.push(format!(
            "{} fill(s) {:.2} s",
            fills.made.len(),
            fills.seconds
        ));
    }
    if !fills.missing.is_empty() {
        parts.push(format!("{} fill(s) missing", fills.missing.len()));
    }
    for (name, why) in &fills.failed {
        tracing::warn!("fill {name}: {why}");
    }
    match detail {
        Some(DetailRan::Gpu) => parts.push("local contrast on the GPU".into()),
        Some(DetailRan::Cpu(s)) => parts.push(format!("local contrast {s:.2} s")),
        Some(DetailRan::Kept) | None => {}
    }
    match dehazed {
        Some(DehazeRan::Gpu) => parts.push("dehaze on the GPU".into()),
        Some(DehazeRan::Cpu) => parts.push("dehaze".into()),
        Some(DehazeRan::Kept) | None => {}
    }
    match sharpened {
        Some(Sharpened::Cpu) => parts.push("sharpen".into()),
        Some(Sharpened::Gpu) => parts.push("sharpen on the GPU".into()),
        None => {}
    }
    tracing::info!(
        "developed {width}x{height}: {}, total {total:.2} s",
        parts.join(", ")
    );
}

/// Whether a base made at one frame turn, turned by a permutation of
/// its pixels, is the base made at another, to the bit.
///
/// Everything the engine does before its orientation step reads the
/// mosaic, which a turn does not touch, and the step itself is a
/// permutation (`develop::orient`), so the engine's picture at one
/// turn is its picture at another permuted. What comes after it on
/// the turned picture is the question. The lens correction is
/// radial about the center, and the defringe's blur and its test
/// against the frame's mean deviation are symmetric, so each is the
/// same picture turned in exact arithmetic; in floats neither is,
/// since a turn changes the order of the sums and which side of a
/// half a coordinate rounds to. A frame with either on is developed
/// afresh at its new turn, as it always was.
fn turns_exactly(edit: &Edit, lenses: Option<&greycard_lens::Database>) -> bool {
    edit.lens.is_identity(lenses.is_some()) && edit.lens.defringe().is_none()
}

/// The kept base, and the learned denoiser's pair beside it when it
/// was made at the same turn, turned to the frame's `turn`: their
/// pixels permuted, the guide plane read again off the turned
/// picture, and everything kept after the base let go, since the
/// retouch's patches and the picture before the sharpen were placed
/// on the base the other way up. A new `stamp`, so the learned masks
/// made on the old base are not taken for this one's.
fn turn_base(
    b: &mut Base,
    learned: &mut Option<LearnedBase>,
    turn: u8,
    stamp: u64,
    gpu: Option<&greycard_gpu::Context>,
) {
    let Some(o) = quarters((turn % 4 + 4 - b.turn % 4) % 4) else {
        return;
    };
    let turned = |image: Arc<WorkingImage>| {
        let image = Arc::try_unwrap(image).unwrap_or_else(|a| (*a).clone());
        Arc::new(greycard_core::develop::orient(image, o))
    };
    // The blend at either end of its strength is one of the pair
    // itself, shared rather than copied: turned once, and shared on.
    let mut image = std::mem::replace(&mut b.image, Arc::new(WorkingImage::new(0, 0)));
    if let Some(l) = learned.as_mut().filter(|l| l.turn == b.turn) {
        let is_model = Arc::ptr_eq(&image, &l.model);
        let is_plain = Arc::ptr_eq(&image, &l.plain);
        if is_model || is_plain {
            image = Arc::new(WorkingImage::new(0, 0));
        }
        let empty = || Arc::new(WorkingImage::new(0, 0));
        l.model = turned(std::mem::replace(&mut l.model, empty()));
        l.plain = turned(std::mem::replace(&mut l.plain, empty()));
        l.turn = turn;
        if is_model {
            image = l.model.clone();
        } else if is_plain {
            image = l.plain.clone();
        } else {
            image = turned(image);
        }
    } else {
        image = turned(image);
    }
    b.image = image;
    b.guide = Arc::new(crate::finish::guide_plane(&b.image));
    b.turn = turn;
    b.stamp = stamp;
    b.patched = None;
    b.haze = None;
    let kept = b.pre.take().is_some() | b.uploaded.take().is_some();
    if kept && let Some(ctx) = gpu {
        ctx.release();
    }
}

/// The engine's own develop of `edit`, before its sharpen, its CA
/// correction by `ca` when one is given (the GPU's).
fn engine_base(
    frame: &RawFrame,
    edit: &Edit,
    turn: u8,
    settings: &DevelopSettings,
    stamp: u64,
    ca: Option<CaCorrector<'_>>,
) -> anyhow::Result<Base> {
    let d = develop_with(frame, settings, ca)?;
    Ok(Base {
        edit: edit.clone(),
        turn,
        image: Arc::new(d.image),
        guide: Arc::new(crate::finish::Guide::NONE),
        white: WhiteBase::from(&d.white_balance, d.clip_level),
        radius: d.sharpen_radius,
        clip_level: d.clip_level,
        source: crate::finish::Source::Scene,
        stamp,
        patched: None,
        pre: None,
        uploaded: None,
        haze: None,
        ca_on_gpu: false,
        stand_in: None,
        profile: None,
    })
}

/// The base with the learned denoiser: its answer from `learned` when
/// that was made for the same mosaic and tier, else from the disk
/// cache, else the network run once and kept; then the blend by the
/// edit's strength. The error is why the engine's path must stand in.
#[allow(clippy::too_many_arguments)]
fn learned_base(
    frame: &RawFrame,
    edit: &Edit,
    turn: u8,
    settings: &DevelopSettings,
    model: &'static greycard_ai::Model,
    learned: &mut Option<LearnedBase>,
    ai: &mut Ai,
    cache: Option<&greycard_ai::DenoiseCache>,
    stamp: u64,
    ca: Option<CaCorrector<'_>>,
    has_gpu: bool,
) -> Result<(Base, LearnedReport), LearnedReport> {
    let report = if learned
        .as_ref()
        .is_some_and(|l| pair_serves(l, turn, edit, has_gpu))
    {
        LearnedReport::Kept
    } else {
        let denoiser = ai.denoiser(model).map_err(|e| match e {
            NoDenoiser::Missing => LearnedReport::Missing(model),
            NoDenoiser::Failed(m) => LearnedReport::Failed(m),
        })?;
        // The network is the demosaic and the denoise both.
        let settings = DevelopSettings {
            denoise: None,
            ..settings.clone()
        };
        let (made, report) = run_learned(frame, edit, turn, &settings, model, denoiser, cache, ca)
            .map_err(|e| LearnedReport::Failed(e.to_string()))?;
        *learned = Some(made);
        report
    };
    let l = learned
        .as_ref()
        .expect("a learned base was just made or kept");
    Ok((
        Base {
            edit: edit.clone(),
            turn,
            image: blend(&l.model, &l.plain, edit.noise.learned_strength),
            guide: Arc::new(crate::finish::Guide::NONE),
            white: l.white,
            radius: l.radius,
            clip_level: l.clip_level,
            source: crate::finish::Source::Scene,
            stamp,
            patched: None,
            pre: None,
            uploaded: None,
            haze: None,
            ca_on_gpu: l.ca_on_gpu,
            stand_in: None,
            profile: None,
        },
        report,
    ))
}

/// The engine's develop with the network between `prepare` and
/// `finish`, and the plain demosaic of the same mosaic beside it. The
/// network's answer comes from the disk cache when it is there, and
/// goes into it when it is not.
#[allow(clippy::too_many_arguments)]
fn run_learned(
    frame: &RawFrame,
    edit: &Edit,
    turn: u8,
    settings: &DevelopSettings,
    model: &'static greycard_ai::Model,
    denoiser: &mut greycard_ai::Denoiser,
    cache: Option<&greycard_ai::DenoiseCache>,
    ca: Option<CaCorrector<'_>>,
) -> anyhow::Result<(LearnedBase, LearnedReport)> {
    use anyhow::Context;
    let prepared = prepare_with(frame, settings, true, ca)?;
    let pattern = prepared
        .pattern
        .clone()
        .context("the learned denoiser needs a Bayer mosaic")?;
    let noise = prepared
        .noise_model()
        .context("the learned denoiser needs the frame's noise model")?;
    let tiling = greycard_ai::denoise::Tiling::default();
    let start = Instant::now();
    let key = cache.map(|c| (c, greycard_ai::DenoiseCache::key(model, tiling, &prepared)));
    let (rgb, report) = match key.and_then(|(c, k)| c.read(k, &prepared)) {
        Some(rgb) => (
            rgb,
            LearnedReport::Cached {
                seconds: start.elapsed().as_secs_f64(),
            },
        ),
        None => {
            let rgb = denoiser.run(
                &prepared.samples,
                prepared.width,
                prepared.height,
                &pattern,
                &noise,
                tiling,
            )?;
            let report = LearnedReport::Ran {
                version: denoiser.version().to_string(),
                provider: denoiser.provider().name(),
                seconds: start.elapsed().as_secs_f64(),
            };
            if let Some((c, k)) = key
                && let Err(e) = c.write(k, frame, &prepared, &rgb)
            {
                tracing::warn!("denoise cache {}: {e}", c.path(k).display());
            }
            (rgb, report)
        }
    };
    let (plain_rgb, dual, _) = demosaic_prepared(&prepared, settings)?;
    let plain = finish(prepared.clone(), plain_rgb, dual, None, settings)?;
    let model = finish(prepared, rgb, None, None, settings)?;
    Ok((
        LearnedBase {
            edit: edit.clone(),
            turn,
            model: Arc::new(model.image),
            plain: Arc::new(plain.image),
            white: WhiteBase::from(&model.white_balance, model.clip_level),
            radius: model.sharpen_radius,
            clip_level: model.clip_level,
            // Set by the caller once it knows where the CA ran.
            ca_on_gpu: false,
        },
        report,
    ))
}

/// `plain` taken `strength` of the way to `model`, per sample: the
/// two are one picture in one space, so the mix is that much of the
/// denoise. At the ends no copy is made.
fn blend(model: &Arc<WorkingImage>, plain: &Arc<WorkingImage>, strength: f32) -> Arc<WorkingImage> {
    let s = strength.clamp(0.0, 1.0);
    if s >= 1.0 {
        return model.clone();
    }
    if s <= 0.0 {
        return plain.clone();
    }
    let mut out = (**plain).clone();
    for (o, m) in out.data.iter_mut().zip(&model.data) {
        *o += (m - *o) * s;
    }
    Arc::new(out)
}

/// How [`thumbnail`] makes a picture, as the cache's entries record
/// it in their names: raise it when the picture it makes changes (the
/// downscale, the turn, rawler's choice of preview, or the fallback
/// develop through `DevelopSettings::default()`, whose definition in
/// core names this constant), and every entry made the old way is a
/// miss and is made again. The long edge is in the key already, so a
/// change of the sizes made (`grid::MADE`) needs no bump.
pub const THUMB_RECIPE: u16 = 2;

/// Count what `cache` holds on a thread of its own, walking the disk
/// without its lock, and hand `then` what is known after: the count
/// seeded, or the one some write had already made. A walk over a few
/// hundred thousand entries takes a second or more, which is not the
/// window's to wait through. Nothing to count is `None`.
pub fn count_thumb_cache(
    cache: &ThumbCache,
    then: impl FnOnce(Option<greycard_library::thumbs::Usage>) + Send + 'static,
) {
    let cache = cache.clone();
    let spawned = std::thread::Builder::new()
        .name("thumbnail cache count".into())
        .spawn(move || {
            let root = cache
                .lock()
                .expect("thumbnail cache")
                .as_ref()
                .map(|c| (c.root().to_path_buf(), c.previews_from()));
            let Some((root, previews_from)) = root else {
                return then(None);
            };
            // The thumbnails and the previews apart, each under its cap.
            let counted = greycard_library::thumbs::split_at(&root, previews_from);
            let known = cache
                .lock()
                .expect("thumbnail cache")
                .as_mut()
                .and_then(|c| {
                    c.seed_split(counted);
                    c.known_usage()
                });
            then(known);
        });
    if let Err(e) = spawned {
        tracing::warn!("thumbnail cache not counted: {e}");
    }
}

/// A file's length and modification time in nanoseconds, the two a
/// change to it past its first 64 KB shows in.
pub(crate) fn file_stat(path: &std::path::Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64);
    Some((meta.len(), mtime))
}

/// What a cache entry for a file of this stat must have been made
/// under: this recipe and the file's modification time. The hash reads
/// the head and the length, and a file can change past its head with
/// neither moving: a picture re-exported (an uncompressed TIFF
/// retouched in its lower half), a DNG's previews or orientation
/// rewritten in place (DNGLab's IFD0 is at the end of the file), and a
/// raw still being copied by a copier that set its length first —
/// Windows' CopyFile, `rsync --preallocate`, many card importers — whose
/// preview is half zeros and decodes mostly grey. The time tells all
/// three apart. A move or rename keeps it; the cost is one more making
/// of each thumbnail after a copy that does not keep it (`cp` without
/// `-p`, a drag to another disk on some desktops).
pub(crate) fn thumb_tag(stat: (u64, u64)) -> Tag {
    Tag {
        recipe: THUMB_RECIPE,
        stamp: stat.1,
    }
}

/// What a file's thumbnails are kept under, its content hash and the
/// stamp of [`thumb_tag`], for taking them out of the cache when the
/// file is deleted: read before it goes, since neither can be after.
pub(crate) fn thumb_key(path: &std::path::Path) -> Option<(String, u64)> {
    let stat = file_stat(path)?;
    let hash = greycard_library::hash_file(path).ok()?;
    Some((hash, thumb_tag(stat).stamp))
}

/// The file's stat and cache key, when the cache is on and the file
/// hashes, and the entry kept under that key if any. What a lookup
/// finds is noted with `previews`, which owes the file a local preview
/// when none is kept under that key: the thumbnail's hash, so the
/// preview reads no second head. A lookup alone, with no making after
/// it, is what the thumbnails' threads do while a develop holds them,
/// since a hit costs a tenth of a millisecond.
///
/// A frame `edited` says shows its edit is looked up first under its
/// develop key, and the camera's entry stands in on a miss: the
/// picture made from the edit, when one is kept, is the frame's
/// thumbnail at every size.
#[allow(clippy::type_complexity)]
fn thumb_lookup(
    cache: &ThumbCache,
    path: &std::path::Path,
    size: u32,
    previews: Option<&crate::previews::Previews>,
    edited: Option<&crate::edited::Edited>,
) -> (Option<(u64, u64)>, Option<(String, Tag)>, Option<Thumb>) {
    let on = cache
        .lock()
        .expect("thumbnail cache")
        .as_ref()
        .is_some_and(|c| c.cap() > 0);
    let before = if on { file_stat(path) } else { None };
    let key = before.and_then(|stat| match greycard_library::hash_file(path) {
        Ok(hash) => Some((hash, thumb_tag(stat))),
        Err(e) => {
            tracing::debug!("thumbnail {}: not hashed: {e}", path.display());
            None
        }
    });
    let from_edit = key.as_ref().and_then(|(hash, _)| {
        let edited = edited?;
        edited.lookup(hash, size, edited.shows(path))
    });
    let hit = from_edit.or_else(|| {
        key.as_ref().and_then(|(hash, tag)| {
            cache
                .lock()
                .expect("thumbnail cache")
                .as_mut()
                .and_then(|c| c.get(hash, size, *tag))
        })
    });
    if let (Some(previews), Some(stat), Some((hash, _))) = (previews, before, key.as_ref()) {
        previews.note(path, stat, hash);
    }
    (before, key, hit)
}

/// A file's thumbnail from the cache when it holds one for the file's
/// content, else made by `make` (the pool's is [`thumbnail`]) and kept,
/// the lookup noted with `previews`. The lookup costs a stat and the
/// content hash, a read of the file's first 64 KB, and nothing else of
/// the file; a cache that cannot be read or written is a miss and a
/// thumbnail made, never an error of its own. Says whether it came
/// from the cache. The file is stat'd before it is hashed and again
/// after the picture is made, and a picture made while the file was
/// changing — its length or its time moved — is shown but not kept: it
/// may be of a file half written, and kept under the key the finished
/// file will have.
fn cached_thumbnail_noting(
    cache: &ThumbCache,
    previews: Option<&crate::previews::Previews>,
    edited: Option<&crate::edited::Edited>,
    path: &std::path::Path,
    size: u32,
    make: impl FnOnce(&std::path::Path, u32) -> anyhow::Result<(u32, u32, Vec<u8>)>,
) -> anyhow::Result<(Thumb, bool)> {
    let (before, key, hit) = thumb_lookup(cache, path, size, previews, edited);
    if let Some(thumb) = hit {
        return Ok((thumb, true));
    }
    let (width, height, rgb) = make(path, size)?;
    let thumb = Thumb { width, height, rgb };
    if let Some((hash, tag)) = key {
        if file_stat(path) != before {
            tracing::debug!(
                "thumbnail {}: the file changed while it was made; not cached",
                path.display()
            );
        } else if let Some(c) = cache.lock().expect("thumbnail cache").as_mut()
            && let Err(e) = c.put(&hash, size, tag, &thumb)
        {
            tracing::debug!("thumbnail {}: not cached: {e}", path.display());
        }
    }
    Ok((thumb, false))
}

/// A small sRGB rendering of a file, `size` on its long edge: the
/// camera's own preview JPEG when the file has one, box downscaled
/// and turned as the camera says; else a bilinear demosaic, box
/// downscaled.
pub(crate) fn thumbnail(path: &std::path::Path, size: u32) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let size = size.max(1) as usize;
    if is_picture_path(path) {
        // The picture itself, as the file encodes it: near enough to
        // sRGB for a thumbnail whatever its profile. The tag is the
        // picture module's to read, since that is the one the
        // develop turns by and this thumbnail has to agree with it.
        let orientation = greycard_core::picture::orientation_path(path)?;
        let decoder = image::ImageReader::open(path)?
            .with_guessed_format()?
            .into_decoder()?;
        let rgb = image::DynamicImage::from_decoder(decoder)?.to_rgb8();
        return Ok(thumbnail_of_preview(&rgb, orientation, size));
    }
    if let Ok(Some((preview, orientation))) = greycard_core::decode::preview_path(path) {
        return Ok(thumbnail_of_preview(&preview, orientation, size));
    }
    let frame = greycard_core::decode::decode_path(path)?;
    let settings = DevelopSettings {
        demosaic: DemosaicMethod::Bilinear,
        chromatic_aberration: None,
        ..Default::default()
    };
    let image = develop(&frame, &settings)?.image;
    // The long edge, as the preview path takes it: dividing the
    // width alone gives a portrait frame half as much again as was
    // asked for, and the memory with it.
    let factor = image.width.max(image.height).div_ceil(size).max(1);
    let (w, h) = (image.width / factor, image.height / factor);
    let to_srgb = greycard_core::color::WORKING_SPACE
        .to_space_matrix(&RgbSpace::SRGB, greycard_core::color::CAT)
        .expect("working space to sRGB")
        .rows
        .map(|row| row.map(|v| v as f32));
    let mut rgb = Vec::with_capacity(w * h * 3);
    for ty in 0..h {
        for tx in 0..w {
            let mut sum = [0.0f32; 3];
            for y in ty * factor..(ty + 1) * factor {
                for x in tx * factor..(tx + 1) * factor {
                    let i = (y * image.width + x) * 3;
                    for (s, v) in sum.iter_mut().zip(&image.data[i..i + 3]) {
                        *s += v.min(1.0);
                    }
                }
            }
            let n = (factor * factor) as f32;
            let lin = sum.map(|s| s / n);
            for row in to_srgb {
                let v = (row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2]).clamp(0.0, 1.0);
                rgb.push((srgb_encode(v) * 255.0).round() as u8);
            }
        }
    }
    Ok((w as u32, h as u32, rgb))
}

/// The long edge a thumbnail is made at for the filmstrip, whose
/// items are 178 wide. The grid asks for more when its cells are
/// larger.
pub const THUMB_WIDTH: u32 = 170;

/// The camera's preview, box downscaled to `size` on its long edge
/// and turned by the engine's own orientation so it agrees with the
/// developed picture.
fn thumbnail_of_preview(
    preview: &image::RgbImage,
    orientation: greycard_core::raw::Orientation,
    size: usize,
) -> (u32, u32, Vec<u8>) {
    let (pw, ph) = (preview.width() as usize, preview.height() as usize);
    // The long side, as the develop's turned picture may be portrait.
    let factor = pw.max(ph).div_ceil(size).max(1);
    let (w, h) = ((pw / factor).max(1), (ph / factor).max(1));
    let mut data = Vec::with_capacity(w * h * 3);
    let px = preview.as_raw();
    for ty in 0..h {
        for tx in 0..w {
            let mut sum = [0u32; 3];
            for y in ty * factor..(ty + 1) * factor {
                for x in tx * factor..(tx + 1) * factor {
                    let i = (y * pw + x) * 3;
                    for (s, v) in sum.iter_mut().zip(&px[i..i + 3]) {
                        *s += *v as u32;
                    }
                }
            }
            let n = (factor * factor) as f32;
            data.extend(sum.map(|s| s as f32 / n / 255.0));
        }
    }
    let turned = greycard_core::develop::orient(
        WorkingImage {
            width: w,
            height: h,
            data,
        },
        orientation,
    );
    let rgb = turned
        .data
        .iter()
        .map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect();
    (turned.width as u32, turned.height as u32, rgb)
}

fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_s_own_frames_are_made_first_then_the_ones_beside_them() {
        let mut pending: Vec<(usize, PathBuf)> = (0..10)
            .map(|i| (i, PathBuf::from(format!("{i}"))))
            .collect();
        order_thumbnails(&mut pending, 4, 6);
        let order: Vec<usize> = pending.iter().map(|(i, _)| *i).collect();
        // The range in file order, then outward from its middle, 5.
        assert_eq!(order, vec![4, 5, 6, 3, 7, 2, 8, 1, 9, 0]);
    }

    /// Run a set of `names` (files of junk, which nothing decodes)
    /// through a worker of its own, cancelling the set as frame
    /// `cancel_at` is begun, and asking for an open (of junk, generation
    /// 7) as frame `open_at` is; what the worker said, in order, in
    /// short.
    fn run_fake_set(
        tag: &str,
        names: &[&str],
        cancel_at: Option<usize>,
        open_at: Option<usize>,
    ) -> Vec<String> {
        let dir = thumb_scratch(tag);
        let junk = dir.join("junk.CR3");
        std::fs::write(&junk, vec![0x33u8; 4096]).unwrap();
        // The worker's queue, for the deliver to put a job on as a
        // window would, mid-set.
        type Shared = Arc<(Mutex<Queue>, Condvar)>;
        let slot: Arc<std::sync::OnceLock<Shared>> = Arc::new(std::sync::OnceLock::new());
        let seen = slot.clone();
        let frames: Vec<crate::queue::Frame> = names
            .iter()
            .map(|n| {
                let source = dir.join(n);
                std::fs::write(&source, vec![0x5au8; 4096]).unwrap();
                crate::queue::Frame {
                    source: source.clone(),
                    edit: Edit::default(),
                    turn: 0,
                    seed_blend: false,
                    moved_from: None,
                    out: dir.join("out").join(format!("{n}.jpg")),
                }
            })
            .collect();
        let set = Arc::new(crate::queue::Set::new(
            frames.len(),
            Some(dir.join("out")),
            crate::export::Settings::default(),
            crate::export::OnExists::Increment,
        ));
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let worker = Worker::new(move |o| {
            let line = match o {
                Outcome::SetFrameStarted { set, index, name } => {
                    // Pressed while this frame is in hand: the deliver
                    // runs on the worker's thread, before the work.
                    if Some(index) == cancel_at {
                        set.cancel();
                    }
                    if Some(index) == open_at {
                        let (lock, cv) = &**seen.get().expect("the queue is handed over");
                        lock.lock().unwrap().develop = Some(Job::Open {
                            path: junk.clone(),
                            edit: Edit::default(),
                            generation: 7,
                            seed_blend: false,
                            turn: 0,
                        });
                        cv.notify_one();
                    }
                    format!("begin {index} {name}")
                }
                Outcome::Failed { generation, .. } => format!("develop {generation} failed"),
                Outcome::SetFrameDone { index, done, .. } => match done {
                    crate::queue::Done::Failed { message } => {
                        assert!(!message.is_empty());
                        format!("failed {index}")
                    }
                    crate::queue::Done::Canceled => format!("canceled {index}"),
                    other => format!("{other:?} {index}"),
                },
                Outcome::SetDone { tally, .. } => format!(
                    "done {} exported, {} failed, {} canceled",
                    tally.exported,
                    tally.failed.len(),
                    tally.canceled
                ),
                _ => return,
            };
            tx.lock().unwrap().send(line).unwrap();
        });
        slot.set(worker.queue.clone()).ok().unwrap();
        worker.send(Job::ExportSet {
            set: set.clone(),
            frames,
        });
        let mut said = Vec::new();
        loop {
            let line = rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("the worker answers");
            let done = line.starts_with("done");
            said.push(line);
            if done {
                break;
            }
        }
        worker.stop();
        assert!(!dir.join("out").exists(), "nothing was written");
        crate::testing::remove_dir_retry(&dir);
        said
    }

    /// A set whose every frame fails: each is begun in order and is
    /// one failure, the rest of the set goes on regardless, and the
    /// set is said done once, after its last, with the count.
    #[test]
    fn a_set_runs_in_order_and_counts_its_failures() {
        let said = run_fake_set("setfail", &["A.CR3", "B.CR3", "C.CR3"], None, None);
        assert_eq!(
            said,
            vec![
                "begin 0 A.CR3.jpg",
                "failed 0",
                "begin 1 B.CR3.jpg",
                "failed 1",
                "begin 2 C.CR3.jpg",
                "failed 2",
                "done 0 exported, 3 failed, 0 canceled",
            ]
        );
    }

    /// Canceled while the second of four is in hand: that one is
    /// finished, the last two are passed over without being begun.
    #[test]
    fn a_cancelled_set_finishes_the_frame_in_hand_and_stops() {
        let said = run_fake_set(
            "setcancel",
            &["A.CR3", "B.CR3", "C.CR3", "D.CR3"],
            Some(1),
            None,
        );
        assert_eq!(
            said,
            vec![
                "begin 0 A.CR3.jpg",
                "failed 0",
                "begin 1 B.CR3.jpg",
                "failed 1",
                "canceled 2",
                "canceled 3",
                "done 0 exported, 2 failed, 2 canceled",
            ]
        );
    }

    /// A stop that finds the worker still in a job says so and keeps
    /// its thread, so the quit can tell it is there and not run the C
    /// library's exit under it; once out of the job the worker sees
    /// the stop, ends, and is joined. The job held here is an open of
    /// junk whose failure the deliver sits on, on the worker's thread,
    /// as a develop sits in the driver.
    #[test]
    fn a_stop_that_finds_the_worker_in_a_job_keeps_it_until_it_ends() {
        let dir = thumb_scratch("busystop");
        let junk = dir.join("junk.CR3");
        std::fs::write(&junk, vec![0x33u8; 4096]).unwrap();
        let (entered, in_job) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (entered, released) = (Mutex::new(entered), Mutex::new(released));
        let worker = Worker::new(move |o| {
            if let Outcome::Failed { .. } = o {
                entered.lock().unwrap().send(()).unwrap();
                let _ = released.lock().unwrap().recv();
            }
        });
        assert!(!worker.finished(), "a worker not asked to stop is there");
        worker.send(Job::Open {
            path: junk,
            edit: Edit::default(),
            generation: 1,
            seed_blend: false,
            turn: 0,
        });
        in_job
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the worker takes the open");
        assert!(
            !worker.stop_within(std::time::Duration::from_millis(50)),
            "the stop finds it in its job"
        );
        assert!(worker.running(), "its thread is kept");
        assert!(!worker.finished());
        release.send(()).unwrap();
        let asked = Instant::now();
        while !worker.finished() {
            assert!(
                asked.elapsed() < std::time::Duration::from_secs(30),
                "out of its job, the worker ends"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!worker.running());
        assert!(worker.stop(), "a stop after the end has nothing to wait on");
        crate::testing::remove_dir_retry(&dir);
    }

    /// A stop that finds the worker between jobs joins it at once.
    #[test]
    fn a_stop_between_jobs_joins_the_worker() {
        let worker = Worker::new(|_| {});
        assert!(worker.stop());
        assert!(worker.finished());
        assert!(!worker.running());
    }

    /// A develop asked for while a frame of a set is in hand goes
    /// before the set's next frame: the set waits behind the window.
    #[test]
    fn a_develop_asked_for_mid_set_goes_before_the_next_frame() {
        let said = run_fake_set("setdevelop", &["A.CR3", "B.CR3", "C.CR3"], None, Some(0));
        assert_eq!(
            said,
            vec![
                "begin 0 A.CR3.jpg",
                "failed 0",
                "develop 7 failed",
                "begin 1 B.CR3.jpg",
                "failed 1",
                "begin 2 C.CR3.jpg",
                "failed 2",
                "done 0 exported, 3 failed, 0 canceled",
            ]
        );
    }

    #[test]
    fn a_panic_in_a_job_is_the_failure_that_job_was_waited_on_for() {
        let develop = Job::Develop {
            edit: Edit::default(),
            generation: 7,
            turn: 0,
        };
        // The panics here are on purpose; the hook has nothing to say
        // about them. The hook is the process's, and the other tests
        // run beside this one: theirs still print, or a failure
        // elsewhere in the run would be a bare FAILED with no words.
        let hook = std::panic::take_hook();
        let mine = std::thread::current().id();
        let hushed: std::sync::Arc<std::sync::Mutex<Option<_>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Some(hook)));
        let shared = hushed.clone();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().id() != mine
                && let Ok(guard) = shared.lock()
                && let Some(hook) = guard.as_ref()
            {
                hook(info);
            }
        }));
        let payload = std::panic::catch_unwind(|| panic!("on purpose {}", 1)).unwrap_err();
        let plain = std::panic::catch_unwind(|| panic!("plain")).unwrap_err();
        let _ = std::panic::take_hook();
        if let Some(hook) = hushed.lock().unwrap().take() {
            std::panic::set_hook(hook);
        }
        let message = panic_message(payload.as_ref());
        assert_eq!(message, "on purpose 1");
        match Blame::of(&develop).outcome(message) {
            Some(Outcome::Failed {
                generation,
                message,
            }) => {
                assert_eq!(generation, 7);
                assert!(message.contains("panicked: on purpose 1"), "{message}");
            }
            _ => panic!("not the develop's failure"),
        }
        let mask = Job::Mask {
            key: (3, 1),
            shape: Shape::Linear {
                from: [0.0, 0.0],
                to: [1.0, 1.0],
            },
            model: None,
        };
        assert!(matches!(
            Blame::of(&mask).outcome("x".into()),
            Some(Outcome::MaskFailed { key: (3, 1), .. })
        ));
        let thumbnail = Job::Thumbnail {
            index: 0,
            path: PathBuf::from("a"),
        };
        assert_eq!(Blame::of(&thumbnail), Blame::Nobody);
        assert!(Blame::Nobody.outcome("x".into()).is_none());
        assert_eq!(panic_message(plain.as_ref()), "plain");
    }

    /// A Bayer frame of a textured scene with lateral CA in it, large
    /// enough for the correction to run, in sensor units.
    fn aberrated_frame(width: usize, height: usize) -> RawFrame {
        use greycard_core::raw::{
            Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Samples, SensorLayout,
        };
        let pattern = CfaPattern::rggb();
        let texture = |x: f32, y: f32| {
            let v = (x * 0.11).sin() * (y * 0.07).cos()
                + (x * 0.031 + y * 0.052).sin()
                + ((x * 0.9 + y * 0.37) * 0.23).sin() * 0.5
                + ((x - y) * 0.017).cos() * 0.7;
            0.5 + 0.3 * v / 3.2
        };
        let (cx, cy) = ((width - 1) as f32 / 2.0, (height - 1) as f32 / 2.0);
        let mut samples = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let (scale, gain) = match pattern.color_at(y, x) {
                    CfaColor::Red => (1.005, 0.5),
                    CfaColor::Blue => (0.996, 0.25),
                    _ => (1.0, 1.0),
                };
                let sx = cx + (x as f32 - cx) / scale;
                let sy = cy + (y as f32 - cy) / scale;
                samples.push(100 + (texture(sx, sy) * gain * 1000.0) as u16);
            }
        }
        let m = greycard_core::color::WORKING_SPACE
            .from_xyz_matrix()
            .expect("the working space has a matrix");
        let color_matrix = m.rows.iter().flatten().map(|v| *v as f32).collect();
        RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            width,
            height,
            channels: 1,
            layout: SensorLayout::Cfa(pattern),
            samples: Samples::U16(samples),
            levels: Levels {
                black: LevelPattern::uniform(100.0, 1),
                white: LevelPattern::uniform(1100.0, 1),
            },
            as_shot_coefficients: Some([2.0, 1.0, 4.0]),
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix,
                forward_matrix: None,
            }],
            crop: None,
            orientation: greycard_core::raw::Orientation::Normal,
            shot: Default::default(),
        }
    }

    /// `develop_job` on `frame` under the default edit with `gpu`,
    /// from `base`: the outcome and the picture.
    fn develop_once(
        frame: &Arc<RawFrame>,
        base: &mut Option<Base>,
        gpu: &mut Option<greycard_gpu::Context>,
    ) -> (Outcome, Option<Arc<WorkingImage>>) {
        let deliver: Deliver = Arc::new(|_| {});
        develop_job(
            &Input::Raw(frame.clone()),
            &Edit::default(),
            0,
            1,
            base,
            &mut None,
            &mut Ai::new(),
            None,
            None,
            gpu,
            &deliver,
        )
    }

    /// `develop_job` on `frame` under `edit` at the frame's `turn`,
    /// on the CPU, from `base`: the picture.
    fn develop_turned(
        frame: &Arc<RawFrame>,
        edit: &Edit,
        turn: u8,
        base: &mut Option<Base>,
    ) -> Arc<WorkingImage> {
        let deliver: Deliver = Arc::new(|_| {});
        let (outcome, image) = develop_job(
            &Input::Raw(frame.clone()),
            edit,
            turn,
            1,
            base,
            &mut None,
            &mut Ai::new(),
            None,
            None,
            &mut None,
            &deliver,
        );
        assert!(matches!(outcome, Outcome::Developed { .. }));
        image.expect("a picture on the CPU")
    }

    /// A base turned where it stands is the base the engine makes at
    /// the new turn, to the bit, and so is its guide plane: for a
    /// frame whose tag is a plain turn and one whose tag mirrors, for
    /// a quarter each way and a half. And a develop that finds a base
    /// at another turn turns it rather than making it again, and
    /// develops what a fresh one would.
    #[test]
    fn a_base_turned_in_place_is_the_base_developed_at_that_turn() {
        use greycard_core::raw::Orientation;
        let edit = Edit::default();
        assert!(
            turns_exactly(&edit, None),
            "the check would not reach the turn"
        );
        for tag in [Orientation::Normal, Orientation::FlipHorizontal] {
            let mut frame = aberrated_frame(120, 80);
            frame.orientation = tag;
            let frame = Arc::new(frame);
            for (from, to) in [(0u8, 1u8), (1, 0), (0, 3), (1, 3), (2, 0)] {
                let mut kept = None;
                develop_turned(&frame, &edit, from, &mut kept);
                let mut b = kept.take().expect("a base");
                let stamp = b.stamp;
                turn_base(&mut b, &mut None, to, stamp + 1, None);
                let mut fresh = None;
                develop_turned(&frame, &edit, to, &mut fresh);
                let fresh = fresh.expect("a base");
                let what = format!("{tag:?}, {from} to {to}");
                assert_eq!(
                    (b.image.width, b.image.height),
                    (fresh.image.width, fresh.image.height),
                    "{what}"
                );
                assert!(
                    b.image.data == fresh.image.data,
                    "{what}: the pixels differ"
                );
                assert_eq!(b.guide.data, fresh.guide.data, "{what}: the guide differs");
                assert_eq!((b.turn, b.stamp), (to, stamp + 1));
                assert!(b.patched.is_none() && b.pre.is_none());

                // Through the develop: the base found at `from` is
                // turned and kept, not made again, and the develop is
                // the fresh one's.
                let mut base = None;
                develop_turned(&frame, &edit, from, &mut base);
                let made = base.as_ref().map(|b| b.stamp);
                let image = develop_turned(&frame, &edit, to, &mut base);
                let b = base.as_ref().expect("a base");
                assert_eq!(b.turn, to);
                assert_eq!(Some(b.stamp), made.map(|s| s + 1));
                let reference = develop_turned(&frame, &edit, to, &mut None);
                assert!(image.data == reference.data, "{what}: the develop differs");
            }
        }
    }

    /// The learned denoiser's pair turns with the base: at either end
    /// of the strength the base is one of the pair itself, and stays
    /// shared with it once both are turned rather than turned twice
    /// or copied; in between it is a blend of its own, turned. A pair
    /// made at another turn than the base's is not the base's, and is
    /// left for `learned_base` to make again.
    #[test]
    fn a_turned_base_takes_the_learned_pair_with_it() {
        use greycard_core::develop::orient;
        use greycard_core::raw::Orientation;
        let frame = Arc::new(aberrated_frame(120, 80));
        let edit = Edit::default();
        let mut made = None;
        develop_turned(&frame, &edit, 0, &mut made);
        let base = made.expect("a base");
        let plain = base.image.clone();
        let model = Arc::new(WorkingImage {
            data: plain.data.iter().map(|v| v * 0.5 + 0.1).collect(),
            ..(*plain).clone()
        });
        let pair = |turn: u8| LearnedBase {
            edit: edit.clone(),
            turn,
            model: Arc::new((*model).clone()),
            plain: Arc::new((*plain).clone()),
            white: WhiteBase::IDENTITY,
            radius: None,
            clip_level: 1.0,
            ca_on_gpu: false,
        };
        let turned = |image: &WorkingImage| orient(image.clone(), Orientation::Rotate90);
        for strength in [0.0f32, 0.5, 1.0] {
            let mut learned = Some(pair(0));
            let l = learned.as_ref().unwrap();
            let image = blend(&l.model, &l.plain, strength);
            let expected = turned(&image);
            let mut b = Base {
                edit: edit.clone(),
                turn: 0,
                image,
                guide: Arc::new(crate::finish::Guide::NONE),
                white: WhiteBase::IDENTITY,
                radius: None,
                clip_level: 1.0,
                source: crate::finish::Source::Scene,
                stamp: 1,
                patched: None,
                pre: None,
                uploaded: None,
                haze: None,
                ca_on_gpu: false,
                stand_in: None,
                profile: None,
            };
            turn_base(&mut b, &mut learned, 1, 2, None);
            let l = learned.as_ref().unwrap();
            assert_eq!(l.turn, 1, "strength {strength}");
            assert!(
                l.model.data == turned(&model).data,
                "strength {strength}: the model"
            );
            assert!(
                l.plain.data == turned(&plain).data,
                "strength {strength}: the plain"
            );
            assert_eq!((b.image.width, b.image.height), (80, 120));
            assert!(
                b.image.data == expected.data,
                "strength {strength}: the base"
            );
            match strength {
                1.0 => assert!(Arc::ptr_eq(&b.image, &l.model), "the model, shared"),
                0.0 => assert!(Arc::ptr_eq(&b.image, &l.plain), "the plain, shared"),
                _ => assert!(!Arc::ptr_eq(&b.image, &l.model) && !Arc::ptr_eq(&b.image, &l.plain)),
            }
        }
        // A pair at another turn stays as it was.
        let mut learned = Some(pair(3));
        let mut b = Base {
            edit: edit.clone(),
            turn: 0,
            image: plain.clone(),
            guide: Arc::new(crate::finish::Guide::NONE),
            white: WhiteBase::IDENTITY,
            radius: None,
            clip_level: 1.0,
            source: crate::finish::Source::Scene,
            stamp: 1,
            patched: None,
            pre: None,
            uploaded: None,
            haze: None,
            ca_on_gpu: false,
            stand_in: None,
            profile: None,
        };
        turn_base(&mut b, &mut learned, 1, 2, None);
        let l = learned.as_ref().unwrap();
        assert_eq!(l.turn, 3);
        assert!(l.plain.data == plain.data && l.model.data == model.data);
        assert!(b.image.data == turned(&plain).data);
    }

    /// A context on a device of our own with `limits`, for driving
    /// the GPU paths into their errors; none without an adapter.
    fn context_with(limits: greycard_gpu::wgpu::Limits) -> Option<greycard_gpu::Context> {
        use greycard_gpu::wgpu;
        let instance = greycard_gpu::instance();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("worker test"),
            required_limits: limits,
            ..Default::default()
        }))
        .ok()?;
        greycard_gpu::Context::from_device(&device, &queue).ok()
    }

    /// A GPU test with no adapter says so and returns; under
    /// `GREYCARD_REQUIRE_GPU` (CI's software adapter, lavapipe here)
    /// that is a failure, as greycard-gpu's own tests have it.
    fn skipped(what: &str) {
        assert!(
            std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
            "{what} has no GPU to run on and GREYCARD_REQUIRE_GPU is set"
        );
        eprintln!("SKIPPED: {what} has no GPU to run on");
        println!("SKIPPED: {what} has no GPU to run on");
    }

    /// The export's picture is the reference's: a base whose CA ran on
    /// the GPU is not reused by a develop without one (the export's),
    /// which makes the base afresh on the CPU, and that picture is the
    /// one a session that never had a GPU makes, byte for byte.
    #[test]
    fn an_export_after_a_gpu_base_develops_the_reference_s() {
        let Some(ctx) = greycard_gpu::Context::own().ok() else {
            skipped("the export's base check");
            return;
        };
        let frame = Arc::new(aberrated_frame(400, 320));
        let mut gpu = Some(ctx);
        let mut base = None;
        let (outcome, _) = develop_once(&frame, &mut base, &mut gpu);
        assert!(matches!(outcome, Outcome::Developed { .. }));
        assert!(base.as_ref().is_some_and(|b| b.ca_on_gpu));
        let stamp = base.as_ref().unwrap().stamp;
        // The export: no GPU, the same edit.
        let (outcome, exported) = develop_once(&frame, &mut base, &mut None);
        assert!(matches!(outcome, Outcome::Developed { .. }));
        let b = base.as_ref().unwrap();
        assert!(
            !b.ca_on_gpu && b.stamp == stamp + 1,
            "a fresh base on the CPU"
        );
        // And once made on the CPU the base is kept.
        let (_, again) = develop_once(&frame, &mut base, &mut None);
        assert_eq!(base.as_ref().unwrap().stamp, stamp + 1);
        // The reference's own develop, from nothing.
        let (_, reference) = develop_once(&frame, &mut None, &mut None);
        let (exported, again, reference) = (exported.unwrap(), again.unwrap(), reference.unwrap());
        assert_eq!(exported.data, reference.data);
        assert_eq!(again.data, reference.data);
    }

    /// A learned pair whose CA ran on the GPU serves a develop with a
    /// GPU and not one without, which makes the pair again on the CPU;
    /// a pair whose CA was the CPU's serves both. The turn and the
    /// learned settings still have to match.
    #[test]
    fn a_gpu_ca_learned_pair_is_not_served_to_an_export() {
        let edit = Edit::default();
        let image = Arc::new(WorkingImage::new(4, 4));
        let pair = |ca_on_gpu: bool| LearnedBase {
            edit: edit.clone(),
            turn: 0,
            model: image.clone(),
            plain: image.clone(),
            white: WhiteBase::IDENTITY,
            radius: None,
            clip_level: 1.0,
            ca_on_gpu,
        };
        let gpu = pair(true);
        assert!(pair_serves(&gpu, 0, &edit, true));
        assert!(
            !pair_serves(&gpu, 0, &edit, false),
            "the export makes it again"
        );
        let cpu = pair(false);
        assert!(pair_serves(&cpu, 0, &edit, true));
        assert!(pair_serves(&cpu, 0, &edit, false));
        assert!(!pair_serves(&cpu, 1, &edit, false), "another turn");
        let mut other = edit.clone();
        other.noise.learned = greycard_edit::Learned::Best;
        assert!(
            !pair_serves(&cpu, 0, &other, false),
            "other learned settings"
        );
    }

    /// The export, with no window and no panel, makes a mask's white
    /// balance from its own develop: the base keeps the profile its
    /// white was resolved through, and a mask over everything at that
    /// same white moves nothing while one at another moves the picture
    /// the way the matrix says.
    #[test]
    fn an_export_makes_a_masks_white_balance_from_its_own_develop() {
        let frame = Arc::new(aberrated_frame(200, 160));
        // Developed at 5000 K, inside the range a mask's white takes.
        let global = Edit {
            white_balance: greycard_edit::WhiteBalance::Custom {
                temperature: 5000.0,
                tint: 0.002,
            },
            ..Edit::default()
        };
        let mut base = None;
        let image = develop_turned(&frame, &global, 0, &mut base);
        assert!(
            base.as_ref().unwrap().white_shift().is_some(),
            "a raw's base keeps its profile"
        );
        let everywhere = |white: greycard_edit::LocalWhite| greycard_edit::Adjustment {
            id: 1,
            mask: greycard_edit::mask::Mask {
                components: vec![greycard_edit::mask::Component {
                    shape: Shape::Linear {
                        from: [0.5, 10.0],
                        to: [0.5, 11.0],
                    },
                    ..Default::default()
                }],
                invert: false,
            },
            look: greycard_edit::Look {
                white_balance: white,
                ..Default::default()
            },
            ..Default::default()
        };
        let settings = crate::export::Settings {
            format: crate::export::Format::Tiff,
            sharpen: crate::export::Sharpen::Off,
            ..Default::default()
        };
        let mut ai = Ai::new();
        let mut render = |white| {
            let mut edit = global.clone();
            edit.adjustments.push(everywhere(white));
            let (r, _) = finish_export(image.clone(), &edit, base.as_ref(), &mut ai, &settings);
            let crate::export::Pixels::Sixteen(p) = r.pixels else {
                panic!("a TIFF is sixteen bits")
            };
            p
        };
        let off = render(greycard_edit::LocalWhite::Off);
        let same = render(greycard_edit::LocalWhite::Absolute {
            temperature: 5000.0,
            tint: 0.002,
        });
        let worst = off.iter().zip(&same).map(|(a, b)| a.abs_diff(*b)).max();
        assert!(worst <= Some(2), "{worst:?}");
        let warm = render(greycard_edit::LocalWhite::Absolute {
            temperature: 2800.0,
            tint: 0.0,
        });
        // Warmer light named: the picture comes out bluer.
        let blue = |p: &[u16]| {
            p.chunks(3).map(|c| c[2] as f64 - c[0] as f64).sum::<f64>() / (p.len() / 3) as f64
        };
        assert!(
            blue(&warm) > blue(&off) + 500.0,
            "{} {}",
            blue(&warm),
            blue(&off)
        );
    }

    /// A last develop whose CA ran on the GPU is not handed to an export:
    /// it develops afresh on the CPU, and that picture is then reused.
    #[test]
    fn an_export_does_not_reuse_a_gpu_ca_develop() {
        let frame = Arc::new(aberrated_frame(400, 320));
        // The reference, from nothing.
        let (_, reference) = develop_once(&frame, &mut None, &mut None);
        let reference = reference.unwrap();
        // A base as a GPU session leaves it, and a last develop from it
        // that is plainly not the reference: a flat grey picture.
        let mut base = None;
        let (_, image) = develop_once(&frame, &mut base, &mut None);
        let image = image.unwrap();
        base.as_mut().unwrap().ca_on_gpu = true;
        let wrong = Arc::new(
            WorkingImage::from_data(image.width, image.height, vec![0.5; image.data.len()])
                .unwrap(),
        );
        let mut last = Some(Last::made(
            Edit::default(),
            base.as_ref().unwrap(),
            wrong.clone(),
            &Outcome::ExportSkipped {
                path: PathBuf::new(),
            },
        ));
        let mut learned = None;
        let mut ai = Ai::new();
        let deliver: Deliver = Arc::new(|_| {});
        let input = Input::Raw(frame.clone());
        let mut export = |last: &mut Option<Last>, base: &mut Option<Base>, ai: &mut Ai| {
            open_picture(
                &Edit::default(),
                0,
                Some(&input),
                last,
                base,
                &mut learned,
                ai,
                None,
                None,
                &deliver,
            )
            .unwrap()
            .0
        };
        let got = export(&mut last, &mut base, &mut ai);
        assert!(!Arc::ptr_eq(&got, &wrong), "the GPU develop was reused");
        assert_eq!(
            got.data, reference.data,
            "the export's picture is the reference's"
        );
        assert!(
            last.as_ref().is_some_and(|l| !l.ca_on_gpu),
            "the last develop is the CPU's now"
        );
        assert!(base.as_ref().is_some_and(|b| !b.ca_on_gpu));
        // Now that it is the CPU's, it is reused.
        let again = export(&mut last, &mut base, &mut ai);
        assert!(Arc::ptr_eq(&got, &again), "the CPU develop is kept");
    }

    /// A device error in the CA correction falls back to the reference
    /// for that develop and drops the context for the session; a
    /// picture the device cannot hold falls back for that op and keeps
    /// it. A device allowed 20 workgroups a dimension cannot run the
    /// 400x320's 26; one whose textures stop at 512 a side holds a
    /// 500-wide picture but not the CA's padded green plane.
    #[test]
    fn a_gpu_error_in_the_ca_falls_back_and_an_unsupported_size_keeps_the_context() {
        use greycard_gpu::wgpu::Limits;
        let Some(ctx) = context_with(Limits {
            max_compute_workgroups_per_dimension: 20,
            ..Limits::default()
        }) else {
            skipped("the CA fallback check");
            return;
        };
        let frame = Arc::new(aberrated_frame(400, 320));
        let mut gpu = Some(ctx);
        let mut base = None;
        let (outcome, image) = develop_once(&frame, &mut base, &mut gpu);
        assert!(matches!(outcome, Outcome::Developed { .. }));
        assert!(gpu.is_none(), "the context is dropped after a device error");
        assert!(base.as_ref().is_some_and(|b| !b.ca_on_gpu));
        let (_, reference) = develop_once(&frame, &mut None, &mut None);
        assert_eq!(image.unwrap().data, reference.unwrap().data);

        let Some(ctx) = context_with(Limits {
            max_texture_dimension_2d: 512,
            ..Limits::default()
        }) else {
            skipped("the CA unsupported-size check");
            return;
        };
        let frame = Arc::new(aberrated_frame(500, 300));
        let mut gpu = Some(ctx);
        let mut base = None;
        let (outcome, _) = develop_once(&frame, &mut base, &mut gpu);
        assert!(matches!(outcome, Outcome::Developed { .. }));
        assert!(
            gpu.is_some(),
            "the context is kept for a picture it cannot hold"
        );
        assert!(base.as_ref().is_some_and(|b| !b.ca_on_gpu));
    }

    /// The Detail section on the GPU: a move runs the local contrast
    /// there from the patched picture's upload, which is kept for the
    /// next; the dehaze runs there after it, from the same upload, its
    /// reduced copy kept for a move of the Dehaze alone and made again
    /// when the local contrast moves; with the sharpen off the last op
    /// writes the viewport's texture itself; with neither op on,
    /// nothing is kept. And each path keeps on the device only what it
    /// uses: the local contrast's planes those its sliders need (none
    /// when they are made for each run, as on an integrated GPU), none
    /// with the Detail section off, and the sharpen's none with the
    /// sharpen off.
    #[test]
    fn the_detail_section_runs_on_the_gpu_from_the_kept_upload() {
        for keep in [true, false] {
            let Some(ctx) = greycard_gpu::Context::own().ok() else {
                skipped("the Detail section's GPU path");
                return;
            };
            ctx.keep_local_contrast_planes(keep);
            detail_section_on_the_gpu(ctx, keep);
        }
    }

    fn detail_section_on_the_gpu(ctx: greycard_gpu::Context, keep: bool) {
        let frame = Arc::new(aberrated_frame(400, 320));
        let deliver: Deliver = Arc::new(|_| {});
        let mut gpu = Some(ctx);
        let mut base = None;
        let develop = |edit: &Edit, base: &mut Option<Base>, gpu: &mut Option<_>| {
            develop_job(
                &Input::Raw(frame.clone()),
                edit,
                0,
                1,
                base,
                &mut None,
                &mut Ai::new(),
                None,
                None,
                gpu,
                &deliver,
            )
        };
        let kept = |gpu: &Option<greycard_gpu::Context>| gpu.as_ref().unwrap().kept_bytes();
        let mut edit = Edit::default();
        edit.detail.clarity = 0.5;
        let (outcome, image) = develop(&edit, &mut base, &mut gpu);
        assert!(
            matches!(
                outcome,
                Outcome::Developed {
                    image: Developed::Texture(_),
                    detail: Some((_, DetailRan::Gpu)),
                    sharpen: Some(_),
                    ..
                }
            ),
            "the local contrast and the sharpen on the GPU"
        );
        assert!(image.is_none());
        let b = base.as_ref().unwrap();
        let uploaded = b
            .uploaded
            .as_ref()
            .expect("the patched picture is kept")
            .1
            .clone();
        assert!(b.pre.as_ref().is_some_and(|p| p.detail_stats.is_some()));
        // A plane of the picture's size; Clarity's exact filter at this
        // size takes the log, its scratch, and the slope and intercept.
        let plane = u64::from(uploaded.width()) * u64::from(uploaded.height()) * 4;
        let planes = |n: u64| if keep { n * plane } else { 0 };
        assert!(kept(&gpu).sharpen > 0);
        assert_eq!(kept(&gpu).local_contrast, planes(4), "Clarity's planes");
        // Another move keeps the upload and remakes the picture
        // before the sharpen from it.
        edit.detail.clarity = -0.5;
        let (outcome, _) = develop(&edit, &mut base, &mut gpu);
        assert!(matches!(
            outcome,
            Outcome::Developed {
                image: Developed::Texture(_),
                detail: Some((_, DetailRan::Gpu)),
                ..
            }
        ));
        let b = base.as_ref().unwrap();
        assert!(Arc::ptr_eq(&b.uploaded.as_ref().unwrap().1, &uploaded));
        assert!(b.pre.as_ref().is_some_and(|p| p.detail == edit.detail));
        // A sharpen move keeps the picture before it, and says so.
        edit.sharpen.iterations += 1;
        let (outcome, _) = develop(&edit, &mut base, &mut gpu);
        assert!(matches!(
            outcome,
            Outcome::Developed {
                detail: Some((_, DetailRan::Kept)),
                sharpen: Some(_),
                ..
            }
        ));
        // Texture as well: its gain beside them.
        edit.detail.texture = 0.5;
        develop(&edit, &mut base, &mut gpu);
        assert_eq!(kept(&gpu).local_contrast, planes(5), "Texture's planes too");
        // The Detail section off, the sharpen on: the sharpen reads the
        // upload itself, and the local contrast keeps nothing.
        edit.detail.texture = 0.0;
        edit.detail.clarity = 0.0;
        develop(&edit, &mut base, &mut gpu);
        let b = base.as_ref().unwrap();
        assert!(Arc::ptr_eq(&b.pre.as_ref().unwrap().image, &uploaded));
        assert_eq!(kept(&gpu).local_contrast, 0, "no Detail, no planes");
        // The dehaze after the local contrast, both on the GPU from the
        // kept upload; its reduced copy kept with what it was made from.
        edit.detail.clarity = -0.5;
        develop(&edit, &mut base, &mut gpu);
        assert_eq!(kept(&gpu).local_contrast, planes(4));
        assert!(base.as_ref().unwrap().haze.is_none());
        edit.detail.dehaze = 0.5;
        let (outcome, _) = develop(&edit, &mut base, &mut gpu);
        let Outcome::Developed {
            image: Developed::Texture(_),
            detail: Some((_, DetailRan::Gpu)),
            dehaze: Some(first),
            ..
        } = outcome
        else {
            panic!("the local contrast and the dehaze on the GPU");
        };
        let b = base.as_ref().unwrap();
        assert!(Arc::ptr_eq(&b.uploaded.as_ref().unwrap().1, &uploaded));
        assert!(b.pre.as_ref().is_some_and(|p| p.dehaze_stats.is_some()));
        let haze = b.haze.as_ref().expect("the reduced copy is kept");
        assert_eq!(haze.detail, edit.detail.options());
        let reduced = haze.reduced.clone();
        assert_eq!(kept(&gpu).local_contrast, planes(4));
        assert!(kept(&gpu).sharpen > 0);
        // A move of the Dehaze alone fits again from the kept copy, so
        // the airlight is the same.
        edit.detail.dehaze = 0.8;
        let (outcome, _) = develop(&edit, &mut base, &mut gpu);
        let Outcome::Developed {
            dehaze: Some(second),
            ..
        } = outcome
        else {
            panic!("a Dehaze move");
        };
        let b = base.as_ref().unwrap();
        assert!(Arc::ptr_eq(&b.haze.as_ref().unwrap().reduced, &reduced));
        assert_eq!(second.airlight, first.airlight);
        assert!(second.strength > first.strength);
        // A sharpen move keeps the picture before it, dehaze and all.
        edit.sharpen.iterations += 1;
        let (outcome, _) = develop(&edit, &mut base, &mut gpu);
        assert!(matches!(
            outcome,
            Outcome::Developed {
                detail: Some((_, DetailRan::Kept)),
                dehaze: Some(_),
                ..
            }
        ));
        // A move of the local contrast makes the copy again.
        edit.detail.clarity = -0.3;
        develop(&edit, &mut base, &mut gpu);
        let b = base.as_ref().unwrap();
        assert!(!Arc::ptr_eq(&b.haze.as_ref().unwrap().reduced, &reduced));
        // The dehaze alone: from the upload, the local contrast's planes
        // let go.
        edit.detail.clarity = 0.0;
        let (outcome, _) = develop(&edit, &mut base, &mut gpu);
        assert!(matches!(
            outcome,
            Outcome::Developed {
                image: Developed::Texture(_),
                detail: None,
                dehaze: Some(_),
                ..
            }
        ));
        let b = base.as_ref().unwrap();
        assert_eq!(b.haze.as_ref().unwrap().detail, None);
        assert_eq!(kept(&gpu).local_contrast, 0, "no local contrast, no planes");
        // With the sharpen off, the dehaze writes the viewport's
        // texture, after the local contrast or alone.
        edit.sharpen.enabled = false;
        for clarity in [0.4, 0.0] {
            edit.detail.clarity = clarity;
            let (outcome, image) = develop(&edit, &mut base, &mut gpu);
            assert!(
                matches!(
                    outcome,
                    Outcome::Developed {
                        image: Developed::Texture(_),
                        sharpen: None,
                        dehaze: Some(_),
                        ..
                    }
                ),
                "the dehaze into the viewport's texture"
            );
            assert!(image.is_none());
            let b = base.as_ref().unwrap();
            assert!(b.pre.is_none() && b.haze.is_some());
            assert_eq!(kept(&gpu).sharpen, 0);
        }
        edit.sharpen.enabled = true;
        edit.detail.clarity = -0.5;
        // Sharpen off, dehaze off: the op writes the viewport's texture
        // itself, and nothing waits for a sharpen: its planes go.
        edit.detail.dehaze = 0.0;
        edit.sharpen.enabled = false;
        let (outcome, image) = develop(&edit, &mut base, &mut gpu);
        assert!(
            matches!(
                outcome,
                Outcome::Developed {
                    image: Developed::Texture(_),
                    detail: Some((_, DetailRan::Gpu)),
                    sharpen: None,
                    ..
                }
            ),
            "the local contrast alone, on the GPU"
        );
        assert!(image.is_none());
        let b = base.as_ref().unwrap();
        assert!(b.uploaded.is_some() && b.pre.is_none());
        assert_eq!(kept(&gpu).sharpen, 0, "the sharpen's planes are let go");
        assert_eq!(kept(&gpu).local_contrast, planes(4));
        // Neither op: the CPU's halves, and nothing kept on the device.
        edit.detail.clarity = 0.0;
        let (outcome, image) = develop(&edit, &mut base, &mut gpu);
        assert!(matches!(
            outcome,
            Outcome::Developed {
                image: Developed::Halves(_),
                detail: None,
                sharpen: None,
                ..
            }
        ));
        assert!(image.is_some());
        let b = base.as_ref().unwrap();
        assert!(b.uploaded.is_none() && b.pre.is_none());
        assert_eq!(kept(&gpu), greycard_gpu::KeptBytes::default());
        assert!(gpu.is_some(), "the context is kept throughout");
    }

    /// The Detail section on the GPU gives the picture the CPU's does:
    /// the local contrast, then the dehaze, in the reference's order,
    /// with the sharpen off so that the viewport's texture is the
    /// Detail section's output alone. From one base made on the CPU,
    /// so that the CA correction is the same on both. The two differ by
    /// the viewport's half floats, the local contrast's rounding and
    /// the dehaze's gain on it; the dehaze run before the local
    /// contrast would be percents off.
    #[test]
    fn the_detail_section_on_the_gpu_is_the_cpus() {
        let Some(ctx) = greycard_gpu::Context::own().ok() else {
            skipped("the Detail section's GPU picture");
            return;
        };
        let frame = Arc::new(aberrated_frame(400, 320));
        let deliver: Deliver = Arc::new(|_| {});
        let mut gpu = Some(ctx);
        let mut base = None;
        let mut edit = Edit::default();
        edit.sharpen.enabled = false;
        for (texture, clarity, dehaze) in [(0.4, 0.6, 0.7), (0.0, 0.0, -0.8), (-0.5, 0.3, 1.0)] {
            edit.detail.texture = texture;
            edit.detail.clarity = clarity;
            edit.detail.dehaze = dehaze;
            let mut develop = |gpu: &mut Option<greycard_gpu::Context>| {
                develop_job(
                    &Input::Raw(frame.clone()),
                    &edit,
                    0,
                    1,
                    &mut base,
                    &mut None,
                    &mut Ai::new(),
                    None,
                    None,
                    gpu,
                    &deliver,
                )
            };
            let (cpu_outcome, cpu) = develop(&mut None);
            let (outcome, _) = develop(&mut gpu);
            let (
                Outcome::Developed {
                    dehaze: Some(cpu_stats),
                    ..
                },
                Outcome::Developed {
                    image: Developed::Texture(out),
                    dehaze: Some(gpu_stats),
                    ..
                },
            ) = (cpu_outcome, outcome)
            else {
                panic!("both develops, the GPU's into a texture");
            };
            let cpu = cpu.unwrap();
            let viewport = read_viewport(gpu.as_ref().unwrap(), &out);
            let worst = cpu
                .data
                .iter()
                .zip(&viewport.data)
                .map(|(a, b)| (a - b).abs() / a.abs().max(0.05))
                .fold(0.0, f32::max);
            println!(
                "texture {texture}, clarity {clarity}, dehaze {dehaze}: worst {worst:.2e}; airlight {:?} / {:?}",
                cpu_stats.airlight, gpu_stats.airlight
            );
            // Two of a half float's ulps.
            assert!(
                worst < 2e-3,
                "the viewport is {worst:.2e} off the CPU's picture"
            );
            for c in 0..3 {
                assert!((cpu_stats.airlight[c] - gpu_stats.airlight[c]).abs() < 1e-4);
            }
            assert!((cpu_stats.transmission_mean - gpu_stats.transmission_mean).abs() < 1e-4);
        }
    }

    /// A GPU error in the dehaze is warned once and takes the session
    /// to the CPU, as the other ops' do: here a device whose storage
    /// buffers are too small for the dehaze's block sums, which its
    /// error scope catches. The develop that met it is the CPU's, to
    /// the bit.
    #[test]
    fn a_gpu_error_in_the_dehaze_takes_the_session_to_the_cpu() {
        let Some(ctx) = context_with(greycard_gpu::wgpu::Limits {
            max_storage_buffer_binding_size: 256,
            ..greycard_gpu::wgpu::Limits::default()
        }) else {
            skipped("the dehaze's device error");
            return;
        };
        let frame = Arc::new(aberrated_frame(400, 320));
        let deliver: Deliver = Arc::new(|_| {});
        let mut edit = Edit::default();
        edit.detail.dehaze = 0.6;
        for sharpen in [true, false] {
            edit.sharpen.enabled = sharpen;
            let mut gpu = Some(
                context_with(greycard_gpu::wgpu::Limits {
                    max_storage_buffer_binding_size: 256,
                    ..greycard_gpu::wgpu::Limits::default()
                })
                .unwrap(),
            );
            let mut base = None;
            let mut develop = |gpu: &mut Option<greycard_gpu::Context>| {
                develop_job(
                    &Input::Raw(frame.clone()),
                    &edit,
                    0,
                    1,
                    &mut base,
                    &mut None,
                    &mut Ai::new(),
                    None,
                    None,
                    gpu,
                    &deliver,
                )
            };
            let (_, cpu) = develop(&mut None);
            let (outcome, image) = develop(&mut gpu);
            assert!(gpu.is_none(), "the context is dropped after a device error");
            assert!(matches!(
                outcome,
                Outcome::Developed {
                    image: Developed::Halves(_),
                    dehaze: Some(_),
                    ..
                }
            ));
            assert_eq!(image.unwrap().data, cpu.unwrap().data);
        }
        drop(ctx);
    }

    /// A picture larger than the device's textures, with a Detail
    /// slider set, is the CPU's for that develop, with the sharpen off
    /// and on, and the context is kept: the next picture that fits is
    /// the GPU's again. No texture is made for it before the upload
    /// has found it fits, so nothing reaches wgpu's uncaptured handler
    /// (which panics the worker).
    #[test]
    fn a_picture_the_device_cannot_hold_is_the_cpus_for_that_develop() {
        let Some(ctx) = context_with(greycard_gpu::wgpu::Limits {
            max_texture_dimension_2d: 256,
            ..greycard_gpu::wgpu::Limits::default()
        }) else {
            skipped("the oversized picture's develop");
            return;
        };
        let deliver: Deliver = Arc::new(|_| {});
        let mut gpu = Some(ctx);
        let large = Arc::new(aberrated_frame(400, 320));
        let small = Arc::new(aberrated_frame(200, 160));
        for sharpen in [false, true] {
            for dehaze in [0.0, 0.6] {
                let mut edit = Edit::default();
                edit.sharpen.enabled = sharpen;
                edit.detail.clarity = 0.5;
                edit.detail.dehaze = dehaze;
                let develop =
                    |frame: &Arc<RawFrame>,
                     base: &mut Option<Base>,
                     gpu: &mut Option<greycard_gpu::Context>| {
                        develop_job(
                            &Input::Raw(frame.clone()),
                            &edit,
                            0,
                            1,
                            base,
                            &mut None,
                            &mut Ai::new(),
                            None,
                            None,
                            gpu,
                            &deliver,
                        )
                    };
                let what = format!("sharpen {sharpen}, dehaze {dehaze}");
                let mut base = None;
                let (outcome, image) = develop(&large, &mut base, &mut gpu);
                assert!(
                    matches!(
                        outcome,
                        Outcome::Developed {
                            image: Developed::Halves(_),
                            detail: Some((_, DetailRan::Cpu(_))),
                            ..
                        }
                    ),
                    "{what}: the CPU's develop"
                );
                assert!(gpu.is_some(), "{what}: the context is kept");
                let (_, reference) = develop(&large, &mut None, &mut None);
                assert_eq!(image.unwrap().data, reference.unwrap().data, "{what}");
                let b = base.as_ref().unwrap();
                assert!(b.uploaded.is_none() && b.pre.is_none() && b.haze.is_none());
                let (outcome, _) = develop(&small, &mut None, &mut gpu);
                assert!(
                    matches!(
                        outcome,
                        Outcome::Developed {
                            image: Developed::Texture(_),
                            detail: Some((_, DetailRan::Gpu)),
                            ..
                        }
                    ),
                    "{what}: the next picture that fits is the GPU's"
                );
            }
        }
    }

    /// A move of the Dehaze alone, which fits again from the kept
    /// reduced copy, gives the picture a develop made afresh at that
    /// amount gives, to the bit: with the sharpen off (the dehaze's own
    /// output in the viewport's texture) and on.
    #[test]
    fn a_dehaze_move_from_the_kept_copy_is_a_fresh_develop() {
        let Some(ctx) = greycard_gpu::Context::own().ok() else {
            skipped("the Dehaze move's picture");
            return;
        };
        let frame = Arc::new(aberrated_frame(400, 320));
        let deliver: Deliver = Arc::new(|_| {});
        let mut gpu = Some(ctx);
        for sharpen in [false, true] {
            let mut edit = Edit::default();
            edit.sharpen.enabled = sharpen;
            edit.detail.texture = 0.3;
            edit.detail.clarity = 0.5;
            let mut develop = |edit: &Edit, base: &mut Option<Base>| {
                let (outcome, _) = develop_job(
                    &Input::Raw(frame.clone()),
                    edit,
                    0,
                    1,
                    base,
                    &mut None,
                    &mut Ai::new(),
                    None,
                    None,
                    &mut gpu,
                    &deliver,
                );
                let Outcome::Developed {
                    image: Developed::Texture(texture),
                    dehaze: Some(_),
                    ..
                } = outcome
                else {
                    panic!("the dehaze on the GPU, into a texture");
                };
                texture
            };
            let mut base = None;
            edit.detail.dehaze = 0.4;
            develop(&edit, &mut base);
            let reduced = base
                .as_ref()
                .unwrap()
                .haze
                .as_ref()
                .unwrap()
                .reduced
                .clone();
            edit.detail.dehaze = 0.9;
            let moved = develop(&edit, &mut base);
            let kept = &base.as_ref().unwrap().haze.as_ref().unwrap().reduced;
            assert!(
                Arc::ptr_eq(kept, &reduced),
                "the move fitted from the kept copy"
            );
            let fresh = develop(&edit, &mut None);
            let ctx = gpu.as_ref().unwrap();
            let (moved, fresh) = (read_viewport(ctx, &moved), read_viewport(ctx, &fresh));
            assert!(
                moved.data == fresh.data,
                "sharpen {sharpen}: the move's picture is not a fresh develop's"
            );
        }
    }

    #[test]
    fn the_blend_is_linear_and_shares_at_the_ends() {
        let model = Arc::new(WorkingImage::from_data(2, 1, vec![1.0; 6]).unwrap());
        let plain = Arc::new(WorkingImage::from_data(2, 1, vec![0.0; 6]).unwrap());
        assert!(Arc::ptr_eq(&blend(&model, &plain, 1.0), &model));
        assert!(Arc::ptr_eq(&blend(&model, &plain, 0.0), &plain));
        assert!(Arc::ptr_eq(&blend(&model, &plain, 2.0), &model));
        let half = blend(&model, &plain, 0.25);
        assert!(half.data.iter().all(|v| (*v - 0.25).abs() < 1e-6));
    }

    fn thumb_scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-thumbjob-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cache_at(dir: &std::path::Path) -> ThumbCache {
        Arc::new(Mutex::new(Some(Thumbs::at(
            dir.join("thumbs"),
            greycard_library::thumbs::DEFAULT_CAP,
        ))))
    }

    /// A raw nothing can decode: its thumbnail can only have come
    /// from the cache.
    #[test]
    fn a_cached_thumbnail_is_not_made_again() {
        let dir = thumb_scratch("hit");
        let raw = dir.join("IMG_0001.CR3");
        std::fs::write(&raw, vec![0x17u8; 90_000]).unwrap();
        let cache = cache_at(&dir);
        assert!(
            cached_thumbnail_noting(&cache, None, None, &raw, THUMB_WIDTH, thumbnail).is_err(),
            "not a raw, and nothing cached"
        );
        let kept = Thumb {
            width: 3,
            height: 2,
            rgb: vec![128; 18],
        };
        let hash = greycard_library::hash_file(&raw).unwrap();
        cache
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .put(
                &hash,
                THUMB_WIDTH,
                thumb_tag(file_stat(&raw).unwrap()),
                &kept,
            )
            .unwrap();
        // Renamed into another folder: still the cache's.
        let moved_dir = dir.join("renamed");
        std::fs::create_dir_all(&moved_dir).unwrap();
        let moved = moved_dir.join("wedding-0001.CR3");
        std::fs::rename(&raw, &moved).unwrap();
        let (thumb, cached) =
            cached_thumbnail_noting(&cache, None, None, &moved, THUMB_WIDTH, thumbnail).unwrap();
        assert!(cached);
        assert_eq!((thumb.width, thumb.height), (3, 2));
        // Another size is not the same entry.
        assert!(cached_thumbnail_noting(&cache, None, None, &moved, 256, thumbnail).is_err());
        crate::testing::remove_dir_retry(&dir);
    }

    /// What keeping the open frame's edit costs, on the raws of the
    /// folder `GREYCARD_SAMPLES` names (ignored; run it in release with
    /// `--ignored --nocapture`): each raw developed once on the CPU under
    /// a brightening, then its pictures made as a save makes them, from
    /// the CPU's picture (reduced, straightened, fitted, finished,
    /// shrunk, encoded and kept) and, when there is a GPU, the same
    /// picture read back from a viewport texture a whole factor smaller.
    #[test]
    #[ignore]
    fn the_edit_s_pictures_of_the_samples() {
        let Some(dir) = std::env::var_os("GREYCARD_SAMPLES") else {
            eprintln!("GREYCARD_SAMPLES names no folder; nothing measured");
            return;
        };
        let mut raws: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| greycard_core::decode::is_raw_path(p))
            .collect();
        raws.sort();
        let gpu = greycard_gpu::Context::own()
            .inspect_err(|e| eprintln!("no GPU for the read back: {e}"))
            .ok();
        let scratch = thumb_scratch("edit-samples");
        for path in raws {
            let (input, _) = timed_open(&path).unwrap();
            let mut edit = Edit::default();
            edit.light.exposure = 1.0;
            let mut base = None;
            let deliver: Deliver = Arc::new(|_| {});
            let (_, image) = develop_job(
                &input,
                &edit,
                0,
                1,
                &mut base,
                &mut None,
                &mut Ai::new(),
                None,
                None,
                &mut None,
                &deliver,
            );
            let image = image.expect("a picture on the CPU");
            let b = base.as_ref().unwrap();
            let source = (image.width as u32, image.height as u32);
            let factor = crate::edited::factor_for(source, &edit.geometry);
            let finish = || crate::edited::Finish {
                edit: edit.clone(),
                turn: 0,
                source,
                rasters: Default::default(),
                clip_level: b.clip_level,
                guide: Some(b.guide.clone()),
                kind: b.source,
                white: b.white_shift(),
            };
            let cache: ThumbCache = Arc::new(Mutex::new(Some(
                Thumbs::at(
                    scratch.join("thumbs"),
                    greycard_library::thumbs::DEFAULT_CAP,
                )
                .with_previews(2048, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
            )));
            let edited = crate::edited::Edited::new(cache);
            let hash = greycard_library::hash_file(&path).unwrap();
            let cpu = Instant::now();
            crate::edited::keep(
                &edited,
                crate::edited::Keeping {
                    path: path.clone(),
                    hash: hash.clone(),
                    key: 1,
                    developed: crate::edited::Developed::Full(image.clone()),
                    factor,
                    read_back: None,
                    marked: edited.mark(&hash, 1).unwrap(),
                    iso: None,
                    finish: finish(),
                },
                |_| {},
            );
            let cpu = cpu.elapsed().as_secs_f64();
            let read_back = gpu.as_ref().map(|ctx| {
                let halves = Halves::from_image(&image, None);
                let texture = ctx.viewport_texture(source.0, source.1);
                ctx.queue().write_texture(
                    greycard_gpu::wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: greycard_gpu::wgpu::Origin3d::ZERO,
                        aspect: greycard_gpu::wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&halves.pixels),
                    greycard_gpu::wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(source.0 * 8),
                        rows_per_image: Some(source.1),
                    },
                    greycard_gpu::wgpu::Extent3d {
                        width: source.0,
                        height: source.1,
                        depth_or_array_layers: 1,
                    },
                );
                ctx.queue().submit([]);
                let started = Instant::now();
                let reduced = crate::edited::read_back_reduced(ctx, &texture, factor).unwrap();
                let read = started.elapsed().as_secs_f64();
                let started = Instant::now();
                crate::edited::keep(
                    &edited,
                    crate::edited::Keeping {
                        path: path.clone(),
                        hash: hash.clone(),
                        key: 2,
                        developed: crate::edited::Developed::Reduced(reduced),
                        factor,
                        read_back: Some(read),
                        marked: edited.mark(&hash, 2).unwrap(),
                        iso: None,
                        finish: finish(),
                    },
                    |_| {},
                );
                (read, started.elapsed().as_secs_f64())
            });
            println!(
                "{}: {}x{} ({:.1} MP), factor {factor}: from the CPU's picture {:.0} ms{}",
                path.file_name().unwrap().to_string_lossy(),
                source.0,
                source.1,
                source.0 as f64 * source.1 as f64 / 1e6,
                cpu * 1e3,
                read_back
                    .map(|(r, k)| format!(
                        "; from the GPU's, read back {:.0} ms and made {:.0} ms",
                        r * 1e3,
                        k * 1e3
                    ))
                    .unwrap_or_default(),
            );
        }
        crate::testing::remove_dir_retry(&scratch);
    }

    /// An offline frame's picture: looked up by the row's hash and
    /// stamp at the size the pool keeps pictures at, the file itself
    /// gone.
    #[test]
    fn a_cached_thumbnail_job_finds_what_the_pool_kept() {
        let dir = thumb_scratch("offline");
        let raw = dir.join("IMG_0002.CR3");
        std::fs::write(&raw, vec![0x2au8; 90_000]).unwrap();
        let hash = greycard_library::hash_file(&raw).unwrap();
        let stamp = thumb_tag(file_stat(&raw).unwrap()).stamp;
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let worker = Worker::new(move |o| {
            let line = match o {
                Outcome::Thumbnail {
                    index,
                    size,
                    width,
                    cached,
                    ..
                } => format!("thumbnail {index} {size} {width} {cached}"),
                Outcome::NoThumbnail { index, .. } => format!("none {index}"),
                _ => return,
            };
            tx.lock().unwrap().send(line).unwrap();
        });
        let mut thumbs = Thumbs::at(dir.join("thumbs"), greycard_library::thumbs::DEFAULT_CAP);
        let made = crate::grid::made_size(worker.pool.size());
        let kept = Thumb {
            width: 5,
            height: 4,
            rgb: vec![90; 60],
        };
        thumbs
            .put(&hash, made, thumb_tag(file_stat(&raw).unwrap()), &kept)
            .unwrap();
        worker.set_thumb_cache(Some(thumbs));
        std::fs::remove_file(&raw).unwrap();
        worker.send(Job::CachedThumbnail {
            index: 3,
            path: raw.clone(),
            hash: hash.clone(),
            stamp,
        });
        let wait = std::time::Duration::from_secs(10);
        assert_eq!(
            rx.recv_timeout(wait).unwrap(),
            format!("thumbnail 3 {made} 5 true")
        );
        // Another stamp is another entry: a miss, and nothing made.
        worker.send(Job::CachedThumbnail {
            index: 4,
            path: raw,
            hash,
            stamp: stamp + 1,
        });
        assert_eq!(rx.recv_timeout(wait).unwrap(), "none 4");
        worker.stop();
        crate::testing::remove_dir_retry(&dir);
    }

    /// A picture file is made once, kept, found; re-written in place,
    /// it is made again, whatever its head says.
    #[test]
    fn a_picture_is_kept_until_it_is_written_again() {
        let dir = thumb_scratch("picture");
        let png = dir.join("export.png");
        let write = |shade: u8| {
            image::RgbImage::from_pixel(40, 20, image::Rgb([shade, shade, shade]))
                .save(&png)
                .unwrap();
        };
        write(200);
        let cache = cache_at(&dir);
        let (first, cached) =
            cached_thumbnail_noting(&cache, None, None, &png, 16, thumbnail).unwrap();
        assert!(!cached);
        let (again, cached) =
            cached_thumbnail_noting(&cache, None, None, &png, 16, thumbnail).unwrap();
        assert!(cached);
        assert_eq!((again.width, again.height), (first.width, first.height));
        // Written again with another mtime.
        write(20);
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&png)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let (fresh, cached) =
            cached_thumbnail_noting(&cache, None, None, &png, 16, thumbnail).unwrap();
        assert!(!cached);
        assert!(
            fresh.rgb.iter().all(|v| *v < 60),
            "the new picture's pixels"
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// A stand-in for the decode: a picture whose every byte is the
    /// file's last, which is zero while a preallocated copy has not
    /// reached its tail, as a cut preview decodes grey.
    fn tail_picture(path: &std::path::Path, _size: u32) -> anyhow::Result<(u32, u32, Vec<u8>)> {
        let bytes = std::fs::read(path)?;
        let last = *bytes.last().unwrap_or(&0);
        Ok((4, 3, vec![last; 36]))
    }

    fn set_time(path: &std::path::Path, secs_from_now: u64) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(
                std::time::SystemTime::now() + std::time::Duration::from_secs(secs_from_now),
            )
            .unwrap();
    }

    /// The review's recipe: a raw whose copier set its length first
    /// and has written only its head. Its key is already the finished
    /// file's, so what keeps the half-made picture from standing for
    /// the finished file is the time the copy moves on.
    #[test]
    fn a_raw_caught_half_copied_is_made_again_when_the_copy_ends() {
        let dir = thumb_scratch("halfcopied");
        let whole: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8 | 1).collect();
        let raw = dir.join("DSC_1981.NEF");
        // `truncate -s` to the full length, then the first 192 KB.
        let mut half = vec![0u8; whole.len()];
        half[..196_608].copy_from_slice(&whole[..196_608]);
        std::fs::write(&raw, &half).unwrap();
        let cache = cache_at(&dir);
        let (grey, cached) =
            cached_thumbnail_noting(&cache, None, None, &raw, 176, tail_picture).unwrap();
        assert!(!cached);
        assert!(grey.rgb.iter().all(|v| *v == 0));
        // The same head and the same length: the same hash.
        let before = greycard_library::hash_file(&raw).unwrap();
        std::fs::write(&raw, &whole).unwrap();
        set_time(&raw, 7);
        assert_eq!(greycard_library::hash_file(&raw).unwrap(), before);
        let (done, cached) =
            cached_thumbnail_noting(&cache, None, None, &raw, 176, tail_picture).unwrap();
        assert!(
            !cached,
            "the half-copied picture is not the finished file's"
        );
        assert!(done.rgb.iter().all(|v| *v == *whole.last().unwrap()));
        // And the finished file's picture is kept, and a rename keeps
        // its time and finds it.
        let renamed = dir.join("wedding-1981.NEF");
        std::fs::rename(&raw, &renamed).unwrap();
        let (again, cached) =
            cached_thumbnail_noting(&cache, None, None, &renamed, 176, tail_picture).unwrap();
        assert!(cached);
        assert_eq!(again.rgb, done.rgb);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A card's frame and its import by a plain `cp`: one head, one
    /// length, two times. Each is made once, and after that both hit,
    /// opened by turns, rather than each removing the other's entry.
    #[test]
    fn two_copies_with_two_times_both_hit() {
        let dir = thumb_scratch("twocopies");
        let card = dir.join("card").join("IMG_0007.CR3");
        let import = dir.join("import").join("IMG_0007.CR3");
        std::fs::create_dir_all(card.parent().unwrap()).unwrap();
        std::fs::create_dir_all(import.parent().unwrap()).unwrap();
        std::fs::write(&card, vec![7u8; 90_000]).unwrap();
        std::fs::copy(&card, &import).unwrap();
        set_time(&import, 60);
        assert_eq!(
            greycard_library::hash_file(&card).unwrap(),
            greycard_library::hash_file(&import).unwrap()
        );
        let cache = cache_at(&dir);
        for (round, path) in [&card, &import, &card, &import, &card]
            .into_iter()
            .enumerate()
        {
            let (_, cached) =
                cached_thumbnail_noting(&cache, None, None, path, 176, tail_picture).unwrap();
            assert_eq!(cached, round >= 2, "round {round}");
        }
        crate::testing::remove_dir_retry(&dir);
    }

    /// A file that grows or is touched while its picture is being made
    /// gets the picture shown and not kept.
    #[test]
    fn a_file_changing_under_the_making_is_not_kept() {
        let dir = thumb_scratch("changing");
        let raw = dir.join("IMG_0002.CR3");
        std::fs::write(&raw, vec![5u8; 80_000]).unwrap();
        let cache = cache_at(&dir);
        let grows = |path: &std::path::Path, size: u32| {
            let made = tail_picture(path, size);
            let mut f = std::fs::File::options().append(true).open(path).unwrap();
            std::io::Write::write_all(&mut f, &[9u8; 1000]).unwrap();
            made
        };
        let (_, cached) = cached_thumbnail_noting(&cache, None, None, &raw, 176, grows).unwrap();
        assert!(!cached);
        let touched = |path: &std::path::Path, size: u32| {
            let made = tail_picture(path, size);
            set_time(path, 30);
            made
        };
        let (_, cached) = cached_thumbnail_noting(&cache, None, None, &raw, 176, touched).unwrap();
        assert!(!cached);
        assert_eq!(
            cache.lock().unwrap().as_ref().unwrap().usage().entries,
            0,
            "neither was kept"
        );
        // Left alone, it is made once and kept.
        let (_, cached) =
            cached_thumbnail_noting(&cache, None, None, &raw, 176, tail_picture).unwrap();
        assert!(!cached);
        let (_, cached) =
            cached_thumbnail_noting(&cache, None, None, &raw, 176, tail_picture).unwrap();
        assert!(cached);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A cache with a cap of nothing is off: nothing looked up, nothing
    /// written, though it is still there to be cleared.
    #[test]
    fn a_cap_of_nothing_is_the_cache_off() {
        let dir = thumb_scratch("capzero");
        let png = dir.join("a.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([9, 9, 9]))
            .save(&png)
            .unwrap();
        let cache: ThumbCache = Arc::new(Mutex::new(Some(Thumbs::at(dir.join("thumbs"), 0))));
        for _ in 0..2 {
            let (_, cached) =
                cached_thumbnail_noting(&cache, None, None, &png, 8, thumbnail).unwrap();
            assert!(!cached);
        }
        assert!(!dir.join("thumbs").exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// The count runs off the caller's thread and seeds the cache.
    #[test]
    fn the_cache_is_counted_on_a_thread_of_its_own() {
        let dir = thumb_scratch("count");
        let root = dir.join("thumbs");
        let mut filled = Thumbs::at(&root, greycard_library::thumbs::DEFAULT_CAP);
        let kept = Thumb {
            width: 3,
            height: 2,
            rgb: vec![128; 18],
        };
        filled
            .put(&"ab".repeat(32), 128, Tag::default(), &kept)
            .unwrap();
        let cache: ThumbCache = Arc::new(Mutex::new(Some(Thumbs::at(
            &root,
            greycard_library::thumbs::DEFAULT_CAP,
        ))));
        let (tx, rx) = std::sync::mpsc::channel();
        count_thumb_cache(&cache, move |known| tx.send(known).unwrap());
        let known = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .expect("counted");
        assert_eq!(known.entries, 1);
        assert_eq!(
            cache.lock().unwrap().as_ref().unwrap().known_usage(),
            Some(known)
        );
        crate::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn without_a_cache_a_thumbnail_is_made_every_time() {
        let dir = thumb_scratch("off");
        let png = dir.join("a.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([9, 9, 9]))
            .save(&png)
            .unwrap();
        let off: ThumbCache = Arc::new(Mutex::new(None));
        for _ in 0..2 {
            let (_, cached) =
                cached_thumbnail_noting(&off, None, None, &png, 8, thumbnail).unwrap();
            assert!(!cached);
        }
        assert!(!dir.join("thumbs").exists());
        crate::testing::remove_dir_retry(&dir);
    }

    /// The Sky shape over real frames, the product's own path: each
    /// file opened and developed as the worker does under the default
    /// edit, then the Sky raster made by `Ai::raster` twice, once on
    /// the providers this machine offers (WebGPU where there is one)
    /// and once on the CPU alone, and the two compared. Set
    /// `GREYCARD_SAMPLES` to a folder of frames (copies: nothing is
    /// written beside them) and `GREYCARD_MODELS` to a store holding
    /// the Sky model and SAM, and run with `--ignored`; without both
    /// it passes at once. `GREYCARD_SKY_OUT` names a folder for each
    /// frame's preview (`<stem>.jpg`), its Sky raster (`<stem>-sky.png`)
    /// and `report.txt`; `GREYCARD_SKY_ONLY` a comma list of stems.
    /// Fails if any frame with no sky gets a single pixel of sky: the
    /// stems in `GREYCARD_SKY_NONE` (a comma list), else the five in the
    /// user's set. A frame that will not open or develop is passed over
    /// with a line.
    #[test]
    #[ignore]
    fn the_sky_over_real_frames() {
        use greycard_ai::Provider;
        let (Some(samples), Some(models)) = (
            std::env::var_os("GREYCARD_SAMPLES"),
            std::env::var_os("GREYCARD_MODELS"),
        ) else {
            return;
        };
        let out = std::env::var_os("GREYCARD_SKY_OUT").map(PathBuf::from);
        if let Some(out) = &out {
            std::fs::create_dir_all(out).unwrap();
        }
        let only: Option<Vec<String>> = std::env::var("GREYCARD_SKY_ONLY")
            .ok()
            .map(|s| s.split(',').map(str::to_string).collect());
        let no_sky: Vec<String> = std::env::var("GREYCARD_SKY_NONE").map_or_else(
            |_| {
                ["5M0A5135", "DSCF0153", "DSCF0186", "5M0A0957", "5M0A0952"]
                    .map(str::to_string)
                    .to_vec()
            },
            |s| s.split(',').map(str::to_string).collect(),
        );
        let store = greycard_ai::Store::at(models);
        let mut develop_ai = Ai::for_test(store.clone(), vec![Provider::Cpu]);
        let mut near = Ai::for_test(store.clone(), Provider::available());
        let mut cpu = Ai::for_test(store, vec![Provider::Cpu]);
        let mut frames: Vec<PathBuf> = std::fs::read_dir(&samples)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| greycard_core::decode::is_raw_path(p))
            .collect();
        frames.sort();
        let mut lines = Vec::new();
        let mut painted = Vec::new();
        let shape = Shape::Sky { picks: Vec::new() };
        let deliver: Deliver = Arc::new(|_| {});
        for path in frames {
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            if only.as_ref().is_some_and(|o| !o.contains(&stem)) {
                continue;
            }
            let input = match open(&path) {
                Ok((input, _)) => input,
                Err(e) => {
                    let line = format!("{stem}: passed over, it will not open: {e:#}");
                    println!("{line}");
                    lines.push(line);
                    continue;
                }
            };
            let mut base = None;
            let t = Instant::now();
            let _ = develop_job(
                &input,
                &Edit::default(),
                0,
                1,
                &mut base,
                &mut None,
                &mut develop_ai,
                None,
                None,
                &mut None,
                &deliver,
            );
            let develop = t.elapsed().as_secs_f64();
            let Some(b) = base.as_ref() else {
                let line = format!("{stem}: passed over, it did not develop");
                println!("{line}");
                lines.push(line);
                continue;
            };
            let run = |ai: &mut Ai| {
                ai.forget(None);
                let t = Instant::now();
                let made = ai
                    .raster(
                        b.stamp,
                        &b.image,
                        &b.edit,
                        b.source,
                        b.turn,
                        (1, 0),
                        &shape,
                        None,
                    )
                    .unwrap_or_else(|e| panic!("{stem}: {e}"));
                let total = t.elapsed().as_secs_f64();
                (made, total, ai.last_sky.clone().expect("a sky report"))
            };
            let (made, total, r) = run(&mut near);
            let (cpu_made, cpu_total, c) = run(&mut cpu);
            let data = made.raster.data();
            let cpu_data = cpu_made.raster.data();
            let n = data.len() as f64;
            let area = data.iter().filter(|&&v| v > 127).count() as f64 / n;
            let (mut max, mut sum, mut flips) = (0u8, 0u64, 0usize);
            for (a, c) in data.iter().zip(cpu_data) {
                max = max.max(a.abs_diff(*c));
                sum += a.abs_diff(*c) as u64;
                flips += ((*a > 127) != (*c > 127)) as usize;
            }
            if no_sky.contains(&stem) && data.iter().any(|&v| v > 0) {
                painted.push(stem.clone());
            }
            let stages = |r: &crate::ai::SkyReport, total: f64| {
                let s = r.stages;
                let known = r.load + r.prior + s.gate + s.seeds + s.outline + s.edge + r.raster;
                format!(
                    "total {total:.2}s = preview {:.2} + prior {:.3} on {} + gate {:.3} + seeds {:.3} \
                     + outline {:.3} (SAM load {:.2}, embed {:.3} on {}, {} decodes) + edge {:.3} \
                     + raster {:.3}",
                    total - known,
                    r.prior,
                    r.prior_on.map_or("-", |p| p.name()),
                    s.gate,
                    s.seeds,
                    s.outline,
                    r.sam_load,
                    r.embed,
                    r.sam_on.map_or("-", |p| p.name()),
                    r.decodes,
                    s.edge,
                    r.raster,
                )
            };
            // The matte made from the frame brought down to the
            // preview's size instead of at its own, for the time.
            let at_preview = match (near.sky_prior(), &r.outline) {
                (Some(prior), Some(outline)) => {
                    let (w, h) = (prior.width, prior.height);
                    let mut small = WorkingImage::new(w, h);
                    for y in 0..h {
                        for x in 0..w {
                            let (sx, sy) = (x * b.image.width / w, y * b.image.height / h);
                            let from = (sy * b.image.width + sx) * 3;
                            small.data[(y * w + x) * 3..][..3]
                                .copy_from_slice(&b.image.data[from..from + 3]);
                        }
                    }
                    let t = Instant::now();
                    let _ = greycard_ai::matte::color_line(outline, prior, &small, &[], w, h);
                    format!("{:.3}s at {w}x{h}", t.elapsed().as_secs_f64())
                }
                _ => "-".to_string(),
            };
            let line = format!(
                "{stem}: {} {:.1}% seeded {} | develop {develop:.1}s | {} | CPU: {} | \
                 against the CPU: max {max}/255, mean {:.2e}, {:.4}% across a half | \
                 matte at the preview's size {at_preview}, at the frame's {}x{}",
                match r.no_sky {
                    Some(why) => format!("no sky ({why})"),
                    None => "sky".to_string(),
                },
                area * 100.0,
                r.seeded,
                stages(&r, total),
                stages(&c, cpu_total),
                sum as f64 / n / 255.0,
                flips as f64 / n * 100.0,
                b.image.width,
                b.image.height,
            );
            println!("{line}");
            lines.push(line);
            if let Some(out) = &out {
                let rgb = crate::ai::preview(&b.image, &b.edit, b.source);
                image::save_buffer(
                    out.join(format!("{stem}.jpg")),
                    &rgb.data,
                    rgb.width as u32,
                    rgb.height as u32,
                    image::ExtendedColorType::Rgb8,
                )
                .unwrap();
                // The trimap the matte started from: sky white, in
                // question grey, not sky black; and the prior's labels.
                if let (Some(prior), Some(outline)) = (near.sky_prior(), &r.outline) {
                    let map: Vec<u8> = greycard_ai::matte::trimap(outline, prior)
                        .into_iter()
                        .map(|k| match k {
                            greycard_ai::matte::Known::Sky => 255,
                            greycard_ai::matte::Known::Unknown => 128,
                            greycard_ai::matte::Known::NotSky => 0,
                        })
                        .collect();
                    for (name, data) in [("trimap", &map), ("labels", &prior.labels)] {
                        image::save_buffer(
                            out.join(format!("{stem}-{name}.png")),
                            data,
                            prior.width as u32,
                            prior.height as u32,
                            image::ExtendedColorType::L8,
                        )
                        .unwrap();
                    }
                }
                image::save_buffer(
                    out.join(format!("{stem}-sky.png")),
                    data,
                    made.raster.width as u32,
                    made.raster.height as u32,
                    image::ExtendedColorType::L8,
                )
                .unwrap();
            }
        }
        if let Some(out) = &out {
            std::fs::write(out.join("report.txt"), lines.join("\n") + "\n").unwrap();
        }
        assert!(
            painted.is_empty(),
            "sky painted on frames with none: {painted:?}"
        );
    }

    /// A learned mask the edit asks for whose model is not in the
    /// store is left out of the picture, as the window draws it, and
    /// said: the export does not pass over it in silence. An Object
    /// with nothing picked asks for nothing and is not said, nor is a
    /// shape switched off or one in an adjustment switched off, which
    /// the finish never draws.
    #[test]
    fn an_export_says_which_learned_mask_it_went_without() {
        use greycard_edit::mask::Component;
        let frame = Arc::new(aberrated_frame(160, 120));
        let mut base = None;
        let (_, image) = develop_once(&frame, &mut base, &mut None);
        let image = image.unwrap();
        let empty = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch")
            .join(format!("left-out-{}/models", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let mut ai = Ai::for_test(greycard_ai::Store::at(&empty), Vec::new());
        let mut edit = Edit::default();
        edit.adjustments.push(greycard_edit::Adjustment {
            name: "Face".into(),
            mask: greycard_edit::mask::Mask {
                components: vec![
                    Component {
                        shape: Shape::Subject {},
                        ..Component::default()
                    },
                    Component {
                        shape: Shape::Object {
                            picks: Vec::new(),
                            boxes: Vec::new(),
                        },
                        ..Component::default()
                    },
                ],
                invert: false,
            },
            ..greycard_edit::Adjustment::default()
        });
        // An adjustment switched off is not drawn, so its Subject is
        // not missed; nor is a Subject shape switched off.
        edit.adjustments.push(greycard_edit::Adjustment {
            name: "Off".into(),
            enabled: false,
            mask: greycard_edit::mask::Mask {
                components: vec![Component {
                    shape: Shape::Subject {},
                    ..Component::default()
                }],
                invert: false,
            },
            ..greycard_edit::Adjustment::default()
        });
        edit.adjustments.push(greycard_edit::Adjustment {
            name: "Shape off".into(),
            mask: greycard_edit::mask::Mask {
                components: vec![Component {
                    shape: Shape::Subject {},
                    enabled: false,
                    ..Component::default()
                }],
                invert: false,
            },
            ..greycard_edit::Adjustment::default()
        });
        let (_, left_out) = finish_export(
            image,
            &edit,
            base.as_ref(),
            &mut ai,
            &crate::export::Settings::default(),
        );
        let _ = std::fs::remove_dir_all(empty.parent().unwrap());
        assert_eq!(left_out.len(), 1, "{left_out:?}");
        assert!(
            left_out[0].starts_with("Face's Subject shape:"),
            "{}",
            left_out[0]
        );
    }

    /// A develop whose learned denoiser or fill model is missing says
    /// so for the export; one that asked for neither says nothing.
    #[test]
    fn a_develop_without_its_model_says_what_was_left_out() {
        let developed = |learned, fills| Outcome::Developed {
            generation: 0,
            turn: 0,
            image: Developed::Halves(Arc::new(Halves {
                width: 1,
                height: 1,
                pixels: vec![half::f16::ZERO; 4],
            })),
            guide: Arc::new(crate::finish::Guide::NONE),
            white: WhiteBase::IDENTITY,
            seconds: 0.0,
            detail: None,
            sharpen: None,
            dehaze: None,
            sources: Vec::new(),
            learned,
            fills,
        };
        assert!(
            left_out_of(&developed(LearnedReport::Off, FillReport::default()), None).is_empty()
        );
        let model = greycard_ai::denoiser(greycard_edit::Learned::Fast.tier().unwrap()).unwrap();
        let said = left_out_of(
            &developed(
                LearnedReport::Missing(model),
                FillReport {
                    missing: vec!["Patch 1".into()],
                    ..FillReport::default()
                },
            ),
            None,
        );
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[0].contains(model.name), "{}", said[0]);
        assert!(said[1].starts_with("fill Patch 1:"), "{}", said[1]);
    }

    /// A base made as the engine's stand-in for a learned denoiser
    /// whose model is missing keeps saying so when a later develop
    /// keeps it: that develop reports `Kept`, and the export of its
    /// picture must still say what it went without.
    #[test]
    fn a_kept_stand_in_base_still_says_its_denoiser_was_missing() {
        let frame = Arc::new(aberrated_frame(160, 120));
        let empty = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch")
            .join(format!("stand-in-{}/models", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let mut ai = Ai::for_test(greycard_ai::Store::at(&empty), Vec::new());
        let mut edit = Edit::default();
        edit.noise.enabled = true;
        edit.noise.learned = greycard_edit::Learned::Fast;
        let deliver: Deliver = Arc::new(|_| {});
        let input = Input::Raw(frame);
        let (mut base, mut learned) = (None, None);
        let mut develop = |edit: &Edit, base: &mut Option<Base>| {
            develop_job(
                &input,
                edit,
                0,
                1,
                base,
                &mut learned,
                &mut ai,
                None,
                None,
                &mut None,
                &deliver,
            )
            .0
        };
        let first = develop(&edit, &mut base);
        assert!(matches!(
            &first,
            Outcome::Developed {
                learned: LearnedReport::Missing(_),
                ..
            }
        ));
        // A move that keeps the base: the exposure.
        edit.light.exposure = 0.5;
        let second = develop(&edit, &mut base);
        let _ = std::fs::remove_dir_all(empty.parent().unwrap());
        assert!(matches!(
            &second,
            Outcome::Developed {
                learned: LearnedReport::Kept,
                ..
            }
        ));
        assert!(left_out_of(&second, None).is_empty(), "the outcome alone");
        let said = left_out_of(&second, base.as_ref());
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("the learned denoiser:"), "{}", said[0]);
    }

    /// The People mask on real frames, through the editor's own path:
    /// each frame of `GREYCARD_PARTS_PICTURES` (the trial's JPEGs)
    /// opened and developed, its people found, and the parts made on
    /// it, with the models of `GREYCARD_MODELS`.
    /// - On a portrait an Iris is two small pieces, and a part of the
    ///   one woman on it, as the panel keeps her, finds her there.
    /// - Hers pasted onto her other frame finds her; a group's woman
    ///   pasted onto the portrait is never decided alone.
    /// - On the group, each one's Top resolves alone on her own picture
    ///   and is hers: centered under her face, and no two overlapping.
    /// - A person picked there is still that person after the picture
    ///   is turned or brightened, and her signature does not move.
    /// - A person pasted to a picture of two or more people asks.
    /// - With a Subject model in the store, a Whole person's edge is
    ///   as fine as Subject's, and on the group no two share more
    ///   than with SAM's edge alone.
    ///
    /// Prints each part's seconds and provider; with
    /// `GREYCARD_PARTS_OUT` set, writes each mask there as a PNG.
    #[test]
    #[ignore]
    fn parts_on_real_frames() {
        use crate::ai::Unresolved;
        use greycard_ai::Provider;
        use greycard_edit::mask::{PARTS, Person, Turned};
        let Some(models) = std::env::var_os("GREYCARD_MODELS") else {
            return;
        };
        let dir = std::env::var_os("GREYCARD_PARTS_PICTURES")
            .map(PathBuf::from)
            .expect("GREYCARD_PARTS_PICTURES: the trial's JPEGs, with GREYCARD_MODELS set");
        let out = std::env::var_os("GREYCARD_PARTS_OUT").map(PathBuf::from);
        if let Some(out) = &out {
            std::fs::create_dir_all(out).unwrap();
        }
        let store = greycard_ai::Store::at(models);
        // Whole person's edge is Subject's where the store has it.
        let edge = store.have(&greycard_ai::SUBJECT_WEBGPU) || store.have(&greycard_ai::SUBJECT);
        let mut develop_ai = Ai::for_test(store.clone(), vec![Provider::Cpu]);
        let mut ai = Ai::for_test(store, Provider::available());
        let deliver: Deliver = Arc::new(|_| {});
        // A frame developed under `edit` at `turn`, and the Parts side
        // told it is that file, as opening it tells the editor's.
        let mut develop = |stem: &str, edit: &Edit, turn: u8, ai: &mut Ai| {
            let path = dir.join(format!("{stem}.jpg"));
            let (input, _) = open(&path).unwrap_or_else(|e| panic!("{stem}: {e:#}"));
            let mut base = None;
            let _ = develop_job(
                &input,
                edit,
                turn,
                1,
                &mut base,
                &mut None,
                &mut develop_ai,
                None,
                None,
                &mut None,
                &deliver,
            );
            ai.forget(Some(path));
            base.unwrap_or_else(|| panic!("{stem} did not develop"))
        };
        let save = |name: &str, raster: &Raster| {
            if let Some(out) = &out {
                let height = raster.data().len() / greycard_edit::brush::RASTER_WIDTH;
                image::save_buffer(
                    out.join(format!("{name}.png")),
                    raster.data(),
                    greycard_edit::brush::RASTER_WIDTH as u32,
                    height as u32,
                    image::ExtendedColorType::L8,
                )
                .unwrap();
            }
        };
        // Each frame's people as a shape stores them, for a sidecar
        // written by hand to look at.
        let dump = |stem: &str, people: &[crate::ai::Candidate]| {
            if let Some(out) = &out {
                let people: Vec<Person> = people.iter().map(|p| p.person()).collect();
                std::fs::write(
                    out.join(format!("people-{stem}.json")),
                    serde_json::to_string(&people).unwrap(),
                )
                .unwrap();
            }
        };
        let on = |r: &Raster| r.data().iter().filter(|&&v| v > 127).count();
        let share = |r: &Raster| on(r) as f64 / r.data().len() as f64;
        // A mask's soft band: its pixels between a tenth and nine
        // tenths for each pixel of its edge (over a half, a neighbor
        // under it).
        let band = |r: &Raster| {
            let (d, w) = (r.data(), greycard_edit::brush::RASTER_WIDTH);
            let h = d.len() / w;
            let soft = d.iter().filter(|&&v| v > 25 && v < 230).count();
            let mut edge = 0usize;
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let under = |j: usize| d[j] <= 127;
                    if d[i] > 127
                        && ((x > 0 && under(i - 1))
                            || (x + 1 < w && under(i + 1))
                            || (y > 0 && under(i - w))
                            || (y + 1 < h && under(i + w)))
                    {
                        edge += 1;
                    }
                }
            }
            soft as f64 / edge.max(1) as f64
        };
        let part = |label: &str, person: Option<Person>| {
            let p = PARTS.iter().find(|p| p.label == label).unwrap();
            Shape::Part {
                phrase: p.phrase.to_string(),
                route: p.route.clone(),
                person,
            }
        };
        let plain = Edit::default();

        // A portrait: every part of everyone, timed; the first run
        // pays for the load and the encoding.
        let b = develop("DSCF0835", &plain, 0, &mut ai);
        let (people, provider, seconds) = ai.people(b.stamp, &b.image, b.source, b.turn).unwrap();
        println!(
            "DSCF0835: {} people on {} in {seconds:.2}s (load and encode)",
            people.len(),
            provider.name()
        );
        assert_eq!(people.len(), 1, "one woman");
        dump("DSCF0835", &people);
        for (i, p) in PARTS.iter().enumerate() {
            let shape = part(p.label, None);
            let made = ai
                .raster(
                    b.stamp,
                    &b.image,
                    &b.edit,
                    b.source,
                    b.turn,
                    (1, i),
                    &shape,
                    None,
                )
                .unwrap_or_else(|e| panic!("{}: {e}", p.label));
            assert!(made.part.is_none());
            println!(
                "  {:<12} {:>6.2}s on {:<6} {:>6.2}% of the frame",
                p.label,
                made.seconds,
                made.provider.map_or("-", |p| p.name()),
                100.0 * share(&made.raster)
            );
            save(&format!("DSCF0835-{}", p.label), &made.raster);
            if p.label == "Iris" {
                let s = share(&made.raster);
                assert!(s > 0.0 && s < 0.002, "two irises, small: {s}");
            }
        }
        // A part chosen on the portrait is hers, the one person on it
        // kept as a pick is, and on her own picture it finds her.
        let her = people[0].person();
        assert!(her.picture.is_some());
        let made = ai
            .raster(
                b.stamp,
                &b.image,
                &b.edit,
                b.source,
                b.turn,
                (1, PARTS.len()),
                &part("Lips", Some(her.clone())),
                None,
            )
            .unwrap();
        assert!(made.part.is_none(), "hers on her own picture");
        assert!(share(&made.raster) > 0.0, "her lips");
        println!(
            "  Lips, hers: {:.2}s, {:.3}% of the frame",
            made.seconds,
            100.0 * share(&made.raster)
        );
        // Her whole, as the menu's first entry makes it on a portrait:
        // hers, and on a picture of one, all but everyone's.
        let everyone = ai
            .raster(
                b.stamp,
                &b.image,
                &b.edit,
                b.source,
                b.turn,
                (1, 0),
                &part("Whole person", None),
                None,
            )
            .unwrap()
            .raster;
        let made = ai
            .raster(
                b.stamp,
                &b.image,
                &b.edit,
                b.source,
                b.turn,
                (1, PARTS.len() + 1),
                &part("Whole person", Some(her.clone())),
                None,
            )
            .unwrap();
        assert!(made.part.is_none(), "her whole on her own picture");
        save("DSCF0835-person-hers", &made.raster);
        let both = made
            .raster
            .data()
            .iter()
            .zip(everyone.data())
            .filter(|&(&a, &b)| a > 127 && b > 127)
            .count();
        println!(
            "  Whole person, hers: {:.2}s, {:.2}% of the frame, {both} of everyone's {}",
            made.seconds,
            100.0 * share(&made.raster),
            on(&everyone)
        );
        assert!(share(&made.raster) > 0.05, "her, whole");
        assert!(both * 10 > on(&everyone) * 9, "the one person is everyone");
        // Her edge is Subject's: its soft band per edge pixel as
        // narrow as Subject's own, within a sixth (SAM's alone ran 1.3
        // times as wide), and Subject's raster the same run's. Without
        // Subject in the store, SAM's edge, and the status line says so.
        if edge {
            let subject = ai
                .raster(
                    b.stamp,
                    &b.image,
                    &b.edit,
                    b.source,
                    b.turn,
                    (1, PARTS.len() + 2),
                    &Shape::Subject {},
                    None,
                )
                .unwrap();
            assert_eq!(subject.seconds, 0.0, "Subject's matte, kept from her edge");
            let (hers, theirs) = (band(&made.raster), band(&subject.raster));
            println!("  Whole person's band {hers:.1}, Subject's {theirs:.1}");
            assert!(
                hers < 1.15 * theirs,
                "her band {hers:.1} against {theirs:.1}"
            );
        } else {
            println!(
                "  Whole person's band {:.1}, no Subject",
                band(&made.raster)
            );
            let note = made.note.as_deref().unwrap_or_default();
            assert!(note.contains("Subject model is not downloaded"), "{note}");
        }

        // The group: each person's Top on her own picture is decided
        // alone, and hers.
        let g = develop("5M0A4169", &plain, 0, &mut ai);
        let (group, _, seconds) = ai.people(g.stamp, &g.image, g.source, g.turn).unwrap();
        println!("5M0A4169: {} people in {seconds:.2}s", group.len());
        assert_eq!(group.len(), 5, "the group frame");
        assert!(group.iter().all(|p| p.picture.is_some()));
        dump("5M0A4169", &group);
        let w = greycard_edit::brush::RASTER_WIDTH;
        let aspect = g.image.height as f32 / g.image.width as f32;
        let mut tops = Vec::new();
        for (i, p) in group.iter().enumerate() {
            let shape = part("Top", Some(p.person()));
            let made = ai
                .raster(
                    g.stamp,
                    &g.image,
                    &g.edit,
                    g.source,
                    g.turn,
                    (2, i),
                    &shape,
                    None,
                )
                .unwrap();
            assert!(made.part.is_none(), "#{i} resolves on her own picture");
            save(&format!("5M0A4169-top-{i}"), &made.raster);
            // Her top's middle, across, is nearer her face than anyone
            // else's.
            let r = made.raster.data();
            let (mut sx, mut n) = (0.0f64, 0usize);
            for (k, &v) in r.iter().enumerate() {
                if v > 127 {
                    sx += (k % w) as f64;
                    n += 1;
                }
            }
            assert!(n > 0, "#{i} has a top");
            let mid = (sx / n as f64 / w as f64) as f32;
            let nearest = (0..group.len())
                .min_by(|&a, &b| {
                    (group[a].at[0] - mid)
                        .abs()
                        .total_cmp(&(group[b].at[0] - mid).abs())
                })
                .unwrap();
            println!(
                "  Top of #{i}: {:.2}s, {:.2}% of the frame, nearest face #{nearest}",
                made.seconds,
                100.0 * share(&made.raster)
            );
            assert_eq!(nearest, i, "#{i}'s top is hers");
            tops.push(made.raster);
        }
        // No two the same, and none taking a piece of another's.
        for i in 0..tops.len() {
            for j in i + 1..tops.len() {
                let both = tops[i]
                    .data()
                    .iter()
                    .zip(tops[j].data())
                    .filter(|&(&a, &b)| a > 127 && b > 127)
                    .count();
                let least = on(&tops[i]).min(on(&tops[j]));
                println!("  #{i} and #{j} overlap {both} of {least}");
                assert!(both * 50 < least, "#{i} and #{j} overlap {both} of {least}");
            }
        }

        // Each one whole, on her own picture: decided alone, over her
        // own face, and no two sharing much; All people holds them all.
        let mut wholes = Vec::new();
        for (i, p) in group.iter().enumerate() {
            let made = ai
                .raster(
                    g.stamp,
                    &g.image,
                    &g.edit,
                    g.source,
                    g.turn,
                    (9, i),
                    &part("Whole person", Some(p.person())),
                    None,
                )
                .unwrap();
            assert!(
                made.part.is_none(),
                "#{i} whole resolves on her own picture"
            );
            save(&format!("5M0A4169-person-{i}"), &made.raster);
            let r = made.raster.data();
            let h = r.len() / w;
            let [cx, cy] = [
                (p.face[0] + p.face[2]) / 2.0,
                (p.face[1] + p.face[3]) / 2.0 / aspect,
            ];
            let at = (cy * h as f32) as usize * w + (cx * w as f32) as usize;
            println!(
                "  Whole person #{i}: {:.2}s, {:.2}% of the frame, body {}, {} at her face",
                made.seconds,
                100.0 * share(&made.raster),
                p.body.is_some(),
                r[at]
            );
            assert!(r[at] > 127, "#{i} whole holds her face");
            wholes.push(made.raster);
        }
        // Each one's edge as fine as Subject's, within a sixth: SAM's
        // alone ran to 1.26 times as wide on these five.
        if edge {
            let subject = ai
                .raster(
                    g.stamp,
                    &g.image,
                    &g.edit,
                    g.source,
                    g.turn,
                    (9, group.len() + 1),
                    &Shape::Subject {},
                    None,
                )
                .unwrap();
            let theirs = band(&subject.raster);
            for (i, one) in wholes.iter().enumerate() {
                let hers = band(one);
                println!("  whole #{i}'s band {hers:.1}, Subject's {theirs:.1}");
                assert!(
                    hers < 1.15 * theirs,
                    "#{i}'s band {hers:.1} against {theirs:.1}"
                );
            }
        } else {
            for (i, one) in wholes.iter().enumerate() {
                println!("  whole #{i}'s band {:.1}, no Subject", band(one));
            }
        }
        // The same five with SAM's edge alone, under other keys.
        ai.sams_edge_only = true;
        let sams: Vec<Arc<Raster>> = group
            .iter()
            .enumerate()
            .map(|(i, p)| {
                ai.raster(
                    g.stamp,
                    &g.image,
                    &g.edit,
                    g.source,
                    g.turn,
                    (10, i),
                    &part("Whole person", Some(p.person())),
                    None,
                )
                .unwrap()
                .raster
            })
            .collect();
        ai.sams_edge_only = false;
        let overlap = |a: &Raster, b: &Raster| {
            a.data()
                .iter()
                .zip(b.data())
                .filter(|&(&a, &b)| a > 127 && b > 127)
                .count()
        };
        // No two share more than SAM's edges alone did, and not much.
        for i in 0..wholes.len() {
            for j in i + 1..wholes.len() {
                let both = overlap(&wholes[i], &wholes[j]);
                let before = overlap(&sams[i], &sams[j]);
                let least = on(&wholes[i]).min(on(&wholes[j]));
                println!("  whole #{i} and #{j} overlap {both} of {least}, SAM's {before}");
                assert!(
                    both <= before && both * 20 < least,
                    "whole #{i} and #{j} overlap {both} of {least}, SAM's {before}"
                );
            }
        }
        let made = ai
            .raster(
                g.stamp,
                &g.image,
                &g.edit,
                g.source,
                g.turn,
                (9, group.len()),
                &part("Whole person", None),
                None,
            )
            .unwrap();
        save("5M0A4169-person-all", &made.raster);
        for (i, one) in wholes.iter().enumerate() {
            let held = one
                .data()
                .iter()
                .zip(made.raster.data())
                .filter(|&(&a, &b)| a > 127 && b > 127)
                .count();
            println!("  All people holds {held} of #{i}'s {}", on(one));
            assert!(held * 10 > on(one) * 9, "All people holds #{i}");
        }

        // One of them after a global edit and a turn: still her, decided
        // on her own picture by where her face was, and her signature
        // the same under the brighter edit.
        let k = 2;
        let first = group[k].person();
        let mut bright = Edit::default();
        bright.light.exposure = 0.7;
        let e = develop("5M0A4169", &bright, 0, &mut ai);
        let (again, _, _) = ai.people(e.stamp, &e.image, e.source, e.turn).unwrap();
        for (a, b) in again.iter().zip(&group) {
            let most = a
                .signature
                .values
                .iter()
                .zip(&b.signature.values)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max);
            assert!(most <= 1e-3, "a signature moved by {most} under +0.7 EV");
        }
        let made = ai
            .raster(
                e.stamp,
                &e.image,
                &bright,
                e.source,
                e.turn,
                (7, 0),
                &part("Top", Some(first.clone())),
                None,
            )
            .unwrap();
        assert!(made.part.is_none(), "brighter: still decided");
        let both = made
            .raster
            .data()
            .iter()
            .zip(tops[k].data())
            .filter(|&(&a, &b)| a > 127 && b > 127)
            .count();
        assert!(both * 10 > on(&tops[k]) * 9, "brighter: the same top");
        println!("+0.7 EV: #{k} decided alone, the same top");

        let t = develop("5M0A4169", &plain, 1, &mut ai);
        let mut turned = part("Top", Some(first));
        turned.turn(Turned::new(1, 1.0 / aspect));
        let made = ai
            .raster(
                t.stamp,
                &t.image,
                &t.edit,
                t.source,
                t.turn,
                (8, 0),
                &turned,
                None,
            )
            .unwrap();
        assert!(made.part.is_none(), "turned: still decided");
        save("5M0A4169-top-turned", &made.raster);
        // Her top, turned: the same cells, a quarter clockwise.
        let tw = w;
        let th = made.raster.data().len() / tw;
        let (gw, gh) = (w, tops[k].data().len() / w);
        let mut hit = 0usize;
        for (idx, &v) in tops[k].data().iter().enumerate() {
            if v > 127 {
                let (x, y) = (idx % gw, idx / gw);
                let (nx, ny) = (
                    ((1.0 - (y as f32 + 0.5) / gh as f32) * tw as f32) as usize,
                    (((x as f32 + 0.5) / gw as f32) * th as f32) as usize,
                );
                if made.raster.data()[ny.min(th - 1) * tw + nx.min(tw - 1)] > 127 {
                    hit += 1;
                }
            }
        }
        println!(
            "turned: #{k} decided alone, {hit} of {} cells",
            on(&tops[k])
        );
        assert!(hit * 10 > on(&tops[k]) * 8, "turned: the same top");

        // The woman of the portrait pasted onto the group: asked
        // about, empty, the people offered to pick from.
        let g = develop("5M0A4169", &plain, 0, &mut ai);
        let made = ai
            .raster(
                g.stamp,
                &g.image,
                &g.edit,
                g.source,
                g.turn,
                (3, 0),
                &part("Lips", Some(her.clone())),
                None,
            )
            .unwrap();
        match &made.part {
            Some(Unresolved::Ask(ask)) => {
                println!("pasted onto the group: asked, guess {:?}", ask.guess);
                assert_eq!(ask.people.len(), 5);
                assert_eq!(ask.why, crate::ai::Asking::Unsure);
            }
            other => panic!("pasted onto the group: {other:?}"),
        }
        assert!(made.raster.data().iter().all(|&v| v == 0));
        // And onto her other frame: found again, her lips.
        let o = develop("DSCF0848", &plain, 0, &mut ai);
        let made = ai
            .raster(
                o.stamp,
                &o.image,
                &o.edit,
                o.source,
                o.turn,
                (4, 0),
                &part("Lips", Some(her)),
                None,
            )
            .unwrap();
        println!(
            "pasted onto DSCF0848: {}, {:.3}% of the frame in {:.2}s",
            if made.part.is_none() {
                "decided"
            } else {
                "not decided"
            },
            100.0 * share(&made.raster),
            made.seconds
        );
        save("DSCF0848-lips-pasted", &made.raster);
        assert!(made.part.is_none() && share(&made.raster) > 0.0);
        // A group frame's person on the portrait: someone else, nobody.
        let b = develop("DSCF0835", &plain, 0, &mut ai);
        let made = ai
            .raster(
                b.stamp,
                &b.image,
                &b.edit,
                b.source,
                b.turn,
                (5, 0),
                &part("Hair", Some(group[0].person())),
                None,
            )
            .unwrap();
        println!(
            "group #0 pasted onto DSCF0835: {}",
            match &made.part {
                None => "decided".to_string(),
                Some(Unresolved::Nobody) => "nobody".to_string(),
                Some(Unresolved::Ask(a)) => format!("asked ({:?}), guess {:?}", a.why, a.guess),
            }
        );
        // Not her: asked as someone else's, the portrait's one face
        // outlined and no guess, never the stranger decided alone.
        match &made.part {
            Some(Unresolved::Ask(ask)) => {
                assert_eq!(ask.why, crate::ai::Asking::SomeoneElse);
                assert_eq!((ask.people.len(), ask.guess), (1, None));
            }
            other => panic!("group #0 pasted onto DSCF0835: {other:?}"),
        }
        assert!(made.raster.data().iter().all(|&v| v == 0));

        // A couple's person pasted onto the other couple: asked.
        let mut couples = Vec::new();
        for stem in ["4Z4A3846", "5M0A0504"] {
            let c = develop(stem, &plain, 0, &mut ai);
            let (people, _, _) = ai.people(c.stamp, &c.image, c.source, c.turn).unwrap();
            println!("{stem}: {} people", people.len());
            dump(stem, &people);
            couples.push((c, people));
        }
        let (c, _) = &couples[1];
        let made = ai
            .raster(
                c.stamp,
                &c.image,
                &c.edit,
                c.source,
                c.turn,
                (6, 0),
                &part("Lips", Some(couples[0].1[0].person())),
                None,
            )
            .unwrap();
        match &made.part {
            Some(Unresolved::Ask(ask)) => {
                println!("4Z4A3846 #0 onto 5M0A0504: asked, guess {:?}", ask.guess);
                assert_eq!(ask.why, crate::ai::Asking::Unsure);
            }
            other => panic!("4Z4A3846 #0 onto 5M0A0504: {other:?}"),
        }
    }

    /// The viewport against the export with the Dehaze on, on real
    /// frames (`GREYCARD_SAMPLES`, a folder of raws; `GREYCARD_DEHAZE_ONLY`,
    /// stems to keep, comma-separated). For each frame and each Detail
    /// setting, the export's picture is developed on the CPU and the
    /// viewport's on the GPU from the same base, so the CA correction
    /// is the CPU's on both and what differs is the Detail section and
    /// the sharpen alone. Both are finished the export's way (the edit's
    /// look, sRGB, eight bits) and compared level by level; the dehaze's
    /// airlight and mean transmission from each path are printed beside
    /// them. A measurement, not a pass: it prints and holds nothing.
    #[test]
    #[ignore]
    fn the_dehaze_viewport_against_the_export_on_real_frames() {
        let Some(samples) = std::env::var_os("GREYCARD_SAMPLES") else {
            eprintln!("set GREYCARD_SAMPLES to a folder of raws");
            return;
        };
        let only: Option<Vec<String>> = std::env::var("GREYCARD_DEHAZE_ONLY")
            .ok()
            .map(|s| s.split(',').map(str::to_string).collect());
        let Some(ctx) = greycard_gpu::Context::own().ok() else {
            skipped("the dehaze's viewport against its export");
            return;
        };
        let mut gpu = Some(ctx);
        let mut frames: Vec<PathBuf> = std::fs::read_dir(&samples)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| greycard_core::decode::is_raw_path(p))
            .collect();
        frames.sort();
        let deliver: Deliver = Arc::new(|_| {});
        let detail = |dehaze: f32, texture: f32, clarity: f32, sharpen: bool| {
            let mut edit = Edit::default();
            edit.detail.enabled = true;
            edit.detail.dehaze = dehaze;
            edit.detail.texture = texture;
            edit.detail.clarity = clarity;
            edit.sharpen.enabled = sharpen;
            edit
        };
        let cases = [
            ("dehaze +50", detail(0.5, 0.0, 0.0, true)),
            ("dehaze +60", detail(0.6, 0.0, 0.0, true)),
            (
                "dehaze +50, texture and clarity +50",
                detail(0.5, 0.5, 0.5, true),
            ),
            ("dehaze -50, clarity +30", detail(-0.5, 0.0, 0.3, true)),
            ("dehaze +100, sharpen off", detail(1.0, 0.0, 0.0, false)),
        ];
        for path in frames {
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            if only.as_ref().is_some_and(|o| !o.contains(&stem)) {
                continue;
            }
            let Ok((input, _)) = open(&path) else {
                println!("{stem}: passed over, it will not open");
                continue;
            };
            let mut base = None;
            for (name, edit) in &cases {
                let run = |base: &mut Option<Base>, gpu: &mut Option<greycard_gpu::Context>| {
                    develop_job(
                        &input,
                        edit,
                        0,
                        1,
                        base,
                        &mut None,
                        &mut Ai::new(),
                        None,
                        None,
                        gpu,
                        &deliver,
                    )
                };
                let (cpu_outcome, cpu) = run(&mut base, &mut None);
                let (gpu_outcome, _) = run(&mut base, &mut gpu);
                assert!(gpu.is_some(), "{stem}: the GPU failed");
                let cpu = cpu.expect("the CPU's develop gives its picture");
                let Outcome::Developed {
                    dehaze: cpu_dehaze, ..
                } = cpu_outcome
                else {
                    panic!("{stem}: the CPU's develop failed");
                };
                let Outcome::Developed {
                    image,
                    dehaze: gpu_dehaze,
                    ..
                } = gpu_outcome
                else {
                    panic!("{stem}: the GPU's develop failed");
                };
                let viewport = match image {
                    Developed::Texture(t) => read_viewport(gpu.as_ref().unwrap(), &t),
                    Developed::Halves(h) => {
                        let data: Vec<f32> = h
                            .pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .flat_map(|px| [px[0].to_f32(), px[1].to_f32(), px[2].to_f32()])
                            .collect();
                        WorkingImage::from_data(h.width as usize, h.height as usize, data).unwrap()
                    }
                };
                let b = base.as_ref().unwrap();
                let global = crate::finish::Baked::global(edit, b.source);
                let to_out = crate::export::Space::Srgb.matrix();
                let finish = |image: &WorkingImage| -> Vec<u8> {
                    crate::finish::finish_with(
                        image,
                        None,
                        &global,
                        &[],
                        |x, y| (x as f32, y as f32),
                        |_, _| (0.0, None),
                        None,
                        None,
                        &to_out,
                        |v| (v.clamp(0.0, 1.0) * 255.0).round() as u8,
                    )
                };
                let (a, v) = (finish(&cpu), finish(&viewport));
                // The sharpen's dots in the blacks (§268): a pixel whose
                // brightest channel is over 0.8 in a 16-bit finish while
                // the 5x5 mean of its Rec.709 luma is under 0.08.
                let dots = |image: &WorkingImage| -> usize {
                    let out: Vec<u16> = crate::finish::finish_with(
                        image,
                        None,
                        &global,
                        &[],
                        |x, y| (x as f32, y as f32),
                        |_, _| (0.0, None),
                        None,
                        None,
                        &to_out,
                        |v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16,
                    );
                    let (w, h) = (image.width, image.height);
                    let at = |i: usize| f32::from(out[i]) / 65535.0;
                    let luma: Vec<f32> = (0..w * h)
                        .map(|i| {
                            0.2126 * at(i * 3) + 0.7152 * at(i * 3 + 1) + 0.0722 * at(i * 3 + 2)
                        })
                        .collect();
                    (0..w * h)
                        .filter(|&i| {
                            let (x, y) = (i % w, i / w);
                            let brightest = at(i * 3).max(at(i * 3 + 1)).max(at(i * 3 + 2));
                            if brightest <= 0.8 {
                                return false;
                            }
                            let (mut sum, mut n) = (0f32, 0f32);
                            for yy in y.saturating_sub(2)..(y + 3).min(h) {
                                for xx in x.saturating_sub(2)..(x + 3).min(w) {
                                    sum += luma[yy * w + xx];
                                    n += 1.0;
                                }
                            }
                            sum / n < 0.08
                        })
                        .count()
                };
                let mut counts = [0usize; 256];
                let mut sq = 0f64;
                for (x, y) in a.iter().zip(&v) {
                    let d = x.abs_diff(*y);
                    counts[d as usize] += 1;
                    sq += f64::from(d) * f64::from(d);
                }
                let n = a.len();
                let worst = counts.iter().rposition(|&c| c > 0).unwrap_or(0);
                let off = n - counts[0];
                let mut seen = 0;
                let p999 = counts
                    .iter()
                    .position(|&c| {
                        seen += c;
                        seen as f64 >= 0.999 * n as f64
                    })
                    .unwrap_or(0);
                let fit = |d: &Option<DehazeStats>| {
                    d.map_or("none".into(), |d| {
                        format!(
                            "A ({:.5}, {:.5}, {:.5}) t {:.5}",
                            d.airlight[0], d.airlight[1], d.airlight[2], d.transmission_mean
                        )
                    })
                };
                println!(
                    "{stem} {}x{}, {name}: {:.4}% of samples off, {:.4}% by 2 or more, \
                     99.9th percentile {p999}, worst {worst} levels, RMSE {:.4} levels; \
                     export {} viewport {}; dots, export {} viewport {}",
                    cpu.width,
                    cpu.height,
                    100.0 * off as f64 / n as f64,
                    100.0 * (off - counts[1]) as f64 / n as f64,
                    (sq / n as f64).sqrt(),
                    fit(&cpu_dehaze),
                    fit(&gpu_dehaze),
                    dots(&cpu),
                    dots(&viewport),
                );
            }
        }
    }

    /// The viewport's half-float texture read back as a working image,
    /// its alpha dropped.
    fn read_viewport(
        ctx: &greycard_gpu::Context,
        texture: &greycard_gpu::wgpu::Texture,
    ) -> WorkingImage {
        use greycard_gpu::wgpu;
        let (w, h) = (texture.width(), texture.height());
        let row = (w * 8).div_ceil(256) * 256;
        let band = ((64u32 << 20) / row).max(1);
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for y0 in (0..h).step_by(band as usize) {
            let rows = band.min(h - y0);
            let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("read back"),
                size: u64::from(row * rows),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = ctx.device().create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: y0, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(rows),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: rows,
                    depth_or_array_layers: 1,
                },
            );
            ctx.queue().submit(Some(encoder.finish()));
            let slice = staging.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            ctx.device()
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            rx.recv().unwrap().unwrap();
            let mapped = slice.get_mapped_range().unwrap();
            for r in 0..rows as usize {
                let line: &[half::f16] =
                    bytemuck::cast_slice(&mapped[r * row as usize..][..(w * 8) as usize]);
                for px in line.as_chunks::<4>().0 {
                    data.extend([px[0].to_f32(), px[1].to_f32(), px[2].to_f32()]);
                }
            }
        }
        WorkingImage::from_data(w as usize, h as usize, data).unwrap()
    }
}
