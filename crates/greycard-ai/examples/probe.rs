//! Run a one-input ONNX graph on the CPU and on a provider with the
//! same random input and print how far apart the two answers are, and
//! how long a run takes on each. For bisecting provider faults and for
//! reading where Core ML puts a graph:
//!
//!     probe MODEL.onnx C H W [IN OUT]
//!
//! `PROBE_EP` names the provider: `webgpu` (the default), or with the
//! `coreml` feature on macOS `coreml` (as the editor asks for it, every
//! compute unit), `coreml-ane` (the CPU and the Neural Engine only), or
//! `coreml-gpu` (the CPU and the GPU only). Core ML prints its compute
//! plan, a line an op saying the device it went to, through NSLog on
//! stderr (macOS 14.4 and up). `PROBE_FIX=h=608,w=608` fixes free
//! dimensions by name on both sessions, as the editor does for the
//! denoiser on Core ML. `PROBE_ONLY=cpu` or `=<provider>`
//! runs one alone; `PROBE_WARM` runs a 32x32 input first (the input's
//! own size under `PROBE_FIX`, the only one the session then takes).

use std::time::Instant;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ep {
    Cpu,
    WebGpu,
    #[cfg(feature = "coreml")]
    CoreMl(&'static str),
}

impl Ep {
    fn parse(s: &str) -> Ep {
        match s {
            "cpu" => Ep::Cpu,
            "webgpu" => Ep::WebGpu,
            #[cfg(feature = "coreml")]
            "coreml" => Ep::CoreMl("ALL"),
            #[cfg(feature = "coreml")]
            "coreml-ane" => Ep::CoreMl("CPUAndNeuralEngine"),
            #[cfg(feature = "coreml")]
            "coreml-gpu" => Ep::CoreMl("CPUAndGPU"),
            other => panic!("no provider {other} in this build"),
        }
    }

    fn name(self) -> String {
        match self {
            Ep::Cpu => "cpu".into(),
            Ep::WebGpu => "webgpu".into(),
            #[cfg(feature = "coreml")]
            Ep::CoreMl(units) => format!("coreml {units}"),
        }
    }

    fn dispatch(self) -> Option<ort::ep::ExecutionProviderDispatch> {
        match self {
            Ep::Cpu => None,
            Ep::WebGpu => Some(ort::ep::WebGPU::default().build().error_on_failure()),
            #[cfg(feature = "coreml")]
            Ep::CoreMl("ALL") => Some(greycard_ai::runtime::coreml(None, true)),
            #[cfg(feature = "coreml")]
            Ep::CoreMl(units) => {
                use ort::ep::ArbitrarilyConfigurableExecutionProvider as _;
                use ort::ep::coreml::ModelFormat;
                Some(
                    ort::ep::CoreML::default()
                        .with_model_format(ModelFormat::MLProgram)
                        .with_static_input_shapes(true)
                        .with_profile_compute_plan(true)
                        .with_arbitrary_config("MLComputeUnits", units)
                        .build()
                        .error_on_failure(),
                )
            }
        }
    }
}

/// `h=608,w=608` as pairs.
fn fixed() -> Vec<(String, i64)> {
    std::env::var("PROBE_FIX")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').expect("PROBE_FIX is name=size,...");
            (k.to_string(), v.parse().expect("a size"))
        })
        .collect()
}

fn run(
    path: &str,
    ep: Ep,
    shape: [usize; 4],
    x: &[f32],
    names: (&str, &str),
) -> ort::Result<Vec<f32>> {
    let mut b = Session::builder()?.with_optimization_level(GraphOptimizationLevel::Level3)?;
    let fixed = fixed();
    for (name, size) in &fixed {
        b = b.with_dimension_override(name, *size)?;
    }
    if let Some(dispatch) = ep.dispatch() {
        b = b.with_execution_providers([dispatch])?;
    }
    let t = Instant::now();
    let mut s = b.commit_from_file(path)?;
    println!(
        "{}: session in {:.2}s",
        ep.name(),
        t.elapsed().as_secs_f64()
    );
    for i in s.inputs() {
        println!("  in  {} {:?}", i.name(), i.dtype());
    }
    for o in s.outputs() {
        println!("  out {} {:?}", o.name(), o.dtype());
    }
    if std::env::var_os("PROBE_WARM").is_some() {
        let (h, w) = if fixed.is_empty() {
            (32, 32)
        } else {
            (shape[2], shape[3])
        };
        let zeros = Tensor::from_array(([1usize, shape[1], h, w], vec![0.0f32; shape[1] * h * w]))?;
        s.run(ort::inputs![names.0 => zeros])?;
    }
    let mut y = Vec::new();
    // The first run pays for compiling; the second is a tile's cost.
    for pass in ["first", "second"] {
        let input = Tensor::from_array((shape, x.to_vec()))?;
        let t = Instant::now();
        let out = s.run(ort::inputs![names.0 => input])?;
        let (_, v) = out[names.1].try_extract_tensor::<f32>()?;
        println!(
            "{}: {pass} run in {:.3}s",
            ep.name(),
            t.elapsed().as_secs_f64()
        );
        y = v.to_vec();
    }
    Ok(y)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (path, c, h, w) = (
        &a[1],
        a[2].parse().unwrap(),
        a[3].parse().unwrap(),
        a[4].parse().unwrap(),
    );
    let names = (
        a.get(5).map(String::as_str).unwrap_or("x"),
        a.get(6).map(String::as_str).unwrap_or("y"),
    );
    let ep = Ep::parse(&std::env::var("PROBE_EP").unwrap_or_else(|_| "webgpu".into()));
    ort::init().with_name("probe").commit();
    let mut seed = 12345u32;
    let x: Vec<f32> = (0..c * h * w)
        .map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        })
        .collect();
    let shape = [1, c, h, w];
    if let Some(only) = std::env::var_os("PROBE_ONLY") {
        let one = Ep::parse(&only.to_string_lossy());
        match run(path, one, shape, &x, names) {
            Ok(v) => println!(
                "{path}: {} alone ran, max |y| {:.3}",
                one.name(),
                v.iter().fold(0f32, |m, v| m.max(v.abs()))
            ),
            Err(e) => println!("{path}: {} alone failed: {e}", one.name()),
        }
        return;
    }
    let cpu = run(path, Ep::Cpu, shape, &x, names).expect("cpu");
    let other = match run(path, ep, shape, &x, names) {
        Ok(v) => v,
        Err(e) => {
            println!("{path}: {} failed: {e}", ep.name());
            return;
        }
    };
    let scale = cpu.iter().fold(0f32, |m, v| m.max(v.abs()));
    let diff = cpu
        .iter()
        .zip(&other)
        .fold(0f32, |m, (a, b)| m.max((a - b).abs()));
    let mean = cpu
        .iter()
        .zip(&other)
        .map(|(a, b)| (a - b).abs() as f64)
        .sum::<f64>()
        / cpu.len().max(1) as f64;
    println!(
        "{path}: max |cpu| {scale:.3}  max |cpu-{0}| {diff:.3e}  mean |cpu-{0}| {mean:.3e}  {1}",
        ep.name(),
        if diff > 1e-3 * scale { "BAD" } else { "ok" }
    );
}
