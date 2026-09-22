//! The non-local means on its own, so the figures in notes §128 can be
//! taken again from the tree.
//!
//! The frame is synthetic and the same every run, so the checksum it
//! prints is a check that a change to the means left the answer alone;
//! it is a mean over every sample, and the last digits of it move when
//! the weights do. Timing is the means alone: no decode, no develop, no
//! encode. The thread count comes from rayon, so pin it the way the
//! notes did:
//!
//! ```text
//! RAYON_NUM_THREADS=6 taskset -c 0-5 \
//!     cargo run --release -p greycard-core --example nlm_bench -- 6000 4000 3
//! ```
//!
//! Defaults are a 24 MP frame and three runs; the notes' other size is
//! `8480 5650`.

use greycard_core::develop::nlm::{NlmOptions, denoise_nlm};
use greycard_core::develop::noise::NoiseModel;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize, fallback: usize| -> usize {
        args.get(i).map_or(fallback, |s| {
            s.parse()
                .unwrap_or_else(|_| panic!("want a number, not {s}"))
        })
    };
    let (w, h, runs) = (arg(1, 6000), arg(2, 4000), arg(3, 3));

    // A noise model of the order a raw at moderate ISO has once it is
    // demosaiced, so the transform and the weights see realistic values.
    let model = NoiseModel {
        a: [2e-3; 3],
        b: [1e-5; 3],
    };
    let mut state = 0x243F_6A88_85A3_08D3u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    // Slow gradients under fine noise: the means have something to keep
    // and something to take.
    let base: Vec<f32> = (0..w * h * 3)
        .map(|i| {
            let (x, y) = ((i / 3) % w, (i / 3) / w);
            let v = 0.2 + 0.25 * ((x as f32 * 0.01).sin() + (y as f32 * 0.013).cos());
            (v + 0.05 * (next() - 0.5)).clamp(0.001, 1.0)
        })
        .collect();

    let mut taken = Vec::with_capacity(runs);
    for _ in 0..runs {
        let mut rgb = base.clone();
        let start = Instant::now();
        let stats = denoise_nlm(&mut rgb, w, h, &model, 1.5, &NlmOptions::default());
        let seconds = start.elapsed().as_secs_f64();
        taken.push(seconds);
        let mean = rgb.iter().map(|&v| v as f64).sum::<f64>() / rgb.len() as f64;
        println!(
            "{w}x{h}  {seconds:.3} s  {} offsets, patch radius {}  checksum {mean:.9}",
            stats.patches, stats.patch_radius,
        );
    }
    taken.sort_by(f64::total_cmp);
    println!("median {:.3} s of {runs}", taken[taken.len() / 2]);
}
