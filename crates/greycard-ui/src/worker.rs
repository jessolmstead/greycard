//! The engine on its own thread: decodes, develops, and makes
//! thumbnails, one job at a time, the newest develop winning.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use greycard_core::CameraProfile;
use greycard_core::color::{as_shot_temp_tint, profile_from_frame};
use greycard_core::develop::ca::{CaOptions, CaStats, correct_ca};
use greycard_core::develop::dehaze::{self, DehazeStats};
use greycard_core::develop::local_contrast::{LocalContrastStats, local_contrast};
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
use greycard_edit::mask::{Pos, Shape};
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
    /// Write the open frame under `edit` to `path`, doing what
    /// `on_exists` says if a file of that name is there already.
    Export {
        edit: Edit,
        path: PathBuf,
        settings: crate::export::Settings,
        on_exists: crate::export::OnExists,
    },
    /// A learned mask's raster for `shape`, the component `key`.
    Mask {
        key: Key,
        shape: Shape,
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
/// working matrix, rows.
#[derive(Debug, Clone, Copy)]
pub struct WhiteBase {
    pub gains: [f32; 3],
    pub matrix: [[f32; 3]; 3],
}

impl WhiteBase {
    /// No gains and no matrix: a picture already in the working space.
    pub const IDENTITY: Self = Self {
        gains: [1.0; 3],
        matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    pub fn from(wb: &greycard_core::WhiteBalance) -> Self {
        let cols = wb.matrix_f32();
        Self {
            gains: wb.coefficients_f32(),
            matrix: std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r])),
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
        image: Developed,
        /// The tone equalizer's plane for this develop, for the
        /// viewport's own texture.
        guide: Arc<crate::finish::Guide>,
        white: WhiteBase,
        seconds: f64,
        /// What the local contrast did, when it ran, and how long it
        /// took when it ran in this develop rather than being kept
        /// from the last.
        detail: Option<(LocalContrastStats, Option<f64>)>,
        /// What the sharpen did, when it ran.
        sharpen: Option<SharpenStats>,
        /// What the dehaze did, when it ran.
        dehaze: Option<DehazeStats>,
        /// Sources the engine chose for patches that had none.
        sources: Vec<(u64, Pos)>,
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
    Failed {
        generation: u64,
        message: String,
    },
    Exported {
        path: PathBuf,
        seconds: f64,
        /// What the policy made of a file of that name being there
        /// already; none when nothing was.
        note: Option<String>,
    },
    /// Nothing written: a file of that name was there and the policy
    /// says to leave it.
    ExportSkipped {
        path: PathBuf,
    },
    ExportFailed {
        message: String,
    },
    /// A learned mask made (or found made) for `shape` at `key`.
    Mask {
        key: Key,
        shape: Shape,
        raster: Arc<Raster>,
        /// The provider that ran it; none when it was cached.
        provider: Option<&'static str>,
        seconds: f64,
    },
    MaskFailed {
        key: Key,
        message: String,
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
}

#[derive(Default)]
struct Queue {
    /// A lens database fetched since the worker read its own, for it
    /// to take up before its next develop.
    lenses: Option<Arc<greycard_lens::Database>>,
    /// Thumbnails, in the order they are wanted.
    thumbnails: std::collections::VecDeque<(usize, PathBuf)>,
    /// The long edge thumbnails are made at now, `THUMB_WIDTH` until
    /// the grid asks otherwise. Read as each one is made, so a
    /// picture still queued is made at the size wanted by then.
    thumb_size: Option<u32>,
    /// The range of the strip the thumbnails are ordered for, so the
    /// strip can report where it stands as often as it likes. Cleared
    /// when thumbnails are pushed, since that is a new folder's list.
    wanted: Option<(usize, usize)>,
    /// The editor's device and queue, handed over once the window has
    /// them, for the engine's GPU ops to run on: the picture they
    /// leave is then the viewport's without a copy.
    gpu: Option<(greycard_gpu::wgpu::Device, greycard_gpu::wgpu::Queue)>,
    /// The newest open or develop; an older one still queued is dropped.
    develop: Option<Job>,
    /// Masks wanted, the newest shape for a key replacing an older.
    masks: std::collections::VecDeque<(Key, Shape)>,
    exports: std::collections::VecDeque<Job>,
    /// The editor is leaving: finish what is in hand and stop, so the
    /// engine's GPU context is dropped on this thread rather than
    /// under a process that is already on its way out.
    stopping: bool,
}

type Deliver = Arc<dyn Fn(Outcome) + Send + Sync>;

/// How long the editor waits on its way out for the worker to finish
/// what it is in the middle of. A develop is a second or two; a job
/// that outlasts this is left where it is, which is what leaving did
/// before there was any waiting at all.
pub const LEAVING: std::time::Duration = std::time::Duration::from_secs(5);

/// The thumbnail cache on disk, shared between the worker, which
/// reads and writes it, and the settings sheet, which counts and
/// clears it. `None` until the editor hands one over, and in tests.
pub type ThumbCache = Arc<Mutex<Option<Thumbs>>>;

pub struct Worker {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    deliver: Deliver,
    /// The worker's thread, until it is joined on the way out.
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    thumbs: ThumbCache,
}

impl Worker {
    pub fn new(deliver: impl Fn(Outcome) + Send + Sync + 'static) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let deliver: Deliver = Arc::new(deliver);
        let thumbs: ThumbCache = Arc::new(Mutex::new(None));
        let (q, d, t) = (queue.clone(), deliver.clone(), thumbs.clone());
        let thread = std::thread::Builder::new()
            .name("greycard worker".into())
            .spawn(move || run(q, d, t))
            .expect("spawning the worker");
        Self {
            queue,
            deliver,
            thread: Mutex::new(Some(thread)),
            thumbs,
        }
    }

    /// Look thumbnails up in `cache`, and keep the ones made, from
    /// the next thumbnail on.
    /// The cache's one walk to count what it holds starts at once, on
    /// a thread of its own, so neither the window nor a thumbnail waits
    /// on it.
    pub fn set_thumb_cache(&self, cache: Option<Thumbs>) {
        *self.thumbs.lock().expect("thumbnail cache") = cache;
        count_thumb_cache(&self.thumbs, |_| {});
    }

    /// The thumbnail cache, for the settings sheet.
    pub fn thumb_cache(&self) -> ThumbCache {
        self.thumbs.clone()
    }

    /// Stop the worker and wait for it to put its GPU buffers down.
    ///
    /// A process that exits while the worker is inside the driver
    /// dies there rather than at its own hand: the buffers of a
    /// develop half way through go with a device the window has
    /// already torn down. So the editor asks the worker to stop as
    /// its window closes and waits [`LEAVING`] for it to say it has;
    /// longer than that and the run ends anyway, as it did before.
    pub fn stop(&self) {
        {
            let (lock, cv) = &*self.queue;
            lock.lock().expect("worker queue").stopping = true;
            cv.notify_all();
        }
        let Some(thread) = self.thread.lock().expect("worker thread").take() else {
            return;
        };
        let asked = std::time::Instant::now();
        while !thread.is_finished() && asked.elapsed() < LEAVING {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if thread.is_finished() {
            let _ = thread.join();
            tracing::debug!("worker: stopped in {:.0} ms", asked.elapsed().as_millis());
        } else {
            tracing::warn!(
                "worker: still busy after {} s; leaving it",
                LEAVING.as_secs()
            );
        }
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
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().expect("worker queue");
        match job {
            Job::Thumbnail { index, path } => {
                q.wanted = None;
                q.thumbnails.push_back((index, path));
            }
            job @ Job::Export { .. } => q.exports.push_back(job),
            Job::Mask { key, shape } => {
                q.masks.retain(|(k, _)| *k != key);
                q.masks.push_back((key, shape));
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

    /// Make the thumbnails for `first..=last` before the folder's
    /// others, the rest outward from that range. The strip says what
    /// it shows; a folder of hundreds is otherwise decoded in file
    /// order, and the frames on screen wait for every earlier one.
    pub fn want_thumbnails(&self, first: usize, last: usize) {
        let (lock, _) = &*self.queue;
        let mut q = lock.lock().expect("worker queue");
        if q.thumbnails.len() < 2 || q.wanted == Some((first, last)) {
            return;
        }
        q.wanted = Some((first, last));
        order_thumbnails(q.thumbnails.make_contiguous(), first, last);
    }

    /// Make thumbnails at this long edge from now on. The grid's
    /// cells grow past the strip's 178, and a picture made for the
    /// strip is mush in a large one; the size follows the cell both
    /// ways, so a cell that has shrunk stops paying for the one
    /// before it. Whether a picture already made is made again is
    /// the caller's business, and that only ever steps up.
    pub fn set_thumb_size(&self, size: u32) {
        let (lock, _) = &*self.queue;
        lock.lock().expect("worker queue").thumb_size = Some(size);
    }
}

/// The frames on the strip first, in file order, then the rest
/// outward from the middle of that range: what a scroll asks for
/// next is whichever end it is heading towards.
fn order_thumbnails(pending: &mut [(usize, PathBuf)], first: usize, last: usize) {
    let center = (first + last) / 2;
    pending.sort_by_key(|(i, _)| {
        if (first..=last).contains(i) {
            0
        } else {
            i.abs_diff(center)
        }
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

fn run(queue: Arc<(Mutex<Queue>, Condvar)>, deliver: Deliver, thumbs: ThumbCache) {
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
    let mut last: Option<(Edit, u8, Arc<WorkingImage>)> = None;
    // The last develop before its dehaze and sharpen, so a change to
    // either costs only those.
    let mut base: Option<Base> = None;
    // The learned denoiser's answer, kept beside the base so its
    // strength is a blend and not another run of the network.
    let mut learned: Option<LearnedBase> = None;
    // The engine's GPU ops, once the window has a device to give.
    let mut gpu: Option<greycard_gpu::Context> = None;
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
                    discard_base(&mut base, gpu.as_ref());
                    learned = None;
                }
                if let Some(d) = q.gpu.take() {
                    device = Some(d);
                }
                if let Some(job) = q.develop.take() {
                    break job;
                }
                if let Some((key, shape)) = q.masks.pop_front() {
                    break Job::Mask { key, shape };
                }
                if let Some(job) = q.exports.pop_front() {
                    break job;
                }
                if let Some((index, path)) = q.thumbnails.pop_front() {
                    break Job::Thumbnail { index, path };
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
                    tracing::info!("engine ops on the GPU: {}", ctx.name());
                    gpu = Some(ctx);
                }
                Err(e) => tracing::warn!("engine ops stay on the CPU: {e}"),
            }
        }
        if matches!(job, Job::Gpu) {
            continue;
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
                    last = image.map(|i| (edit, turn, i));
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
                    last = image.map(|i| (edit, turn, i));
                    deliver(outcome);
                }
            }
            Job::Export {
                edit,
                path,
                settings,
                on_exists,
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
                let image = match &last {
                    Some((e, t, image)) if *t == turn && e.same_develop(&edit) => {
                        Some(image.clone())
                    }
                    _ => match &input {
                        // The export's picture is the reference's,
                        // on the CPU, whatever the viewport ran on.
                        Some(f) => match develop_job(
                            f,
                            &edit,
                            turn,
                            0,
                            &mut base,
                            &mut learned,
                            &mut ai,
                            cache.as_ref(),
                            lenses.as_deref(),
                            &mut None,
                            &deliver,
                        ) {
                            (_, Some(image)) => {
                                last = Some((edit.clone(), turn, image.clone()));
                                Some(image)
                            }
                            (Outcome::Failed { message, .. }, None) => {
                                deliver(Outcome::ExportFailed { message });
                                None
                            }
                            _ => None,
                        },
                        None => None,
                    },
                };
                if let Some(image) = image {
                    let source = (image.width as u32, image.height as u32);
                    // The learned masks, made now if they are not yet.
                    let mut learned = std::collections::HashMap::new();
                    if let Some(b) = &base {
                        for a in &edit.adjustments {
                            for (i, c) in a.mask.live() {
                                if c.shape.is_learned()
                                    && let Ok(made) = ai.raster(
                                        b.stamp,
                                        &b.image,
                                        &b.edit,
                                        b.source,
                                        (a.id, i),
                                        &c.shape,
                                    )
                                {
                                    learned.insert((a.id, i), made.raster);
                                }
                            }
                        }
                    }
                    let framed;
                    let image: &WorkingImage = if edit.geometry.is_identity() {
                        &image
                    } else {
                        framed = crate::geometry::apply(&image, &edit.geometry);
                        &framed
                    };
                    let clip_level = base.as_ref().map(|b| b.clip_level).unwrap_or(f32::INFINITY);
                    let mut rendered = crate::export::render(
                        image,
                        &edit,
                        source,
                        &settings,
                        &learned,
                        clip_level,
                        base.as_ref().map(|b| &*b.guide),
                        base.as_ref().map(|b| b.source).unwrap_or_default(),
                    );
                    let origin = crate::export::Origin {
                        source_name: opened_path
                            .as_ref()
                            .and_then(|s| s.file_name())
                            .map(|n| n.to_string_lossy().into_owned()),
                        edit: Some(edit.to_json()),
                    };
                    // The mark, on the export alone; one that cannot be
                    // drawn fails the export rather than let an unmarked
                    // picture out.
                    let result = crate::export::mark(&mut rendered, &settings)
                        .and_then(|()| {
                            crate::export::write(
                                &rendered,
                                &settings,
                                &path,
                                metadata.as_deref(),
                                &origin,
                            )
                        })
                        .map_err(|e| format!("{e:#}"));
                    deliver(match result {
                        Ok(()) => Outcome::Exported {
                            path,
                            seconds: start.elapsed().as_secs_f64(),
                            note,
                        },
                        Err(message) => Outcome::ExportFailed { message },
                    });
                }
            }
            Job::Mask { key, shape } => {
                let outcome = match &base {
                    Some(b) => match ai.raster(b.stamp, &b.image, &b.edit, b.source, key, &shape) {
                        Ok(made) => Outcome::Mask {
                            key,
                            shape,
                            raster: made.raster,
                            provider: made.provider.map(|p| p.name()),
                            seconds: made.seconds,
                        },
                        Err(message) => Outcome::MaskFailed { key, message },
                    },
                    None => Outcome::MaskFailed {
                        key,
                        message: "nothing developed yet".into(),
                    },
                };
                deliver(outcome);
            }
            Job::Fetch { .. } | Job::FetchLenses | Job::Gpu => {
                unreachable!("fetches run on their own thread, and the device is taken above")
            }
            Job::Thumbnail { index, path } => {
                let size = lock
                    .lock()
                    .expect("worker queue")
                    .thumb_size
                    .unwrap_or(THUMB_WIDTH);
                let size = crate::grid::made_size(size);
                let started = Instant::now();
                match cached_thumbnail(&thumbs, &path, size) {
                    Ok((thumb, cached)) => deliver(Outcome::Thumbnail {
                        index,
                        path,
                        size,
                        width: thumb.width,
                        height: thumb.height,
                        rgb: thumb.rgb,
                        cached,
                        seconds: started.elapsed().as_secs_f64(),
                    }),
                    Err(e) => {
                        tracing::debug!("thumbnail {}: {e}", path.display());
                        deliver(Outcome::NoThumbnail { index, path });
                    }
                }
            }
        }));
        if let Err(payload) = held {
            discard_base(&mut base, gpu.as_ref());
            learned = None;
            last = None;
            // The next stamp restarts with the base; what was cached
            // under the old ones goes with them.
            ai.forget(opened_path.clone());
            let message = panic_message(payload.as_ref());
            if let Some(outcome) = blame.outcome(message) {
                deliver(outcome);
            }
        }
    }
}

/// Who a job's panic is reported to: the outcome the UI is waiting
/// on for that job, when it is waiting on one.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Blame {
    Develop(u64),
    Export,
    Mask(Key),
    Nobody,
}

impl Blame {
    fn of(job: &Job) -> Self {
        match job {
            Job::Open { generation, .. } | Job::Develop { generation, .. } => {
                Blame::Develop(*generation)
            }
            Job::Export { .. } => Blame::Export,
            Job::Mask { key, .. } => Blame::Mask(*key),
            Job::Thumbnail { .. } | Job::Fetch { .. } | Job::FetchLenses | Job::Gpu => {
                Blame::Nobody
            }
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
            Blame::Nobody => return None,
        })
    }
}

/// A panic's message, which is a `&str` or a `String` for every
/// `panic!` with a text, and nothing readable otherwise.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
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

/// Forget the base, and with it the sharpen's and the CA correction's
/// working textures on the GPU: a new file or a new lens database
/// develops afresh.
fn discard_base(base: &mut Option<Base>, gpu: Option<&greycard_gpu::Context>) {
    if base.take().is_some()
        && let Some(gpu) = gpu
    {
        gpu.release();
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
    /// slider: uploaded once, re-read on every move.
    pre: Option<PreSharpen>,
    /// Whether the CA correction in this base ran on the GPU. The
    /// export's picture is the reference's, and the CA is in the base
    /// (unlike the sharpen, which the export re-runs), so an export
    /// makes a fresh base on the CPU when this is set.
    ca_on_gpu: bool,
}

/// The picture the sharpen reads, on the GPU, with what it was made
/// from and what the steps before the sharpen reported.
struct PreSharpen {
    patched: Arc<WorkingImage>,
    detail: greycard_edit::Detail,
    image: greycard_gpu::Image,
    detail_stats: Option<LocalContrastStats>,
    dehaze_stats: Option<DehazeStats>,
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
/// With `gpu`, the sharpen runs there: the picture before it is
/// uploaded once and kept with the base, and the result is a texture
/// the viewport draws as it is, so a sharpen slider costs the
/// sharpen alone. No CPU picture comes back then; the export, which
/// passes no `gpu`, develops its own on the reference path. A GPU
/// error (out of memory, a lost device) is warned once and the
/// context is dropped: the session stays on the CPU from then on.
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
                white: WhiteBase::IDENTITY,
                radius: None,
                clip_level: meta.clip_level(),
                source: crate::finish::Source::Display,
                stamp,
                patched: None,
                pre: None,
                ca_on_gpu: false,
            }),
            (Input::Raw(frame), Some(model)) => {
                match learned_base(
                    frame, edit, turn, &settings, model, learned, ai, cache, stamp, ca,
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
        match made {
            Ok(mut b) => {
                b.ca_on_gpu = matches!(ca_note, Some((CaRan::Gpu, _)));
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
    let sources = edit.retouch.choose_sources(&b.image);
    let retouch = edit.retouch.with_sources(&sources);
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
    // picture; none when neither is asked for.
    let before_sharpen = |copy: &mut Option<WorkingImage>| {
        let detail = edit.detail.options().map(|options| {
            let started = Instant::now();
            let image = copy.get_or_insert_with(|| (*patched).clone());
            let stats = local_contrast(image, &options, b.clip_level);
            (stats, started.elapsed().as_secs_f64())
        });
        let dehaze_stats = edit.detail.dehaze_options().map(|options| {
            let image = copy.get_or_insert_with(|| (*patched).clone());
            dehaze::dehaze(image, &options)
        });
        (detail, dehaze_stats)
    };
    // The sharpen on the GPU, when there is one and it is on: the
    // picture before it kept on the device from the last develop
    // when nothing before the sharpen changed. With the sharpen off
    // that picture and the op's working textures are let go.
    match (gpu.as_ref(), edit.sharpen.options()) {
        (Some(ctx), Some(options)) => {
            let kept = b
                .pre
                .as_ref()
                .is_some_and(|p| Arc::ptr_eq(&p.patched, &patched) && p.detail == edit.detail);
            let mut detail_seconds = None;
            let uploaded = if kept {
                Ok(())
            } else {
                let mut copy = None;
                let (detail, dehaze_stats) = before_sharpen(&mut copy);
                detail_seconds = detail.map(|(_, s)| s);
                let image: &WorkingImage = copy.as_ref().unwrap_or(&patched);
                ctx.upload(image).map(|image| {
                    b.pre = Some(PreSharpen {
                        patched: patched.clone(),
                        detail: edit.detail,
                        image,
                        detail_stats: detail.map(|(s, _)| s),
                        dehaze_stats,
                    });
                })
            };
            let sharpened = uploaded.and_then(|()| {
                let pre = b.pre.as_ref().expect("uploaded, or kept");
                let texture = ctx.viewport_texture(pre.image.width(), pre.image.height());
                ctx.sharpen(&pre.image, &options, b.radius, b.clip_level, &texture)
                    .map(|stats| (stats, texture))
            });
            match sharpened {
                Ok((stats, texture)) => {
                    let pre = b.pre.as_ref().expect("uploaded, or kept");
                    let seconds = start.elapsed().as_secs_f64();
                    note_developed(
                        pre.image.width(),
                        pre.image.height(),
                        fresh_base,
                        ca_note,
                        &report,
                        &fills,
                        detail_seconds,
                        pre.dehaze_stats.is_some(),
                        Some(Sharpened::Gpu),
                        seconds,
                    );
                    return (
                        Outcome::Developed {
                            generation,
                            image: Developed::Texture(texture),
                            guide: b.guide.clone(),
                            white: b.white,
                            seconds,
                            detail: pre.detail_stats.map(|s| (s, detail_seconds)),
                            sharpen: Some(stats),
                            dehaze: pre.dehaze_stats,
                            sources,
                            learned: report,
                            fills,
                        },
                        None,
                    );
                }
                Err(e) => {
                    // Once is enough: the device is not trusted again
                    // this session, and every develop from here is the
                    // CPU's.
                    tracing::warn!("the GPU sharpen: {e}; the CPU's from now on");
                    b.pre = None;
                    if let Some(ctx) = gpu.take() {
                        ctx.release();
                    }
                }
            }
        }
        (Some(ctx), None) => {
            if b.pre.take().is_some() {
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
        detail.as_ref().map(|(_, s)| *s),
        dehaze_stats.is_some(),
        stats.map(|_| Sharpened::Cpu),
        seconds,
    );
    (
        Outcome::Developed {
            generation,
            image: Developed::Halves(halves),
            guide: b.guide.clone(),
            white: b.white,
            seconds,
            detail: detail.map(|(s, secs)| (s, Some(secs))),
            sharpen: stats,
            dehaze: dehaze_stats,
            sources,
            learned: report,
            fills,
        },
        Some(image),
    )
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
    detail_seconds: Option<f64>,
    dehazed: bool,
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
    if let Some(s) = detail_seconds {
        parts.push(format!("local contrast {s:.2} s"));
    }
    if dehazed {
        parts.push("dehaze".into());
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
        white: WhiteBase::from(&d.white_balance),
        radius: d.sharpen_radius,
        clip_level: d.clip_level,
        source: crate::finish::Source::Scene,
        stamp,
        patched: None,
        pre: None,
        ca_on_gpu: false,
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
) -> Result<(Base, LearnedReport), LearnedReport> {
    let report = if learned
        .as_ref()
        .is_some_and(|l| l.turn == turn && l.edit.same_learned(edit))
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
            ca_on_gpu: false,
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
            white: WhiteBase::from(&model.white_balance),
            radius: model.sharpen_radius,
            clip_level: model.clip_level,
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
pub const THUMB_RECIPE: u16 = 1;

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
                .map(|c| c.root().to_path_buf());
            let Some(root) = root else {
                return then(None);
            };
            let counted = greycard_library::thumbs::usage_at(&root);
            let known = cache
                .lock()
                .expect("thumbnail cache")
                .as_mut()
                .and_then(|c| {
                    c.seed_usage(counted);
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
fn file_stat(path: &std::path::Path) -> Option<(u64, u64)> {
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
fn thumb_tag(stat: (u64, u64)) -> Tag {
    Tag {
        recipe: THUMB_RECIPE,
        stamp: stat.1,
    }
}

/// A file's thumbnail from the cache when it holds one for the file's
/// content, else made by [`thumbnail`] and kept. The lookup costs a
/// stat and the content hash, a read of the file's first 64 KB, and
/// nothing else of the file; a cache that cannot be read or written is
/// a miss and a thumbnail made, never an error of its own. Says
/// whether it came from the cache.
fn cached_thumbnail(
    cache: &ThumbCache,
    path: &std::path::Path,
    size: u32,
) -> anyhow::Result<(Thumb, bool)> {
    cached_thumbnail_with(cache, path, size, thumbnail)
}

/// [`cached_thumbnail`] with the making handed in, for the tests. The
/// file is stat'd before it is hashed and again after the picture is
/// made, and a picture made while the file was changing — its length
/// or its time moved — is shown but not kept: it may be of a file
/// half written, and kept under the key the finished file will have.
fn cached_thumbnail_with(
    cache: &ThumbCache,
    path: &std::path::Path,
    size: u32,
    make: impl FnOnce(&std::path::Path, u32) -> anyhow::Result<(u32, u32, Vec<u8>)>,
) -> anyhow::Result<(Thumb, bool)> {
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
    if let Some((hash, tag)) = &key {
        let hit = cache
            .lock()
            .expect("thumbnail cache")
            .as_mut()
            .and_then(|c| c.get(hash, size, *tag));
        if let Some(thumb) = hit {
            return Ok((thumb, true));
        }
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
fn thumbnail(path: &std::path::Path, size: u32) -> anyhow::Result<(u32, u32, Vec<u8>)> {
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

    #[test]
    fn a_panic_in_a_job_is_the_failure_that_job_was_waited_on_for() {
        let develop = Job::Develop {
            edit: Edit::default(),
            generation: 7,
            turn: 0,
        };
        // The panics here are on purpose; the hook has nothing to say.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let payload = std::panic::catch_unwind(|| panic!("on purpose {}", 1)).unwrap_err();
        let plain = std::panic::catch_unwind(|| panic!("plain")).unwrap_err();
        std::panic::set_hook(hook);
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

    /// A context on a device of our own with `limits`, for driving
    /// the GPU paths into their errors; none without an adapter.
    fn context_with(limits: greycard_gpu::wgpu::Limits) -> Option<greycard_gpu::Context> {
        use greycard_gpu::wgpu;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
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

    fn skipped(what: &str) {
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
            cached_thumbnail(&cache, &raw, THUMB_WIDTH).is_err(),
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
        let (thumb, cached) = cached_thumbnail(&cache, &moved, THUMB_WIDTH).unwrap();
        assert!(cached);
        assert_eq!((thumb.width, thumb.height), (3, 2));
        // Another size is not the same entry.
        assert!(cached_thumbnail(&cache, &moved, 256).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
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
        let (first, cached) = cached_thumbnail(&cache, &png, 16).unwrap();
        assert!(!cached);
        let (again, cached) = cached_thumbnail(&cache, &png, 16).unwrap();
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
        let (fresh, cached) = cached_thumbnail(&cache, &png, 16).unwrap();
        assert!(!cached);
        assert!(
            fresh.rgb.iter().all(|v| *v < 60),
            "the new picture's pixels"
        );
        std::fs::remove_dir_all(&dir).unwrap();
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
        let (grey, cached) = cached_thumbnail_with(&cache, &raw, 176, tail_picture).unwrap();
        assert!(!cached);
        assert!(grey.rgb.iter().all(|v| *v == 0));
        // The same head and the same length: the same hash.
        let before = greycard_library::hash_file(&raw).unwrap();
        std::fs::write(&raw, &whole).unwrap();
        set_time(&raw, 7);
        assert_eq!(greycard_library::hash_file(&raw).unwrap(), before);
        let (done, cached) = cached_thumbnail_with(&cache, &raw, 176, tail_picture).unwrap();
        assert!(
            !cached,
            "the half-copied picture is not the finished file's"
        );
        assert!(done.rgb.iter().all(|v| *v == *whole.last().unwrap()));
        // And the finished file's picture is kept, and a rename keeps
        // its time and finds it.
        let renamed = dir.join("wedding-1981.NEF");
        std::fs::rename(&raw, &renamed).unwrap();
        let (again, cached) = cached_thumbnail_with(&cache, &renamed, 176, tail_picture).unwrap();
        assert!(cached);
        assert_eq!(again.rgb, done.rgb);
        std::fs::remove_dir_all(&dir).unwrap();
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
            let (_, cached) = cached_thumbnail_with(&cache, path, 176, tail_picture).unwrap();
            assert_eq!(cached, round >= 2, "round {round}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
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
        let (_, cached) = cached_thumbnail_with(&cache, &raw, 176, grows).unwrap();
        assert!(!cached);
        let touched = |path: &std::path::Path, size: u32| {
            let made = tail_picture(path, size);
            set_time(path, 30);
            made
        };
        let (_, cached) = cached_thumbnail_with(&cache, &raw, 176, touched).unwrap();
        assert!(!cached);
        assert_eq!(
            cache.lock().unwrap().as_ref().unwrap().usage().entries,
            0,
            "neither was kept"
        );
        // Left alone, it is made once and kept.
        let (_, cached) = cached_thumbnail_with(&cache, &raw, 176, tail_picture).unwrap();
        assert!(!cached);
        let (_, cached) = cached_thumbnail_with(&cache, &raw, 176, tail_picture).unwrap();
        assert!(cached);
        std::fs::remove_dir_all(&dir).unwrap();
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
            let (_, cached) = cached_thumbnail(&cache, &png, 8).unwrap();
            assert!(!cached);
        }
        assert!(!dir.join("thumbs").exists());
        std::fs::remove_dir_all(&dir).unwrap();
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
        std::fs::remove_dir_all(&dir).unwrap();
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
            let (_, cached) = cached_thumbnail(&off, &png, 8).unwrap();
            assert!(!cached);
        }
        assert!(!dir.join("thumbs").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
