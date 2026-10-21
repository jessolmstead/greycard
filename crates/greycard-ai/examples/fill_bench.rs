//! Time the fill model at one size, on one provider:
//! `fill_bench MODEL.onnx cpu|webgpu SIZE IMAGE [RUNS] [OUT.png]`.
//!
//! A `SIZE`×`SIZE` crop from the middle of `IMAGE` (read with the
//! `image` crate) with a hole of a quarter of its area in the middle.
//! The first run at the size is timed alone (the WebGPU provider
//! builds its pipelines for a new shape then), then `RUNS` more
//! (default 3). Prints the peak resident memory, and with `OUT.png`
//! writes the answer.

use std::time::Instant;

use greycard_ai::{Provider, runtime};
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

fn peak_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map(|kb| kb / 1024.0)
        .unwrap_or(0.0)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: fill_bench MODEL.onnx cpu|webgpu SIZE IMAGE [RUNS] [OUT.png]");
        std::process::exit(2);
    }
    let (path, provider) = (&a[1], a[2].as_str());
    let n: usize = a[3].parse().expect("SIZE");
    let runs: usize = a.get(5).map(|r| r.parse().expect("RUNS")).unwrap_or(3);

    let rgb = image::open(&a[4]).expect("the picture").to_rgb8();
    let (iw, ih) = (rgb.width() as usize, rgb.height() as usize);
    let side = n.min(iw).min(ih);
    assert_eq!(side, n, "the picture is smaller than {n}");
    let (x0, y0) = ((iw - n) / 2, (ih - n) / 2);
    let mut planes = vec![0.0f32; 3 * n * n];
    let mut mask = vec![0.0f32; n * n];
    for y in 0..n {
        for x in 0..n {
            let p = rgb.get_pixel((x0 + x) as u32, (y0 + y) as u32);
            let hole = (n / 4..3 * n / 4).contains(&x) && (n / 4..3 * n / 4).contains(&y);
            mask[y * n + x] = if hole { 1.0 } else { 0.0 };
            for c in 0..3 {
                planes[c * n * n + y * n + x] = p[c] as f32 / 255.0 * if hole { 0.0 } else { 1.0 };
            }
        }
    }

    let chosen = match provider {
        "webgpu" => Provider::WebGpu,
        "cpu" => Provider::Cpu,
        p => panic!("no provider {p}"),
    };
    let t = Instant::now();
    let mut session = runtime::open(
        std::path::Path::new(path),
        GraphOptimizationLevel::Level3,
        &[chosen],
        None,
        |_| Ok(()),
    )
    .expect("session")
    .session;
    println!(
        "{provider} {n}: session in {:.2}s",
        t.elapsed().as_secs_f64()
    );

    let run = |session: &mut Session| -> Vec<f32> {
        let image = Tensor::from_array(([1usize, 3, n, n], planes.clone())).unwrap();
        let m = Tensor::from_array(([1usize, 1, n, n], mask.clone())).unwrap();
        let out = session
            .run(ort::inputs!["image" => image, "mask" => m])
            .expect("run");
        out["output"]
            .try_extract_tensor::<f32>()
            .unwrap()
            .1
            .to_vec()
    };
    let t = Instant::now();
    let mut out = run(&mut session);
    println!(
        "{provider} {n}: first run {:.3}s",
        t.elapsed().as_secs_f64()
    );
    let mut times = Vec::new();
    for _ in 0..runs {
        let t = Instant::now();
        out = run(&mut session);
        times.push(t.elapsed().as_secs_f64());
    }
    times.sort_by(f64::total_cmp);
    println!(
        "{provider} {n}: warm {:.3}s (best of {runs}, median {:.3}s), peak resident {:.0} MB",
        times[0],
        times[times.len() / 2],
        peak_mb()
    );
    if let Some(png) = a.get(6) {
        let mut data = vec![0u8; 3 * n * n];
        for i in 0..n * n {
            for c in 0..3 {
                data[i * 3 + c] = (out[c * n * n + i].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
        image::save_buffer(
            png,
            &data,
            n as u32,
            n as u32,
            image::ExtendedColorType::Rgb8,
        )
        .expect("write");
    }
}
