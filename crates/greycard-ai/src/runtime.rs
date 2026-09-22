//! ONNX Runtime, and which device runs a model: the providers are tried
//! in order and a model settles on the first one that loads and runs,
//! since a provider can accept a graph and then fail on it (WebGPU on
//! BiRefNet: a Split with more outputs than the shader may bind).
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

use std::path::Path;
use std::sync::{Once, OnceLock};

use ort::ep::ExecutionProvider;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

mod record;

/// An execution provider, in the order they are tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    /// NVIDIA through CUDA and cuDNN; only in a build with the `cuda`
    /// feature, and only when the libraries are on the system.
    Cuda,
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
            #[cfg(feature = "webgpu")]
            Provider::WebGpu,
            Provider::Cpu,
        ]
        .to_vec()
    }

    /// Those that say they are available here.
    pub fn available() -> Vec<Provider> {
        Self::all()
            .into_iter()
            .filter(|p| p.is_available())
            .collect()
    }

    pub fn is_available(self) -> bool {
        match self {
            #[cfg(feature = "cuda")]
            Provider::Cuda => ort::ep::CUDA::default().is_available().unwrap_or(false),
            #[cfg(not(feature = "cuda"))]
            Provider::Cuda => false,
            #[cfg(feature = "webgpu")]
            Provider::WebGpu => ort::ep::WebGPU::default().is_available().unwrap_or(false),
            #[cfg(not(feature = "webgpu"))]
            Provider::WebGpu => false,
            Provider::Cpu => true,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Cuda => "CUDA",
            Provider::WebGpu => "WebGPU",
            Provider::Cpu => "CPU",
        }
    }

    fn dispatch(self) -> Option<ort::ep::ExecutionProviderDispatch> {
        match self {
            #[cfg(feature = "cuda")]
            Provider::Cuda => Some(ort::ep::CUDA::default().build().error_on_failure()),
            #[cfg(feature = "webgpu")]
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
            #[cfg(feature = "webgpu")]
            Provider::WebGpu => webgpu_adapter_identity(),
            #[cfg(not(feature = "webgpu"))]
            Provider::WebGpu => None,
            Provider::Cpu => None,
        }
    }
}

/// wgpu's own idea of the adapter it would hand a device: name,
/// backend and driver, the same fields `greycard-ui`'s viewport logs
/// off its Slint device (`main.rs`). This is a second, short-lived
/// `wgpu::Instance` used only to read that struct — Dawn, which ONNX
/// Runtime's WebGPU execution provider actually runs on, keeps its
/// own adapter choice to itself. On one GPU, which is what greycard
/// is built for today, the two agree. Probed with `Backends::PRIMARY`
/// (Vulkan, Metal, DX12): closer to what Dawn itself offers than
/// every backend wgpu knows, and it skips GL/EGL, which a process
/// that already holds Slint's own wgpu device has no business opening
/// a second context on. Asked once a process and kept: the adapter
/// does not change under a running greycard, and asking again would
/// pay this same cost once a model file, not once a launch.
#[cfg(feature = "webgpu")]
fn webgpu_adapter_identity() -> Option<String> {
    static IDENTITY: OnceLock<Option<String>> = OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            use pollster::FutureExt as _;
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::PRIMARY,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .block_on()
                .ok()?;
            let info = adapter.get_info();
            Some(format!(
                "{}|{:?}|{} {}",
                info.name, info.backend, info.driver, info.driver_info
            ))
        })
        .clone()
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
}

pub type Result<T> = std::result::Result<T, Error>;

fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        ort::init().with_name("greycard").commit();
    });
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
pub fn open(
    path: &Path,
    level: GraphOptimizationLevel,
    providers: &[Provider],
    remember: Option<Remembered<'_>>,
    mut warm: impl FnMut(&mut Session) -> ort::Result<()>,
) -> Result<Loaded> {
    init();
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

    for (i, &provider) in providers.iter().enumerate() {
        let last = i + 1 == providers.len();
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

        let outcome = try_open(path, level, provider, skip_warm, &mut warm);
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
            Ok(session) => {
                log::info!(
                    "{name} on {} in {:.2}s",
                    provider.name(),
                    t.elapsed().as_secs_f64()
                );
                return Ok(Loaded { session, provider });
            }
            Err(e) => {
                let text = e.to_string();
                let why = first_line(&text);
                log::warn!("{name} on {}: {why}", provider.name());
                tried.push(format!("{}: {why}", provider.name()));
            }
        }
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

fn try_open(
    path: &Path,
    level: GraphOptimizationLevel,
    provider: Provider,
    skip_warm: bool,
    warm: &mut impl FnMut(&mut Session) -> ort::Result<()>,
) -> ort::Result<Session> {
    let mut builder = Session::builder()?.with_optimization_level(level)?;
    if let Some(ep) = provider.dispatch() {
        builder = builder.with_execution_providers([ep])?;
    }
    let mut session = builder.commit_from_file(path)?;
    if !skip_warm {
        warm(&mut session)?;
    }
    Ok(session)
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
        assert!(Provider::available().contains(&Provider::Cpu));
    }

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
        assert!(matches!(e, Error::NoProvider { .. }));
        assert!(e.to_string().contains("CPU"));
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
