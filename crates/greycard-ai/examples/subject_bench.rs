//! Time the Subject model on one picture, on one provider, warm:
//! `subject_bench MODEL.onnx cpu|webgpu IMAGE [RUNS] [MATTE.f32]`.
//!
//! The picture is read with the `image` crate and handed to the model
//! as the editor hands it the preview (squashed to 1024², ImageNet
//! normalised). The session is built and run once before timing, then
//! `RUNS` runs (default 5) are timed, each the whole `mask` call:
//! planes, the run, the sigmoid. With `MATTE.f32` the last matte is
//! written as raw little-endian f32, 1024 × 1024, for comparing
//! providers or models (`tools/ai/birefnet_webgpu.py compare`).
//!
//! `BENCH_FORCE_CPU=a,b,c` names nodes the WebGPU provider must leave
//! to the CPU (ONNX Runtime's `forceCpuNodeNames`), which is how the
//! original model is measured on WebGPU "with the fallbacks": its
//! sixteen- and thirty-two-way Splits are handed to the CPU and the
//! rest stays on the card. `BENCH_VERBOSE=1` turns on the session's
//! verbose log, whose "Node placements" block says which node ran on
//! which provider. `BENCH_PROFILE=prefix` writes ONNX Runtime's
//! per-node profile (a Chrome trace, `prefix_<date>.json`).

use std::io::Write;
use std::time::Instant;

use greycard_ai::image::{IMAGENET_MEAN, IMAGENET_STD, Mask, Rgb8};
use greycard_ai::subject::SIZE;
use ort::logging::LogLevel;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: subject_bench MODEL.onnx cpu|webgpu IMAGE [RUNS] [MATTE.f32]");
        std::process::exit(2);
    }
    let (path, provider, picture) = (&a[1], a[2].as_str(), &a[3]);
    let runs: usize = a.get(4).map(|r| r.parse().expect("RUNS")).unwrap_or(5);
    let out = a.get(5);

    let rgb = image::open(picture).expect("the picture").to_rgb8();
    let image = Rgb8::new(rgb.width() as usize, rgb.height() as usize, rgb.into_raw());

    ort::init().with_name("subject_bench").commit();
    let t = Instant::now();
    let mut b = Session::builder()
        .unwrap()
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .unwrap();
    if std::env::var_os("BENCH_VERBOSE").is_some() {
        // Without ort's `tracing` feature its default logger says
        // nothing, so the session gets one that writes to stderr.
        b = b
            .with_logger(std::sync::Arc::new(|level, _, _, _, message: &str| {
                eprintln!("[{level:?}] {message}")
            }))
            .unwrap()
            .with_log_level(LogLevel::Verbose)
            .unwrap();
    }
    if let Ok(profile) = std::env::var("BENCH_PROFILE") {
        b = b.with_profiling(profile).unwrap();
    }
    match provider {
        "cpu" => {}
        "webgpu" => {
            if let Ok(names) = std::env::var("BENCH_FORCE_CPU") {
                // Set as a session entry: ort rc.13's
                // `with_force_cpu_node_names` prefixes the key twice
                // and ONNX Runtime never sees it. One name a line.
                b = b
                    .with_config_entry(
                        "ep.webgpuexecutionprovider.forceCpuNodeNames",
                        names.replace(',', "\n"),
                    )
                    .unwrap();
            }
            b = b
                .with_execution_providers([ort::ep::WebGPU::default().build().error_on_failure()])
                .unwrap();
        }
        other => panic!("no provider {other}"),
    }
    let mut session = b.commit_from_file(path).expect("session");
    let built = t.elapsed().as_secs_f64();

    let t = Instant::now();
    let first = mask(&mut session, &image);
    println!(
        "{path} on {provider}: session {built:.2}s, first run {:.2}s",
        t.elapsed().as_secs_f64()
    );
    let first = match first {
        Ok(m) => m,
        Err(e) => {
            println!("failed: {e}");
            std::process::exit(1);
        }
    };
    let mut times = Vec::new();
    let mut last = first;
    for _ in 0..runs {
        let t = Instant::now();
        last = mask(&mut session, &image).expect("a warm run");
        times.push(t.elapsed().as_secs_f64());
    }
    let mut sorted = times.clone();
    sorted.sort_by(f64::total_cmp);
    println!(
        "warm runs: {} ; median {:.3}s, min {:.3}s",
        times
            .iter()
            .map(|t| format!("{t:.3}"))
            .collect::<Vec<_>>()
            .join(" "),
        sorted[sorted.len() / 2],
        sorted[0]
    );
    if let Some(out) = out {
        let mut f = std::fs::File::create(out).expect("the matte file");
        for v in &last.data {
            f.write_all(&v.to_le_bytes()).unwrap();
        }
    }
}

fn mask(session: &mut Session, image: &Rgb8) -> ort::Result<Mask> {
    let planes = image.to_planes(SIZE, IMAGENET_MEAN, IMAGENET_STD);
    let input = Tensor::from_array(([1usize, 3, SIZE, SIZE], planes))?;
    let outputs = session.run(ort::inputs!["input_image" => input])?;
    let (_, logits) = outputs["output_image"].try_extract_tensor::<f32>()?;
    Ok(Mask::from_logits(SIZE, SIZE, logits))
}
