//! Time the generative remove on WebGPU, from a model store:
//! `remove_bench IMAGE MASK [RUNS] [OUT.png]` with `GREYCARD_MODELS`
//! set to a store root holding `rorem-mixed-fp16`.
//!
//! `IMAGE` is a window of no side over 1024 and `MASK` a grey image of
//! its size, the hole where it is over 127. Each of `RUNS` removals
//! (default 2, seeds 1, 2, ...) prints its parts' times; in resident
//! mode (`GREYCARD_REMOVE_RESIDENT=1`) the first run opens the
//! sessions and the rest do not. Prints the peak GPU memory over what
//! was in use before, from `nvidia-smi` sampled in a thread, and the
//! process's peak resident memory; with `OUT.png` writes the last
//! answer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use greycard_ai::{Provider, Remove, Rgbf, Store};

fn gpu_mb() -> Option<u64> {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

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
    if a.len() < 3 {
        eprintln!("usage: remove_bench IMAGE MASK [RUNS] [OUT.png] (GREYCARD_MODELS set)");
        std::process::exit(2);
    }
    let store = Store::at(std::env::var_os("GREYCARD_MODELS").expect("GREYCARD_MODELS"));
    let runs: u64 = a.get(3).map(|r| r.parse().expect("RUNS")).unwrap_or(2);
    let rgb = image::open(&a[1]).expect("IMAGE").to_rgb8();
    let mask = image::open(&a[2]).expect("MASK").to_luma8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    assert_eq!((mask.width() as usize, mask.height() as usize), (w, h));
    let image = Rgbf::new(
        w,
        h,
        rgb.as_raw().iter().map(|&v| v as f32 / 255.0).collect(),
    );
    let hole: Vec<f32> = mask
        .as_raw()
        .iter()
        .map(|&v| if v > 127 { 1.0 } else { 0.0 })
        .collect();

    let base = gpu_mb().unwrap_or(0);
    let peak = Arc::new(AtomicU64::new(base));
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let (peak, stop) = (peak.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Some(mb) = gpu_mb() {
                    peak.fetch_max(mb, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };

    let mut remove = Remove::load(&store, &[Provider::WebGpu]).expect("load");
    println!("{w}x{h}, {:?}", remove.memory());
    let mut out = None;
    for seed in 1..=runs {
        let answer = remove.remove(&image, &hole, seed).expect("remove");
        let t = remove.times();
        println!(
            "seed {seed} at {}: loads {:.2}s, encode {:.2}s, {} UNet calls {:.2}s, decode {:.2}s, total {:.2}s; outside off by {:.4}, detail {:.4}",
            t.size,
            t.loads,
            t.encode,
            t.unet_calls,
            t.steps,
            t.decode,
            t.total,
            t.drift.off,
            t.drift.detail
        );
        out = Some(answer);
    }
    stop.store(true, Ordering::Relaxed);
    let _ = sampler.join();
    let p = peak.load(Ordering::Relaxed);
    println!(
        "GPU memory peak {p} MiB ({} MiB over the {base} MiB before); peak resident {:.0} MiB",
        p.saturating_sub(base),
        peak_mb()
    );
    if let (Some(path), Some(out)) = (a.get(4), out) {
        let data: Vec<u8> = out
            .data
            .iter()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect();
        image::save_buffer(
            path,
            &data,
            w as u32,
            h as u32,
            image::ExtendedColorType::Rgb8,
        )
        .expect("write");
    }
}
