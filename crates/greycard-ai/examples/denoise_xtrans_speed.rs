//! The X-Trans tiers' speed and memory on a frame of 26 and one of 40
//! megapixels, for the real card: `denoise_xtrans_speed [TIER...]` with
//! `GREYCARD_MODELS` set to a store root holding the tiers (all three
//! when none is named). `GREYCARD_PROVIDER` names the one provider to
//! run on (`webgpu`, `cuda`, `coreml`, or `cpu`); unset, the editor's
//! own order. The frames are made here, a noisy X-Trans mosaic of a
//! textured scene at the sizes of a 26 and a 40 MP Fujifilm sensor,
//! so the numbers are the network's and the tiling's, not a decode's.
//!
//! Each tier is loaded once and runs each frame at its default tiling;
//! a line a run gives its seconds, the seconds per megapixel, the
//! card's peak over the loaded tier (the free figure before the run
//! less the least seen while it ran, read every 5 ms, on WebGPU only),
//! and the process's peak resident memory so far (Linux; the two
//! frames made here, about 0.3 GB, are in it).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use greycard_ai::runtime::{self, Provider};
use greycard_ai::{Denoiser, Store};
use greycard_core::develop::noise::NoiseModel;
use greycard_core::raw::CfaPattern;

/// The least free figure on the card seen while `run` runs.
fn least_while<T>(run: impl FnOnce() -> T) -> (T, Option<u64>) {
    let free = || runtime::webgpu_free_memory().map(|(b, _)| b);
    let least = Arc::new(AtomicU64::new(u64::MAX));
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let (least, stop) = (least.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Some(b) = free() {
                    least.fetch_min(b, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };
    let out = run();
    stop.store(true, Ordering::Relaxed);
    let _ = sampler.join();
    let l = least.load(Ordering::Relaxed);
    (out, (l != u64::MAX).then_some(l))
}

/// The process's peak resident memory, in GiB, where the system says.
fn peak_resident() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kib: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / (1u64 << 20) as f64)
}

/// A noisy X-Trans mosaic `w` by `h` of a textured scene, at about ISO
/// 3200 on a recent body: Poisson-Gaussian noise from a fixed seed.
fn frame(w: usize, h: usize, model: &NoiseModel) -> Vec<f32> {
    let pattern = CfaPattern::xtrans();
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut normal = move || {
        // Box-Muller on a 64-bit LCG: quick, and the same every run.
        let mut uniform = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (u, v) = (uniform(), uniform());
        ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
    };
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let texture = 0.5 + 0.5 * ((x as f32 * 0.21).sin() * (y as f32 * 0.17).cos());
            let rgb = [
                0.05 + 0.3 * u * texture,
                0.08 + 0.25 * v,
                0.04 + 0.2 * (1.0 - u) * texture,
            ];
            let c = pattern.color_at(y, x).rgb_index().unwrap();
            let clean = rgb[c];
            let sigma = (model.a[c] * clean + model.b[c]).sqrt();
            out.push((clean + sigma * normal()).max(0.0));
        }
    }
    out
}

fn main() {
    let named: Vec<String> = std::env::args().skip(1).collect();
    let tiers: Vec<&str> = if named.is_empty() {
        vec!["fast", "balanced", "best"]
    } else {
        named.iter().map(String::as_str).collect()
    };
    let store = Store::at(std::env::var_os("GREYCARD_MODELS").expect("GREYCARD_MODELS"));
    let providers = match std::env::var("GREYCARD_PROVIDER").ok().as_deref() {
        None => Provider::available(),
        Some(name) => {
            let p = Provider::available()
                .into_iter()
                .find(|p| p.name().eq_ignore_ascii_case(name))
                .unwrap_or_else(|| panic!("{name} is not on offer here"));
            vec![p]
        }
    };
    let model = NoiseModel {
        a: [4e-4, 3e-4, 5e-4],
        b: [4e-7, 3e-7, 5e-7],
    };
    // A 26 MP sensor (X-T4's 6240 x 4160) and a 40 MP one (X-T5's
    // 7728 x 5152).
    let sizes = [(6240usize, 4160usize), (7728, 5152)];
    let frames: Vec<Vec<f32>> = sizes.iter().map(|&(w, h)| frame(w, h, &model)).collect();
    for tier in tiers {
        let model_file = greycard_ai::denoiser_for(tier, true).expect("fast, balanced, or best");
        assert!(
            store.have(model_file),
            "{} is not in the store",
            model_file.name
        );
        let start = Instant::now();
        // By path, so nothing is written to the store's record.
        let mut net = Denoiser::load(&store.path(model_file, &model_file.files[0]), &providers)
            .expect("the tier loads");
        println!(
            "{tier}: {} on {} in {:.1} s, tile {}",
            net.version(),
            net.label(),
            start.elapsed().as_secs_f64(),
            net.tiling().tile
        );
        let tiling = net.tiling();
        for (&(w, h), samples) in sizes.iter().zip(&frames) {
            let before = runtime::webgpu_free_memory().map(|(b, _)| b);
            let start = Instant::now();
            let (rgb, least) = least_while(|| {
                net.run(samples, w, h, &CfaPattern::xtrans(), &model, tiling)
                    .expect("the tier runs")
            });
            let seconds = start.elapsed().as_secs_f64();
            assert!(rgb.iter().all(|v| v.is_finite()));
            let mp = (w * h) as f64 / 1e6;
            let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
            let card = before
                .zip(least)
                .filter(|_| net.provider() == Provider::WebGpu)
                .map(|(b, l)| gib(b.saturating_sub(l)));
            println!(
                "  {w}x{h} ({mp:.1} MP): {seconds:.2} s, {:.3} s per MP, card peak {}, process peak {}",
                seconds / mp,
                card.map_or("not known".to_string(), |p| format!("{p:.2} GiB")),
                peak_resident().map_or("not known".to_string(), |p| format!("{p:.2} GiB")),
            );
        }
    }
}
