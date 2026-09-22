//! The GPU chromatic aberration correction against its CPU reference,
//! on the same mosaics.
//!
//! Without a GPU adapter each test says so and returns; it does not
//! pass in silence. (`cargo test` shows the line with `--nocapture`.)

use greycard_core::develop::ca::{CaOptions, CaStats, correct_ca, fit_votes, measure_votes};
use greycard_core::raw::{CfaColor, CfaPattern};
use greycard_gpu::{Context, wgpu};

fn context() -> Option<Context> {
    match Context::own() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("SKIPPED: the GPU CA correction has nothing to run on ({e})");
            println!("SKIPPED: the GPU CA correction has nothing to run on ({e})");
            None
        }
    }
}

/// A context on a device of our own with `limits`, for driving the op
/// into a device's refusals; none without an adapter.
fn context_with(limits: wgpu::Limits) -> Option<Context> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("ca test"),
        required_limits: limits,
        ..Default::default()
    }))
    .ok()?;
    Context::from_device(&device, &queue).ok()
}

/// A smooth random texture with plenty of edges: sums of a few sine
/// gratings at different angles and frequencies, in 0.2..0.8, as the
/// reference's own tests make it.
fn texture(width: usize, height: usize) -> Vec<f32> {
    (0..width * height)
        .map(|i| {
            let (x, y) = ((i % width) as f32, (i / width) as f32);
            let v = (x * 0.11).sin() * (y * 0.07).cos()
                + (x * 0.031 + y * 0.052).sin()
                + ((x * 0.9 + y * 0.37) * 0.23).sin() * 0.5
                + ((x - y) * 0.017).cos() * 0.7;
            0.5 + 0.3 * v / 3.2
        })
        .collect()
}

/// Bilinear sample with edge clamping.
fn sample(img: &[f32], width: usize, height: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (width - 1) as f32);
    let y = y.clamp(0.0, (height - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let p = |xx: usize, yy: usize| img[yy * width + xx];
    (1.0 - fy) * ((1.0 - fx) * p(x0, y0) + fx * p(x1, y0))
        + fy * ((1.0 - fx) * p(x0, y1) + fx * p(x1, y1))
}

/// A grey texture mosaicked with lateral CA: red magnified about the
/// center by `1 + red`, blue by `1 + blue`, so the displacement grows
/// with distance from the center as it does optically. A clipped
/// patch and a dark corner exercise the guards.
fn aberrated(width: usize, height: usize, pattern: &CfaPattern, red: f32, blue: f32) -> Vec<f32> {
    let mut tex = texture(width, height);
    for y in 0..height {
        for x in 0..width {
            if (x / 90) % 4 == 1 && (y / 70) % 3 == 1 {
                tex[y * width + x] = 1.0;
            }
            if x < 30 && y < 30 {
                tex[y * width + x] *= 1e-6;
            }
        }
    }
    let (cx, cy) = ((width - 1) as f32 / 2.0, (height - 1) as f32 / 2.0);
    let mut shifted = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let scale = match pattern.color_at(y, x) {
                CfaColor::Red => 1.0 + red,
                CfaColor::Blue => 1.0 + blue,
                _ => 1.0,
            };
            let sx = cx + (x as f32 - cx) / scale;
            let sy = cy + (y as f32 - cy) / scale;
            shifted[y * width + x] = sample(&tex, width, height, sx, sy);
        }
    }
    shifted
}

/// Rounding's reach. The two paths differ by the GPU compiler's
/// fusing of a multiply and an add: 4e-6 relative at worst after one
/// pass on the synthetic mosaics, and the second pass, reading the
/// first's result through weights with the reference's EPS of 1e-5
/// in their denominators, widens that to a few 1e-5 at a handful of
/// samples. A sample over this has taken a different branch of a
/// per-pixel guard. Every test prints what it measured.
const TOLERANCE: f32 = 1e-4;
/// The relative difference's floor: a sample under this (the mosaic
/// is in 0..1, black at zero) is measured against it, not itself.
const FLOOR: f32 = 1e-5;
/// A guard flip picks another of the reference's candidates for the
/// sample, every one of which is within the correction of the sample,
/// so a step is bounded by the larger correction the two paths
/// applied there: measured at 1.0 times it (the flip between keeping
/// and correcting) and at 1.7 where the guard's factor then scales
/// the two apart, so 3 is the bound.
const STEP_RATIO: f32 = 3.0;
/// The mean over the mosaic must be rounding's whatever the steps;
/// measured under 1e-7.
const MEAN_TOLERANCE: f64 = 1e-6;
/// The votes: each tile's shifts, in pixels, between the paths.
/// Measured under 4e-5 px on the synthetic mosaics and 2.5e-4 on the
/// real frames; one tile's vote is a quotient of two sums over six
/// thousand terms, and a tile with little to vote on divides two
/// small sums whose rounding is the terms' fusing.
const VOTE_TOLERANCE: f32 = 1e-3;

/// How far two mosaics are apart: the largest absolute and relative
/// differences (relative to the reference's sample, floored at
/// [`FLOOR`]), the mean, how many samples differ by more than
/// [`TOLERANCE`] relative (a guard flip: a step), and the largest
/// step as a fraction of the correction applied at its sample.
struct Apart {
    max: f32,
    relative: f32,
    mean: f64,
    steps: usize,
    step_ratio: f32,
}

fn apart(input: &[f32], reference: &[f32], other: &[f32]) -> Apart {
    assert_eq!(reference.len(), other.len());
    let mut max = 0f32;
    let mut relative = 0f32;
    let mut sum = 0f64;
    let mut steps = 0;
    let mut step_ratio = 0f32;
    for ((&a, &b), &i) in reference.iter().zip(other).zip(input) {
        let d = (a - b).abs();
        let r = d / a.abs().max(FLOOR);
        max = max.max(d);
        relative = relative.max(r);
        sum += f64::from(d);
        if r > TOLERANCE {
            steps += 1;
            let correction = (a - i).abs().max((b - i).abs()).max(FLOOR);
            step_ratio = step_ratio.max(d / correction);
        }
    }
    Apart {
        max,
        relative,
        mean: sum / reference.len() as f64,
        steps,
        step_ratio,
    }
}

/// The two paths' first-pass votes tile by tile, and how close the
/// nearest tile came to the variance gate on each: printed, the
/// shifts held to [`VOTE_TOLERANCE`], the sentinel for a tile that
/// could not vote to equality.
fn check_votes(
    ctx: &Context,
    what: &str,
    mosaic: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) {
    let cpu = measure_votes(mosaic, width, height, pattern).expect("the reference measures");
    let gpu = ctx
        .ca_votes(mosaic, width, height, pattern)
        .expect("the GPU measures");
    assert_eq!(cpu.len(), gpu.len(), "{what}: the tile grids");
    let mut shift = 0f32;
    let mut weight = 0f32;
    for (i, (a, b)) in cpu.iter().zip(&gpu).enumerate() {
        for c in 0..2 {
            for dir in 0..2 {
                let (sa, sb) = (a.shift[c][dir], b.shift[c][dir]);
                if sa == 17.0 || sb == 17.0 {
                    assert_eq!(sa, sb, "{what}: tile {i} voted on one path only");
                } else {
                    shift = shift.max((sa - sb).abs());
                }
            }
        }
        weight = weight.max((a.weight - b.weight).abs() / a.weight.abs().max(FLOOR));
    }
    let tiles_across = greycard_core::develop::ca::origins(width).len();
    let tiles_down = greycard_core::develop::ca::origins(height).len();
    let margin = |votes: &[_]| {
        fit_votes(votes, tiles_down, tiles_across)
            .ok()
            .map(|fit| (fit.gate_margin, fit.blocks))
    };
    let (cpu_margin, gpu_margin) = (margin(&cpu), margin(&gpu));
    println!(
        "{what} votes: {} tiles, shifts within {shift:.3e} px, weights within {weight:.3e}; \
         nearest tile to the gate (a fraction of it; blocks): cpu {cpu_margin:?}, gpu {gpu_margin:?}",
        cpu.len(),
    );
    assert!(
        shift <= VOTE_TOLERANCE,
        "{what}: the votes' shifts are {shift:.3e} px apart, over {VOTE_TOLERANCE:.0e}"
    );
    // The same tiles voted (the block counts), and the nearest tile
    // sits the same distance from the gate on both paths, to a tenth
    // of that distance: a flip would show here as a block count and
    // a margin that do not agree, before it showed in the pixels.
    match (cpu_margin, gpu_margin) {
        (Some((a, ba)), Some((b, bb))) => {
            assert_eq!(ba, bb, "{what}: the voting tiles");
            assert!(
                (a - b).abs() <= 0.1 * a,
                "{what}: the gate margins are {a:.3e} and {b:.3e}"
            );
        }
        (a, b) => assert_eq!(
            a.is_some(),
            b.is_some(),
            "{what}: one path fitted, the other not"
        ),
    }
}

/// Run both paths and compare: the stats' decisions equal (corrected,
/// the voting tiles, the fit's order), the largest fitted shifts
/// within `1e-4` px, the votes within [`VOTE_TOLERANCE`], and the
/// mosaics within [`TOLERANCE`] relative at every sample but for at
/// most `steps_allowed` guard flips, each within [`STEP_RATIO`] of
/// its correction, with the mean under [`MEAN_TOLERANCE`].
#[allow(clippy::too_many_arguments)]
fn check(
    ctx: &Context,
    what: &str,
    mosaic: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    options: &CaOptions,
    steps_allowed: usize,
) -> (CaStats, CaStats) {
    let (cpu, cpu_stats) =
        correct_ca(mosaic, width, height, pattern, options).expect("the reference runs");
    let (gpu, gpu_stats) = ctx
        .correct_ca(mosaic, width, height, pattern, options)
        .expect("the GPU correction runs");
    let a = apart(mosaic, &cpu, &gpu);
    println!(
        "{what}: max {:.3e} (relative {:.3e}) mean {:.3e}, {} of {} samples step, the largest \
         {:.2} of its correction; stats cpu {cpu_stats:?} gpu {gpu_stats:?}",
        a.max,
        a.relative,
        a.mean,
        a.steps,
        cpu.len(),
        a.step_ratio
    );
    check_votes(ctx, what, mosaic, width, height, pattern);
    assert_eq!(
        cpu_stats.corrected, gpu_stats.corrected,
        "{what}: corrected"
    );
    assert_eq!(cpu_stats.blocks, gpu_stats.blocks, "{what}: voting tiles");
    assert_eq!(cpu_stats.order, gpu_stats.order, "{what}: the fit's order");
    for c in 0..2 {
        assert!(
            (cpu_stats.max_shift[c] - gpu_stats.max_shift[c]).abs() <= 1e-4,
            "{what}: max shift {} and {}",
            cpu_stats.max_shift[c],
            gpu_stats.max_shift[c]
        );
    }
    assert!(
        a.steps <= steps_allowed,
        "{what}: {} samples differ by more than {TOLERANCE:.0e} relative, over {steps_allowed}",
        a.steps
    );
    assert!(
        a.step_ratio <= STEP_RATIO,
        "{what}: a step is {:.2} of its correction, over {STEP_RATIO}",
        a.step_ratio
    );
    assert!(
        a.mean <= MEAN_TOLERANCE,
        "{what}: the mosaics are {:.3e} apart on average, over {MEAN_TOLERANCE:.0e}",
        a.mean
    );
    (cpu_stats, gpu_stats)
}

/// A synthetic mosaic with radial aberration of each color, at sizes
/// where the tiles hang off the picture's right and bottom edges by
/// varying amounts (the tile grid is 112 a step) and the half-size
/// factor planes round up. A clipped patch and a dark corner give
/// the guards something to decide, and one pixel of the 523x389 has
/// been seen to take a different branch on the second pass.
#[test]
fn a_synthetic_mosaic_matches_the_reference() {
    let Some(ctx) = context() else {
        return;
    };
    let pattern = CfaPattern::rggb();
    for (w, h) in [(400, 320), (523, 389), (672, 448)] {
        let mosaic = aberrated(w, h, &pattern, 0.005, -0.004);
        let (cpu, _) = check(
            &ctx,
            &format!("{w}x{h}"),
            &mosaic,
            w,
            h,
            &pattern,
            &CaOptions::default(),
            3,
        );
        assert!(cpu.corrected && cpu.max_shift[0] > 0.4, "{cpu:?}");
    }
}

/// Every other Bayer arrangement, one pass, and the guard off.
#[test]
fn the_other_patterns_and_options_match_the_reference() {
    let Some(ctx) = context() else {
        return;
    };
    let pattern_of = |colors: [CfaColor; 4]| CfaPattern::new(2, 2, colors.to_vec()).unwrap();
    use CfaColor::{Blue, Green, Red};
    let patterns = [
        ("BGGR", pattern_of([Blue, Green, Green, Red])),
        ("GRBG", pattern_of([Green, Red, Blue, Green])),
        ("GBRG", pattern_of([Green, Blue, Red, Green])),
    ];
    for (name, pattern) in &patterns {
        let mosaic = aberrated(401, 321, pattern, -0.004, 0.006);
        check(
            &ctx,
            name,
            &mosaic,
            401,
            321,
            pattern,
            &CaOptions::default(),
            3,
        );
    }
    let pattern = CfaPattern::rggb();
    let mosaic = aberrated(400, 320, &pattern, 0.005, -0.004);
    let one = CaOptions {
        iterations: 1,
        avoid_color_shift: true,
    };
    check(&ctx, "one pass", &mosaic, 400, 320, &pattern, &one, 0);
    let bare = CaOptions {
        iterations: 2,
        avoid_color_shift: false,
    };
    check(&ctx, "no guard", &mosaic, 400, 320, &pattern, &bare, 3);
}

/// A mosaic with nothing to correct, one too small for a fit, and a
/// flat one where no tile can vote: the same answers, including the
/// mosaic handed back untouched.
#[test]
fn the_degenerate_cases_match_the_reference() {
    let Some(ctx) = context() else {
        return;
    };
    let pattern = CfaPattern::rggb();
    let clean = aberrated(400, 320, &pattern, 0.0, 0.0);
    let (cpu, _) = check(
        &ctx,
        "unaberrated",
        &clean,
        400,
        320,
        &pattern,
        &CaOptions::default(),
        3,
    );
    assert!(cpu.max_shift[0] < 0.15, "{cpu:?}");
    let small = texture(200, 300);
    let (out, stats) = ctx
        .correct_ca(&small, 200, 300, &pattern, &CaOptions::default())
        .expect("runs");
    assert_eq!(out, small);
    assert!(!stats.corrected);
    let flat = vec![0.4f32; 400 * 320];
    let (cpu_out, cpu_stats) =
        correct_ca(&flat, 400, 320, &pattern, &CaOptions::default()).unwrap();
    let (gpu_out, gpu_stats) = ctx
        .correct_ca(&flat, 400, 320, &pattern, &CaOptions::default())
        .expect("runs");
    println!("flat: cpu {cpu_stats:?} gpu {gpu_stats:?}");
    assert_eq!(cpu_stats, gpu_stats);
    assert_eq!(cpu_out, gpu_out);
    let xtrans = CfaPattern::new(6, 6, vec![CfaColor::Green; 36]).unwrap();
    assert!(
        ctx.correct_ca(&[0.0; 36], 6, 6, &xtrans, &CaOptions::default())
            .is_err()
    );
}

/// What the device rejects comes back as an error, not a panic, and
/// the context is usable after it: a device allowed 20 workgroups a
/// dimension cannot run a 400x320's 26 (a validation error in the
/// compute pass, caught by the op's error scope), but runs a 300x300
/// afterwards and matches the reference on it. A picture the device's
/// textures cannot hold is refused before any GPU work, as
/// `Unsupported`, which a caller may fall back from without dropping
/// the context.
#[test]
fn a_device_error_is_an_error_not_a_panic() {
    let Some(ctx) = context_with(wgpu::Limits {
        max_compute_workgroups_per_dimension: 20,
        ..wgpu::Limits::default()
    }) else {
        eprintln!("SKIPPED: the GPU CA correction has nothing to run on");
        println!("SKIPPED: the GPU CA correction has nothing to run on");
        return;
    };
    let pattern = CfaPattern::rggb();
    let mosaic = aberrated(400, 320, &pattern, 0.005, -0.004);
    let err = ctx
        .correct_ca(&mosaic, 400, 320, &pattern, &CaOptions::default())
        .expect_err("too many workgroups for the device");
    println!("caught: {err}");
    assert!(matches!(err, greycard_gpu::Error::Gpu { .. }), "{err}");
    let mosaic = aberrated(300, 300, &pattern, 0.005, -0.004);
    check(
        &ctx,
        "300x300 after the error",
        &mosaic,
        300,
        300,
        &pattern,
        &CaOptions::default(),
        3,
    );

    let Some(ctx) = context_with(wgpu::Limits {
        max_texture_dimension_2d: 512,
        ..wgpu::Limits::default()
    }) else {
        return;
    };
    let mosaic = aberrated(500, 300, &pattern, 0.005, -0.004);
    let err = ctx
        .correct_ca(&mosaic, 500, 300, &pattern, &CaOptions::default())
        .expect_err("the padded green plane is wider than the device's textures");
    println!("refused: {err}");
    assert!(matches!(err, greycard_gpu::Error::Unsupported(_)), "{err}");
}

/// The balanced mosaics of the two sample frames, as `prepare` hands
/// them to the correction, under the engine's defaults. Wants
/// `GREYCARD_SAMPLES`, a directory with `5M0A3976.CR3` (24 MP) and
/// `4Z4A3525.CR3` (45 MP) in it. The guard flips allowed are the
/// measured 15 and 35 with a little headroom.
#[test]
#[ignore]
fn the_real_frames_match_the_reference() {
    let Some(dir) = std::env::var_os("GREYCARD_SAMPLES") else {
        println!("SKIPPED: GREYCARD_SAMPLES is not set");
        return;
    };
    let Some(ctx) = context() else {
        return;
    };
    for (file, steps_allowed, blocks) in [("5M0A3976.CR3", 20, 1000), ("4Z4A3525.CR3", 50, 3000)] {
        let path = std::path::Path::new(&dir).join(file);
        let frame = greycard_core::decode::decode_path(&path).expect("the sample decodes");
        let settings = greycard_core::develop::DevelopSettings {
            chromatic_aberration: None,
            ..Default::default()
        };
        let prepared = greycard_core::develop::prepare(&frame, &settings, false).expect("prepares");
        let pattern = prepared.pattern.as_ref().expect("a Bayer frame");
        let (cpu, _) = check(
            &ctx,
            file,
            &prepared.samples,
            prepared.width,
            prepared.height,
            pattern,
            &CaOptions::default(),
            steps_allowed,
        );
        assert!(cpu.corrected && cpu.blocks > blocks, "{cpu:?}");
    }
}
