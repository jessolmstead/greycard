//! The denoiser's tiling and phase handling, against a fixed network
//! (`fixtures/replicate.onnx`, made by `tools/denoise/export.py
//! --fixture`) whose answer is known: every pixel of a 2x2 RGGB block
//! becomes that block's R, first G and B.

use greycard_ai::denoise::{Denoiser, Tiling};
use greycard_ai::runtime::Provider;
use greycard_core::develop::noise::NoiseModel;
use greycard_core::raw::CfaPattern;

fn fixture() -> Denoiser {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/replicate.onnx");
    Denoiser::load(std::path::Path::new(path), &[Provider::Cpu]).expect("fixture loads on CPU")
}

/// A smooth scene, mosaicked with `pattern`.
fn scene(width: usize, height: usize, pattern: &CfaPattern) -> (Vec<f32>, Vec<f32>) {
    let rgb: Vec<f32> = (0..height)
        .flat_map(|y| {
            (0..width).flat_map(move |x| {
                let u = x as f32 / width as f32;
                let v = y as f32 / height as f32;
                [0.1 + 0.6 * u, 0.2 + 0.5 * v, 0.3 + 0.3 * u * v]
            })
        })
        .collect();
    let cfa: Vec<f32> = (0..height)
        .flat_map(|y| {
            let rgb = &rgb;
            (0..width).map(move |x| {
                let c = pattern.color_at(y, x).rgb_index().unwrap();
                rgb[(y * width + x) * 3 + c]
            })
        })
        .collect();
    (rgb, cfa)
}

fn fold(i: isize, n: usize) -> usize {
    let period = 2 * (n as isize - 1);
    let mut i = i.rem_euclid(period);
    if i >= n as isize {
        i = period - i;
    }
    i as usize
}

/// What the replicate net must give: for the sample at (x, y), the R,
/// first G and B of the RGGB block it sits in, read back off the
/// mosaic with the same mirroring the denoiser uses.
fn expected(cfa: &[f32], width: usize, height: usize, dx: usize, dy: usize) -> Vec<f32> {
    let mut out = vec![0.0; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let by = ((y as isize - dy as isize) & !1) + dy as isize;
            let bx = ((x as isize - dx as isize) & !1) + dx as isize;
            let at = |yy: isize, xx: isize| cfa[fold(yy, height) * width + fold(xx, width)];
            let o = (y * width + x) * 3;
            out[o] = at(by, bx);
            out[o + 1] = at(by, bx + 1);
            out[o + 2] = at(by + 1, bx + 1);
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
fn every_phase_and_an_odd_size_come_back_as_the_block_samples() {
    let mut net = fixture();
    let model = NoiseModel {
        a: [3e-4, 2.9e-4, 3.1e-4],
        b: [6e-8, 7e-8, 8e-8],
    };
    let rggb = CfaPattern::rggb();
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let pattern = rggb.shifted(dx, dy);
        let (width, height) = (37, 29);
        let (_, cfa) = scene(width, height, &pattern);
        let want = expected(&cfa, width, height, dx, dy);
        let got = net
            .run(&cfa, width, height, &pattern, &model, Tiling::default())
            .unwrap();
        let err = max_abs_diff(&got, &want);
        assert!(err < 1e-5, "phase ({dx}, {dy}): max error {err}");
    }
}

#[test]
fn small_tiles_give_the_same_answer_as_one() {
    let mut net = fixture();
    let model = NoiseModel {
        a: [1e-3; 3],
        b: [1e-5; 3],
    };
    let pattern = CfaPattern::rggb().shifted(1, 1);
    let (width, height) = (70, 45);
    let (_, cfa) = scene(width, height, &pattern);
    let whole = net
        .run(&cfa, width, height, &pattern, &model, Tiling::default())
        .unwrap();
    let tiled = net
        .run(
            &cfa,
            width,
            height,
            &pattern,
            &model,
            Tiling {
                tile: 32,
                margin: 4,
            },
        )
        .unwrap();
    assert!(max_abs_diff(&whole, &tiled) < 1e-6);
    assert!(max_abs_diff(&whole, &expected(&cfa, width, height, 1, 1)) < 1e-5);
}

#[test]
fn a_silent_model_still_gives_a_finite_answer() {
    let mut net = fixture();
    let pattern = CfaPattern::rggb();
    let (width, height) = (24, 20);
    let (_, cfa) = scene(width, height, &pattern);
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

#[test]
fn the_fixture_declares_its_version() {
    assert_eq!(fixture().version(), "fixture");
}
