//! ONNX Runtime, and which device runs a model: the providers are tried
//! in order and a model settles on the first one that loads and runs,
//! since a provider can accept a graph and then fail on it (WebGPU on
//! the onnx-community BiRefNet: a Split with more outputs than the
//! shader may bind; greycard's rewrite of that file, `SUBJECT_WEBGPU`,
//! runs whole on WebGPU).
//!
//! Two costs that failure used to pay every launch, fixed together
//! (notes §97): the last provider in the list is skipped past a
//! warm-up when it is CPU, which cannot fail the way WebGPU does — it
//! either builds a session or it does not (`&[Provider::WebGpu]`
//! alone still gets warmed up: it is last, but it is not CPU); and a
//! provider that failed to load or run a model is remembered, keyed
//! by the model file and the adapter that failed it, so the next
//! launch does not pay to find that out again (`record`). A
//! remembered *success* is not used to skip a warm-up: the win §97
//! measured is in not re-discovering a failure, and skipping the
//! warm-up on a remembered success would hand back a session that has
//! never actually run the graph this launch, with no provider left to
//! fall through to and nothing written back if it turns out wrong.
//!
//! ONNX Runtime itself is a shared library loaded the first time a
//! model is opened, never linked in (`dylib` has why: the build greycard
//! ships needs x86-64-v3, and linked in it killed an older processor
//! at exec). [`status`] says, without loading anything, whether it can
//! be had here; where it cannot, the AI features are off, every
//! [`open`] answers [`Error::Unavailable`] with the one line that says
//! why, before any provider is tried. The provider-outcome record is
//! not written then: it is about what a provider did with a model
//! file, and none was tried; the line is the log's and the caller's.
//!
//! Where the processor is why, Linux and Windows on x86-64 have a way
//! back: Microsoft's own CPU-only release ([`fetchable`] names it),
//! fetched into the model store as a model is. On Windows on ARM,
//! where greycard ships no runtime at all, that release is the only
//! way the models run. Once it is there, [`refresh`] decides again and
//! it is the library loaded, with the CPU the only provider
//! ([`cpu_only`]).

use std::path::Path;
use std::sync::{OnceLock, RwLock};

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

mod dylib;
mod record;

pub use dylib::{FETCH_RUNTIME, LIBRARY, SHIPS, Unavailable};

/// An execution provider, in the order they are tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    /// NVIDIA through CUDA and cuDNN; only in a build with the `cuda`
    /// feature, and only when the libraries are on the system.
    Cuda,
    /// Apple's Core ML on macOS, which puts each op on the Neural
    /// Engine, the GPU, or the CPU; only in a build with the `coreml`
    /// feature, and only on macOS, where greycard's library carries it.
    CoreMl,
    /// Any Vulkan, Metal or DX12 device through Dawn.
    WebGpu,
    Cpu,
}

impl Provider {
    /// Every provider this build can offer, fastest first.
    pub fn all() -> Vec<Provider> {
        [
            #[cfg(feature = "cuda")]
            Provider::Cuda,
            // Before WebGPU, but not on trust: `open` times the two
            // and keeps the faster.
            #[cfg(all(feature = "coreml", target_os = "macos"))]
            Provider::CoreMl,
            #[cfg(feature = "webgpu")]
            Provider::WebGpu,
            Provider::Cpu,
        ]
        .to_vec()
    }

    /// Those that can be tried here: every one this build offers, or
    /// none at all where ONNX Runtime cannot be had ([`status`]).
    /// Asked without loading the runtime, so the first ask (the window
    /// asks on its render path) costs nothing: a provider this build
    /// was compiled with is one the library it ships carries, and one
    /// a library named by hand lacks fails its session build in
    /// [`open`] and the next is tried, as a provider that cannot run a
    /// graph always has been.
    ///
    /// Microsoft's CPU-only runtime, fetched where the processor cannot
    /// run greycard's own or there is none, offers the CPU alone: it
    /// carries no other provider, and trying one would only fail a
    /// session build each time a model is opened.
    pub fn available() -> Vec<Provider> {
        if status().is_err() {
            return Vec::new();
        }
        if cpu_only() {
            return vec![Provider::Cpu];
        }
        Self::all()
    }

    /// `given` less Core ML, for a model that stays off it: Core ML is
    /// for the denoiser and Sky, whose shapes are fixed; the others
    /// (LaMa's free size, SAM's prompts) would pay a second session and
    /// two more warm runs at every load to lose the race, or fall to
    /// the CPU under static shapes, and Subject fails to compile
    /// there at all (§441).
    pub fn without_core_ml(given: &[Provider]) -> Vec<Provider> {
        given
            .iter()
            .copied()
            .filter(|p| *p != Provider::CoreMl)
            .collect()
    }

    pub fn is_available(self) -> bool {
        Self::available().contains(&self)
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Cuda => "CUDA",
            Provider::CoreMl => "CoreML",
            Provider::WebGpu => "WebGPU",
            Provider::Cpu => "CPU",
        }
    }

    /// The provider as the window and the command line name it to the
    /// user: its name, and for the CPU under the fetched CPU-only
    /// runtime, that it is the slower way, since on such a machine
    /// there is no other. Not for the provider-outcome record, which
    /// keys by [`Provider::name`].
    pub fn label(self) -> &'static str {
        if self == Provider::Cpu && cpu_only() {
            "CPU (slower)"
        } else {
            self.name()
        }
    }

    /// The provider's registration for a session of `path` whose free
    /// dimensions `fixed` sets.
    #[cfg_attr(
        not(all(feature = "coreml", target_os = "macos")),
        allow(unused_variables)
    )]
    fn dispatch(
        self,
        path: &Path,
        fixed: &[(&str, i64)],
    ) -> Option<ort::ep::ExecutionProviderDispatch> {
        match self {
            #[cfg(feature = "cuda")]
            Provider::Cuda => Some(ort::ep::CUDA::default().build().error_on_failure()),
            #[cfg(all(feature = "coreml", target_os = "macos"))]
            Provider::CoreMl => Some(coreml(coreml_cache(path, fixed).as_deref(), false)),
            #[cfg(feature = "webgpu")]
            // The storage buffer cache is ONNX Runtime's default for
            // every session, one mode for the process: it lives on the
            // WebGPU context, made once and shared, so the first
            // session's choice would be everyone's. Lazy release (the
            // `ep.webgpuexecutionprovider.storageBufferCacheMode`
            // config entry; ort rc.13's builder option never reaches
            // ONNX Runtime) gives a dropped session's buffers back, but
            // on an RTX 5070 Ti it slowed the other models' runs: the
            // fast denoiser on a 2048 mosaic by 22 percent, the sky by
            // 20, SAM's encoder by 43 and its decoder by 40, Subject by
            // 9, and LaMa at 1024 by 5. What it saves is little: in the
            // editor, a 1024 remove after the denoiser peaked 9.4 GB
            // over the desktop under the default and 8.3 GB under lazy
            // release, the remove's own share 7.3 GB either way.
            Provider::WebGpu => Some(ort::ep::WebGPU::default().build().error_on_failure()),
            Provider::Cpu => None,
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// The adapter this provider would run on, named well enough to
    /// tell it apart from another one, or `None` when there is
    /// nothing to remember it by: CPU is not a piece of hardware that
    /// changes under us the way a GPU and its driver do, and neither
    /// `ort` nor ONNX Runtime's own execution providers say which
    /// adapter they picked, so this asks the graphics stack directly
    /// and only for that name, not to run anything on it.
    fn adapter_identity(self) -> Option<String> {
        match self {
            #[cfg(feature = "cuda")]
            Provider::Cuda => cuda_adapter_identity(),
            #[cfg(not(feature = "cuda"))]
            Provider::Cuda => None,
            #[cfg(all(feature = "coreml", target_os = "macos"))]
            Provider::CoreMl => coreml_machine_identity(),
            #[cfg(not(all(feature = "coreml", target_os = "macos")))]
            Provider::CoreMl => None,
            #[cfg(feature = "webgpu")]
            Provider::WebGpu => webgpu_adapter_identity(),
            #[cfg(not(feature = "webgpu"))]
            Provider::WebGpu => None,
            Provider::Cpu => None,
        }
    }
}

/// Core ML as greycard asks for it: the ML Program format (the older
/// NeuralNetwork format takes fewer ops and no fp16 program), every
/// compute unit (Core ML picks the Neural Engine, the GPU, or the CPU
/// per op), inputs of static shape only (the Neural Engine wants one;
/// a model with free dimensions takes them fixed at load, `open_fixed`),
/// and the compiled model kept in `cache`, since Core ML compiles the
/// graph on a session's first load and that takes seconds.
/// `profile` logs where each op went and its estimated cost (Core ML's
/// compute plan, macOS 14.4 and up), to stderr through NSLog; for the
/// probe, not the editor. Built wherever the feature is on, so a Linux
/// build checks it, but registered only on macOS: no other library
/// greycard loads carries the provider.
#[cfg(feature = "coreml")]
pub fn coreml(cache: Option<&Path>, profile: bool) -> ort::ep::ExecutionProviderDispatch {
    use ort::ep::coreml::{ComputeUnits, ModelFormat};
    let mut ep = ort::ep::CoreML::default()
        .with_model_format(ModelFormat::MLProgram)
        .with_compute_units(ComputeUnits::All)
        .with_static_input_shapes(true)
        .with_profile_compute_plan(profile);
    if let Some(dir) = cache {
        ep = ep.with_model_cache_dir(dir.display());
    }
    ep.build().error_on_failure()
}

/// Where Core ML keeps its compile of the model at `path` with the
/// free dimensions `fixed`: `greycard/coreml/<key>` in the user's
/// cache, made if need be. ONNX Runtime recompiles a cached model only
/// when the graph's structure or metadata changes, not its weights, so
/// each file gets a folder of its own, keyed by its path, size, and
/// modification time and the dimensions fixed. `None`, and no cache,
/// when there is nowhere to put it.
#[cfg(all(feature = "coreml", target_os = "macos"))]
fn coreml_cache(path: &Path, fixed: &[(&str, i64)]) -> Option<std::path::PathBuf> {
    let key = coreml_cache_key(path, fixed);
    let dir = crate::store::cache_home()?
        .join("greycard")
        .join("coreml")
        .join(key);
    match std::fs::create_dir_all(&dir) {
        Ok(()) => Some(dir),
        Err(e) => {
            log::warn!(
                "no Core ML cache at {}: {e}; it compiles at every load",
                dir.display()
            );
            None
        }
    }
}

/// The folder name for [`coreml_cache`]: the file's stem and the first
/// sixteen hex digits of a hash of what names that build of it.
#[cfg_attr(not(all(feature = "coreml", target_os = "macos")), allow(dead_code))]
fn coreml_cache_key(path: &Path, fixed: &[(&str, i64)]) -> String {
    use sha2::{Digest, Sha256};
    let meta = std::fs::metadata(path).ok();
    let modified = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut h = Sha256::new();
    h.update(path.to_string_lossy().as_bytes());
    h.update(meta.map(|m| m.len()).unwrap_or_default().to_le_bytes());
    h.update(modified.to_le_bytes());
    for (name, size) in fixed {
        h.update(name.as_bytes());
        h.update(size.to_le_bytes());
    }
    let digest = h.finalize();
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{stem}-{hex}")
}

/// What Core ML runs on, for the provider-outcome record: the chip and
/// the macOS version, since a system update brings a new Core ML. Asked
/// once a process.
#[cfg(all(feature = "coreml", target_os = "macos"))]
fn coreml_machine_identity() -> Option<String> {
    static IDENTITY: OnceLock<Option<String>> = OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            let ask = |program: &str, args: &[&str]| {
                let out = std::process::Command::new(program)
                    .args(args)
                    .output()
                    .ok()?;
                let text = String::from_utf8(out.stdout).ok()?;
                let line = text.trim();
                (out.status.success() && !line.is_empty()).then(|| line.to_string())
            };
            let chip = ask("sysctl", &["-n", "machdep.cpu.brand_string"])?;
            let os = ask("sw_vers", &["-productVersion"])?;
            Some(format!("{chip}|macOS {os}"))
        })
        .clone()
}

/// wgpu's own idea of the adapter it would hand a device: name,
/// backend and driver, the same fields `greycard-ui`'s viewport logs
/// off its Slint device (`main.rs`). This is a second, short-lived
/// `wgpu::Instance` used only to read that struct — Dawn, which ONNX
/// Runtime's WebGPU execution provider actually runs on, keeps its
/// own adapter choice to itself. Probed with `Backends::PRIMARY`
/// (Vulkan, Metal, DX12), or those of them `WGPU_BACKEND` names:
/// closer to what Dawn itself offers than every backend wgpu knows,
/// and it skips GL/EGL, which a process that already holds Slint's
/// own wgpu device has no business opening a second context on. Asked
/// once a process and kept: the adapter does not change under a
/// running greycard, and asking again would pay this same cost once a
/// model file, not once a launch. It asks for what `WGPU_POWER_PREF`
/// names, high performance where it names nothing, as the window
/// does; greycard-gpu always asks high performance.
#[cfg(feature = "webgpu")]
fn webgpu_adapter_identity() -> Option<String> {
    webgpu_adapter().map(|a| a.identity.clone())
}

/// Whether the WebGPU adapter shares the machine's memory: an
/// integrated GPU as wgpu reports it, or any Apple silicon Mac, whose
/// GPU is on the same memory however the backend names it; `false`
/// for a discrete or virtual card. `None` when no adapter answers or
/// the one that does is a CPU or of no known type. On a machine with
/// two GPUs this follows wgpu's choice of adapter, which may not be
/// ONNX Runtime's, the same guess the adapter's identity makes.
pub fn webgpu_unified() -> Option<bool> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Some(true);
    }
    #[cfg(feature = "webgpu")]
    {
        match webgpu_adapter()?.device_type {
            wgpu::DeviceType::IntegratedGpu => Some(true),
            wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::VirtualGpu => Some(false),
            wgpu::DeviceType::Cpu | wgpu::DeviceType::Other => None,
        }
    }
    #[cfg(not(feature = "webgpu"))]
    {
        None
    }
}

/// The bytes of the WebGPU adapter's own memory: the sum of its
/// device-local heaps on Vulkan, its dedicated video memory on DX12.
/// `None` when no adapter answers, on Metal (a Mac's GPU has the
/// machine's memory, which the caller reads), and on any backend that
/// will not say. On an integrated GPU it is the share the driver sets
/// aside, not what the GPU can reach; [`webgpu_unified`] says when to
/// read the machine's memory instead. Same adapter, and same guess,
/// as [`webgpu_unified`].
pub fn webgpu_memory() -> Option<u64> {
    #[cfg(feature = "webgpu")]
    {
        webgpu_adapter()?.memory
    }
    #[cfg(not(feature = "webgpu"))]
    {
        None
    }
}

/// What the WebGPU probe learned of the adapter.
#[cfg(feature = "webgpu")]
struct Adapter {
    /// Name, backend, and driver.
    identity: String,
    device_type: wgpu::DeviceType,
    /// The bytes of its own memory ([`webgpu_memory`]).
    memory: Option<u64>,
}

/// The bytes of `adapter`'s own memory, asked of the backend under
/// wgpu, which keeps it to itself.
#[cfg(feature = "webgpu")]
fn adapter_memory(adapter: &wgpu::Adapter) -> Option<u64> {
    // Where wgpu builds its Vulkan backend: not on a Mac, where Metal
    // is the backend and the memory the machine's.
    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
    {
        // VK_MEMORY_HEAP_DEVICE_LOCAL_BIT.
        const DEVICE_LOCAL: u32 = 0x1;
        // SAFETY: the guard is only read, the adapter outlives it, and
        // nothing is created or destroyed through it.
        if let Some(vk) = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() } {
            // SAFETY: the physical device is the live one the adapter
            // wraps, on the instance that enumerated it.
            let props = unsafe {
                vk.shared_instance()
                    .raw_instance()
                    .get_physical_device_memory_properties(vk.raw_physical_device())
            };
            let count = (props.memory_heap_count as usize).min(props.memory_heaps.len());
            let bytes: u64 = props.memory_heaps[..count]
                .iter()
                .filter(|h| h.flags.as_raw() & DEVICE_LOCAL != 0)
                .map(|h| h.size)
                .sum();
            return (bytes > 0).then_some(bytes);
        }
    }
    #[cfg(windows)]
    {
        // SAFETY: as above; the DXGI adapter is only described.
        if let Some(dx) = unsafe { adapter.as_hal::<wgpu::hal::api::Dx12>() } {
            // SAFETY: `GetDesc1` reads the adapter's description and
            // has no preconditions on a live adapter.
            let desc = unsafe { dx.raw_adapter().GetDesc1() }.ok()?;
            let bytes = desc.DedicatedVideoMemory as u64;
            return (bytes > 0).then_some(bytes);
        }
    }
    let _ = adapter;
    None
}

/// The adapter's identity, type, and memory, asked once a process and
/// kept.
#[cfg(feature = "webgpu")]
fn webgpu_adapter() -> Option<&'static Adapter> {
    static ADAPTER: OnceLock<Option<Adapter>> = OnceLock::new();
    ADAPTER
        .get_or_init(|| {
            use pollster::FutureExt as _;
            // What `WGPU_BACKEND` names, as the window's own instance
            // takes it: a start that died loading one backend's driver
            // sets it to go round that one, and so must this. Naming
            // none of these, it is not widened: the driver it keeps
            // out might be one of them.
            let asked = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
            let backends = asked & wgpu::Backends::PRIMARY;
            if backends.is_empty() {
                log::info!(
                    "WGPU_BACKEND names none of Vulkan, Metal, or DX12: the WebGPU adapter is not asked for, nor the provider remembered"
                );
                return None;
            }
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::from_env()
                        .unwrap_or(wgpu::PowerPreference::HighPerformance),
                    ..Default::default()
                })
                .block_on()
                .ok()?;
            let info = adapter.get_info();
            Some(Adapter {
                identity: format!(
                    "{}|{:?}|{} {}",
                    info.name, info.backend, info.driver, info.driver_info
                ),
                device_type: info.device_type,
                memory: adapter_memory(&adapter),
            })
        })
        .as_ref()
}

/// `nvidia-smi` is what is actually on the machines CUDA runs on
/// (Linux and Windows, with the NVIDIA driver that CUDA itself
/// needs); `ort`'s CUDA execution provider exposes no device name
/// either. `None` on any hiccup — a missing binary, an odd answer —
/// same as WebGPU with no adapter: the provider is just not
/// remembered this run. Asked once a process and kept, same reasoning
/// as `webgpu_adapter_identity`.
#[cfg(feature = "cuda")]
fn cuda_adapter_identity() -> Option<String> {
    static IDENTITY: OnceLock<Option<String>> = OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            let out = std::process::Command::new("nvidia-smi")
                .args(["--query-gpu=name,driver_version", "--format=csv,noheader"])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let text = String::from_utf8(out.stdout).ok()?;
            let line = text.lines().next()?.trim();
            (!line.is_empty()).then(|| line.to_string())
        })
        .clone()
}

/// What names the model file a session is opened from, for the
/// provider-outcome record: the registry's id for the model and the
/// specific file's published hash (already known, checked when the
/// file arrived — `store.rs`), plus where the record lives, beside
/// that model's store directory. `None` at the call site turns the
/// record off for that load, which is what a caller without a
/// registered model (an arbitrary path from the CLI, say) passes.
#[derive(Debug, Clone, Copy)]
pub struct Remembered<'a> {
    pub store_root: &'a Path,
    pub model: &'a str,
    pub hash: &'a str,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Ort(#[from] ort::Error),
    #[error("no provider could run {model}: {tried}")]
    NoProvider { model: String, tried: String },
    #[error("{0} is not in the model store")]
    Missing(&'static str),
    #[error("the model answered with the wrong shape: {0}")]
    Shape(String),
    #[error(
        "the model's answer cannot be right ({0}); the device may be out of memory, \
         which the WebGPU provider does not report: try a smaller tile"
    )]
    Implausible(String),
    /// ONNX Runtime cannot be had on this machine: the processor, or
    /// no library, or one that would not load.
    #[error("{0}")]
    Unavailable(Unavailable),
    /// The model is not offered on these providers: one too slow on
    /// the CPU to be worth running there, say.
    #[error("{0}")]
    Unsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Whether ONNX Runtime can be had here, and from where: the
/// processor asked and the library looked for, once a process, and
/// nothing of it loaded (`dylib::probe`). Once a load has been tried,
/// its outcome: a library that is there and would not load is as off
/// as one that is not there.
pub fn status() -> std::result::Result<&'static Path, &'static Unavailable> {
    if let Some(Err(why)) = LOADED.get() {
        return Err(why);
    }
    probed().as_ref().map(|found| found.path.as_path())
}

/// Whether the AI features run on Microsoft's CPU-only runtime, fetched
/// because the processor cannot run greycard's own or greycard ships
/// none for the platform: the CPU is then the only provider, and the
/// slower one.
pub fn cpu_only() -> bool {
    !matches!(LOADED.get(), Some(Err(_)))
        && matches!(probed(), Ok(found) if found.from == dylib::Source::Fetched)
}

/// The CPU-only runtime that would turn the AI features on here, where
/// that is why they are off: the processor cannot run greycard's own
/// library, or greycard ships none for this platform, and the registry
/// has Microsoft's for it.
pub fn fetchable() -> Option<&'static crate::Runtime> {
    status().err().and_then(Unavailable::fetchable)
}

type Decision = std::result::Result<dylib::Found, Unavailable>;

/// The decision, made once a process and again by [`refresh`]: each
/// is kept for the rest of the process (a few bytes, made at most once
/// for each fetch), so `status` can hand out a reference to it.
static DECIDED: RwLock<Option<&'static Decision>> = RwLock::new(None);

fn probed() -> &'static Decision {
    if let Some(decided) = *DECIDED.read().unwrap_or_else(|e| e.into_inner()) {
        return decided;
    }
    let mut decided = DECIDED.write().unwrap_or_else(|e| e.into_inner());
    decided.get_or_insert_with(|| Box::leak(Box::new(dylib::probe())))
}

/// Decide again, for a CPU-only runtime just fetched into the store to
/// be the one loaded; the new status. Only before a library has been
/// loaded: once one is in the process, it is the one for the rest of
/// it.
pub fn refresh() -> std::result::Result<&'static Path, &'static Unavailable> {
    if LOADED.get().is_none() {
        let fresh: &'static Decision = Box::leak(Box::new(dylib::probe()));
        *DECIDED.write().unwrap_or_else(|e| e.into_inner()) = Some(fresh);
    }
    status()
}

/// After a CPU-only runtime has been fetched into `store`: decide
/// again and, where it is now the library to use, load it at once
/// rather than at the first model, so a library that will not load on
/// this machine is said where its download was asked for.
/// `Ok(true)` when it is loaded and the AI features run on it;
/// `Ok(false)` when another library comes first (one named by hand,
/// or greycard's own on a processor that can run it), which is left
/// for the first model to load.
///
/// One that arrived whole and will not load here (a glibc older than
/// it needs, say) is removed from the store, so the next launch
/// decides as before it was fetched and offers the download again,
/// rather than taking it on its hash and offering models that cannot
/// run. This process keeps the failure: the AI features are off with
/// the loader's reason.
pub fn load_fetched(
    store: &crate::Store,
    runtime: &crate::Runtime,
) -> std::result::Result<bool, &'static Unavailable> {
    refresh()?;
    if !cpu_only() {
        return Ok(false);
    }
    init().map(|()| true).inspect_err(|why| {
        log::warn!("{why}; removing it from the store");
        if let Err(e) = store.remove_runtime(runtime) {
            log::warn!(
                "{} could not be removed: {e}",
                store.runtime_dir(runtime).display()
            );
        }
    })
}

/// The outcome of loading the library, once a process.
static LOADED: OnceLock<std::result::Result<(), Unavailable>> = OnceLock::new();

/// Whether ONNX Runtime was loaded in this process: a model was opened.
pub fn loaded() -> bool {
    matches!(LOADED.get(), Some(Ok(())))
}

/// End the process with `code`, its work done and said, without the
/// exit handlers when ONNX Runtime was loaded; return `code` for
/// `main` to hand back otherwise.
///
/// With the runtime loaded, `ort` releases its environment from an
/// exit handler (a `.fini_array` entry on Linux, `__cxa_atexit` on a
/// Mac), and the release tears down the WebGPU provider's Dawn
/// instance and with it Dawn's Vulkan instance, whose destructor calls
/// through libvulkan into a driver the loader may already have let go
/// of by then: one run in about thirty of `greycard develop
/// --ai-denoise` died with SIGSEGV, exit 139, after its file was
/// written. `ort` 2.0.0-rc.13 offers no way to release the environment
/// earlier (it is a private static, and a release of our own through
/// the C API would be released again by that handler), so the handler
/// is not run: `_exit` ends the process at once, the kernel takes back
/// the driver's state, and no exit handler or destructor runs on this
/// thread or another. Stdout, stderr and the C library's streams are
/// flushed first. A caller has every file it wrote closed by then.
///
/// On Windows the process returns as it otherwise would; Dawn runs on
/// D3D12 there and the crash has not been seen.
pub fn leave(code: u8) -> std::process::ExitCode {
    use std::io::Write;
    if loaded() {
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        #[cfg(unix)]
        // SAFETY: `fflush(NULL)` flushes every C stdio stream, so a C or
        // C++ library's buffered output is not lost with the exit
        // handlers that would have flushed it; `_exit` then ends the
        // process at once and returns nothing to undo.
        unsafe {
            libc::fflush(std::ptr::null_mut());
            libc::_exit(i32::from(code))
        }
    }
    std::process::ExitCode::from(code)
}

/// For a test that needs ONNX Runtime and finds `why` it cannot have
/// it: a failure where `GREYCARD_REQUIRE_ORT` is set (CI, where
/// `scripts/ort-dylib.sh` has run, so a library the tests cannot find
/// is a broken build and not a machine without it), and a word on
/// stderr otherwise, for the test to return on. libtest keeps that
/// word to itself on a pass, which is why the variable exists.
#[doc(hidden)]
pub fn absent_in_test(why: &Unavailable) {
    if std::env::var_os("GREYCARD_REQUIRE_ORT").is_some_and(|v| !v.is_empty()) {
        panic!("GREYCARD_REQUIRE_ORT is set and ONNX Runtime cannot be had: {why}");
    }
    eprintln!("skipped, no ONNX Runtime (run scripts/ort-dylib.sh): {why}");
}

/// ONNX Runtime loaded, the first time a model is asked for, and the
/// environment's options set; or why it cannot be. Nothing of the
/// library is in the process before this returns `Ok` the first time.
fn init() -> std::result::Result<(), &'static Unavailable> {
    // Nothing to load is not an outcome to keep: a CPU-only runtime
    // fetched later is decided again (`refresh`) and loaded then.
    let found = probed().as_ref()?;
    LOADED
        .get_or_init(|| {
            let path = &found.path;
            let started = std::time::Instant::now();
            match ort::init_from(path) {
                Ok(builder) => {
                    builder.with_name("greycard").commit();
                    log::info!(
                        "ONNX Runtime loaded from {} in {:.0} ms",
                        path.display(),
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                    Ok(())
                }
                Err(e) => {
                    // `ort`'s error says the path (ours already does)
                    // and "dlopen failed", and keeps the loader's own
                    // reason inside the libloading error it holds
                    // rather than as its source: that reason is what
                    // is worth saying.
                    let why = match &e {
                        ort::LoadDynamicError::Dlopen { error, .. } => dylib::chain(error),
                        other => dylib::chain(other),
                    };
                    let why = Unavailable::WouldNotLoad {
                        path: path.clone(),
                        why,
                    };
                    log::warn!("{why}");
                    Err(why)
                }
            }
        })
        .as_ref()
        .map(|_| ())
}

/// A session and the provider it settled on.
pub struct Loaded {
    pub session: Session,
    pub provider: Provider,
}

/// Load `path` on the first of `providers` that loads it and passes
/// `warm`, which should run the model once on representative input —
/// except the last provider in the list when it is CPU, which is not
/// warmed up (it cannot accept a graph and then fail on the first
/// real run the way WebGPU can, so a warm-up proves nothing a session
/// build did not already prove; `&[Provider::WebGpu]` alone still
/// gets warmed up, since it is last but not CPU), and except a
/// provider `remember` already knows *failed* on this exact model
/// file and adapter, which is not even tried. A remembered success is
/// still warmed up fresh: skipping that would hand back a session
/// that has not actually run the graph this launch, with nothing to
/// fall through to and nothing written back if that turns out wrong.
///
/// Core ML is the one exception to "first that passes": a graph Core
/// ML partly declines runs the rest on the CPU, with a copy at each
/// cut, and it says so only in its compute plan. So where a GPU
/// provider follows it in the list, both are opened, `warm` is run
/// once more on each and timed, and the faster is kept; the other
/// session is dropped. The CPU after them is not tried.
pub fn open(
    path: &Path,
    level: GraphOptimizationLevel,
    providers: &[Provider],
    remember: Option<Remembered<'_>>,
    warm: impl FnMut(&mut Session) -> ort::Result<()>,
) -> Result<Loaded> {
    open_fixed(path, level, providers, remember, &[], warm)
}

/// [`open`], with the model's free dimensions named in `fixed` set to
/// their sizes on Core ML, which compiles a static graph for the
/// Neural Engine only when every shape is known at load. The other
/// providers keep the graph's free dimensions. `warm` should then run
/// the fixed shape, on every provider, so the timing in `open` compares
/// like with like.
pub fn open_fixed(
    path: &Path,
    level: GraphOptimizationLevel,
    providers: &[Provider],
    remember: Option<Remembered<'_>>,
    fixed: &[(&str, i64)],
    mut warm: impl FnMut(&mut Session) -> ort::Result<()>,
) -> Result<Loaded> {
    if let Err(why) = init() {
        return Err(Error::Unavailable(why.clone()));
    }
    let mut tried = Vec::new();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let build = record::build_fingerprint(providers);
    // Read once, lazily: only a provider that can be remembered
    // (`remember` is set and the adapter could be named) touches
    // disk, and only once for the whole call.
    let mut entries: Option<Vec<record::Entry>> = None;
    // Core ML's session and its timed run, held while the provider
    // after it is timed against it.
    let mut held: Option<(Loaded, f64)> = None;

    for (i, &provider) in providers.iter().enumerate() {
        if held.is_some() && provider == Provider::Cpu {
            break;
        }
        let last = i + 1 == providers.len();
        let racing = held.is_none() && races(provider, &providers[i + 1..]);
        let timed = racing || held.is_some();
        // Started before the adapter probe (cached after its first
        // call a process, but the first call still costs something),
        // so the logged duration is honest about the whole cost of
        // settling on this provider, not just the session build.
        let t = std::time::Instant::now();
        let adapter = remember.and_then(|_| provider.adapter_identity());
        let remembered = if let (Some(r), Some(adapter)) = (remember, &adapter) {
            let entries = entries.get_or_insert_with(|| record::read(r.store_root));
            plan(entries, r.hash, provider, adapter, &build)
        } else {
            None
        };

        if let Some(record::Outcome::Failed { reason }) = &remembered {
            log::info!(
                "{name} on {}: remembered failing ({}): {reason}",
                provider.name(),
                remember
                    .map(|r| record::path(r.store_root).display().to_string())
                    .unwrap_or_default()
            );
            tried.push(format!("{}: {reason} (remembered)", provider.name()));
            continue;
        }
        let skip_warm = skip_warm(last, provider);

        let outcome = try_open(path, level, provider, fixed, skip_warm, timed, &mut warm);
        if let (Some(r), Some(adapter)) = (remember, &adapter) {
            let entries = entries.get_or_insert_with(|| record::read(r.store_root));
            let recorded = match &outcome {
                Ok(_) => record::Outcome::Worked,
                Err(e) => record::Outcome::Failed {
                    reason: first_line(&e.to_string()).to_string(),
                },
            };
            let entry = record::Entry {
                model: r.model.to_string(),
                hash: r.hash.to_string(),
                provider: provider.name().to_string(),
                adapter: adapter.clone(),
                build: build.clone(),
                outcome: recorded,
            };
            // Nothing changed since the entry already on disk: not
            // worth a write, and not worth racing another process
            // over one.
            if record::find(entries, r.hash, provider) != Some(&entry) {
                record::upsert(entries, entry);
                record::write(r.store_root, entries);
            }
        }
        match outcome {
            Ok((session, run)) => {
                log::info!(
                    "{name} on {} in {:.2}s",
                    provider.name(),
                    t.elapsed().as_secs_f64()
                );
                let loaded = Loaded { session, provider };
                match (held.take(), run) {
                    (None, Some(run)) if racing => {
                        log::info!(
                            "{name} on {}: a warm run in {run:.3}s, timed against the next",
                            provider.name()
                        );
                        held = Some((loaded, run));
                    }
                    (Some((first, first_run)), Some(run)) => {
                        let first_name = first.provider.name();
                        let keep = if run < first_run { loaded } else { first };
                        log::info!(
                            "{name}: a warm run in {first_run:.3}s on {first_name} and {run:.3}s on {}; keeping {}",
                            provider.name(),
                            keep.provider.name()
                        );
                        return Ok(keep);
                    }
                    (Some((first, _)), None) => return Ok(first),
                    (None, _) => return Ok(loaded),
                }
            }
            Err(e) => {
                let text = e.to_string();
                let why = first_line(&text);
                log::warn!("{name} on {}: {why}", provider.name());
                tried.push(format!("{}: {why}", provider.name()));
            }
        }
    }
    // What Core ML was timed against failed, or was the last: it stands.
    if let Some((loaded, _)) = held {
        return Ok(loaded);
    }
    // Each provider warned as it failed; the caller decides what the
    // whole means.
    Err(Error::NoProvider {
        model: path.display().to_string(),
        tried: tried.join("; "),
    })
}

/// The remembered outcome to trust for `provider` running the file
/// named by `hash`, given the entries already on disk and this
/// launch's adapter and build — or `None` when there is nothing to
/// trust (no entry, or one that no longer applies), in which case the
/// provider is tried fresh. Takes the adapter as a plain string,
/// rather than asking the hardware for it itself, so this — the whole
/// decision `open` makes from the record — can be driven by a test
/// without touching a real GPU.
fn plan(
    entries: &[record::Entry],
    hash: &str,
    provider: Provider,
    adapter: &str,
    build: &str,
) -> Option<record::Outcome> {
    record::find(entries, hash, provider)
        .filter(|e| record::valid(e, adapter, build))
        .map(|e| e.outcome.clone())
}

/// Whether `provider` is on record as having failed the file named by
/// `hash`, on the adapter and build this launch would use — for a
/// caller that wants the answer before ever opening a session, to
/// decide what to offer rather than what to load
/// (`subject::original_failed_on_webgpu`). `false` with nothing
/// trusted on record, or where the adapter cannot even be named (no
/// `webgpu` feature, or the probe failed).
pub fn remembered_failure(
    store_root: &Path,
    hash: &str,
    provider: Provider,
    providers: &[Provider],
) -> bool {
    // Nothing runs here at all; and the build's fingerprint is read
    // off the loaded runtime, so this is the first use where it can be.
    if init().is_err() {
        return false;
    }
    let Some(adapter) = provider.adapter_identity() else {
        return false;
    };
    let entries = record::read(store_root);
    let build = record::build_fingerprint(providers);
    matches!(
        plan(&entries, hash, provider, &adapter, &build),
        Some(record::Outcome::Failed { .. })
    )
}

/// Whether to skip the warm-up run: only for CPU, and only when it is
/// the last provider tried (it always is, today, but a caller can
/// pass any slice: `&[Provider::WebGpu]` alone is CPU-free, so
/// `last` is true but this still warms up). CPU cannot accept a graph
/// and then fail on the first real run the way WebGPU can, so a
/// warm-up run proves nothing a session build did not already prove.
/// A remembered success does not skip it: see `open`'s doc comment
/// for why.
fn skip_warm(last: bool, provider: Provider) -> bool {
    last && provider == Provider::Cpu
}

/// Whether `provider` is timed against the next GPU provider in
/// `rest` rather than kept on its warm run alone: Core ML only, which
/// can pass and still run most of a graph on the CPU.
fn races(provider: Provider, rest: &[Provider]) -> bool {
    provider == Provider::CoreMl && rest.iter().any(|p| *p != Provider::Cpu)
}

/// A session on `provider`, warmed up unless `skip_warm`, and with
/// `timed` the seconds of a second warm run: the first pays for
/// compiling shaders or a Core ML specialization, which a tile does
/// not.
fn try_open(
    path: &Path,
    level: GraphOptimizationLevel,
    provider: Provider,
    fixed: &[(&str, i64)],
    skip_warm: bool,
    timed: bool,
    warm: &mut impl FnMut(&mut Session) -> ort::Result<()>,
) -> ort::Result<(Session, Option<f64>)> {
    let mut builder = Session::builder()?.with_optimization_level(level)?;
    if provider == Provider::CoreMl {
        for &(name, size) in fixed {
            builder = builder.with_dimension_override(name, size)?;
        }
    }
    if let Some(ep) = provider.dispatch(path, fixed) {
        builder = builder.with_execution_providers([ep])?;
    }
    let mut session = builder.commit_from_file(path)?;
    let mut run = None;
    if !skip_warm {
        warm(&mut session)?;
        if timed {
            let t = std::time::Instant::now();
            warm(&mut session)?;
            run = Some(t.elapsed().as_secs_f64());
        }
    }
    Ok((session, run))
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_is_always_offered_last() {
        let all = Provider::all();
        assert_eq!(all.last(), Some(&Provider::Cpu));
        // Where the runtime can be had, CPU is among what is offered;
        // where it cannot, nothing is.
        match status() {
            Ok(_) => assert!(Provider::available().contains(&Provider::Cpu)),
            Err(why) => {
                absent_in_test(why);
                assert!(Provider::available().is_empty());
            }
        }
    }

    /// With the library (scripts/ort-dylib.sh has run), a file that is
    /// not there fails on each provider tried; without it, nothing is
    /// tried, and the answer is why.
    #[test]
    fn a_missing_file_fails_on_every_provider() {
        let e = open(
            Path::new("/nonexistent/model.onnx"),
            GraphOptimizationLevel::Level1,
            &[Provider::Cpu],
            None,
            |_| Ok(()),
        )
        .err()
        .expect("no such file");
        match status() {
            Ok(_) => {
                assert!(matches!(e, Error::NoProvider { .. }));
                assert!(e.to_string().contains("CPU"));
            }
            Err(why) => {
                absent_in_test(why);
                assert!(matches!(&e, Error::Unavailable(u) if u == why));
                assert!(e.to_string().starts_with("the AI features are off: "));
            }
        }
    }

    #[test]
    fn the_last_provider_skips_the_warm_up_only_when_it_is_cpu() {
        assert!(skip_warm(true, Provider::Cpu));
        assert!(!skip_warm(true, Provider::WebGpu));
        assert!(!skip_warm(true, Provider::Cuda));
    }

    #[test]
    fn cpu_still_warms_up_when_it_is_not_last() {
        assert!(!skip_warm(false, Provider::Cpu));
    }

    #[test]
    fn core_ml_is_timed_against_a_gpu_provider_after_it_and_nothing_else() {
        assert!(races(Provider::CoreMl, &[Provider::WebGpu, Provider::Cpu]));
        assert!(!races(Provider::CoreMl, &[Provider::Cpu]));
        assert!(!races(Provider::CoreMl, &[]));
        assert!(!races(Provider::WebGpu, &[Provider::Cpu]));
        assert!(!races(Provider::Cuda, &[Provider::WebGpu]));
    }

    #[test]
    fn a_model_kept_off_core_ml_keeps_the_rest_in_order() {
        let all = [Provider::CoreMl, Provider::WebGpu, Provider::Cpu];
        assert_eq!(
            Provider::without_core_ml(&all),
            vec![Provider::WebGpu, Provider::Cpu]
        );
        assert_eq!(
            Provider::without_core_ml(&[Provider::Cpu]),
            vec![Provider::Cpu]
        );
    }

    #[test]
    fn core_ml_comes_before_webgpu_where_it_is_built() {
        let all = Provider::all();
        let at = |p| all.iter().position(|q| *q == p);
        if cfg!(all(feature = "coreml", target_os = "macos")) {
            assert!(at(Provider::CoreMl) < at(Provider::WebGpu));
        } else {
            assert_eq!(at(Provider::CoreMl), None);
        }
        assert_eq!(Provider::CoreMl.name(), "CoreML");
    }

    /// A file's Core ML compile is kept apart from another file's, and
    /// from the same file's at other fixed sizes or after it changes.
    #[test]
    fn each_model_file_and_fixed_shape_has_a_core_ml_cache_of_its_own() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-runtime-coreml-key-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("denoise-fast.onnx");
        let b = dir.join("denoise-best.onnx");
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(&b, b"one").unwrap();
        let fixed = [("h", 608), ("w", 608)];
        let key = coreml_cache_key(&a, &fixed);
        assert!(key.starts_with("denoise-fast-"), "{key}");
        assert_eq!(key, coreml_cache_key(&a, &fixed));
        assert_ne!(key, coreml_cache_key(&b, &fixed));
        assert_ne!(key, coreml_cache_key(&a, &[("h", 304), ("w", 304)]));
        assert_ne!(key, coreml_cache_key(&a, &[]));
        std::fs::write(&a, b"other").unwrap();
        assert_ne!(key, coreml_cache_key(&a, &fixed));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_trusts_a_matching_entry_and_ignores_a_mismatched_one() {
        let entries = vec![record::test_entry(
            "birefnet-lite-2024-fp16",
            "abc123",
            Provider::WebGpu,
            "Test GPU|Vulkan|1.2.3",
            "WebGPU,CPU greycard 0.1.0",
            record::Outcome::Failed {
                reason: "Split node".to_string(),
            },
        )];
        // The exact file, provider, adapter and build: trusted.
        assert_eq!(
            plan(
                &entries,
                "abc123",
                Provider::WebGpu,
                "Test GPU|Vulkan|1.2.3",
                "WebGPU,CPU greycard 0.1.0"
            ),
            Some(record::Outcome::Failed {
                reason: "Split node".to_string()
            })
        );
        // A different adapter, build, provider or file: not trusted,
        // tried fresh.
        assert_eq!(
            plan(
                &entries,
                "abc123",
                Provider::WebGpu,
                "Other GPU|Metal|9.9.9",
                "WebGPU,CPU greycard 0.1.0"
            ),
            None
        );
        assert_eq!(
            plan(
                &entries,
                "abc123",
                Provider::WebGpu,
                "Test GPU|Vulkan|1.2.3",
                "CUDA,WebGPU,CPU greycard 0.1.0"
            ),
            None
        );
        assert_eq!(
            plan(
                &entries,
                "abc123",
                Provider::Cpu,
                "Test GPU|Vulkan|1.2.3",
                "WebGPU,CPU greycard 0.1.0"
            ),
            None
        );
        assert_eq!(
            plan(
                &entries,
                "def456",
                Provider::WebGpu,
                "Test GPU|Vulkan|1.2.3",
                "WebGPU,CPU greycard 0.1.0"
            ),
            None
        );
    }

    /// A provider that worked is not read as one that failed:
    /// `remembered_failure` (and `subject::pick`, which decides off
    /// it) only ever wants to know about a failure, and a `Worked`
    /// entry for the exact file, provider, adapter and build must not
    /// be mistaken for one.
    #[test]
    fn plan_does_not_read_a_worked_entry_as_a_failure() {
        let entries = vec![record::test_entry(
            "birefnet-lite-2024-fp16",
            "abc123",
            Provider::WebGpu,
            "Test GPU|Vulkan|1.2.3",
            "WebGPU,CPU greycard 0.1.0",
            record::Outcome::Worked,
        )];
        assert_eq!(
            plan(
                &entries,
                "abc123",
                Provider::WebGpu,
                "Test GPU|Vulkan|1.2.3",
                "WebGPU,CPU greycard 0.1.0"
            ),
            Some(record::Outcome::Worked)
        );
    }

    /// Drives the same wiring `open` does — a fake store root's file,
    /// read, then handed to `plan` with an adapter identity given
    /// directly rather than asked of real hardware — to check that a
    /// remembered failure is what a launch would find and trust,
    /// without a model or a GPU. `open` itself is not called here:
    /// `Provider::adapter_identity` has no seam to hand it a fake
    /// answer without either adding a test-only override to the
    /// `Provider` enum's real, hardware-backed probes or threading an
    /// adapter override through `open`'s public signature, and either
    /// would contort an API only this one test needs; `plan` is
    /// exactly the decision `open` hands it, so this drives the same
    /// logic with the same data.
    #[test]
    fn a_remembered_failure_on_a_fake_store_root_is_found_and_trusted() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-runtime-plan-fake-store-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        record::write(
            &dir,
            &[record::test_entry(
                "birefnet-lite-2024-fp16",
                "abc123",
                Provider::WebGpu,
                "Test GPU|Vulkan|1.2.3",
                "WebGPU,CPU greycard 0.1.0",
                record::Outcome::Failed {
                    reason: "Split node".to_string(),
                },
            )],
        );
        let before = std::fs::read_to_string(record::path(&dir)).unwrap();

        let entries = record::read(&dir);
        let outcome = plan(
            &entries,
            "abc123",
            Provider::WebGpu,
            "Test GPU|Vulkan|1.2.3",
            "WebGPU,CPU greycard 0.1.0",
        );
        assert_eq!(
            outcome,
            Some(record::Outcome::Failed {
                reason: "Split node".to_string()
            })
        );

        // `open` would `continue` past this provider without ever
        // reaching a write; nothing here touched the file either.
        let after = std::fs::read_to_string(record::path(&dir)).unwrap();
        assert_eq!(before, after);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_place_to_remember_nothing_is_written() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-runtime-open-no-remember-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = open(
            Path::new("/nonexistent/model.onnx"),
            GraphOptimizationLevel::Level1,
            &[Provider::Cpu],
            Some(Remembered {
                store_root: &dir,
                model: "test-model",
                hash: "abc123",
            }),
            |_| Ok(()),
        );
        // CPU never has an adapter identity, so nothing is looked up
        // or written for it even with somewhere to write to.
        assert!(!record::path(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
