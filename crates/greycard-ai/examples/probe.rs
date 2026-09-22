//! Run a one-input ONNX graph on the CPU and on WebGPU with the same
//! random input and print how far apart the two answers are. For
//! bisecting provider faults: `probe MODEL.onnx C H W [IN OUT]`.

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

fn run(
    path: &str,
    webgpu: bool,
    shape: [usize; 4],
    x: &[f32],
    names: (&str, &str),
) -> ort::Result<Vec<f32>> {
    let mut b = Session::builder()?.with_optimization_level(GraphOptimizationLevel::Level3)?;
    if webgpu {
        b = b.with_execution_providers([ort::ep::WebGPU::default().build().error_on_failure()])?;
    }
    let mut s = b.commit_from_file(path)?;
    if std::env::var_os("PROBE_WARM").is_some() {
        let zeros =
            Tensor::from_array(([1usize, shape[1], 32, 32], vec![0.0f32; shape[1] * 32 * 32]))?;
        s.run(ort::inputs![names.0 => zeros])?;
    }
    let input = Tensor::from_array((shape, x.to_vec()))?;
    let out = s.run(ort::inputs![names.0 => input])?;
    let (_, y) = out[names.1].try_extract_tensor::<f32>()?;
    Ok(y.to_vec())
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
        let webgpu = only == "webgpu";
        match run(path, webgpu, shape, &x, names) {
            Ok(v) => println!(
                "{path}: {} alone ran, max |y| {:.3}",
                if webgpu { "webgpu" } else { "cpu" },
                v.iter().fold(0f32, |m, v| m.max(v.abs()))
            ),
            Err(e) => println!(
                "{path}: {} alone failed: {e}",
                if webgpu { "webgpu" } else { "cpu" }
            ),
        }
        return;
    }
    let cpu = run(path, false, shape, &x, names).expect("cpu");
    let gpu = match run(path, true, shape, &x, names) {
        Ok(v) => v,
        Err(e) => {
            println!("{path}: webgpu failed: {e}");
            return;
        }
    };
    let scale = cpu.iter().fold(0f32, |m, v| m.max(v.abs()));
    let diff = cpu
        .iter()
        .zip(&gpu)
        .fold(0f32, |m, (a, b)| m.max((a - b).abs()));
    println!(
        "{path}: max |cpu| {scale:.3}  max |cpu-webgpu| {diff:.3e}  {}",
        if diff > 1e-3 * scale { "BAD" } else { "ok" }
    );
}
