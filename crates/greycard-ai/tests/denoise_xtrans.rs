//! The denoiser's X-Trans tiling, phase, and mirror, against a fixed
//! network (`fixtures/replicate-xtrans.onnx`, made by
//! `tools/denoise/xtrans_export.py --fixture`) whose answer is known:
//! each sample's R, G and B are the first sample of that color, in
//! reading order, in the canonical 3x3 block it sits in. At any phase
//! but the canonical one the blocks fall elsewhere, and the answer is
//! wrong.

use greycard_ai::denoise::{Denoiser, Takes, Tiling, xtrans_offset};
use greycard_ai::runtime::Provider;
use greycard_core::develop::noise::NoiseModel;
use greycard_core::raw::CfaPattern;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/replicate-xtrans.onnx"
);
const BAYER_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/replicate.onnx");

/// The fixture on the CPU, or nothing where ONNX Runtime cannot be
/// had, said on stderr.
fn fixture() -> Option<Denoiser> {
    if let Err(why) = greycard_ai::runtime::status() {
        greycard_ai::runtime::absent_in_test(why);
        return None;
    }
    Some(
        Denoiser::load(std::path::Path::new(FIXTURE), &[Provider::Cpu])
            .expect("fixture loads on CPU"),
    )
}

fn model() -> NoiseModel {
    NoiseModel {
        a: [3e-4, 2.9e-4, 3.1e-4],
        b: [6e-8, 7e-8, 8e-8],
    }
}

/// A smooth scene with a little texture, mosaicked with `pattern`.
fn mosaic(width: usize, height: usize, pattern: &CfaPattern) -> Vec<f32> {
    (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
                let rgb = [
                    0.1 + 0.6 * u,
                    0.2 + 0.5 * v,
                    0.3 + 0.3 * u * v + 0.05 * ((x * 7 + y * 3) % 5) as f32,
                ];
                rgb[pattern.color_at(y, x).rgb_index().unwrap()]
            })
        })
        .collect()
}

/// The reflection the contract asks for along one axis of `n` samples
/// whose canonical origin is at `offset`: about the samples nearest
/// each edge whose canonical coordinate is 1 mod 3.
fn fold(i: isize, n: usize, offset: usize) -> usize {
    let n = n as isize;
    if (0..n).contains(&i) {
        return i as usize;
    }
    let lo = ((offset + 1) % 3) as isize;
    let hi = (0..n).rev().find(|&a| (a - lo) % 3 == 0).unwrap();
    let span = hi - lo;
    let mut j = (i - lo).rem_euclid(2 * span);
    if j > span {
        j = 2 * span - j;
    }
    (lo + j) as usize
}

/// What the fixture must give: for each sample, the first sample of
/// each color in its canonical 3x3 block, read off the mosaic through
/// the mirror.
fn expected(cfa: &[f32], width: usize, height: usize, dx: usize, dy: usize) -> Vec<f32> {
    let canonical = CfaPattern::xtrans();
    let mut out = vec![0.0; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let i = (y as isize - dy as isize).rem_euclid(6) as usize;
            let j = (x as isize - dx as isize).rem_euclid(6) as usize;
            let (by, bx) = (3 * (i / 3), 3 * (j / 3));
            for c in 0..3 {
                let (si, sj) = (0..9)
                    .map(|k| (by + k / 3, bx + k % 3))
                    .find(|&(a, b)| canonical.color_at(a, b).rgb_index() == Some(c))
                    .unwrap();
                let sy = fold(y as isize + si as isize - i as isize, height, dy);
                let sx = fold(x as isize + sj as isize - j as isize, width, dx);
                out[(y * width + x) * 3 + c] = cfa[sy * width + sx];
            }
        }
    }
    out
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

#[test]
fn the_fixture_takes_xtrans_and_declares_its_version() {
    let Some(net) = fixture() else {
        return;
    };
    assert_eq!(net.takes(), Takes::XTrans);
    assert_eq!(net.version(), "fixture-xtrans");
    // Read off the file alone, as a load with Core ML on offer does.
    assert_eq!(
        Denoiser::peek(std::path::Path::new(FIXTURE)).unwrap(),
        Takes::XTrans
    );
    assert_eq!(
        Denoiser::peek(std::path::Path::new(BAYER_FIXTURE)).unwrap(),
        Takes::Bayer
    );
}

/// Every one of the 36 shifts (the 18 phases X-Trans has, each twice,
/// since the pattern is itself again three across and three down), on
/// a frame of odd sides that are no multiple of 6: the right blocks,
/// and the mirror at every edge.
#[test]
fn every_phase_and_an_odd_size_come_back_as_the_block_samples() {
    let Some(mut net) = fixture() else {
        return;
    };
    let canonical = CfaPattern::xtrans();
    let (width, height) = (41, 37);
    for dy in 0..6 {
        for dx in 0..6 {
            // The pattern whose canonical origin is at sample (dx, dy).
            let pattern = canonical.shifted((6 - dx) % 6, (6 - dy) % 6);
            assert!(pattern.is_xtrans());
            let (ox, oy) = xtrans_offset(&pattern).unwrap();
            assert_eq!((ox % 3, oy % 3), (dx % 3, dy % 3), "({dx}, {dy})");
            let cfa = mosaic(width, height, &pattern);
            let got = net
                .run(&cfa, width, height, &pattern, &model(), Tiling::default())
                .unwrap();
            let err = max_abs_diff(&got, &expected(&cfa, width, height, ox, oy));
            assert!(err < 1e-5, "phase ({dx}, {dy}): max error {err}");
        }
    }
}

/// Frames as small as the pattern, and ones narrower than the margin
/// many times over, mirror back and forth and still give the blocks.
#[test]
fn a_frame_smaller_than_the_margin_mirrors_back_and_forth() {
    let Some(mut net) = fixture() else {
        return;
    };
    for (width, height, sx, sy) in [(6, 6, 0, 0), (7, 11, 1, 4), (13, 6, 5, 2)] {
        let pattern = CfaPattern::xtrans().shifted(sx, sy);
        let (dx, dy) = xtrans_offset(&pattern).unwrap();
        let cfa = mosaic(width, height, &pattern);
        let got = net
            .run(&cfa, width, height, &pattern, &model(), Tiling::default())
            .unwrap();
        let err = max_abs_diff(&got, &expected(&cfa, width, height, dx, dy));
        assert!(err < 1e-5, "{width}x{height}: max error {err}");
    }
    let pattern = CfaPattern::xtrans();
    let too_small = net.run(&[0.1; 30], 6, 5, &pattern, &model(), Tiling::default());
    assert!(too_small.is_err(), "under one period down");
}

/// Small tiles, several down and across, give the answer one tile
/// gives: the tiles meet where they should, at the pattern's phase.
#[test]
fn small_tiles_give_the_same_answer_as_one() {
    let Some(mut net) = fixture() else {
        return;
    };
    let pattern = CfaPattern::xtrans().shifted(4, 1);
    let (dx, dy) = xtrans_offset(&pattern).unwrap();
    let (width, height) = (131, 101);
    let cfa = mosaic(width, height, &pattern);
    let whole = net
        .run(&cfa, width, height, &pattern, &model(), Tiling::default())
        .unwrap();
    // 40 is cut to 48 and 7 to 24: three tiles across and three down.
    let small = Tiling {
        tile: 40,
        margin: 7,
    };
    assert_eq!(small.xtrans(), (48, 24));
    let tiled = net
        .run(&cfa, width, height, &pattern, &model(), small)
        .unwrap();
    assert!(max_abs_diff(&whole, &tiled) < 1e-6);
    assert!(max_abs_diff(&whole, &expected(&cfa, width, height, dx, dy)) < 1e-5);
}

/// A crop of the frame starts elsewhere in the pattern: its answer is
/// the whole frame's over the same samples, away from the crop's edges
/// (where the whole frame reads real samples and the crop mirrors).
/// The denoiser runs on the mosaic before the frame's geometry, so a
/// turn reaches it only as the crop of the sensor it is.
#[test]
fn a_crop_answers_as_the_whole_frame_does_inside_it() {
    let Some(mut net) = fixture() else {
        return;
    };
    let pattern = CfaPattern::xtrans();
    let (width, height) = (72, 60);
    let cfa = mosaic(width, height, &pattern);
    let whole = net
        .run(&cfa, width, height, &pattern, &model(), Tiling::default())
        .unwrap();
    let (cx, cy, cw, ch) = (5, 4, 41, 37);
    let cropped: Vec<f32> = (0..ch)
        .flat_map(|y| (0..cw).map(move |x| (y, x)))
        .map(|(y, x)| cfa[(y + cy) * width + x + cx])
        .collect();
    let crop_pattern = pattern.shifted(cx, cy);
    let got = net
        .run(&cropped, cw, ch, &crop_pattern, &model(), Tiling::default())
        .unwrap();
    let mut worst = 0.0f32;
    for y in 3..ch - 3 {
        for x in 3..cw - 3 {
            for c in 0..3 {
                let a = got[(y * cw + x) * 3 + c];
                let b = whole[((y + cy) * width + x + cx) * 3 + c];
                worst = worst.max((a - b).abs());
            }
        }
    }
    assert!(worst < 1e-5, "max error {worst}");
}

#[test]
fn a_silent_model_still_gives_a_finite_answer() {
    let Some(mut net) = fixture() else {
        return;
    };
    let pattern = CfaPattern::xtrans();
    let (width, height) = (24, 18);
    let cfa = mosaic(width, height, &pattern);
    let got = net
        .run(
            &cfa,
            width,
            height,
            &pattern,
            &NoiseModel::default(),
            Tiling::default(),
        )
        .unwrap();
    assert!(got.iter().all(|v| v.is_finite()));
    assert!(max_abs_diff(&got, &expected(&cfa, width, height, 0, 0)) < 1e-4);
}

/// Each network refuses the other's mosaic, and the Bayer fixture
/// still takes its own.
#[test]
fn each_network_refuses_the_other_mosaic() {
    let Some(mut xtrans) = fixture() else {
        return;
    };
    let mut bayer = Denoiser::load(std::path::Path::new(BAYER_FIXTURE), &[Provider::Cpu]).unwrap();
    assert_eq!(bayer.takes(), Takes::Bayer);
    let (w, h) = (24, 18);
    let rggb = CfaPattern::rggb();
    let x = CfaPattern::xtrans();
    let refused = |r: greycard_ai::runtime::Result<Vec<f32>>, what: &str| {
        let e = r.unwrap_err().to_string();
        assert!(e.contains(what), "{e}");
    };
    refused(
        xtrans.run(
            &mosaic(w, h, &rggb),
            w,
            h,
            &rggb,
            &model(),
            Tiling::default(),
        ),
        "not an X-Trans pattern",
    );
    refused(
        bayer.run(&mosaic(w, h, &x), w, h, &x, &model(), Tiling::default()),
        "not a 2x2 Bayer pattern",
    );
    assert!(
        bayer
            .run(
                &mosaic(w, h, &rggb),
                w,
                h,
                &rggb,
                &model(),
                Tiling::default()
            )
            .is_ok()
    );
}

/// A graph with no input at all, a Constant for its `rgb` (made with
/// `onnx.helper`, 105 bytes): a file named by path that is no denoiser
/// is refused with an error, at the load and at the peek, not a panic.
#[test]
fn a_graph_with_no_input_is_refused_not_a_panic() {
    if fixture().is_none() {
        return;
    }
    const NO_INPUT: [u8; 105] = [
        8, 8, 58, 95, 10, 56, 18, 3, 114, 103, 98, 34, 8, 67, 111, 110, 115, 116, 97, 110, 116, 42,
        39, 10, 5, 118, 97, 108, 117, 101, 42, 27, 8, 1, 8, 3, 8, 1, 8, 1, 16, 1, 34, 12, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 66, 1, 118, 160, 1, 4, 18, 4, 110, 111, 110, 101, 98, 29, 10, 3,
        114, 103, 98, 18, 22, 10, 20, 8, 1, 18, 16, 10, 2, 8, 1, 10, 2, 8, 3, 10, 2, 8, 1, 10, 2,
        8, 1, 66, 4, 10, 0, 16, 17,
    ];
    let path = std::env::temp_dir().join(format!(
        "greycard-no-input-{}-{:?}.onnx",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, NO_INPUT).unwrap();
    let peeked = Denoiser::peek(&path);
    let loaded = Denoiser::load(&path, &[Provider::Cpu]);
    std::fs::remove_file(&path).unwrap();
    let e = peeked.unwrap_err().to_string();
    assert!(e.contains("a network with no input"), "{e}");
    assert!(loaded.is_err());
}

// Against the real X-Trans tiers: set `GREYCARD_MODELS` to a store root
// that holds them and run with `--ignored`. Without it each says so on
// stderr and passes, as the other model tests do.

/// The store at `GREYCARD_MODELS`, or nothing, said.
fn store() -> Option<greycard_ai::Store> {
    let root = std::env::var_os("GREYCARD_MODELS");
    if root.is_none() {
        eprintln!("GREYCARD_MODELS is not set: the X-Trans tier was not run");
    }
    root.map(greycard_ai::Store::at)
}

/// A noisy X-Trans mosaic `w` by `h` of a textured scene, about ISO
/// 3200 on a recent body, and the clean RGB it was made from.
fn noisy_scene(w: usize, h: usize, model: &NoiseModel) -> (Vec<f32>, Vec<f32>) {
    let pattern = CfaPattern::xtrans();
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut normal = move || {
        let mut uniform = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (u, v) = (uniform(), uniform());
        ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
    };
    let mut clean = Vec::with_capacity(w * h * 3);
    let mut noisy = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let texture = 0.5 + 0.5 * ((x as f32 * 0.05).sin() * (y as f32 * 0.04).cos());
            let rgb = [
                0.05 + 0.3 * u * texture,
                0.08 + 0.25 * v,
                0.04 + 0.2 * (1.0 - u) * texture,
            ];
            let c = pattern.color_at(y, x).rgb_index().unwrap();
            let sigma = (model.a[c] * rgb[c] + model.b[c]).sqrt();
            noisy.push((rgb[c] + sigma * normal()).max(0.0));
            clean.extend(rgb);
        }
    }
    (noisy, clean)
}

fn psnr(a: &[f32], b: &[f32]) -> f64 {
    let mse = a
        .iter()
        .zip(b)
        .map(|(x, y)| ((x - y) as f64).powi(2))
        .sum::<f64>()
        / a.len() as f64;
    // White at 0.4, about where the scene's brightest sits.
    10.0 * (0.16 / mse).log10()
}

/// One tier against the contract: it loads from the store on the
/// providers on offer and takes X-Trans; on a frame smaller than a
/// tile it denoises (its answer nearer the clean scene than the noisy
/// mosaic is by 6 dB), and on any provider but the CPU it agrees with
/// the CPU; on the CPU, a frame of four tiles at the default margin
/// of 96 gives what one tile over the whole frame gives, to the float.
fn check_tier(tier: &str) {
    let Some(store) = store() else { return };
    let model = greycard_ai::denoiser_for(tier, true).unwrap();
    assert!(store.have(model), "{} is not in the store", model.name);
    let noise = NoiseModel {
        a: [4e-4, 3e-4, 5e-4],
        b: [4e-7, 3e-7, 5e-7],
    };
    let pattern = CfaPattern::xtrans();
    let path = store.path(model, &model.files[0]);
    let mut cpu = Denoiser::load(&path, &[Provider::Cpu]).expect("loads on the CPU");
    assert_eq!(cpu.takes(), Takes::XTrans);
    let mut net = Denoiser::from_store(&store, model, &Provider::available()).expect("loads");
    println!("{tier}: {} on {}", net.version(), net.label());

    let (w, h) = (720, 576);
    let (noisy, clean) = noisy_scene(w, h, &noise);
    let tiling = net.tiling();
    let t = std::time::Instant::now();
    let got = net.run(&noisy, w, h, &pattern, &noise, tiling).unwrap();
    println!("  {w}x{h} in {:.2} s", t.elapsed().as_secs_f64());
    assert!(got.iter().all(|v| v.is_finite()));
    let own: Vec<f32> = (0..w * h)
        .map(|i| clean[i * 3 + pattern.color_at(i / w, i % w).rgb_index().unwrap()])
        .collect();
    let (before, after) = (psnr(&noisy, &own), psnr(&got, &clean));
    println!("  noisy {before:.2} dB, denoised {after:.2} dB");
    assert!(after > before + 6.0, "{before:.2} to {after:.2} dB");
    if net.provider() != Provider::Cpu {
        let on_cpu = cpu.run(&noisy, w, h, &pattern, &noise, tiling).unwrap();
        let err = max_abs_diff(&got, &on_cpu);
        println!(
            "  {} against the CPU: max difference {err:.2e}",
            net.label()
        );
        assert!(err < 1e-3, "{err}");
    }

    let (w, h) = (1300, 1100);
    let (noisy, _) = noisy_scene(w, h, &noise);
    let tiled = cpu
        .run(&noisy, w, h, &pattern, &noise, Tiling::default())
        .unwrap();
    let whole = Tiling {
        tile: 1344,
        margin: 96,
    };
    assert_eq!(whole.xtrans().0, 1344, "one tile over the frame");
    let one = cpu.run(&noisy, w, h, &pattern, &noise, whole).unwrap();
    let err = max_abs_diff(&tiled, &one);
    println!("  four tiles against one: max difference {err:.2e}");
    assert!(err < 1e-4, "a seam: {err}");
}

#[test]
#[ignore]
fn the_xtrans_fast_tier_denoises_and_tiles_without_a_seam() {
    check_tier("fast");
}

#[test]
#[ignore]
fn the_xtrans_balanced_tier_denoises_and_tiles_without_a_seam() {
    check_tier("balanced");
}

#[test]
#[ignore]
fn the_xtrans_best_tier_denoises_and_tiles_without_a_seam() {
    check_tier("best");
}
