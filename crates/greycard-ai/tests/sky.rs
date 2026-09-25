//! The Sky prior against the real model: set `GREYCARD_MODELS` to a
//! store root that holds it and run with `--ignored`. Without the
//! variable each test returns at once and passes, as the other model
//! tests do.
//!
//! `GREYCARD_SKY_REFERENCE` names a directory holding `eomt.in.f32`
//! (a real frame letterboxed and normalized, 1 × 3 × 640 × 640 little
//! endian) and `eomt.cpu.f32` (the class logits ONNX Runtime's CPU
//! provider gave for it, 200 × 134), and optionally
//! `eomt.masks.f32` (its mask logits, 200 × 160 × 160): the export's
//! own check, from `tools/ai/eomt_export.py`'s run. Without it the
//! comparison is skipped. `GREYCARD_SKY_PICTURE` names a picture (a
//! PNG or JPEG, say a develop of a real frame) for the CPU against
//! WebGPU comparison; without it, a picture made up here.

use greycard_ai::sky::{self, Logits, Prior};
use greycard_ai::{Provider, Rgb8, Sky, Store};

fn store() -> Option<Store> {
    std::env::var_os("GREYCARD_MODELS").map(Store::at)
}

fn read_f32(path: &std::path::Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

fn load(store: &Store, providers: &[Provider]) -> Sky {
    let t = std::time::Instant::now();
    let sky = Sky::load(store, providers).expect("load the Sky model");
    println!(
        "sky loaded on {} in {:.2}s",
        sky.provider().name(),
        t.elapsed().as_secs_f64()
    );
    sky
}

/// A blue gradient over a brown ground with a dark ridge between: a
/// picture a sky model should call half sky.
fn made_up() -> Rgb8 {
    let (w, h) = (960usize, 640usize);
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let ridge = 0.45 + 0.08 * ((x as f32 / w as f32) * 9.0).sin();
            let t = y as f32 / h as f32;
            if t < ridge {
                let k = t / ridge;
                data.extend([
                    (90.0 + 110.0 * k) as u8,
                    (140.0 + 80.0 * k) as u8,
                    (220.0 + 25.0 * k) as u8,
                ]);
            } else {
                let n = ((x * 7 + y * 13) % 17) as u8;
                data.extend([60 + n, 45 + n, 30 + n]);
            }
        }
    }
    Rgb8::new(w, h, data)
}

fn picture() -> Rgb8 {
    match std::env::var_os("GREYCARD_SKY_PICTURE") {
        Some(path) => {
            let rgb = image::open(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.to_string_lossy()))
                .to_rgb8();
            Rgb8::new(rgb.width() as usize, rgb.height() as usize, rgb.into_raw())
        }
        None => made_up(),
    }
}

/// The Rust side runs the exported graph as Python's ONNX Runtime
/// did, on the trial's own input: the class logits to 1e-4 (the
/// export matched PyTorch to 3e-5 in the trial) and, when given, the
/// mask logits.
#[test]
#[ignore]
fn the_sky_model_answers_as_the_reference_on_the_cpu() {
    let Some(store) = store() else { return };
    let Some(dir) = std::env::var_os("GREYCARD_SKY_REFERENCE").map(std::path::PathBuf::from) else {
        println!("no GREYCARD_SKY_REFERENCE: nothing to compare against");
        return;
    };
    let input = read_f32(&dir.join("eomt.in.f32"));
    let class = read_f32(&dir.join("eomt.cpu.f32"));
    let mut sky = load(&store, &[Provider::Cpu]);
    let t = std::time::Instant::now();
    let logits = sky.logits(input.clone()).expect("run");
    println!("CPU run {:.3}s", t.elapsed().as_secs_f64());
    let d = max_diff(&logits.class, &class);
    println!("class logits against the reference: max {d:.2e}");
    assert!(d < 1e-4, "class logits differ by {d}");
    let masks = dir.join("eomt.masks.f32");
    if masks.exists() {
        let d = max_diff(&logits.masks, &read_f32(&masks));
        println!("mask logits against the reference: max {d:.2e}");
        assert!(d < 1e-3, "mask logits differ by {d}");
    }
    if Provider::available().contains(&Provider::WebGpu) {
        let mut gpu = load(&store, &[Provider::WebGpu]);
        assert_eq!(gpu.provider(), Provider::WebGpu);
        let on_gpu = gpu.logits(input).expect("run on WebGPU");
        let (dc, dm) = (
            max_diff(&on_gpu.class, &logits.class),
            max_diff(&on_gpu.masks, &logits.masks),
        );
        println!("WebGPU against the CPU: class max {dc:.2e}, masks max {dm:.2e}");
        assert!(dc < 1e-3 && dm < 1e-2, "class {dc}, masks {dm}");
    }
}

/// The share of pixels whose label differs, and the largest and mean
/// difference of the soft sky maps.
fn prior_difference(a: &Prior, b: &Prior) -> (f32, f32, f32) {
    let n = a.labels.len() as f32;
    let flips = a
        .labels
        .iter()
        .zip(&b.labels)
        .filter(|(x, y)| x != y)
        .count() as f32
        / n;
    let max = max_diff(&a.sky, &b.sky);
    let mean = a
        .sky
        .iter()
        .zip(&b.sky)
        .map(|(x, y)| (x - y).abs())
        .sum::<f32>()
        / n;
    (flips, max, mean)
}

/// The prior on WebGPU is the prior on the CPU: timed warm, and the
/// labels and the soft map compared.
#[test]
#[ignore]
fn the_sky_prior_is_the_same_on_the_cpu_and_on_webgpu() {
    let Some(store) = store() else { return };
    let picture = picture();
    let time = |sky: &mut Sky| -> (Prior, Logits) {
        let boxed = sky::letterbox(&picture);
        let content = (boxed.width, boxed.height);
        let logits = sky.logits(boxed.planes).expect("run");
        let mut runs = Vec::new();
        let mut prior = None;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let boxed = sky::letterbox(&picture);
            let t_box = t.elapsed().as_secs_f64();
            let logits = sky.logits(boxed.planes).expect("run");
            let t_run = t.elapsed().as_secs_f64() - t_box;
            let p = Prior::label(&logits, content, picture.width, picture.height);
            let t_all = t.elapsed().as_secs_f64();
            runs.push(format!(
                "{t_box:.3}+{t_run:.3}+{:.3}={t_all:.3}",
                t_all - t_box - t_run
            ));
            prior = Some(p);
        }
        println!(
            "{}: letterbox+model+labels, warm: {}",
            sky.provider().name(),
            runs.join(" ")
        );
        (prior.expect("five runs"), logits)
    };
    let (cpu, cpu_logits) = time(&mut load(&store, &[Provider::Cpu]));
    println!(
        "CPU: {:.1}% labeled sky, gate {:?}",
        cpu.sky_area() * 100.0,
        sky::gate(&cpu)
    );
    if !Provider::available().contains(&Provider::WebGpu) {
        println!("no WebGPU here; the CPU run is the whole test");
        return;
    }
    let mut gpu = load(&store, &[Provider::WebGpu]);
    assert_eq!(gpu.provider(), Provider::WebGpu);
    let (on_gpu, gpu_logits) = time(&mut gpu);
    let dc = max_diff(&cpu_logits.class, &gpu_logits.class);
    let dm = max_diff(&cpu_logits.masks, &gpu_logits.masks);
    let (flips, max, mean) = prior_difference(&cpu, &on_gpu);
    println!(
        "WebGPU against the CPU: class logits max {dc:.2e}, mask logits max {dm:.2e}; \
         labels differ on {:.4}% of pixels, sky map max {max:.2e} mean {mean:.2e}",
        flips * 100.0
    );
    assert_eq!(sky::gate(&cpu), sky::gate(&on_gpu));
    assert!(flips < 1e-3, "{flips} of the labels differ");
    assert!(max < 0.05 && mean < 1e-3, "sky map max {max} mean {mean}");
}

/// A made-up blue sky over a ridge is found, and seeds land in it.
#[test]
#[ignore]
fn the_sky_prior_finds_a_made_up_sky() {
    let Some(store) = store() else { return };
    let image = made_up();
    let mut sky = load(&store, &Provider::available());
    let prior = sky.prior(&image).expect("prior");
    let area = prior.sky_area();
    println!(
        "made-up sky: {:.1}% labeled sky, gate {:?}",
        area * 100.0,
        sky::gate(&prior)
    );
    assert!(area > 0.25 && area < 0.6, "{area}");
    assert_eq!(sky::gate(&prior), Ok(()));
    let seeds = sky::seeds(&prior);
    assert!(!seeds.positive.is_empty());
    assert!(seeds.positive.iter().all(|p| p[1] < 0.5), "{seeds:?}");
}

/// What the gate has to go on, frame by frame, for tuning it:
/// `GREYCARD_SKY_PREVIEWS` names a folder of previews (the editor's,
/// as the Sky acceptance test in greycard-ui writes them), and each
/// gets a line: the sky queries kept and their class probabilities,
/// the labeled area, the areas over 0.8, 0.9 and 0.95, the peak.
#[test]
#[ignore]
fn what_the_gate_sees() {
    let Some(store) = store() else { return };
    let Some(dir) = std::env::var_os("GREYCARD_SKY_PREVIEWS").map(std::path::PathBuf::from) else {
        return;
    };
    let mut sky = load(&store, &Provider::available());
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|e| e == "jpg" || e == "png")
                && !p.file_stem().unwrap().to_string_lossy().ends_with("-sky")
        })
        .collect();
    paths.sort();
    for path in paths {
        let rgb = image::open(&path).unwrap().to_rgb8();
        let picture = Rgb8::new(rgb.width() as usize, rgb.height() as usize, rgb.into_raw());
        let boxed = sky::letterbox(&picture);
        let content = (boxed.width, boxed.height);
        let logits = sky.logits(boxed.planes).unwrap();
        let prior = Prior::label(&logits, content, picture.width, picture.height);
        // Each query whose best class is sky: its probability, and how
        // much of the frame its mask (over a half) covers.
        let mut queries = Vec::new();
        for q in 0..sky::QUERIES {
            let l = &logits.class[q * sky::CLASSES..(q + 1) * sky::CLASSES];
            let max = l.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let e: Vec<f32> = l.iter().map(|v| (v - max).exp()).collect();
            let t: f32 = e.iter().sum();
            let p = e[sky::SKY_CLASS as usize] / t;
            let best = e[..sky::CLASSES - 1]
                .iter()
                .enumerate()
                .fold((0, 0.0f32), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
            if best.0 == sky::SKY_CLASS as usize && p > 0.2 {
                let m = &logits.masks[q * sky::MASK_SIDE * sky::MASK_SIDE
                    ..(q + 1) * sky::MASK_SIDE * sky::MASK_SIDE];
                let area = m.iter().filter(|&&v| v > 0.0).count() as f32 / m.len() as f32;
                queries.push(format!("{p:.3}@{:.1}%", area * 100.0));
            }
        }
        let n = prior.sky.len() as f32;
        let over = |t: f32| prior.sky.iter().filter(|&&v| v > t).count() as f32 / n * 100.0;
        let peak = prior.sky.iter().copied().fold(0.0f32, f32::max);
        let (w, h) = (prior.width, prior.height);
        let r = (0.01 * w.max(h) as f32).round() as usize;
        let core: Vec<bool> = prior.sky.iter().map(|&v| v > 0.8).collect();
        let core = sky::erode(&core, w, h, r).iter().filter(|&&v| v).count() as f32 / n * 100.0;
        let core9: Vec<bool> = prior.sky.iter().map(|&v| v > 0.9).collect();
        let core9 = sky::erode(&core9, w, h, r).iter().filter(|&&v| v).count() as f32 / n * 100.0;
        println!(
            "{}: labeled {:.2}% | >0.8 {:.2}% >0.9 {:.2}% >0.95 {:.2}% peak {peak:.3} | core0.8 {core:.2}% core0.9 {core9:.2}% | gate {:?} | sky queries {}",
            path.file_stem().unwrap().to_string_lossy(),
            prior.sky_area() * 100.0,
            over(0.8),
            over(0.9),
            over(0.95),
            sky::gate(&prior),
            queries.join(" ")
        );
    }
}
