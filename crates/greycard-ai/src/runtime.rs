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

use std::borrow::Cow;
use std::path::Path;
use std::sync::{OnceLock, RwLock};

use ort::session::Session;
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};

mod dylib;
mod on_card;
mod record;

#[cfg(test)]
pub(crate) use on_card::flag_lock;
pub use on_card::{
    HELD, Held, OnCard, denoisers_off_card, denoisers_off_here, forget_off_card,
    let_denoisers_back_on, off_card_path, on_card_now, put_off_card, read_denoisers_off_card,
    runs_held, set_build_label,
};
#[cfg(any(test, feature = "testing"))]
pub use on_card::{off_card_for_tests, off_card_is_for_tests, set_test_here, set_test_store};

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
            #[cfg(webgpu_ep)]
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
            #[cfg(webgpu_ep)]
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
            #[cfg(webgpu_ep)]
            Provider::WebGpu => webgpu_adapter_identity(),
            #[cfg(not(webgpu_ep))]
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
        Ok(()) => {
            let shape = record::shape(fixed);
            mark_coreml_cache(&dir, &shape);
            sweep_coreml_cache(&dir, path, &shape);
            Some(dir)
        }
        Err(e) => {
            log::warn!(
                "no Core ML cache at {}: {e}; it compiles at every load",
                dir.display()
            );
            None
        }
    }
}

/// The file in a compile folder that names the shape it was compiled
/// at, beside what ONNX Runtime writes there.
const SHAPE_MARK: &str = "greycard-shape.txt";

/// Writes [`SHAPE_MARK`] in `dir` with `shape` (`record::shape`), if it
/// is not there already. Best effort: a folder without one is taken
/// for one made before marks, and swept as older.
#[cfg_attr(not(all(feature = "coreml", target_os = "macos")), allow(dead_code))]
fn mark_coreml_cache(dir: &Path, shape: &str) {
    let mark = dir.join(SHAPE_MARK);
    if !mark.exists() {
        let _ = std::fs::write(mark, shape);
    }
}

/// Removes the older compiles of the model at `path` beside `keep`:
/// folders under the same parent whose compile ONNX Runtime recorded
/// as made from that file (the `model.txt` it writes in each, which
/// holds the source's path), at the same `shape`, or with no shape
/// marked, which only a build from before the marks leaves. Each is a
/// compile under an older key: the file replaced, or a graph greycard
/// hands the provider differently, as `CORE_ML_UNFUSED` changed, which
/// left 431 MB for Sky alone. The same file's compile at another shape
/// (the bench's `--tile` beside the editor's default) is in use, not
/// older, and is left; so is a folder of another file, or one that
/// cannot be read.
#[cfg_attr(not(all(feature = "coreml", target_os = "macos")), allow(dead_code))]
fn sweep_coreml_cache(keep: &Path, path: &Path, shape: &str) {
    let Some(parent) = keep.parent() else { return };
    let Ok(folders) = std::fs::read_dir(parent) else {
        return;
    };
    let source = path.to_string_lossy();
    for folder in folders.flatten().map(|e| e.path()) {
        if folder == keep || !folder.is_dir() {
            continue;
        }
        let same_shape = match std::fs::read_to_string(folder.join(SHAPE_MARK)) {
            Ok(marked) => marked == shape,
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };
        if !same_shape {
            continue;
        }
        let made_from_this = std::fs::read_dir(&folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path().join("model.txt")).ok())
            .any(|text| text.trim() == source);
        if !made_from_this {
            continue;
        }
        match std::fs::remove_dir_all(&folder) {
            Ok(()) => log::info!(
                "an older Core ML compile of {} removed: {}",
                path.display(),
                folder.display()
            ),
            Err(e) => log::warn!(
                "an older Core ML compile of {} not removed ({}): {e}",
                path.display(),
                folder.display()
            ),
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
    // The optimizers kept off, which change the graph Core ML is
    // handed: a compile cached from another graph is not this one's.
    h.update(CORE_ML_UNFUSED.as_bytes());
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

/// A console program we ask something of, run without a console
/// window: under the editor, which has none of its own on Windows,
/// every one we start would otherwise flash one up. Elsewhere, and
/// for a program with a window of its own, the flag changes nothing.
#[cfg(any(feature = "cuda", all(feature = "coreml", target_os = "macos")))]
fn windowless(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW.
        command.creation_flags(0x0800_0000);
    }
    command
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
                let out = windowless(&mut std::process::Command::new(program))
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
#[cfg(webgpu_ep)]
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
    #[cfg(webgpu_ep)]
    {
        match webgpu_adapter()?.device_type {
            wgpu::DeviceType::IntegratedGpu => Some(true),
            wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::VirtualGpu => Some(false),
            wgpu::DeviceType::Cpu | wgpu::DeviceType::Other => None,
        }
    }
    #[cfg(not(webgpu_ep))]
    {
        None
    }
}

/// Whether the WebGPU adapter's shaders take 16-bit floats, with its
/// name: the remove's models are fp16 through and through, and ONNX
/// Runtime's WebGPU provider refuses them on a card without (an
/// NVIDIA card before Turing). `None` when no adapter answers. Same
/// adapter, and same guess, as [`webgpu_unified`].
pub fn webgpu_shader_f16() -> Option<(bool, String)> {
    #[cfg(webgpu_ep)]
    {
        webgpu_adapter().map(|a| (a.shader_f16, a.name.clone()))
    }
    #[cfg(not(webgpu_ep))]
    {
        None
    }
}

/// The bytes of the WebGPU adapter's own memory: its largest
/// device-local heap on Vulkan ([`card_heap`]), its dedicated video
/// memory on DX12.
/// `None` when no adapter answers, on Metal (a Mac's GPU has the
/// machine's memory, which the caller reads), and on any backend that
/// will not say. On an integrated GPU it is the share the driver sets
/// aside, not what the GPU can reach; [`webgpu_unified`] says when to
/// read the machine's memory instead. Same adapter, and same guess,
/// as [`webgpu_unified`].
pub fn webgpu_memory() -> Option<u64> {
    webgpu_memory_read().map(|(bytes, _)| bytes)
}

/// [`webgpu_memory`], with what it was read from, for the log:
/// Vulkan's largest device-local heap or DXGI's dedicated video memory.
pub fn webgpu_memory_read() -> Option<(u64, &'static str)> {
    #[cfg(webgpu_ep)]
    {
        webgpu_adapter()?.memory
    }
    #[cfg(not(webgpu_ep))]
    {
        None
    }
}

/// The bytes of the WebGPU adapter's own memory free now, and what
/// said so: on Vulkan the budget less this process's use of the
/// largest device-local heap (`VK_EXT_memory_budget`), on DX12 DXGI's
/// budget less the current use of the local segment. Both budgets are
/// the driver's estimate of what this process may have, other
/// processes' use taken out. Read from the backend at each call, not
/// kept; a query of the physical device, no device made. `None` when
/// no adapter answers, on unified memory (an integrated GPU's
/// device-local heap is the firmware's carve-out, not what it can
/// reach, and a Mac's is Metal's), on a CPU adapter (lavapipe), and
/// where the backend will not say. The first answer goes to the log.
pub fn webgpu_free_memory() -> Option<(u64, &'static str)> {
    #[cfg(webgpu_ep)]
    {
        if webgpu_unified() == Some(true) {
            return None;
        }
        let a = webgpu_adapter()?;
        if matches!(a.device_type, wgpu::DeviceType::Cpu) {
            return None;
        }
        let free = adapter_free_memory(&a.adapter);
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| match free {
            Some((bytes, from)) => log::info!(
                "{}: {:.2} GiB free, from {from}",
                a.name,
                bytes as f64 / (1u64 << 30) as f64
            ),
            None => log::info!("{}: its free memory not known", a.name),
        });
        free
    }
    #[cfg(not(webgpu_ep))]
    {
        None
    }
}

/// The heap the allocator keeps after the models' sessions are dropped
/// given back to the system: on Linux with glibc, `malloc_trim(0)`,
/// which took a process from 3.65 GiB resident to 0.18 after a removal
/// (glibc keeps what ONNX Runtime freed on the host). Elsewhere it does
/// nothing.
pub fn trim_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `malloc_trim` only returns free memory at the heap's top
    // and in its free lists to the system; it touches nothing in use.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// The WebGPU adapter's name, as wgpu reports it, for words that name
/// the card. `None` when no adapter answers.
pub fn webgpu_name() -> Option<String> {
    #[cfg(webgpu_ep)]
    {
        webgpu_adapter().map(|a| a.name.clone())
    }
    #[cfg(not(webgpu_ep))]
    {
        None
    }
}

/// An adapter's identity, as the provider and off-card records key on
/// it: its name, backend, and driver with the driver's own words, so a
/// driver update makes it another adapter to them.
#[cfg_attr(not(webgpu_ep), allow(dead_code))]
fn adapter_identity_of(name: &str, backend: &str, driver: &str, driver_info: &str) -> String {
    format!("{name}|{backend}|{driver} {driver_info}")
}

/// What the WebGPU probe learned of the adapter.
#[cfg(webgpu_ep)]
struct Adapter {
    /// The adapter itself, kept so its free memory can be asked of the
    /// backend each time ([`webgpu_free_memory`]). It and its instance
    /// live for the process: on a laptop with two GPUs that may keep the
    /// discrete one's device nodes open, and so keep it awake.
    adapter: wgpu::Adapter,
    /// Name, backend, and driver.
    identity: String,
    /// The adapter's name alone.
    name: String,
    device_type: wgpu::DeviceType,
    /// Whether its shaders take 16-bit floats, which the remove's
    /// models are made of.
    shader_f16: bool,
    /// The bytes of its own memory, and what said so
    /// ([`webgpu_memory_read`]).
    memory: Option<(u64, &'static str)>,
}

/// Which of a Vulkan device's heaps, each its size and whether it is
/// device-local, is the card's own memory: the largest device-local
/// one. A second, the 256 MB BAR window (NVIDIA's, AMD's with
/// resizable BAR the whole card), is a view into the same memory, so
/// summed it counts some of the card twice: a tester's 2 GB card read
/// as 2.2 GiB.
#[cfg_attr(not(webgpu_ep), allow(dead_code))]
fn card_heap(heaps: &[(u64, bool)]) -> Option<usize> {
    (0..heaps.len())
        .filter(|&i| heaps[i].1)
        .max_by_key(|&i| heaps[i].0)
}

/// The bytes of a Vulkan card's own memory, from its heaps as
/// [`card_heap`] takes them: what [`webgpu_memory`] says on Vulkan.
#[cfg_attr(not(webgpu_ep), allow(dead_code))]
fn vulkan_card_bytes(heaps: &[(u64, bool)]) -> u64 {
    card_heap(heaps).map_or(0, |i| heaps[i].0)
}

/// [`adapter_memory`]'s answer on Vulkan from the device's heaps, each
/// its size and whether it is device-local: the whole of what it does
/// with them, so its test is the reading's.
#[cfg_attr(not(webgpu_ep), allow(dead_code))]
fn vulkan_card_memory(heaps: &[(u64, bool)]) -> Option<(u64, &'static str)> {
    let bytes = vulkan_card_bytes(heaps);
    (bytes > 0).then_some((bytes, "Vulkan's largest device-local heap"))
}

/// The bytes of `adapter`'s own memory, asked of the backend under
/// wgpu, which keeps it to itself, and which of the backend's answers
/// it is.
#[cfg(webgpu_ep)]
fn adapter_memory(adapter: &wgpu::Adapter) -> Option<(u64, &'static str)> {
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
            let heaps: Vec<(u64, bool)> = props.memory_heaps[..count]
                .iter()
                .map(|h| (h.size, h.flags.as_raw() & DEVICE_LOCAL != 0))
                .collect();
            return vulkan_card_memory(&heaps);
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
            return (bytes > 0).then_some((bytes, "DXGI's dedicated video memory"));
        }
    }
    let _ = adapter;
    None
}

/// The bytes of `adapter`'s own memory free now ([`webgpu_free_memory`]).
#[cfg(webgpu_ep)]
fn adapter_free_memory(adapter: &wgpu::Adapter) -> Option<(u64, &'static str)> {
    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
    {
        use ash::{ext, khr, vk};
        // SAFETY: as in `adapter_memory`: only read.
        if let Some(vk) = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() } {
            // wgpu leaves the extension out where a driver's answer is
            // known to be wrong (RADV), and enables
            // VK_KHR_get_physical_device_properties2 on its instance
            // wherever the loader has it; without either, not known.
            if !vk
                .physical_device_capabilities()
                .supports_extension(ext::memory_budget::NAME)
            {
                return None;
            }
            let shared = vk.shared_instance();
            if !shared
                .extensions()
                .contains(&khr::get_physical_device_properties2::NAME)
            {
                return None;
            }
            let props2 = khr::get_physical_device_properties2::Instance::new(
                shared.entry(),
                shared.raw_instance(),
            );
            let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
            let mut props = vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut budget);
            // SAFETY: the physical device is the live one the adapter
            // wraps, the instance enabled the extension the function is
            // loaded from, and the device supports the chained struct.
            unsafe {
                props2.get_physical_device_memory_properties2(vk.raw_physical_device(), &mut props)
            };
            // The largest device-local heap: a card's VRAM. A second one,
            // the 256 MB BAR window (NVIDIA's, AMD's with resizable BAR),
            // is a view into the same memory, and summed would count it
            // twice.
            let heaps = props.memory_properties;
            let count = (heaps.memory_heap_count as usize).min(heaps.memory_heaps.len());
            let sized: Vec<(u64, bool)> = heaps.memory_heaps[..count]
                .iter()
                .map(|h| (h.size, h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL)))
                .collect();
            let largest = card_heap(&sized)?;
            let (budget, used) = (budget.heap_budget[largest], budget.heap_usage[largest]);
            return (budget > 0).then_some((budget.saturating_sub(used), "Vulkan's memory budget"));
        }
    }
    #[cfg(windows)]
    {
        // SAFETY: as above; the DXGI adapter is only asked.
        if let Some(dx) = unsafe { adapter.as_hal::<wgpu::hal::api::Dx12>() } {
            // The default segment group is the local one, the card's
            // own memory.
            let info = dx
                .raw_adapter()
                .query_video_memory_info(Default::default())
                .ok()?;
            return (info.Budget > 0).then_some((
                info.Budget.saturating_sub(info.CurrentUsage),
                "DXGI's local video memory budget",
            ));
        }
    }
    let _ = adapter;
    None
}

/// The adapter's identity, type, and memory, asked once a process and
/// kept.
#[cfg(webgpu_ep)]
static ADAPTER: OnceLock<Option<Adapter>> = OnceLock::new();

/// [`webgpu_free_memory`] only once the adapter has been asked for
/// by something else: `None` while it has not, so a caller that must
/// not set off the probe (the worker's develop, logging the card's
/// free memory) blocks on nothing.
pub fn webgpu_free_memory_if_probed() -> Option<(u64, &'static str)> {
    #[cfg(webgpu_ep)]
    {
        ADAPTER.get()?;
        return webgpu_free_memory();
    }
    #[allow(unreachable_code)]
    None
}

/// [`webgpu_card_memory`] only once the adapter has been asked for by
/// something else: `None` while it has not, so the window's thread
/// blocks on nothing.
pub fn webgpu_card_memory_if_probed() -> Option<u64> {
    #[cfg(webgpu_ep)]
    {
        ADAPTER.get()?;
        return webgpu_card_memory();
    }
    #[allow(unreachable_code)]
    None
}

/// [`webgpu_memory`] of a card of its own (a discrete or virtual GPU,
/// not a Mac's): `None` on unified memory, a CPU adapter, or one of no
/// known type. Blocks on the adapter's probe the first time.
pub fn webgpu_card_memory() -> Option<u64> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return None;
    }
    #[cfg(webgpu_ep)]
    {
        let a = webgpu_adapter()?;
        return match a.device_type {
            wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::VirtualGpu => {
                a.memory.map(|(bytes, _)| bytes)
            }
            _ => None,
        };
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(webgpu_ep)]
fn webgpu_adapter() -> Option<&'static Adapter> {
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
                identity: adapter_identity_of(&info.name, &format!("{:?}", info.backend), &info.driver, &info.driver_info),
                name: info.name.clone(),
                device_type: info.device_type,
                shader_f16: adapter.features().contains(wgpu::Features::SHADER_F16),
                memory: adapter_memory(&adapter),
                adapter,
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
            let out = windowless(&mut std::process::Command::new("nvidia-smi"))
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
    /// Not begun: the card had not the memory free for it.
    #[error("{0}")]
    Short(String),
    /// Not loaded: the card was lost while the model ran on it, and it
    /// is on record as off this card, driver, and build
    /// ([`denoisers_off_here`]). WORDING PENDING the user's approval.
    #[error("the learned denoiser is not on this graphics card")]
    OffCard,
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
                    // Microsoft's own Windows builds carry an ETW
                    // telemetry provider, on unless told otherwise, that
                    // Windows' diagnostic data can forward: model file
                    // names and hashes, providers, devices, run counts,
                    // errors. The CPU-only fallback is one of those
                    // builds. Off, on every library: nothing of what a
                    // user runs leaves the machine.
                    builder.with_name("greycard").with_telemetry(false).commit();
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
/// session is dropped. The CPU after them is not tried. Each side's
/// time is remembered with its outcome, and a later load of the same
/// file at the same shape, on the same adapter and build, opens the
/// winner first without a race, the other after it only if the winner
/// fails ([`settled_race`]): one session a load, not two.
pub fn open(
    path: &Path,
    level: GraphOptimizationLevel,
    providers: &[Provider],
    remember: Option<Remembered<'_>>,
    warm: impl FnMut(&mut Session) -> ort::Result<()>,
) -> Result<Loaded> {
    open_fixed(path, None, level, providers, remember, &[], warm)
}

/// A model's sessions being opened, counted across threads, and who
/// hears when the first starts and the last ends ([`on_loading`]).
struct Loads {
    state: std::sync::Mutex<(usize, Option<Listener>)>,
}

type Listener = std::sync::Arc<dyn Fn(bool) + Send + Sync>;

impl Loads {
    const fn new() -> Self {
        Self {
            state: std::sync::Mutex::new((0, None)),
        }
    }

    fn listen(&self, listener: Listener) {
        self.lock().1 = Some(listener);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (usize, Option<Listener>)> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// One more under way; the listener told under the lock, so its
    /// starts and ends arrive in the order they happened.
    fn begin(&'static self) -> Loading {
        let mut s = self.lock();
        s.0 += 1;
        if s.0 == 1
            && let Some(l) = &s.1
        {
            l(true);
        }
        Loading { loads: self }
    }

    fn end(&self) {
        let mut s = self.lock();
        s.0 = s.0.saturating_sub(1);
        if s.0 == 0
            && let Some(l) = &s.1
        {
            l(false);
        }
    }
}

static LOADS: Loads = Loads::new();

/// A load under way, from [`loading`] until dropped.
#[must_use = "the load ends when this is dropped"]
pub struct Loading {
    loads: &'static Loads,
}

impl Drop for Loading {
    fn drop(&mut self) {
        self.loads.end();
    }
}

/// Marks a load under way until the answer is dropped: every [`open`]
/// holds one from the start, the runtime's own load included, to its
/// session warmed or its failure.
pub fn loading() -> Loading {
    LOADS.begin()
}

/// `listener` told `true` when a model starts loading with none other
/// under way, and `false` when the last under way is ready or has
/// failed, on the loading thread: for a window's word while a model
/// loads or compiles. One listener a process; a later call replaces
/// it. It must not load a model itself.
pub fn on_loading(listener: impl Fn(bool) + Send + Sync + 'static) {
    LOADS.listen(std::sync::Arc::new(listener));
}

/// [`open`], with the model's free dimensions named in `fixed` set to
/// their sizes on Core ML, which compiles a static graph for the
/// Neural Engine only when every shape is known at load. The other
/// providers keep the graph's free dimensions. `warm` should then run
/// the fixed shape, on every provider, so the timing in `open` compares
/// like with like.
///
/// `core_ml`, where given, is the file Core ML opens in place of
/// `path`, the same network at fp16 for the denoiser
/// ([`crate::registry::core_ml_file`]); every other provider opens
/// `path`. A `remember` for such a load should name both files in its
/// hash, so a record of `path` alone on Core ML is not taken for it.
pub fn open_fixed(
    path: &Path,
    core_ml: Option<&Path>,
    level: GraphOptimizationLevel,
    providers: &[Provider],
    remember: Option<Remembered<'_>>,
    fixed: &[(&str, i64)],
    mut warm: impl FnMut(&mut Session) -> ort::Result<()>,
) -> Result<Loaded> {
    let _loading = loading();
    if let Err(why) = init() {
        return Err(Error::Unavailable(why.clone()));
    }
    let mut tried = Vec::new();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(file) = core_ml.filter(|_| providers.contains(&Provider::CoreMl)) {
        log::info!("{name}: Core ML opens {}", file.display());
    }
    let build = record::build_fingerprint(providers);
    let shape = record::shape(fixed);
    // Read once, lazily, and only with `remember` set: for a list that
    // races, to see whether the race is settled, and otherwise for a
    // provider whose adapter could be named.
    let mut entries: Option<Vec<record::Entry>> = None;
    // A race this file already ran at this shape, on this adapter and
    // build, is not run again: the winner is opened first and not
    // timed, and the other stays after it in case the winner fails.
    let settled = match remember {
        Some(r) if racer(providers).is_some() => {
            let entries = entries.get_or_insert_with(|| record::read(r.store_root));
            settled_race(entries, r.hash, providers, &build, &shape, |p| {
                p.adapter_identity()
            })
        }
        _ => None,
    };
    let order = match settled {
        Some(winner) => {
            log::info!(
                "{name}: {} won the race at its last load; opened without one",
                winner.name()
            );
            winner_first(providers, winner)
        }
        None => providers.to_vec(),
    };
    // Core ML's session and its timed run, held while the provider
    // after it is timed against it.
    let mut held: Option<(Loaded, f64)> = None;

    for (i, &provider) in order.iter().enumerate() {
        if held.is_some() && provider == Provider::Cpu {
            break;
        }
        let last = i + 1 == order.len();
        let racing = settled.is_none() && held.is_none() && races(provider, &order[i + 1..]);
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

        let file = match core_ml {
            Some(file) if provider == Provider::CoreMl => file,
            _ => path,
        };
        let outcome = try_open(file, level, provider, fixed, skip_warm, timed, &mut warm);
        if let (Some(r), Some(adapter)) = (remember, &adapter) {
            let entries = entries.get_or_insert_with(|| record::read(r.store_root));
            let recorded = match &outcome {
                Ok(_) => record::Outcome::Worked,
                Err(e) => record::Outcome::Failed {
                    reason: first_line(&e.to_string()).to_string(),
                },
            };
            let race = race_to_record(
                outcome.as_ref().ok().map(|(_, run)| *run),
                record::find(entries, r.hash, provider),
                adapter,
                &build,
                &shape,
            );
            let entry = record::Entry {
                model: r.model.to_string(),
                hash: r.hash.to_string(),
                provider: provider.name().to_string(),
                adapter: adapter.clone(),
                build: build.clone(),
                outcome: recorded,
                race,
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

/// Core ML and the provider it would be timed against, where
/// `providers` races them.
fn racer(providers: &[Provider]) -> Option<(Provider, Provider)> {
    let i = providers.iter().position(|&p| p == Provider::CoreMl)?;
    let rival = providers[i + 1..]
        .iter()
        .copied()
        .find(|&p| p != Provider::Cpu)?;
    Some((Provider::CoreMl, rival))
}

/// The winner of the race `providers` would run, as the record has it
/// for the file named by `hash` at `shape`: both sides' entries worked,
/// were timed at that shape, and still apply to the adapter `adapter`
/// names for each and to `build`. A tie goes to Core ML, as it does in
/// the race. `None` when either side is missing, failed, untimed,
/// timed at another shape, or stale, and the race is run.
fn settled_race(
    entries: &[record::Entry],
    hash: &str,
    providers: &[Provider],
    build: &str,
    shape: &str,
    adapter: impl Fn(Provider) -> Option<String>,
) -> Option<Provider> {
    let (core_ml, rival) = racer(providers)?;
    let run = |p: Provider| {
        let adapter = adapter(p)?;
        let e = record::find(entries, hash, p)?;
        let race = e.race.as_ref()?;
        (record::valid(e, &adapter, build)
            && e.outcome == record::Outcome::Worked
            && race.shape == shape)
            .then_some(race.run_us)
    };
    let (first, second) = (run(core_ml)?, run(rival)?);
    Some(if second < first { rival } else { core_ml })
}

/// The race to write with a provider's outcome: `run` is `None` when
/// it failed, `Some(None)` when it worked untimed, and `Some(Some(s))`
/// when its warm run was timed at `s` seconds. A timed run is this
/// load's side of the race. An untimed success keeps the side on
/// record in `previous` while that entry still applies to `adapter`
/// and `build`, so a settled race stays settled. A failure has none,
/// which unsettles the race.
fn race_to_record(
    run: Option<Option<f64>>,
    previous: Option<&record::Entry>,
    adapter: &str,
    build: &str,
    shape: &str,
) -> Option<record::Race> {
    match run? {
        Some(seconds) => Some(record::Race {
            shape: shape.to_string(),
            run_us: (seconds * 1e6).round() as u64,
        }),
        None => previous
            .filter(|e| record::valid(e, adapter, build))
            .and_then(|e| e.race.clone()),
    }
}

/// `providers` with `winner` moved to where the race's first side
/// stood, the other side after it; unchanged when Core ML won.
fn winner_first(providers: &[Provider], winner: Provider) -> Vec<Provider> {
    let mut order: Vec<Provider> = providers.to_vec();
    if winner == Provider::CoreMl {
        return order;
    }
    let (Some(at), Some(from)) = (
        order.iter().position(|&p| p == Provider::CoreMl),
        order.iter().position(|&p| p == winner),
    ) else {
        return order;
    };
    let p = order.remove(from);
    order.insert(at, p);
    order
}

/// The graph optimizers kept off a Core ML session. ONNX Runtime's
/// `MatMulAddFusion` turns a MatMul and the Add after it into a Gemm,
/// and the Core ML provider writes a Gemm's weight transposed into
/// the program as an inline literal rather than into the weight file,
/// where a MatMul's weight goes by reference (`gemm_op_builder.cc`,
/// 1.28). On the Sky model that put 130 weights, 336 MB of hex text,
/// into the program, and Core ML prints the whole program to text to
/// key its own cache and again in each validator: a session took 27 s
/// cold and 4.5 s from the cache, most of it in `snprintf`. Unfused,
/// Core ML fuses the pair itself from the weight file.
pub const CORE_ML_UNFUSED: &str = "MatMulAddFusion";

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
    // The CPU takes its weights from ONNX Runtime's own maps of the
    // files, without a copy; handed over in memory they would be copied
    // onto the heap, 5.5 GB for the remove's UNet.
    let weights = if provider == Provider::Cpu {
        ExternalWeights::default()
    } else {
        ExternalWeights::beside(path)
    };
    let mut session = build_session(path, level, provider, fixed, &weights)?;
    drop(weights);
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

/// A session on `provider`, `weights` handed to ONNX Runtime in memory
/// where there are any. ONNX Runtime does not fall back to reading a
/// file once a table is given: every external file the graph names
/// must be in it, under the graph's own `location`, or the build fails
/// ("not found from the table user provided"). So when the build with
/// the table fails, it is made once more without it, from the files as
/// ONNX Runtime reads them, and a model the table does not suit opens
/// as it did before the table. The options are set once and copied for
/// each attempt, and each attempt registers its provider afresh; a
/// failure setting the options or registering the provider is not
/// retried, and a provider that registers but fails when the session
/// is made (Core ML and CUDA make theirs then) or fails the graph
/// itself fails both builds and is recorded as failing once.
fn build_session(
    path: &Path,
    level: GraphOptimizationLevel,
    provider: Provider,
    fixed: &[(&str, i64)],
    weights: &ExternalWeights,
) -> ort::Result<Session> {
    let mut options = Session::builder()?.with_optimization_level(level)?;
    if provider == Provider::CoreMl {
        options = options.with_disabled_optimizers(CORE_ML_UNFUSED)?;
        for &(name, size) in fixed {
            options = options.with_dimension_override(name, size)?;
        }
    }
    // The provider is registered on each attempt's own copy, never on
    // the options both copy: a copy of the options shares the provider
    // factory registered on them, and ONNX Runtime's WebGPU factory
    // takes its hold on the process's WebGPU context once, when it is
    // made, but lets go once for every session made from it, so two
    // sessions from one registration let the context go once too often
    // and the next session on it crashes.
    // A provider that will not register fails here, once, and is not
    // retried: only the build is.
    let registered = |builder: SessionBuilder| -> ort::Result<SessionBuilder> {
        Ok(match provider.dispatch(path, fixed) {
            Some(ep) => builder.with_execution_providers([ep])?,
            None => builder,
        })
    };
    let table = weights.files();
    if !table.is_empty() {
        match with_table(registered(options.clone())?, &table, path) {
            Ok(session) => return Ok(session),
            Err(e) => log::info!(
                "{}: the session with its weights in memory failed ({}); built again from the files",
                path.display(),
                first_line(&e.to_string())
            ),
        }
    }
    with_table(registered(options)?, &[], path)
}

/// The session `builder` makes with `table`'s files handed over in
/// memory, or from the files alone when `table` is empty.
fn with_table(
    mut builder: SessionBuilder,
    table: &[(&str, &[u8])],
    path: &Path,
) -> ort::Result<Session> {
    for &(name, bytes) in table {
        // SAFETY: the builder keeps the slice only until it is dropped,
        // at the end of this function, on every path out of it (an
        // error drops it inside the `?`); the `Session` keeps none of
        // it, and ONNX Runtime copies the tensors into the graph ("the
        // buffer can be freed after session creation",
        // `AddExternalInitializersFromFilesInMemory`, 1.28). The maps
        // behind `table` outlive this call.
        let bytes: &'static [u8] =
            unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) };
        builder = builder.with_external_initializer_file_in_memory(name, Cow::Borrowed(bytes))?;
    }
    builder.commit_from_file(path)
}

/// A model's external weight files, each mapped once for the session
/// being built.
///
/// ONNX Runtime maps an external initializer's bytes from its file one
/// tensor at a time, an `mmap` each. The remove's UNet names 3216
/// tensors in two files, and on macOS those calls took three quarters
/// of a session's build: 10.4 s for the Fast UNet on an M5, 1.7 to 2.6 s
/// with each file mapped once and handed over in memory, 9.2 s and 1.4 s
/// for the 25-step one (§482). Every `*.onnx_data` beside the model is
/// offered, under its own name, as the graph's `location` fields name
/// them; ONNX Runtime takes the tensors a graph names from the files it
/// names and passes over files it does not name. Either every file is
/// mapped or none is (a table missing one the graph names fails the
/// build), and a build the table does not suit is made again without it
/// ([`build_session`]). Not for the CPU, which takes ONNX Runtime's own
/// maps without a copy (`try_open`).
#[derive(Default)]
struct ExternalWeights {
    files: Vec<(String, memmap2::Mmap)>,
}

impl ExternalWeights {
    fn beside(model: &Path) -> Self {
        let Some(dir) = model.parent() else {
            return Self { files: Vec::new() };
        };
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "onnx_data") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // SAFETY: the map is read only, and only while the session
            // is built. The model store replaces a file by renaming a
            // finished copy over it (`store.rs`), never by writing into
            // it, so the mapped file is not truncated under the map.
            let mapped = std::fs::File::open(&path).and_then(|f| unsafe { memmap2::Mmap::map(&f) });
            match mapped {
                Ok(map) => files.push((name.to_string(), map)),
                Err(e) => {
                    // A table missing one file the graph names fails the
                    // build, so one file not mapped means none offered.
                    log::debug!(
                        "{}: not mapped ({e}); ONNX Runtime reads the weights itself",
                        path.display()
                    );
                    return Self::default();
                }
            }
        }
        Self { files }
    }

    /// Each file's name and bytes, for as long as `self` lives.
    fn files(&self) -> Vec<(&str, &[u8])> {
        self.files
            .iter()
            .map(|(name, map)| (name.as_str(), &map[..]))
            .collect()
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The card's free memory, where the backend says it, is no more
    /// than its whole memory; unified memory, a CPU adapter (lavapipe),
    /// and no adapter say nothing. Only the physical device is asked:
    /// no device is made.
    #[test]
    fn the_free_memory_is_within_the_cards() {
        let first = webgpu_free_memory();
        let second = webgpu_free_memory();
        eprintln!(
            "free: {first:?}, then {second:?}; whole: {:?}",
            webgpu_memory_read()
        );
        if let Some((free, _)) = first {
            let whole = webgpu_memory().expect("a card that says what is free says its size");
            assert!(free <= whole, "{free} free of {whole}");
        }
        assert_eq!(first.is_some(), second.is_some());
        if webgpu_unified() == Some(true) {
            assert!(first.is_none());
        }
    }

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
        if cfg!(all(feature = "coreml", target_os = "macos", webgpu_ep)) {
            assert!(at(Provider::CoreMl) < at(Provider::WebGpu));
        } else if cfg!(all(feature = "coreml", target_os = "macos")) {
            // An Intel Mac: Core ML and the CPU, no WebGPU.
            assert_eq!(at(Provider::CoreMl), Some(0));
            assert_eq!(at(Provider::WebGpu), None);
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
        greycard_core::testing::remove_dir_retry(&dir);
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
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Making a file's compile folder removes that file's compiles
    /// under older keys, as ONNX Runtime lays them out (a folder a
    /// partition, its `model.txt` naming the source): one at the same
    /// shape, and one from before the shape marks. It leaves the same
    /// file's compile at another shape, another file's, and a folder
    /// with no record of its source.
    #[test]
    fn an_older_core_ml_compile_of_the_same_file_is_swept() {
        let root = std::env::temp_dir().join(format!(
            "greycard-runtime-coreml-sweep-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        greycard_core::testing::remove_dir_retry(&root);
        let cache = root.join("coreml");
        let denoiser = root.join("models").join("denoise-v20.onnx");
        let sky = root.join("models").join("eomt-s-coco-640.onnx");
        let at = record::shape(&[("h", 608), ("w", 608)]);
        let compile = |name: &str, source: Option<&Path>, shape: Option<&str>| {
            let folder = cache.join(name);
            let part = folder.join("15121039830728902357");
            std::fs::create_dir_all(part.join("0_static_mlprogram")).unwrap();
            if let Some(source) = source {
                std::fs::write(part.join("model.txt"), source.to_string_lossy().as_bytes())
                    .unwrap();
            }
            if let Some(shape) = shape {
                mark_coreml_cache(&folder, shape);
            }
            folder
        };
        let replaced = compile("denoise-v20-1111111111111111", Some(&denoiser), Some(&at));
        let unmarked = compile("denoise-v20-3b5ae551c565f67b", Some(&denoiser), None);
        let other_tile = compile(
            "denoise-v20-2222222222222222",
            Some(&denoiser),
            Some("h=352,w=352"),
        );
        let keep = compile("denoise-v20-25b1585071afb68d", Some(&denoiser), Some(&at));
        let other_file = compile("eomt-s-coco-640-5ccff4f64851c189", Some(&sky), Some(""));
        let unknown = compile("denoise-v20-0000000000000000", None, Some(&at));

        sweep_coreml_cache(&keep, &denoiser, &at);
        assert!(!replaced.exists());
        assert!(!unmarked.exists());
        assert!(other_tile.exists());
        assert!(keep.exists());
        assert!(other_file.exists());
        assert!(unknown.exists());
        greycard_core::testing::remove_dir_retry(&root);
    }

    /// The weight files offered with a model are the `*.onnx_data`
    /// beside it, under their own names and with their bytes; nothing
    /// else in the folder, and nothing for a model with none.
    #[test]
    fn the_weights_beside_a_model_are_mapped_by_name() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-runtime-weights-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        greycard_core::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("unet-512.onnx");
        std::fs::write(&model, b"graph").unwrap();
        std::fs::write(dir.join("unet.onnx_data"), b"weights").unwrap();
        std::fs::write(dir.join("unet-lora.onnx_data"), b"lora").unwrap();
        std::fs::write(dir.join("LICENSE.txt"), b"text").unwrap();
        let weights = ExternalWeights::beside(&model);
        let mut files: Vec<(String, Vec<u8>)> = weights
            .files()
            .into_iter()
            .map(|(name, bytes)| (name.to_string(), bytes.to_vec()))
            .collect();
        files.sort();
        assert_eq!(
            files,
            vec![
                ("unet-lora.onnx_data".to_string(), b"lora".to_vec()),
                ("unet.onnx_data".to_string(), b"weights".to_vec()),
            ]
        );
        drop(weights);
        let alone = dir.join("alone");
        std::fs::create_dir_all(&alone).unwrap();
        std::fs::write(alone.join("sky.onnx"), b"graph").unwrap();
        assert!(
            ExternalWeights::beside(&alone.join("sky.onnx"))
                .files()
                .is_empty()
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// An `ExternalWeights` holding one file of `values`, from an
    /// anonymous map rather than a file beside the model, so that what
    /// a session built with it answers shows whether it read the table.
    fn table_of(name: &str, values: &[f32]) -> ExternalWeights {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut map = memmap2::MmapMut::map_anon(bytes.len()).unwrap();
        map.copy_from_slice(&bytes);
        ExternalWeights {
            files: vec![(name.to_string(), map.make_read_only().unwrap())],
        }
    }

    /// The fixtures' model run on `[1, 2, 3, 4]`.
    fn add(session: &mut Session) -> Vec<f32> {
        let x = ort::value::Tensor::from_array(([1usize, 4], vec![1.0f32, 2.0, 3.0, 4.0])).unwrap();
        let outputs = session.run(ort::inputs!["x" => x]).unwrap();
        outputs["y"].try_extract_tensor::<f32>().unwrap().1.to_vec()
    }

    /// A build with the table that fails on WebGPU, and the fallback
    /// after it, leave ONNX Runtime's WebGPU context whole: the session
    /// the fallback made runs, and a session made, run and dropped
    /// beside it does not take the context down under it. With one
    /// provider registration shared by both builds the fallback's
    /// session crashed the process on its first run. Where the
    /// `webgpu` feature is built and WebGPU opens the fixture at all.
    #[cfg(webgpu_ep)]
    #[test]
    fn a_fallback_on_webgpu_leaves_the_context_whole() {
        if let Err(why) = init() {
            absent_in_test(why);
            return;
        }
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/external");
        let unoffered = fixtures.join("unoffered").join("add.onnx");
        let named = fixtures.join("named").join("add.onnx");
        let none = ExternalWeights::default();
        let Ok(mut first) = build_session(
            &named,
            GraphOptimizationLevel::Level1,
            Provider::WebGpu,
            &[],
            &none,
        ) else {
            eprintln!("skipped: WebGPU does not open the fixture here");
            return;
        };
        assert_eq!(add(&mut first), [11.0, 22.0, 33.0, 44.0]);
        // The table names a file the graph does not: refused, then
        // built again from the files.
        let mut fallback = build_session(
            &unoffered,
            GraphOptimizationLevel::Level1,
            Provider::WebGpu,
            &[],
            &table_of("other.onnx_data", &[0.0; 4]),
        )
        .unwrap();
        assert_eq!(add(&mut fallback), [11.0, 22.0, 33.0, 44.0]);
        drop(first);
        assert_eq!(add(&mut fallback), [11.0, 22.0, 33.0, 44.0]);
        let mut later = build_session(
            &named,
            GraphOptimizationLevel::Level1,
            Provider::WebGpu,
            &[],
            &none,
        )
        .unwrap();
        assert_eq!(add(&mut later), [11.0, 22.0, 33.0, 44.0]);
        drop(fallback);
        assert_eq!(add(&mut later), [11.0, 22.0, 33.0, 44.0]);
    }

    /// A model whose weights are in a file it names beside it builds
    /// from the table and answers right; one naming a file the table
    /// does not hold (`add.weights.bin`, with an unrelated
    /// `other.onnx_data` beside it, which is offered) fails with the
    /// table, as ONNX Runtime does not read a file it was not handed,
    /// and is built again from the files. Both add `[10, 20, 30, 40]`
    /// to their input. The fixtures are `tests/fixtures/external`, made
    /// with `onnx`'s `convert_model_to_external_data`. Through the CPU,
    /// which `try_open` keeps off the table, so that the table's path
    /// runs wherever the runtime can be had.
    #[test]
    fn a_table_builds_or_falls_back_to_the_files() {
        if let Err(why) = init() {
            absent_in_test(why);
            return;
        }
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/external");
        for case in ["named", "unoffered"] {
            let model = fixtures.join(case).join("add.onnx");
            let weights = ExternalWeights::beside(&model);
            assert!(!weights.files().is_empty(), "{case}: something to offer");
            let mut session = build_session(
                &model,
                GraphOptimizationLevel::Level1,
                Provider::Cpu,
                &[],
                &weights,
            )
            .unwrap_or_else(|e| panic!("{case}: {e}"));
            drop(weights);
            let x =
                ort::value::Tensor::from_array(([1usize, 4], vec![1.0f32, 2.0, 3.0, 4.0])).unwrap();
            let outputs = session.run(ort::inputs!["x" => x]).unwrap();
            let (_, y) = outputs["y"].try_extract_tensor::<f32>().unwrap();
            assert_eq!(y, &[11.0, 22.0, 33.0, 44.0], "{case}");
        }
        // `build_session` takes the table when it suits, not the files:
        // a table holding other weights than the file's gives their sum.
        let model = fixtures.join("named").join("add.onnx");
        let mut session = build_session(
            &model,
            GraphOptimizationLevel::Level1,
            Provider::Cpu,
            &[],
            &table_of("add.onnx_data", &[100.0, 200.0, 300.0, 400.0]),
        )
        .unwrap();
        assert_eq!(add(&mut session), [101.0, 202.0, 303.0, 404.0]);
        // And the fallback is what opened the second: with the table
        // alone, ONNX Runtime refuses it rather than read its file.
        let model = fixtures.join("unoffered").join("add.onnx");
        let refused = with_table(
            Session::builder().unwrap(),
            &[("other.onnx_data", &[0; 16])],
            &model,
        );
        let why = refused.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(
            why.contains("not found from the table"),
            "a table without the graph's file was taken, or refused otherwise: {why}"
        );
    }

    /// A raced entry for `provider`, timed at `shape` in `run_us`.
    fn raced(
        provider: Provider,
        shape: &str,
        run_us: u64,
        outcome: record::Outcome,
    ) -> record::Entry {
        let mut e = record::test_entry(
            "greycard-denoise-best-v20",
            "file-hash",
            provider,
            &format!("{} adapter", provider.name()),
            "CoreML,WebGPU,CPU greycard 0.9.1",
            outcome,
        );
        e.race = Some(record::Race {
            shape: shape.to_string(),
            run_us,
        });
        e
    }

    fn settled(entries: &[record::Entry], shape: &str) -> Option<Provider> {
        settled_race(
            entries,
            "file-hash",
            &[Provider::CoreMl, Provider::WebGpu, Provider::Cpu],
            "CoreML,WebGPU,CPU greycard 0.9.1",
            shape,
            |p| Some(format!("{} adapter", p.name())),
        )
    }

    /// The faster side of a race on record wins the next load, Core ML
    /// on a tie, and only while both sides worked, were timed at this
    /// shape, and still apply.
    #[test]
    fn a_race_on_record_settles_the_next_load() {
        let at = "h=1024,w=1024";
        let worked = || record::Outcome::Worked;
        let core_ml_faster = [
            raced(Provider::CoreMl, at, 206_000, worked()),
            raced(Provider::WebGpu, at, 592_000, worked()),
        ];
        assert_eq!(settled(&core_ml_faster, at), Some(Provider::CoreMl));
        let webgpu_faster = [
            raced(Provider::CoreMl, at, 700_000, worked()),
            raced(Provider::WebGpu, at, 592_000, worked()),
        ];
        assert_eq!(settled(&webgpu_faster, at), Some(Provider::WebGpu));
        let tie = [
            raced(Provider::CoreMl, at, 592_000, worked()),
            raced(Provider::WebGpu, at, 592_000, worked()),
        ];
        assert_eq!(settled(&tie, at), Some(Provider::CoreMl));

        // Another tile, one side alone, an untimed side, a failure, or
        // a stale build: the race is run.
        assert_eq!(settled(&core_ml_faster, "h=608,w=608"), None);
        assert_eq!(settled(&core_ml_faster[..1], at), None);
        let mut untimed = core_ml_faster.clone();
        untimed[1].race = None;
        assert_eq!(settled(&untimed, at), None);
        let failed = [
            raced(Provider::CoreMl, at, 206_000, worked()),
            raced(
                Provider::WebGpu,
                at,
                592_000,
                record::Outcome::Failed {
                    reason: "lost".to_string(),
                },
            ),
        ];
        assert_eq!(settled(&failed, at), None);
        let mut stale = core_ml_faster.clone();
        stale[0].build = "CoreML,WebGPU,CPU greycard 0.9.0".to_string();
        assert_eq!(settled(&stale, at), None);
        // No race in the list: nothing to settle.
        assert_eq!(
            settled_race(
                &core_ml_faster,
                "file-hash",
                &[Provider::WebGpu, Provider::Cpu],
                "CoreML,WebGPU,CPU greycard 0.9.1",
                at,
                |p| Some(format!("{} adapter", p.name())),
            ),
            None
        );
    }

    /// What a load writes for its race: a timed run its own, an
    /// untimed success the one on record while it applies, a failure
    /// none.
    #[test]
    fn a_settled_load_keeps_its_race_and_a_failure_drops_it() {
        let at = "h=1024,w=1024";
        let build = "CoreML,WebGPU,CPU greycard 0.9.1";
        let adapter = "CoreML adapter";
        let on_record = raced(Provider::CoreMl, at, 206_000, record::Outcome::Worked);
        let timed = race_to_record(Some(Some(0.2155)), Some(&on_record), adapter, build, at);
        assert_eq!(
            timed,
            Some(record::Race {
                shape: at.to_string(),
                run_us: 215_500
            })
        );
        assert_eq!(
            race_to_record(Some(None), Some(&on_record), adapter, build, at),
            on_record.race
        );
        assert_eq!(race_to_record(Some(None), None, adapter, build, at), None);
        assert_eq!(
            race_to_record(Some(None), Some(&on_record), adapter, "another build", at),
            None
        );
        assert_eq!(
            race_to_record(Some(None), Some(&on_record), "another adapter", build, at),
            None
        );
        assert_eq!(
            race_to_record(None, Some(&on_record), adapter, build, at),
            None
        );
    }

    #[test]
    fn the_winner_goes_first_and_the_other_stays_behind_it() {
        let all = [Provider::CoreMl, Provider::WebGpu, Provider::Cpu];
        assert_eq!(winner_first(&all, Provider::CoreMl), all.to_vec());
        assert_eq!(
            winner_first(&all, Provider::WebGpu),
            vec![Provider::WebGpu, Provider::CoreMl, Provider::Cpu]
        );
        assert_eq!(racer(&all), Some((Provider::CoreMl, Provider::WebGpu)));
        assert_eq!(racer(&[Provider::CoreMl, Provider::Cpu]), None);
        assert_eq!(racer(&[Provider::WebGpu, Provider::Cpu]), None);
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
        greycard_core::testing::remove_dir_retry(&dir);
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
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn without_a_place_to_remember_nothing_is_written() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-runtime-open-no-remember-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        greycard_core::testing::remove_dir_retry(&dir);
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
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A fake loader's loads, two of them overlapping: the listener
    /// hears the start once, at the first, and the end once, at the
    /// last; a load after that starts it again.
    #[test]
    fn the_listener_hears_the_first_start_and_the_last_end() {
        let loads: &'static Loads = Box::leak(Box::new(Loads::new()));
        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let heard = heard.clone();
            loads.listen(std::sync::Arc::new(move |on| {
                heard.lock().unwrap().push(on)
            }));
        }
        let first = loads.begin();
        let second = std::thread::spawn(move || loads.begin()).join().unwrap();
        drop(first);
        assert_eq!(*heard.lock().unwrap(), [true]);
        std::thread::spawn(move || drop(second)).join().unwrap();
        assert_eq!(*heard.lock().unwrap(), [true, false]);
        drop(loads.begin());
        assert_eq!(*heard.lock().unwrap(), [true, false, true, false]);
    }

    /// An adapter's identity carries its driver, its version and its
    /// words: a driver update is another adapter to the records, which
    /// is how it clears a card's off-card entry.
    #[test]
    fn an_adapters_identity_carries_its_driver() {
        let id = |driver, info| adapter_identity_of("NVIDIA GeForce 830A", "Vulkan", driver, info);
        assert_eq!(
            id("NVIDIA", "582.66"),
            "NVIDIA GeForce 830A|Vulkan|NVIDIA 582.66"
        );
        assert_ne!(id("NVIDIA", "582.66"), id("NVIDIA", "591.44"));
        assert_ne!(id("NVIDIA", "582.66"), id("nouveau", "582.66"));
    }

    /// The card's own memory is its largest device-local heap: NVIDIA's
    /// BAR window beside a 2 GB card's VRAM is not added to it, nor a
    /// resizable BAR's view of the whole card; a host heap never counts.
    #[test]
    fn the_cards_memory_is_its_largest_device_local_heap() {
        const MIB: u64 = 1 << 20;
        // A 2 GB NVIDIA card on Vulkan: VRAM, the host's, the BAR.
        let nvidia = [(2048 * MIB, true), (16384 * MIB, false), (224 * MIB, true)];
        assert_eq!(card_heap(&nvidia), Some(0));
        // What the card is said to have: 2 GiB, not the 2.2 the sum read.
        assert_eq!(vulkan_card_bytes(&nvidia), 2048 * MIB);
        assert_eq!(
            vulkan_card_memory(&nvidia),
            Some((2048 * MIB, "Vulkan's largest device-local heap"))
        );
        assert_eq!(vulkan_card_memory(&[(16384 * MIB, false)]), None);
        // Resizable BAR: two device-local heaps of the card's size.
        let rebar = [(8192 * MIB, true), (32768 * MIB, false), (8192 * MIB, true)];
        assert_eq!(vulkan_card_bytes(&rebar), 8192 * MIB);
        assert_eq!(vulkan_card_bytes(&[(16384 * MIB, false)]), 0);
        assert_eq!(card_heap(&[]), None);
    }
}
