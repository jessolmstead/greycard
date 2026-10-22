//! The GPU lens correction against its CPU reference, on the same
//! pictures.
//!
//! Without a GPU adapter each test says so and returns; it does not
//! pass in silence. (`cargo test` shows the line with `--nocapture`.)

use greycard_core::develop::lens::{
    ChromaticAberration, Distortion, LensCorrection, Scale, Vignetting, correct,
};
use greycard_core::image::WorkingImage;
use greycard_gpu::Context;

fn context() -> Option<Context> {
    match Context::own() {
        Ok(c) => Some(c),
        Err(e) => {
            assert!(
                std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
                "the GPU lens correction has nothing to run on ({e}) and GREYCARD_REQUIRE_GPU is set"
            );
            eprintln!("SKIPPED: the GPU lens correction has nothing to run on ({e})");
            println!("SKIPPED: the GPU lens correction has nothing to run on ({e})");
            None
        }
    }
}

/// A gradient under a checker of four-pixel squares, every channel
/// different and all of it in 0 to 1: smooth places where the resample
/// is near exact, and steps where a position's rounding shows most.
fn scene(w: usize, h: usize) -> WorkingImage {
    let mut image = WorkingImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let check = if (x / 4 + y / 4) % 2 == 0 { 0.25 } else { 0.0 };
            let k = (y * w + x) * 3;
            image.data[k] = 0.1 + 0.6 * u + check;
            image.data[k + 1] = 0.1 + 0.6 * v + check;
            image.data[k + 2] = 0.7 - 0.3 * u * v + check;
        }
    }
    image
}

/// The largest difference between the GPU's and the reference's
/// pictures, leaving out the channels whose source position the
/// reference puts within `EDGE` of the picture's border, where the two
/// can disagree on whether the light is on the picture at all (a
/// value or a zero); and how many were left out.
fn apart(cpu: &WorkingImage, gpu: &WorkingImage, c: &LensCorrection) -> (f32, usize, usize) {
    const EDGE: f32 = 1e-3;
    let (w, h) = (cpu.width as f32, cpu.height as f32);
    let scale = c.scale_for(w, h);
    let mut worst = (0f32, 0usize);
    let mut skipped = 0;
    for y in 0..cpu.height {
        for x in 0..cpu.width {
            for ch in 0..3 {
                if c.moves() {
                    let (sx, sy) = c.source_of(ch, x as f32 + 0.5, y as f32 + 0.5, w, h, scale);
                    let near = |v: f32, side: f32| v.abs() < EDGE || (v - side).abs() < EDGE;
                    if near(sx, w) || near(sy, h) {
                        skipped += 1;
                        continue;
                    }
                }
                let k = (y * cpu.width + x) * 3 + ch;
                let d = (cpu.data[k] - gpu.data[k]).abs();
                if d > worst.0 || d.is_nan() {
                    worst = (d, k);
                }
            }
        }
    }
    (worst.0, worst.1, skipped)
}

/// The tolerance of a resample of a picture `w` by `h`. The two sides
/// evaluate the same expressions in single precision, the shader's
/// quotients and roots corrected to the reference's rounding, but the
/// device may still fuse a multiply and an add, so a source position
/// lands a unit or two in the last place from the reference's, and a
/// unit there grows with the coordinate: 3e-5 pixels at 300, 2.4e-4
/// at 3000, 9.8e-4 near 8000. Across one of the checker's steps of
/// 0.25 the Catmull-Rom's slope is at most 1.25 steps a pixel, 0.31,
/// and with the corner's gain under 1.8 at most 0.56 a pixel. So the
/// bound is 0.56 times four units at the longer side, with 1e-4 as a
/// floor for the weights' and the sixteen taps' own roundings. Seen:
/// 3.7e-5 at 301 wide (bound 1e-4), 2.9e-4 at 3000 (bound 5.5e-4),
/// 1.5e-4 at 1200 tall (bound 2.7e-4), on NVIDIA and lavapipe both. The same bound at a 45 MP frame's 8192
/// is 2.2e-3; a real one's worst was 1.7e-3.
fn resample_tolerance(w: usize, h: usize) -> f32 {
    let side = w.max(h) as f32;
    let ulp = f32::from_bits(side.to_bits() + 1) - side;
    (0.56 * 4.0 * ulp).max(1e-4)
}

fn check(ctx: &Context, what: &str, image: &WorkingImage, c: &LensCorrection) -> WorkingImage {
    let (cpu, cpu_stats) = correct(image, c);
    let (gpu, gpu_stats) = ctx.correct_lens(image, c).expect("the GPU lens correction");
    assert_eq!(gpu_stats, cpu_stats, "{what}: the stats");
    assert_eq!((gpu.width, gpu.height), (cpu.width, cpu.height));
    let (worst, at, skipped) = apart(&cpu, &gpu, c);
    let total = cpu.data.len();
    let tolerance = resample_tolerance(cpu.width, cpu.height);
    println!(
        "{what}: worst {worst:.2e} (bound {tolerance:.1e}) at channel {} of pixel ({}, {}), {skipped} of {total} at the border left out",
        at % 3,
        (at / 3) % cpu.width,
        (at / 3) / cpu.width
    );
    assert!(
        worst <= tolerance,
        "{what}: {worst} apart at channel {} of pixel ({}, {}): cpu {} gpu {}",
        at % 3,
        (at / 3) % cpu.width,
        (at / 3) / cpu.width,
        cpu.data[at],
        gpu.data[at]
    );
    // The border case is a line of pixels at most, never a region.
    assert!(skipped * 100 < total, "{what}: {skipped} at the border");
    gpu
}

const CA: ChromaticAberration = ChromaticAberration {
    red: [1.0008, -0.0003, 0.0004],
    blue: [0.9991, 0.0002, -0.0003],
};

const VIGNETTING: Vignetting = Vignetting {
    k: [-0.45, 0.08, -0.05],
};

#[test]
fn distortion_ca_and_vignetting_match_the_reference() {
    let Some(ctx) = context() else { return };
    let image = scene(301, 203);
    let cases = [
        (
            "poly3 barrel, auto scale",
            LensCorrection {
                distortion: Some(Distortion::Poly3 { k1: -0.06 }),
                chromatic_aberration: Some(CA),
                vignetting: Some(VIGNETTING),
                ..Default::default()
            },
        ),
        (
            "ptlens, fixed scale with empty corners, a bigger sensor's vignetting",
            LensCorrection {
                distortion: Some(Distortion::Ptlens {
                    a: 0.012,
                    b: -0.035,
                    c: 0.01,
                }),
                chromatic_aberration: Some(CA),
                vignetting: Some(VIGNETTING),
                radius_scale: 0.9,
                vignetting_scale: 1.25,
                scale: Scale::Fixed(0.9),
            },
        ),
        (
            "poly5 pincushion, no CA",
            LensCorrection {
                distortion: Some(Distortion::Poly5 { k1: 0.03, k2: 0.01 }),
                vignetting: Some(VIGNETTING),
                ..Default::default()
            },
        ),
        (
            "acm",
            LensCorrection {
                distortion: Some(Distortion::Acm {
                    k: [-0.12, 0.03, 0.0, 0.001, -0.0007],
                    focal_scale: 0.6,
                }),
                chromatic_aberration: Some(CA),
                ..Default::default()
            },
        ),
        (
            "CA alone",
            LensCorrection {
                chromatic_aberration: Some(ChromaticAberration {
                    red: [1.004, 0.0, 0.0],
                    blue: [0.996, 0.0, 0.0],
                }),
                scale: Scale::Fixed(1.0),
                ..Default::default()
            },
        ),
    ];
    for (what, c) in cases {
        assert!(c.moves(), "{what}");
        check(&ctx, what, &image, &c);
    }
}

/// A frame a few thousand pixels across, where a unit in the last
/// place of a position is ten times what it is at three hundred, and
/// one standing up (taller than wide), where the shorter side that
/// sets the models' radius is the width.
#[test]
fn a_large_frame_and_a_portrait_one_match_the_reference() {
    let Some(ctx) = context() else { return };
    let c = LensCorrection {
        distortion: Some(Distortion::Ptlens {
            a: 0.023,
            b: -0.059,
            c: 0.049,
        }),
        chromatic_aberration: Some(CA),
        vignetting: Some(VIGNETTING),
        ..Default::default()
    };
    for (what, w, h) in [("3000 by 2000", 3000, 2000), ("800 by 1200", 800, 1200)] {
        check(&ctx, what, &scene(w, h), &c);
    }
}

#[test]
fn vignetting_alone_matches_the_reference_and_moves_nothing() {
    let Some(ctx) = context() else { return };
    let image = scene(257, 170);
    for vignetting_scale in [1.0, 0.7, 1.3] {
        let c = LensCorrection {
            vignetting: Some(VIGNETTING),
            vignetting_scale,
            ..Default::default()
        };
        let (cpu, _) = correct(&image, &c);
        let (gpu, stats) = ctx
            .correct_lens(&image, &c)
            .expect("the GPU lens correction");
        assert!(!stats.resampled);
        // One multiply by a gain computed the same way: a few units in
        // the last place of a value under 2.
        let worst = cpu
            .data
            .iter()
            .zip(&gpu.data)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        println!("vignetting alone at {vignetting_scale}: worst {worst:.2e}");
        assert!(worst <= 2e-6, "{vignetting_scale}: {worst}");
    }
}

#[test]
fn the_identity_gives_the_picture_back() {
    let Some(ctx) = context() else { return };
    let image = scene(130, 77);
    let zero = LensCorrection {
        distortion: Some(Distortion::Ptlens {
            a: 0.0,
            b: 0.0,
            c: 0.0,
        }),
        chromatic_aberration: Some(ChromaticAberration::IDENTITY),
        vignetting: Some(Vignetting { k: [0.0; 3] }),
        ..Default::default()
    };
    for c in [LensCorrection::default(), zero] {
        assert!(c.is_identity());
        let (gpu, stats) = ctx
            .correct_lens(&image, &c)
            .expect("the GPU lens correction");
        assert_eq!(gpu, image);
        assert_eq!(stats, correct(&image, &c).1);
    }
}

#[test]
fn the_bands_meet_without_a_seam() {
    let Some(ctx) = context() else { return };
    let image = scene(190, 133);
    let c = LensCorrection {
        distortion: Some(Distortion::Poly3 { k1: -0.06 }),
        chromatic_aberration: Some(CA),
        vignetting: Some(VIGNETTING),
        ..Default::default()
    };
    let (whole, _) = ctx.correct_lens(&image, &c).expect("one band");
    // Bands of one row, of seven (the last one short), and larger than
    // the picture: the same picture to the bit.
    for rows in [1, 7, 500] {
        let (banded, _) = ctx
            .correct_lens_in_bands(&image, &c, Some(rows))
            .expect("in bands");
        assert_eq!(banded, whole, "bands of {rows}");
    }
}
