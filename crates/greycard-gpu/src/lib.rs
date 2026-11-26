//! greycard-gpu: GPU implementations of greycard-core's engine ops.
//!
//! Every op here has its CPU reference in greycard-core (notes §5),
//! and is held to it by a test that runs both on the same picture and
//! states the tolerance. The reference stays the export's path; this
//! crate is the accelerator for the viewport, where an op re-runs on
//! a slider.
//!
//! A [`Context`] is built either from a device and queue the caller
//! already has (the editor's, which the viewport shares) or from an
//! adapter of its own (the CLI and the tests). greycard-core never
//! sees wgpu; that boundary is this crate's reason to exist.
//!
//! The ops so far: the capture sharpening ([`Context::sharpen`]), the
//! lateral chromatic aberration correction ([`Context::correct_ca`]),
//! the lens correction ([`Context::correct_lens`]), the local contrast
//! ([`Context::local_contrast`]), the dehaze's block sums and apply
//! ([`Context::dehaze_reduced`], [`Context::dehaze`]), its fit staying
//! the reference's on the CPU, and the Bayer demosaics: AMaZE
//! ([`Context::demosaic_amaze`]), RCD, VNG4, and their duals
//! ([`Context::demosaic_rcd`], [`Context::demosaic_vng4`],
//! [`Context::demosaic_rcd_dual`]).

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use greycard_core::image::WorkingImage;

pub use wgpu;

/// A shader that stores half floats, `half.wgsl` before it: its
/// `halves_nearest` rounds what the shader stores as the CPU's halves
/// are rounded.
macro_rules! with_halves {
    ($file:literal) => {
        concat!(include_str!("half.wgsl"), "\n", include_str!($file))
    };
}

mod amaze;
mod ca;
mod dehaze;
mod demosaic;
mod lens;
mod local_contrast;
mod plumbing;
mod sharpen;

pub use sharpen::{BACKGROUND_BUDGET, Batching, SUBMIT_BUDGET, SharpenWatch};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no GPU adapter")]
    NoAdapter,
    #[error("the GPU device: {0}")]
    Device(String),
    /// The device reported an error while `what` ran: a validation
    /// failure, out of memory, or an internal one. Caught in an error
    /// scope, so it comes back as this rather than going to the
    /// device's uncaptured handler: wgpu's default one panics, and the
    /// editor's (set by greycard-ui on the device it shares with
    /// Slint, which sets none) panics on all but a surface configure
    /// that raced another thread's submits.
    #[error("the GPU while {what}: {error}")]
    Gpu { what: &'static str, error: String },
    #[error("{0}")]
    Unsupported(String),
    #[error("reading back from the GPU: {0}")]
    ReadBack(String),
    /// The device is gone: a driver reset it (Windows' TDR after a
    /// submission ran over its two seconds, amdgpu's ring timeout), or
    /// it was destroyed. Nothing made on it works again; the caller
    /// lets the context go.
    #[error("the GPU device was lost: {0}")]
    Lost(String),
    /// The caller asked the op to give way part way, for newer work:
    /// nothing is wrong with the device, and what the op was writing
    /// is not to be read.
    #[error("given way part way")]
    GaveWay,
}

thread_local! {
    /// Whether this thread is inside a wait on the device: wgpu panics
    /// there when the device is lost, the panic is caught and becomes
    /// [`Error::Lost`], and a panic hook can say so without a
    /// backtrace.
    static WAITING: Cell<bool> = const { Cell::new(false) };
}

/// Whether the calling thread is inside one of this crate's waits on
/// the device, for a panic hook: a panic there is a lost device, and is
/// caught.
pub fn inside_wait() -> bool {
    WAITING.with(Cell::get)
}

/// Whether a wgpu error, or any of its causes, is the device's loss.
fn is_loss(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut cause = Some(error);
    while let Some(e) = cause {
        if matches!(
            e.downcast_ref::<wgpu::wgc::device::DeviceError>(),
            Some(wgpu::wgc::device::DeviceError::Lost)
        ) {
            return true;
        }
        cause = e.source();
    }
    false
}

/// Wait for everything submitted to `device` so far. wgpu panics in the
/// wait when the device has been lost, rather than return an error, so
/// the panic is caught here and comes back as [`Error::Lost`], and
/// `lost` is set.
fn wait_on(device: &wgpu::Device, lost: &AtomicBool) -> Result<()> {
    if lost.load(Ordering::Acquire) {
        return Err(Error::Lost("lost before this wait".into()));
    }
    struct Waiting;
    impl Drop for Waiting {
        fn drop(&mut self) {
            WAITING.with(|w| w.set(false));
        }
    }
    WAITING.with(|w| w.set(true));
    let waiting = Waiting;
    let polled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        device.poll(wgpu::PollType::wait_indefinitely())
    }));
    drop(waiting);
    match polled {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(Error::ReadBack(format!("waiting for the device: {e}"))),
        Err(payload) => {
            lost.store(true, Ordering::Release);
            let message = panic_text(payload.as_ref()).unwrap_or("wgpu panicked in its wait");
            Err(Error::Lost(
                message.lines().last().unwrap_or(message).trim().to_string(),
            ))
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A panic's text, which is a `&str` or a `String` for every `panic!`
/// with one; `None` otherwise.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

/// A panic's message, which is a `&str` or a `String` for every
/// `panic!` with a text, and nothing readable otherwise.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    panic_text(payload).unwrap_or("(no message)").to_string()
}

/// What [`Context::kept_bytes`] reports, per op.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeptBytes {
    pub sharpen: u64,
    pub local_contrast: u64,
}

/// The texture limit asked of an adapter of our own: 45 MP frames are
/// 8192 wide and more, over the 8192 default.
const WANTED_TEXTURE_DIMENSION: u32 = 16384;

/// A wgpu instance of the caller's own, made one at a time across the
/// process. Two made at once on two threads can kill it: NVIDIA's
/// Vulkan driver (libGLX_nvidia, seen on 615.71) sets its
/// initialized flag on entering the loader's interface negotiation,
/// before it has opened its X display and filled the table the
/// negotiation jumps through, so a second thread negotiating meanwhile
/// jumps to a null pointer. The loader negotiates on every load of the
/// driver, which is every instance made while no other is alive. Test
/// binaries make one a test, a test a thread; every instance this crate
/// and greycard-ui make comes here (Slint's, the WebGPU probe's and
/// ONNX Runtime's are their own). One instance shared would not do: the GL
/// backend's adapter enumeration on a shared instance from two threads
/// fails with EGL_BAD_ACCESS.
pub fn instance() -> wgpu::Instance {
    make_instance(wgpu::InstanceDescriptor::new_without_display_handle_from_env())
}

fn make_instance(descriptor: wgpu::InstanceDescriptor) -> wgpu::Instance {
    static MAKING: Mutex<()> = Mutex::new(());
    let _one_at_a_time = MAKING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    wgpu::Instance::new(descriptor)
}

/// Every adapter wgpu finds on `backends`, on an instance of our own
/// made for them alone, so the probe loads no other driver stack:
/// what the window could draw on, asked when Slint took none of it.
pub fn adapters(backends: wgpu::Backends) -> Vec<wgpu::AdapterInfo> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    descriptor.backends = backends;
    let instance = make_instance(descriptor);
    pollster::block_on(instance.enumerate_adapters(backends))
        .iter()
        .map(wgpu::Adapter::get_info)
        .collect()
}

/// The adapter a device of our own is made on: the one
/// `WGPU_ADAPTER_NAME` names where it is set, else the one wgpu takes
/// for the power preference `WGPU_POWER_PREF` names, high performance
/// where it names none, as the window asks for its own. Its kind is the
/// caller's to judge: a software one (lavapipe, WARP) comes back too.
fn adapter() -> Result<wgpu::Adapter> {
    let instance = instance();
    if let Ok(name) = std::env::var(WGPU_ADAPTER_NAME) {
        let name = name.to_lowercase();
        return pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
            .into_iter()
            .find(|a| a.get_info().name.to_lowercase().contains(&name))
            .ok_or(Error::NoAdapter);
    }
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference:
            wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance),
        ..Default::default()
    }))
    .map_err(|_| Error::NoAdapter)
}

/// A device of our own on `adapter`, with the limits the ops want.
fn device_on(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue)> {
    let limits = adapter.limits();
    let required_limits = wgpu::Limits {
        max_texture_dimension_2d: WANTED_TEXTURE_DIMENSION.min(limits.max_texture_dimension_2d),
        ..wgpu::Limits::default()
    };
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("greycard-gpu"),
        required_limits,
        ..Default::default()
    }))
    .map_err(|e| Error::Device(e.to_string()))
}

/// The variable Slint reads to take a CPU adapter.
pub const SLINT_WGPU_CPU: &str = "SLINT_WGPU_CPU";

/// The variable wgpu reads to take the adapter it names.
pub const WGPU_ADAPTER_NAME: &str = "WGPU_ADAPTER_NAME";

/// Set beside [`SLINT_WGPU_CPU`] where a greycard set it, so a greycard
/// started from one does not read it as asked for by hand.
pub const SLINT_WGPU_CPU_OURS: &str = "GREYCARD_SET_SLINT_WGPU_CPU";

/// Whether the adapter was asked for by hand in the environment `var`
/// reads: [`SLINT_WGPU_CPU`] that no greycard set, or
/// [`WGPU_ADAPTER_NAME`]. The window and the command line's export
/// both take a software adapter only then.
pub fn asked_by_hand(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    let slint_cpu = var(SLINT_WGPU_CPU).is_some_and(|v| v.to_str().is_some());
    (slint_cpu && var(SLINT_WGPU_CPU_OURS).is_none()) || var(WGPU_ADAPTER_NAME).is_some()
}

/// Whether an export takes an adapter of `kind`: any but a software
/// one, which only when `software` says it was asked for.
fn takes(kind: wgpu::DeviceType, software: bool) -> bool {
    kind != wgpu::DeviceType::Cpu || software
}

/// Why [`Context::for_export`] gave no context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declined {
    /// No adapter at all, or none named.
    NoGpu,
    /// Only a software adapter, not asked for.
    Software,
    /// The adapter would not make a device or build the ops.
    Failed(String),
}

impl std::fmt::Display for Declined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Declined::NoGpu => f.write_str("no GPU"),
            Declined::Software => f.write_str("software adapter declined"),
            Declined::Failed(e) => write!(f, "no GPU: {e}"),
        }
    }
}

/// A device, its queue, and the ops' pipelines built for it.
pub struct Context {
    device: wgpu::Device,
    queue: wgpu::Queue,
    name: String,
    /// The device's largest texture side; a picture wider than this
    /// cannot be worked here.
    max_dimension: u32,
    sharpen: sharpen::Pipelines,
    ca: ca::Pipelines,
    lens: lens::Pipelines,
    local_contrast: local_contrast::Pipelines,
    dehaze: dehaze::Pipelines,
    /// The demosaics' pipelines, built on their first use: AMaZE's is
    /// the largest shader here, and a device that cannot build them
    /// still runs every other op.
    demosaic: OnceLock<std::result::Result<demosaic::Pipelines, String>>,
    amaze: OnceLock<std::result::Result<amaze::Pipelines, String>>,
    /// At most this many rows a demosaic band, where not zero: for the
    /// tests ([`Context::limit_demosaic_bands`]).
    demosaic_rows: std::sync::atomic::AtomicUsize,
    /// At most this many rows a buffer of a detail result kept on the
    /// device, where not zero: for the tests
    /// ([`Context::limit_demosaic_frame_buffers`]).
    demosaic_frame_rows: std::sync::atomic::AtomicUsize,
    /// At most this many rows of tiles an AMaZE batch for the dual,
    /// where not zero: for the tests
    /// ([`Context::limit_amaze_dual_batches`]).
    amaze_dual_tile_rows: std::sync::atomic::AtomicUsize,
    /// The bytes the demosaics have sent up and read back since
    /// [`Context::take_demosaic_traffic`] last took them: for the
    /// ignored timing test.
    demosaic_traffic: [std::sync::atomic::AtomicU64; 2],
    /// Set once the device is found lost: by a wait that panicked, an
    /// error naming the loss, or the device's lost callback on a device
    /// of our own.
    lost: Arc<AtomicBool>,
}

impl Context {
    /// On a device the caller holds: the editor's, so the result of
    /// an op is a texture the viewport draws without a copy. An error
    /// building the pipelines is returned, not raised.
    pub fn from_device(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self> {
        let info = device.adapter_info();
        Self::build(device.clone(), queue.clone(), info)
    }

    /// On an adapter of our own ([`adapter`]), or [`Error::NoAdapter`]
    /// when the machine has none.
    pub fn own() -> Result<Self> {
        let adapter = adapter()?;
        let (device, queue) = device_on(&adapter)?;
        let ctx = Self::build(device, queue, adapter.get_info())?;
        ctx.watch_loss();
        Ok(ctx)
    }

    /// A context of our own for an export with no window, on the
    /// adapter [`adapter`] takes: refused when that is a software one
    /// (lavapipe, WARP) unless `software` says it was asked for, since
    /// its shaders run slower than the CPU runs the same stages.
    pub fn for_export(software: bool) -> std::result::Result<Self, Declined> {
        let adapter = adapter().map_err(|_| Declined::NoGpu)?;
        let info = adapter.get_info();
        if !takes(info.device_type, software) {
            log::info!("greycard-gpu: {} declined, a software adapter", info.name);
            return Err(Declined::Software);
        }
        let (device, queue) = device_on(&adapter).map_err(|e| Declined::Failed(e.to_string()))?;
        let ctx = Self::build(device, queue, info).map_err(|e| Declined::Failed(e.to_string()))?;
        ctx.watch_loss();
        Ok(ctx)
    }

    /// The device is ours, so its lost callback is too: it marks the
    /// context lost. On a device the caller lent, the callback is the
    /// caller's.
    fn watch_loss(&self) {
        let lost = self.lost.clone();
        self.device
            .set_device_lost_callback(move |reason, message| {
                lost.store(true, Ordering::Release);
                log::warn!("greycard-gpu: the device was lost ({reason:?}): {message}");
            });
    }

    fn build(device: wgpu::Device, queue: wgpu::Queue, info: wgpu::AdapterInfo) -> Result<Self> {
        let name = info.name;
        let max_dimension = device.limits().max_texture_dimension_2d;
        // A discrete GPU's memory is its own, so the sharpen's and the
        // local contrast's planes are kept there between runs; an
        // integrated or unified-memory GPU (Apple's), a virtual one or
        // the CPU shares the machine's: the local contrast's planes are
        // made for a run, and the sharpen's kept only until the picture
        // rests.
        let keep = info.device_type == wgpu::DeviceType::DiscreteGpu;
        let sharpen = scoped(&device, "building the sharpen's pipelines", || {
            Ok(sharpen::Pipelines::new(&device, keep))
        })?;
        let ca = scoped(&device, "building the CA correction's pipelines", || {
            Ok(ca::Pipelines::new(&device))
        })?;
        let lens = scoped(&device, "building the lens correction's pipeline", || {
            Ok(lens::Pipelines::new(&device))
        })?;
        let local_contrast = scoped(&device, "building the local contrast's pipelines", || {
            Ok(local_contrast::Pipelines::new(&device, keep))
        })?;
        let dehaze = scoped(&device, "building the dehaze's pipelines", || {
            Ok(dehaze::Pipelines::new(&device))
        })?;
        log::info!("greycard-gpu on {name}, textures up to {max_dimension}");
        Ok(Self {
            device,
            queue,
            name,
            max_dimension,
            sharpen,
            ca,
            lens,
            local_contrast,
            dehaze,
            demosaic: OnceLock::new(),
            amaze: OnceLock::new(),
            demosaic_rows: std::sync::atomic::AtomicUsize::new(0),
            demosaic_frame_rows: std::sync::atomic::AtomicUsize::new(0),
            amaze_dual_tile_rows: std::sync::atomic::AtomicUsize::new(0),
            demosaic_traffic: Default::default(),
            lost: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The demosaics' pipelines, built the first time one is asked for;
    /// a build the device refused is [`Error::Gpu`] then and after.
    fn demosaic_pipelines(&self) -> Result<&demosaic::Pipelines> {
        built(
            self.demosaic.get_or_init(|| {
                scoped(&self.device, "building the demosaic's pipelines", || {
                    Ok(demosaic::Pipelines::new(&self.device))
                })
                .map_err(|e| e.to_string())
            }),
            "building the demosaic's pipelines",
        )
    }

    /// AMaZE's pipelines, as [`Context::demosaic_pipelines`] are built.
    fn amaze_pipelines(&self) -> Result<&amaze::Pipelines> {
        built(
            self.amaze.get_or_init(|| {
                scoped(&self.device, "building AMaZE's pipelines", || {
                    Ok(amaze::Pipelines::new(&self.device))
                })
                .map_err(|e| e.to_string())
            }),
            "building AMaZE's pipelines",
        )
    }

    /// The bytes the demosaics sent up to the device and read back
    /// from it since the last call, `(up, back)`, and zero again: for
    /// the ignored timing test, which prints what each op moves.
    #[doc(hidden)]
    pub fn take_demosaic_traffic(&self) -> (u64, u64) {
        let [up, back] = &self.demosaic_traffic;
        (
            up.swap(0, Ordering::Relaxed),
            back.swap(0, Ordering::Relaxed),
        )
    }

    /// Count bytes sent up (`back` false) or read back by a demosaic.
    fn count_demosaic_traffic(&self, back: bool, bytes: u64) {
        self.demosaic_traffic[usize::from(back)].fetch_add(bytes, Ordering::Relaxed);
    }

    /// Whether the device has been found lost. Every op on a lost
    /// context returns [`Error::Lost`].
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }

    /// Mark the device lost, for a caller told by the device's own
    /// lost callback, which is the caller's on a lent device.
    pub fn mark_lost(&self) {
        self.lost.store(true, Ordering::Release);
    }

    /// Let go of the working textures the ops keep between runs (the
    /// sharpen's four planes and its atlas, about 900 MB at 45 MP, kept
    /// at rest on a discrete GPU only;
    /// the local contrast's, up to five planes and its grid, about 950
    /// MB, kept on a discrete GPU only; the CA correction keeps
    /// nothing, its 812 MiB at 45 MP being made and dropped within a
    /// run, and neither do the lens correction and the dehaze). For
    /// when the ops are switched off or the picture is closed; the
    /// next run makes them again.
    pub fn release(&self) {
        self.release_sharpen();
        self.release_local_contrast();
    }

    /// A hold of the ops' kept working sets that lets them go without
    /// the context in hand ([`Releaser`]).
    pub fn releaser(&self) -> Releaser {
        Releaser {
            sharpen: self.sharpen.releaser(),
            local_contrast: self.local_contrast.releaser(),
            device: self.device.clone(),
            lost: self.lost.clone(),
        }
    }

    /// Let go of the sharpen's working textures alone: for a develop
    /// that runs the local contrast here with the sharpen off.
    pub fn release_sharpen(&self) {
        self.sharpen.release();
    }

    /// Let go of the local contrast's working textures alone: for a
    /// develop that runs the sharpen here without it, or with it on
    /// the CPU.
    pub fn release_local_contrast(&self) {
        self.local_contrast.release();
    }

    /// The bytes of working textures and buffers the ops keep between
    /// runs, the sharpen's and the local contrast's, from their sizes
    /// and formats: what [`Context::release`] lets go.
    pub fn kept_bytes(&self) -> KeptBytes {
        KeptBytes {
            sharpen: self.sharpen.kept_bytes(),
            local_contrast: self.local_contrast.kept_bytes(),
        }
    }

    /// The picture sizes the kept working sets were made for, the
    /// sharpen's and the local contrast's, where one is kept: so a
    /// caller that ran a picture of another size can let its sets go
    /// rather than leave them where another's were.
    pub fn kept_sizes(&self) -> KeptSizes {
        KeptSizes {
            sharpen: self.sharpen.kept_size(),
            local_contrast: self.local_contrast.kept_size(),
        }
    }

    /// Whether the sharpen keeps its working set while the picture
    /// rests: on a discrete GPU by default; else it is kept through an
    /// interaction (a develop's first round, its settle, and the
    /// develops that take the settle's place) and let go at rest.
    pub fn keeps_sharpen_planes(&self) -> bool {
        self.sharpen.keeps()
    }

    /// Keep the sharpen's working set at rest or not, against the
    /// device's default: for the tests and for measuring what each
    /// costs. Turning it off lets the kept one go.
    pub fn keep_sharpen_planes(&self, keep: bool) {
        self.sharpen.set_keeps(keep);
    }

    /// The picture on screen is the edit's at rest, no settle to come:
    /// a working set kept only through an interaction (the sharpen's,
    /// off a discrete GPU, 1.8 GB at 102 MP) is let go, the next
    /// develop making it again.
    pub fn at_rest(&self) {
        if !self.sharpen.keeps() {
            self.sharpen.release();
        }
    }

    /// A hold of the sharpen's working set's bytes and of how many were
    /// made, for the tests.
    pub fn sharpen_watch(&self) -> SharpenWatch {
        self.sharpen.watch()
    }

    /// Whether the local contrast keeps its planes between runs: on a
    /// discrete GPU by default, else made for each run and let go.
    pub fn keeps_local_contrast_planes(&self) -> bool {
        self.local_contrast.keeps()
    }

    /// Keep the local contrast's planes between runs or make them for
    /// each, against the device's default: for the tests and for
    /// measuring what each costs. Turning it off lets the kept ones
    /// go.
    pub fn keep_local_contrast_planes(&self, keep: bool) {
        self.local_contrast.set_keeps(keep);
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The adapter's name, for the log.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether a picture of this size can be worked here.
    fn fits(&self, width: usize, height: usize) -> Result<(u32, u32)> {
        let max = self.max_dimension as usize;
        if width == 0 || height == 0 || width > max || height > max {
            return Err(Error::Unsupported(format!(
                "a {width}x{height} picture; this device takes up to {max} a side"
            )));
        }
        Ok((width as u32, height as u32))
    }

    /// A developed picture put on the GPU, in full floats, for the ops
    /// to read: once per base develop, then every slider re-run reads
    /// it there.
    pub fn upload(&self, image: &WorkingImage) -> Result<Image> {
        let (w, h) = self.fits(image.width, image.height)?;
        self.scoped("uploading the picture", || {
            self.upload_unscoped(image, w, h)
        })
    }

    fn upload_unscoped(&self, image: &WorkingImage, w: u32, h: u32) -> Result<Image> {
        use rayon::prelude::*;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("developed, full floats"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            // Read back as it is when nothing after the upload changed it.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        // A band of rows at a time, each submitted before the next: the
        // interleave to four channels is a pass over the picture, and
        // the queue keeps every `write_texture`'s staging copy until
        // the next submit, so without the submits a 45 MP frame would
        // hold another 720 MB on the CPU and as much again in staging
        // while the copy waits.
        let band = (32usize << 20).div_ceil(image.width * 16).max(1);
        let mut rgba = vec![0f32; image.width * band * 4];
        for y0 in (0..image.height).step_by(band) {
            let rows = band.min(image.height - y0);
            let src = &image.data[y0 * image.width * 3..(y0 + rows) * image.width * 3];
            rgba[..rows * image.width * 4]
                .par_chunks_mut(image.width * 4)
                .zip(src.par_chunks(image.width * 3))
                .for_each(|(out, row)| {
                    let (out, _) = out.as_chunks_mut::<4>();
                    let (row, _) = row.as_chunks::<3>();
                    for (o, px) in out.iter_mut().zip(row) {
                        *o = [px[0], px[1], px[2], 1.0];
                    }
                });
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: y0 as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&rgba[..rows * image.width * 4]),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 16),
                    rows_per_image: Some(rows as u32),
                },
                wgpu::Extent3d {
                    width: w,
                    height: rows as u32,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit([]);
        }
        Ok(Image {
            texture,
            width: w,
            height: h,
            threshold: OnceLock::new(),
        })
    }

    /// A picture on the GPU read back as a working image, its alpha
    /// dropped: for an export's finish on the CPU, the CLI, and the
    /// tests, not a slider. A band of rows at a time, so the four
    /// channels' copy is never the whole picture's: at 45 MP that
    /// would be 720 MB beside the 540 MB result.
    pub fn download(&self, image: &Image) -> Result<WorkingImage> {
        let (w, h) = (image.width, image.height);
        let band = ((64u32 << 20) / (w * 16)).max(1);
        let mut data = Vec::with_capacity(w as usize * h as usize * 3);
        for y0 in (0..h).step_by(band as usize) {
            let rows = band.min(h - y0);
            let rgba = self.read_texture(&image.texture, 0, y0, w, rows, 16)?;
            let rgba: &[f32] = bytemuck::cast_slice(&rgba);
            data.extend(
                rgba.as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|px| [px[0], px[1], px[2]]),
            );
        }
        WorkingImage::from_data(w as usize, h as usize, data)
            .map_err(|e| Error::ReadBack(e.to_string()))
    }

    /// Rows `y..y + rows` of a texture made by
    /// [`viewport_texture`](Self::viewport_texture), read back as they
    /// are: RGBA half floats, eight bytes a texel, row after row, the
    /// mask in the alpha. For a caller that keeps a smaller copy of the
    /// developed picture and reads it a band at a time rather than whole.
    pub fn read_viewport_rows(
        &self,
        texture: &wgpu::Texture,
        y: u32,
        rows: u32,
    ) -> Result<Vec<u8>> {
        self.read_texture(texture, 0, y, texture.width(), rows, 8)
    }

    /// A texture for an op's result as the viewport draws it: RGBA
    /// half floats, the op's mask in the alpha.
    pub fn viewport_texture(&self, width: u32, height: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("developed"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn encoder(&self, label: &str) -> wgpu::CommandEncoder {
        self.device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) })
    }

    /// Wait for everything submitted so far.
    fn wait(&self) -> Result<()> {
        wait_on(&self.device, &self.lost)
    }

    /// [`scoped`] on this context's device: a lost device is
    /// [`Error::Lost`] before anything is asked of it, and after.
    fn scoped<T>(&self, what: &'static str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        if self.is_lost() {
            return Err(Error::Lost(format!("lost before {what}")));
        }
        let out = match scoped(&self.device, what, f) {
            // A lost device's errors mostly name what it made invalid
            // and not the loss, which a wait then shows.
            Err(Error::Gpu { what, error }) => match self.wait() {
                Err(Error::Lost(why)) => Err(Error::Lost(format!("{what}: {why}"))),
                _ => Err(Error::Gpu { what, error }),
            },
            out => out,
        };
        if matches!(out, Err(Error::Lost(_))) {
            self.mark_lost();
        }
        out
    }

    /// `len` bytes of a buffer the GPU wrote, from `offset`, after
    /// everything submitted so far has run.
    fn read_buffer(&self, buffer: &wgpu::Buffer, offset: u64, len: u64) -> Result<Vec<u8>> {
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("read back"),
            size: len.max(4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("read back"),
            });
        encoder.copy_buffer_to_buffer(buffer, offset, &staging, 0, len);
        self.queue.submit(Some(encoder.finish()));
        let slice = staging.slice(..len.max(4));
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.wait()?;
        rx.recv()
            .map_err(|_| Error::ReadBack("the map callback never came".into()))?
            .map_err(|e| Error::ReadBack(e.to_string()))?;
        let data = slice
            .get_mapped_range()
            .map_err(|e| Error::ReadBack(e.to_string()))?;
        let out = data[..len as usize].to_vec();
        drop(data);
        staging.unmap();
        Ok(out)
    }

    /// A rectangle of a texture, rows packed, `bytes` a texel. In bands
    /// of rows so that no staging buffer outgrows a device's limit.
    fn read_texture(
        &self,
        texture: &wgpu::Texture,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        bytes: u32,
    ) -> Result<Vec<u8>> {
        self.scoped("reading back", || {
            self.read_texture_unscoped(texture, x, y, width, height, bytes)
        })
    }

    fn read_texture_unscoped(
        &self,
        texture: &wgpu::Texture,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        bytes: u32,
    ) -> Result<Vec<u8>> {
        let row = (width * bytes).div_ceil(256) * 256;
        let band = ((64u32 << 20) / row).max(1);
        let mut out = Vec::with_capacity(width as usize * bytes as usize * height as usize);
        for y0 in (0..height).step_by(band as usize) {
            let rows = band.min(height - y0);
            let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("read back"),
                size: (row * rows) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("read back"),
                });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y: y + y0, z: 0 },
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
                    width,
                    height: rows,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit(Some(encoder.finish()));
            let slice = staging.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            self.wait()?;
            rx.recv()
                .map_err(|_| Error::ReadBack("the map callback never came".into()))?
                .map_err(|e| Error::ReadBack(e.to_string()))?;
            let data = slice
                .get_mapped_range()
                .map_err(|e| Error::ReadBack(e.to_string()))?;
            for r in 0..rows as usize {
                let line = &data[r * row as usize..][..(width * bytes) as usize];
                out.extend_from_slice(line);
            }
            drop(data);
            staging.unmap();
        }
        Ok(out)
    }
}

/// Run `f` under error scopes for validation, out-of-memory and
/// internal errors, so that whatever the device rejects comes back as
/// [`Error::Gpu`] rather than through the device's uncaptured handler,
/// which panics (see [`Error::Gpu`]). Scopes are the calling thread's
/// own, so they catch the ops' errors and none of the window's.
/// Scopes pop in the reverse of their pushes.
fn scoped<T>(
    device: &wgpu::Device,
    what: &'static str,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let result = f();
    let caught = [
        pollster::block_on(internal.pop()),
        pollster::block_on(memory.pop()),
        pollster::block_on(validation.pop()),
    ]
    .into_iter()
    .flatten()
    .next();
    match (result, caught) {
        (_, Some(error)) if is_loss(&error) => Err(Error::Lost(format!("{what}: {error}"))),
        (_, Some(error)) => Err(Error::Gpu {
            what,
            error: error.to_string(),
        }),
        (result, None) => result,
    }
}

/// Pipelines built once, or why they could not be.
fn built<'a, T>(
    pipelines: &'a std::result::Result<T, String>,
    what: &'static str,
) -> Result<&'a T> {
    pipelines.as_ref().map_err(|error| Error::Gpu {
        what,
        error: error.clone(),
    })
}

/// Lets one op's kept working set go and says its bytes.
pub(crate) type ReleaseOne = Box<dyn Fn() -> u64 + Send + Sync>;

/// What [`Context::release`] lets go, held apart from the context
/// ([`Context::releaser`]): for a caller that holds the context
/// elsewhere, such as a learned model asking the card for room while
/// the engine's thread has it. Each op's set is behind the lock its
/// run holds, so a release waits for a run in progress and the next
/// run makes the set again.
pub struct Releaser {
    sharpen: ReleaseOne,
    local_contrast: ReleaseOne,
    device: wgpu::Device,
    lost: Arc<AtomicBool>,
}

impl Releaser {
    /// Let go of the ops' kept working sets, and wait for the device to
    /// finish what used them, so their memory is the driver's again
    /// when this returns: the bytes each held.
    pub fn release(&self) -> KeptBytes {
        let freed = KeptBytes {
            sharpen: (self.sharpen)(),
            local_contrast: (self.local_contrast)(),
        };
        if freed.sharpen + freed.local_contrast > 0
            && let Err(e) = wait_on(&self.device, &self.lost)
        {
            log::warn!("greycard-gpu: after letting the working sets go: {e}");
        }
        freed
    }
}

/// The picture sizes the ops' kept working sets were made for
/// ([`Context::kept_sizes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeptSizes {
    pub sharpen: Option<(u32, u32)>,
    pub local_contrast: Option<(u32, u32)>,
}

/// A developed picture on the GPU, as [`Context::upload`] put it.
pub struct Image {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
    /// The sharpen's automatic contrast threshold, which is a property
    /// of the picture alone: found once, kept for every re-run.
    threshold: OnceLock<f32>,
}

impl Image {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An export takes any adapter but a software one, and that only
    /// when asked for.
    #[test]
    fn a_software_adapter_is_taken_only_when_asked() {
        use wgpu::DeviceType;
        for kind in [
            DeviceType::DiscreteGpu,
            DeviceType::IntegratedGpu,
            DeviceType::VirtualGpu,
            DeviceType::Other,
        ] {
            assert!(takes(kind, false), "{kind:?}");
        }
        assert!(!takes(DeviceType::Cpu, false));
        assert!(takes(DeviceType::Cpu, true));
    }

    /// `SLINT_WGPU_CPU` is asked for by hand unless a greycard set it,
    /// which its marker says; `WGPU_ADAPTER_NAME` always is.
    #[test]
    fn asked_by_hand_reads_the_two_variables_and_the_marker() {
        let env = |set: &'static [&'static str]| {
            move |name: &str| set.contains(&name).then(|| std::ffi::OsString::from("1"))
        };
        assert!(!asked_by_hand(env(&[])));
        assert!(asked_by_hand(env(&[SLINT_WGPU_CPU])));
        assert!(!asked_by_hand(env(&[SLINT_WGPU_CPU, SLINT_WGPU_CPU_OURS])));
        assert!(asked_by_hand(env(&[WGPU_ADAPTER_NAME])));
    }
}
