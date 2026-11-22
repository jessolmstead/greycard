//! The learned denoiser on a real frame at two tiles, to see what the
//! smaller tile costs: `denoise_tiles RAW [TIER] [TILE_A TILE_B]` with
//! `GREYCARD_MODELS` set to a store root holding the tier (`best`,
//! the default, `balanced`, or `fast`), its network for the raw's
//! mosaic, Bayer or X-Trans. Runs the tier on WebGPU at
//! `TILE_A` and `TILE_B` (1024 and 512 by default) and prints each
//! run's time, the two answers' largest difference and PSNR in the
//! samples' units (white at 1), and what the editor would choose here.
//! Each run's peak on the card is the free figure before it less the
//! least seen while it ran, read every 5 ms as the editor's guard reads
//! the card; the first run's is the tile's own, ONNX Runtime's buffer
//! cache serving the second's in part, so one tile a process
//! (`denoise_tiles RAW best 384 384`) measures one tile.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use greycard_ai::denoise::{Tiling, choose_tile};
use greycard_ai::runtime::{self, Provider};
use greycard_ai::{Denoiser, Store};
use greycard_core::develop::{DevelopSettings, prepare};

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

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 2 {
        eprintln!("usage: denoise_tiles RAW [TIER] [TILE_A TILE_B] (GREYCARD_MODELS set)");
        std::process::exit(2);
    }
    let tier = a.get(2).map(String::as_str).unwrap_or("best");
    let tiles: [usize; 2] = [
        a.get(3).map_or(1024, |t| t.parse().expect("a tile")),
        a.get(4).map_or(512, |t| t.parse().expect("a tile")),
    ];
    let store = Store::at(std::env::var_os("GREYCARD_MODELS").expect("GREYCARD_MODELS"));

    let unified = runtime::webgpu_unified();
    let card = runtime::webgpu_memory();
    println!(
        "adapter: unified {unified:?}, memory {:?} GiB; the editor would choose {:?}",
        card.map(|b| b as f64 / (1u64 << 30) as f64),
        choose_tile(
            Provider::WebGpu,
            unified,
            card,
            (unified == Some(true))
                .then(greycard_ai::ram::physical)
                .flatten(),
            None
        )
    );

    let frame = greycard_core::decode::decode_path(&a[1]).expect("the raw decodes");
    let prepared = prepare(&frame, &DevelopSettings::default(), true).expect("prepared");
    let pattern = prepared.pattern.clone().expect("a mosaic");
    // The tier's network for the frame's mosaic.
    let model =
        greycard_ai::denoiser_for(tier, pattern.is_xtrans()).expect("best, balanced, or fast");
    let noise = prepared.noise_model().expect("a noise model");
    // Opened by path, not through the store, so nothing is written to
    // the store's record of providers.
    assert!(store.have(model), "{} is not in the store", model.name);
    let mut net = Denoiser::load(&store.path(model, &model.files[0]), &[Provider::WebGpu])
        .expect("the tier loads");
    println!(
        "{} on {}, {}x{}",
        net.version(),
        net.provider().name(),
        prepared.width,
        prepared.height
    );
    let answers: Vec<Vec<f32>> = tiles
        .iter()
        .map(|&tile| {
            let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
            let before = runtime::webgpu_free_memory().map(|(b, _)| b);
            let start = Instant::now();
            let (rgb, least) = least_while(|| {
                net.run(
                    &prepared.samples,
                    prepared.width,
                    prepared.height,
                    &pattern,
                    &noise,
                    Tiling {
                        tile,
                        ..Default::default()
                    },
                )
                .expect("the tier runs")
            });
            let peak = before.zip(least).map(|(b, l)| gib(b.saturating_sub(l)));
            println!(
                "tile {tile}: {:.1} s, the card's peak over the loaded tier {}",
                start.elapsed().as_secs_f64(),
                peak.map_or("not known".to_string(), |p| format!("{p:.2} GiB"))
            );
            rgb
        })
        .collect();
    let (mut max, mut sq) = (0.0f32, 0.0f64);
    for (x, y) in answers[0].iter().zip(&answers[1]) {
        let d = (x - y).abs();
        max = max.max(d);
        sq += (d as f64).powi(2);
    }
    let mse = sq / answers[0].len() as f64;
    let peak = answers[0].iter().fold(0.0f32, |m, &v| m.max(v)).max(1.0) as f64;
    println!(
        "tile {} against {}: max difference {max:.3e}, RMS {:.3e}, PSNR {:.1} dB (peak {peak:.2})",
        tiles[1],
        tiles[0],
        mse.sqrt(),
        10.0 * (peak * peak / mse).log10()
    );
}
