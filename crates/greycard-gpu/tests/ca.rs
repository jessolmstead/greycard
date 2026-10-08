//! The GPU chromatic aberration correction against its CPU reference,
//! on the same mosaics.
//!
//! Without a GPU adapter each test says so and returns; it does not
//! pass in silence. (`cargo test` shows the line with `--nocapture`.)

mod common;

use common::{Rng, Sweep};
use greycard_core::develop::ca::{
    self as reference, BlockVote, CaOptions, CaStats, Coefficients, Conditioning, EPS2, Fit, NoFit,
    avoid_color_shift, correct_ca, fit_votes, measure_coefficients, measure_coefficients_edged,
    measure_votes, resample_with, scaled, vote_of,
};
use greycard_core::raw::{CfaColor, CfaPattern};
use greycard_gpu::{Context, wgpu};

fn context() -> Option<Context> {
    match Context::own() {
        Ok(c) => Some(c),
        Err(e) => {
            require_gpu(&format!(
                "the GPU CA correction has nothing to run on ({e})"
            ));
            eprintln!("SKIPPED: the GPU CA correction has nothing to run on ({e})");
            println!("SKIPPED: the GPU CA correction has nothing to run on ({e})");
            None
        }
    }
}

/// Under CI the skip is a failure: the runner installs a software
/// adapter so that these tests run, and a run that finds none has
/// lost it, not earned a pass.
fn require_gpu(why: &str) {
    assert!(
        std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
        "{why} and GREYCARD_REQUIRE_GPU is set"
    );
}

/// A context on a device of our own with `limits`, for driving the op
/// into a device's refusals; none without an adapter.
fn context_with(limits: wgpu::Limits) -> Option<Context> {
    let adapter = pollster::block_on(greycard_gpu::instance().request_adapter(
        &wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        },
    ))
    .inspect_err(|e| require_gpu(&format!("the CA limits test has no adapter ({e})")))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("ca test"),
        required_limits: limits,
        ..Default::default()
    }))
    .inspect_err(|e| require_gpu(&format!("the CA limits test got no device ({e})")))
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
    aberrate(&tex, width, height, pattern, red, blue)
}

/// `tex` mosaicked with lateral CA, as [`aberrated`] makes it.
fn aberrate(
    tex: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    red: f32,
    blue: f32,
) -> Vec<f32> {
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
            shifted[y * width + x] = sample(tex, width, height, sx, sy);
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
/// step as a fraction of the correction applied at its sample (which
/// the triangle inequality holds to two, so it is printed, not held).
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
/// most `steps_allowed` guard flips, with the mean under
/// [`MEAN_TOLERANCE`].
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
    // One sample of this mosaic takes the other side of a guard on
    // RADV, which fuses differently; none does on NVIDIA or lavapipe.
    let one_pass_steps = usize::from(ctx.device().adapter_info().name.contains("RADV"));
    check(
        &ctx,
        "one pass",
        &mosaic,
        400,
        320,
        &pattern,
        &one,
        one_pass_steps,
    );
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

/// On the device as in the reference: a flat mosaic, and a patch
/// inside a textured one where green is flat and red and blue are not
/// (a highlight whose green channel clipped first), cast no vote on the
/// patch's tiles and have exactly zero gradient energy there, so the
/// GPU's fit is made of the votes the reference's is. Without the
/// reference's exact green there, a GPU's fused rounding left residue
/// energy over the gate and cast votes of whole pixels on such tiles.
#[test]
fn a_flat_patch_casts_no_vote_on_the_device() {
    let Some(ctx) = context() else {
        return;
    };
    let pattern = CfaPattern::rggb();
    let (w, h) = (600, 520);
    let level = 0.8137;
    let flat = vec![level; w * h];
    let votes = ctx
        .ca_votes(&flat, w, h, &pattern)
        .expect("the GPU measures");
    assert!(
        votes
            .iter()
            .all(|v| v.shift.iter().flatten().all(|&s| s == 17.0)),
        "a flat mosaic votes"
    );
    let (out, stats) = ctx
        .correct_ca(&flat, w, h, &pattern, &CaOptions::default())
        .expect("the GPU correction runs");
    assert!(!stats.corrected && out == flat, "{stats:?}");
    let mut mosaic = aberrated(w, h, &pattern, 0.005, -0.004);
    let (x0, y0, x1, y1) = (100, 90, 480, 430);
    for y in y0..y1 {
        for x in x0..x1 {
            if pattern.color_at(y, x) == CfaColor::Green {
                mosaic[y * w + x] = level;
            }
        }
    }
    let sums = ctx
        .ca_coefficients(&mosaic, w, h, &pattern)
        .expect("the GPU measures");
    let gpu: Vec<BlockVote> = sums.iter().map(|&c| vote_of(c)).collect();
    let cpu = measure_votes(&mosaic, w, h, &pattern).expect("the reference measures");
    let (tops, lefts) = (reference::origins(h), reference::origins(w));
    let mut inside = 0;
    for (i, (g, c)) in gpu.iter().zip(&cpu).enumerate() {
        let (t, l) = (tops[i / lefts.len()], lefts[i % lefts.len()]);
        let within = |o: isize, lo: usize, hi: usize| {
            o >= lo as isize && o + reference::TS as isize <= hi as isize
        };
        if within(t, y0, y1) && within(l, x0, x1) {
            inside += 1;
            let energy = scaled(sums[i]);
            for (dir, sums) in energy.iter().enumerate() {
                for (c, &e) in sums[2].iter().enumerate() {
                    assert_eq!(e, 0.0, "tile {i}'s energy ({c}, {dir}) on the GPU");
                }
            }
            assert!(
                g.shift.iter().flatten().all(|&s| s == 17.0),
                "tile {i} inside the patch voted {:?} on the GPU",
                g.shift
            );
            assert_eq!(g.shift, c.shift, "tile {i}");
        }
    }
    assert_eq!(inside, 6, "the tiles inside the patch");
}

/// The real frames: a pass's fit within this many pixels of the
/// reference's, every tile's shift. Over the 31 Bayer frames of the
/// sample folder, from six makers' cameras (two more files do not
/// decode), within 5.5e-6 px (RADV) and 4.3e-6 (NVIDIA); 1.2e-5.
const REAL_FIT_PIXELS: f64 = 1.2e-5;
/// The real frames: the share of a whole run's samples that may differ
/// by more than [`TOLERANCE`] relative (guard flips). Over the sample
/// frames, 0.0055% of a frame nearly half black, on both devices, and
/// no vote cast on one path only; 0.012%.
const REAL_STEP_SHARE: f64 = 1.2e-4;

/// The balanced mosaic of every Bayer raw in `GREYCARD_SAMPLES`, as
/// `prepare` hands it to the correction, under the editor's defaults.
/// Each pass on the reference's trajectory: the same tiles vote on both
/// paths, and the fits make the same decisions with every tile's shift
/// within [`REAL_FIT_PIXELS`]; and the whole run's samples are within
/// [`TOLERANCE`] of the reference's at all but [`REAL_STEP_SHARE`] of
/// them. A file that does not decode, or is not a Bayer mosaic, is
/// named and passed over. Run it in release: the reference takes
/// minutes a frame unoptimized.
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
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("read GREYCARD_SAMPLES")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| greycard_core::decode::is_raw_path(p))
        .collect();
    files.sort();
    let options = CaOptions::default();
    let one = CaOptions {
        iterations: 1,
        avoid_color_shift: false,
    };
    let (mut checked, mut fit_most, mut step_most) = (0usize, 0f64, 0f64);
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let frame = match greycard_core::decode::decode_path(&path) {
            Ok(f) => f,
            Err(e) => {
                println!("{name}: passed over, it does not decode ({e})");
                continue;
            }
        };
        let settings = greycard_core::develop::DevelopSettings {
            chromatic_aberration: None,
            ..Default::default()
        };
        let prepared = greycard_core::develop::prepare(&frame, &settings, false).expect("prepares");
        let Some(pattern) = prepared.pattern.as_ref().filter(|p| reference::is_bayer(p)) else {
            println!("{name}: passed over, not a Bayer mosaic");
            continue;
        };
        let (w, h, mosaic) = (prepared.width, prepared.height, &prepared.samples);
        let (across, down) = (reference::origins(w).len(), reference::origins(h).len());
        let mut x = mosaic.clone();
        for pass in 0..options.iterations {
            let what = format!("{name} pass {pass}");
            let cpu = measure_votes(&x, w, h, pattern).expect("the reference measures");
            let gpu = ctx.ca_votes(&x, w, h, pattern).expect("the GPU measures");
            let one_sided = cpu
                .iter()
                .zip(&gpu)
                .flat_map(|(a, b)| a.shift.iter().flatten().zip(b.shift.iter().flatten()))
                .filter(|&(&a, &b)| (a == 17.0) != (b == 17.0))
                .count();
            let (cf, gf) = (fit_votes(&cpu, down, across), fit_votes(&gpu, down, across));
            let apart = match (&cf, &gf) {
                (Ok(a), Ok(b)) => fits_apart(a, b, down, across),
                _ => 0.0,
            };
            fit_most = fit_most.max(apart);
            println!(
                "{what}: {w}x{h}, fits {:?} and {:?} within {apart:.3e} px, {one_sided} votes on \
                 one path only",
                decisions(&cf),
                decisions(&gf)
            );
            assert_eq!(one_sided, 0, "{what}: votes cast on one path only");
            assert_eq!(
                decisions(&cf),
                decisions(&gf),
                "{what}: the fits' decisions"
            );
            assert!(
                apart <= REAL_FIT_PIXELS,
                "{what}: the fits are {apart:.3e} px apart, over {REAL_FIT_PIXELS:.0e}"
            );
            if cf.is_err() {
                break;
            }
            x = correct_ca(&x, w, h, pattern, &one)
                .expect("the reference runs")
                .0;
        }
        let (cpu, cpu_stats) =
            correct_ca(mosaic, w, h, pattern, &options).expect("the reference runs");
        let (gpu, gpu_stats) = ctx
            .correct_ca(mosaic, w, h, pattern, &options)
            .expect("the GPU correction runs");
        let a = apart(mosaic, &cpu, &gpu);
        let steps = a.steps as f64 / cpu.len() as f64;
        step_most = step_most.max(steps);
        println!(
            "{name}: max {:.3e} (relative {:.3e}), {} samples ({:.4}%) past {TOLERANCE:.0e}; stats \
             cpu {cpu_stats:?} gpu {gpu_stats:?}",
            a.max,
            a.relative,
            a.steps,
            100.0 * steps
        );
        assert_eq!(
            cpu_stats.corrected, gpu_stats.corrected,
            "{name}: corrected"
        );
        assert_eq!(cpu_stats.blocks, gpu_stats.blocks, "{name}: voting tiles");
        assert_eq!(cpu_stats.order, gpu_stats.order, "{name}: the fit's order");
        assert!(
            steps <= REAL_STEP_SHARE,
            "{name}: {:.4}% of the samples past {TOLERANCE:.0e}, over {:.4}%",
            100.0 * steps,
            100.0 * REAL_STEP_SHARE
        );
        checked += 1;
    }
    println!(
        "real frames: {checked} checked, fits within {fit_most:.3e} px, at most {:.4}% of a \
         frame's samples past {TOLERANCE:.0e}",
        100.0 * step_most
    );
    assert!(checked > 0, "no Bayer raw in GREYCARD_SAMPLES");
}

/// The kinds of scene the random sweep mosaics, each with its numbers
/// drawn from the case's own seed.
#[derive(Debug, Clone, Copy)]
enum Scene {
    /// Sums of sine gratings, as [`texture`] makes them, at drawn
    /// frequencies, phases, level and contrast.
    Gratings,
    /// Hard-edged bars and checks: the steepest gradients a tile votes
    /// on, and the overshoot the guards are for.
    Bars,
    /// Mostly flat, with a few soft discs: few tiles able to vote, so
    /// the reduced fit or none.
    Sparse,
    /// Noise and nothing else: votes on nothing.
    Noise,
}

/// The fixed Bayer arrangements the correction takes.
const PATTERNS: [[CfaColor; 4]; 4] = {
    use CfaColor::{Blue as B, Green as G, Red as R};
    [[R, G, G, B], [B, G, G, R], [G, R, B, G], [G, B, R, G]]
};

/// A scene of `kind` before the aberration, its numbers from `seed`,
/// in 0..1 with clipped patches at one, a dark corner and noise drawn
/// on any kind.
fn drawn_texture(kind: Scene, seed: u64, w: usize, h: usize) -> Vec<f32> {
    let mut r = Rng::new(seed, 0);
    let mut noise = Rng::new(seed, 1);
    let mut rand = move || noise.unit() - 0.5;
    let level = r.range(0.05, 0.7);
    let contrast = r.range(0.02, 0.3);
    let mut tex: Vec<f32> = match kind {
        Scene::Gratings => {
            let f: Vec<f32> = (0..6).map(|_| r.range(0.3, 2.0)).collect();
            let phase: Vec<f32> = (0..4).map(|_| r.range(0.0, 6.3)).collect();
            (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32, (i / w) as f32);
                    let v = (x * 0.11 * f[0] + phase[0]).sin() * (y * 0.07 * f[1]).cos()
                        + (x * 0.031 * f[2] + y * 0.052 * f[3] + phase[1]).sin()
                        + ((x * 0.9 + y * 0.37) * 0.23 * f[4] + phase[2]).sin() * 0.5
                        + ((x - y) * 0.017 * f[5] + phase[3]).cos() * 0.7;
                    level + contrast * v / 3.2
                })
                .collect()
        }
        Scene::Bars => {
            let (across, down) = (r.between(3, 40), r.between(3, 40));
            let checks = r.chance(0.5);
            (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    let on = if checks {
                        (x / across + y / down) % 2 == 0
                    } else {
                        (x / across) % 2 == 0
                    };
                    if on { level + contrast } else { level }
                })
                .collect()
        }
        Scene::Sparse => {
            let discs: Vec<(f32, f32, f32)> = (0..r.between(1, 8))
                .map(|_| {
                    (
                        r.range(0.0, w as f32),
                        r.range(0.0, h as f32),
                        r.range(4.0, 60.0),
                    )
                })
                .collect();
            (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32, (i / w) as f32);
                    let lift: f32 = discs
                        .iter()
                        .map(|&(cx, cy, s)| {
                            let d2 = (x - cx) * (x - cx) + (y - cy) * (y - cy);
                            (-d2 / (2.0 * s * s)).exp()
                        })
                        .sum();
                    level + contrast * lift
                })
                .collect()
        }
        Scene::Noise => vec![level; w * h],
    };
    // A raw frame always has some noise; a quarter of the scenes have
    // none, which puts the reference's guards on exact ties.
    let grain = match kind {
        Scene::Noise => r.range(0.001, 0.05),
        _ if r.chance(0.75) => r.range(0.001, 0.01),
        _ => 0.0,
    };
    let patches = r
        .chance(0.5)
        .then(|| (r.between(30, 120), r.between(30, 120)));
    let corner = r
        .chance(0.5)
        .then(|| (r.between(10, 80), r.pick(&[1e-6, 0.0])));
    for (i, v) in tex.iter_mut().enumerate() {
        let (x, y) = (i % w, i / w);
        *v = (*v + grain * rand()).max(0.0);
        if let Some((a, d)) = patches
            && (x / a) % 4 == 1
            && (y / d) % 3 == 1
        {
            *v = 1.0;
        }
        if let Some((side, by)) = corner
            && x < side
            && y < side
        {
            *v *= by;
        }
    }
    tex
}

/// One random case: the mosaic and every input the correction reads.
#[derive(Debug)]
struct Drawn {
    width: usize,
    height: usize,
    pattern: usize,
    scene: Scene,
    scene_seed: u64,
    red: f32,
    blue: f32,
    options: CaOptions,
}

fn draw(r: &mut Rng) -> Drawn {
    // A side on either side of the smallest the correction takes, of
    // a step in the tile count (a tile more from 112k - 7), or
    // anywhere modest.
    let side = |r: &mut Rng| match r.below(10) {
        0 => r.pick(&[
            reference::MIN_SIDE - 1,
            reference::MIN_SIDE,
            reference::MIN_SIDE + 1,
        ]),
        1 | 2 => {
            reference::STEP * r.between(3, 7) - r.pick(&[reference::BORDER, reference::BORDER - 1])
        }
        _ => r.between(reference::MIN_SIDE, 760),
    };
    let (width, height) = match r.below(20) {
        // Under the smallest side: left as it is.
        0 => {
            let small = r.between(2, reference::MIN_SIDE - 1);
            let other = side(r);
            if r.chance(0.5) {
                (small, other)
            } else {
                (other, small)
            }
        }
        // Many tiles, the full fit.
        1 => (r.between(900, 1300), r.between(700, 1000)),
        _ => (side(r), side(r)),
    };
    let pattern = r.below(PATTERNS.len());
    let scene = r.pick(&[
        Scene::Gratings,
        Scene::Gratings,
        Scene::Bars,
        Scene::Sparse,
        Scene::Noise,
    ]);
    let scene_seed = r.next();
    // Magnifications either way that shift a corner by up to about
    // five pixels on the largest mosaics, past the correction's limit.
    let (red, blue) = (r.signed(-0.01, 0.01), r.signed(-0.01, 0.01));
    let options = CaOptions {
        iterations: r.pick(&[0, 1, 1, 1, 2, 2, 2, 2, 3, 4]),
        avoid_color_shift: r.chance(0.75),
    };
    Drawn {
        width,
        height,
        pattern,
        scene,
        scene_seed,
        red,
        blue,
        options,
    }
}

/// The random sweep's checks take the correction apart where the
/// GPU's work is: the tiles' votes on each pass's input, the resample
/// on the fit the GPU makes of them, its passes chained, and the
/// color-shift guard.
///
/// The votes are where the two paths' rounding meets the correction's
/// cliffs. A vote is a quotient of two sums over a tile, and on some
/// tiles the reference itself moves it by more than a thousandth of a
/// pixel for an ulp of input, or casts it or not by rounding alone.
/// Those tiles are told from the reference alone, by running it again
/// on its input nudged by a few ulps and with its greens past the
/// picture's edge moved by one (see [`EDGE_ULPS`]), so that no fault on
/// the GPU's side can make a tile one of them. Their votes are counted,
/// not held to a value, and a run caps them; every other vote is held
/// to a bound.
///
/// No one pass's fit is held. It is the reference's own `fit_votes` on
/// both paths, so the GPU's part in it is its votes, their shifts and
/// weights, each checked above; and the fit is made of medians and a
/// gate over those votes, which can turn a difference under any vote's
/// floor into pixels. Over seed 255's case 7 a single reference vote
/// moved by a millionth of itself moves the reference's fit by 6.2 px,
/// and an ulp's nudge of the mosaic moves it 3.2 px with three more
/// tiles voting; a bound on the GPU's fit there, or on its fit with the
/// counted votes put back, measures that and not the GPU. How far the
/// fits are apart is printed, and the share of a run's passes that fit
/// off the reference is capped as a backstop ([`FIT_OFF_SHARE_RUN`]).
/// The real frames' test holds the fit, where the frames make it within
/// a few millionths of a pixel.
///
/// Every cap below is from seeds 1 to 200 and 0xfeedface,
/// 0x123456789, 0xc0ffee and 0xabcdef01 on NVIDIA and RADV (7,758 and
/// 7,754 passes), with at least twice the most seen; seeds 321 to 470
/// were held out and pass on both. (Seeds 81 to 200 were held out
/// first, against caps from the first 80, and failed at the edge
/// votes the probes now tell and at the reach's share of a pass; seeds
/// 201 to 320 then failed at a bound on the fit, since dropped, and
/// again at the share of a pass, now capped over a run only.)
/// Lavapipe, which rounds as the reference does, counts no vote.
///
/// The GPU's rounding of the values a resampled sample is made from,
/// as a share of their [`Conditioning::scale`]: its green at a red or
/// blue site is a weighted mean of four greens and the shifted green a
/// bilinear read of those, a few roundings each, fused or not. Over
/// the tuning seeds the least that passes every sample, by the reach
/// or at a tie, is 1.9 ulps (RADV); sixteen, at which the shares below
/// were measured.
const VALUE_ROUNDING: f32 = 16.0 * f32::EPSILON / 2.0;

/// The most a sample may take of its [`Conditioning::gain`] reach, as a
/// share of its [`Conditioning::scale`]. The gain is the resample's
/// slope taken at its worst, where the weighted branch's weights have
/// the reference's EPS in their denominators and reach tens of
/// thousands; what the GPU's rounding does there is far less. Over the
/// tuning seeds the samples using it were off by up to 2.7e-3 of their
/// scale (RADV); 6e-3.
const REACH_SCALE: f32 = 6e-3;

/// Of a run's red and blue samples, the share that may use the gain
/// reach. Over the tuning seeds, 0.0056% (NVIDIA); 0.012%. A pass's own
/// share is printed, not capped: it went from 0.021% over the first 80
/// seeds to 0.073% over 200 and 0.22% at seed 259, a few hundred
/// samples of a small mosaic, while a run's stayed under 0.006%.
const REACH_SHARE_RUN: f64 = 1.2e-4;

/// Of a pass's red and blue samples, the share that may take the
/// other side of a guard at a tie (one of its
/// [`Conditioning::flips`] with a margin within twice the rounding):
/// one case and the whole run. Each such sample is the reference's own
/// answer with that guard taken the other way. Over the tuning seeds,
/// 1.3% of a pass and 0.025% of a run (RADV); 3% and 0.06%.
const TIE_SHARE: f64 = 0.03;
const TIE_SHARE_RUN: f64 = 6e-4;

/// The graded nudges: each sample of a pass's input moved by this
/// many parts in 2^23, up and down by a fixed pattern of signs.
const NUDGE_ULPS: [f32; 3] = [1.0, 4.0, 16.0];

/// A vote within this of the reference's (of the vote, or of a pixel
/// if it is smaller) is the reference's: its quotient's own rounding.
const VOTE_FLOOR: f32 = 1e-6;

/// A tile is chaotic in a color and direction when the graded nudges
/// move the reference's own vote there by this many pixels an ulp of
/// input, or more.
const CHAOTIC_PER_ULP: f32 = 1e-3;

/// A tile is at the gate in a color and direction when the reference's
/// vote there is cast under some of the graded nudges and not under
/// others, or when they move its gradient energy (the vote's divisor,
/// which the gate holds against `EPS2`) from a nonzero value by more
/// than this factor either way, so that the energy is rounding
/// residues.
const RESIDUE_MOVE: f32 = 2.0;

/// The edge probes: every green the reference interpolates past the
/// picture's edge moved by this many parts in 2^23, up and then down
/// ([`measure_coefficients_edged`]). A reflected green mirrors one
/// inside, made of the same samples, so no nudge of the mosaic can part
/// the two; where the reference's rounding leaves them exactly equal
/// and a GPU's an ulp apart, the high-pass weighting a site's terms is
/// exactly zero on one path and a residue on the other, and a residue
/// times a clipped edge's gradient moves a vote by a hundredth of a
/// pixel or casts one: seeds 46, 148, 160 and 169 did, each on a tile
/// at the edge, on NVIDIA and RADV alike. A vote the probes move by
/// [`CHAOTIC_PER_ULP`] an ulp or more, or cast or not, or whose energy
/// they move as the nudges' rule has it, is an edge vote and counted
/// as the chaotic are. The probes take an ulp each way because the
/// residue is a triangle inequality's, `|a - b| + |c - a| - |c - b|`,
/// which cancels exactly for one sign of the error; reordering the
/// reflected green's sum instead, as another implementation might,
/// parted the votes of seed 160 only. Over the tuning seeds the probes
/// alone (the nudges finding nothing) classify 0.13% of the reference's
/// votes on tiles at the edge, 642 of 507,471, and none of the 321,223
/// inside.
const EDGE_ULPS: f32 = 1.0;

/// Every vote on a tile neither chaotic nor at the gate must be cast on
/// both paths and be within this many pixels of the reference's, and
/// this share of it. Over the tuning seeds, every such vote was cast
/// on both; those under a pixel were within 2.1e-3 px, and the largest
/// past one 3.9e-3 px off a vote of 3.4 px (NVIDIA); the bound is twice
/// each.
const VOTE_PIXELS: f32 = 4.3e-3;
const VOTE_SHARE: f32 = 1.1e-3;

/// Of a pass's votes, the share that may be chaotic or at the gate and
/// off the reference's (past [`VOTE_FLOOR`] or cast on one path only):
/// one case and the whole run. Over the tuning seeds, 25.9% of a pass
/// (seed 158's case 18, a mosaic of bars) and 2.4% of a run (NVIDIA);
/// 55% and 5%.
const COUNTED_SHARE: f64 = 0.55;
const COUNTED_SHARE_RUN: f64 = 0.05;

/// The steady votes' tiles' weights (see [`check_pass_votes`]): within
/// this share of the reference's. Over the tuning seeds, 8.0e-3 (seed
/// 81's case 12, RADV; 5.3e-3 on NVIDIA); 1.6e-2. A fault in a weight's
/// terms, which moves no shift, moves it by a sixth or more.
const WEIGHT_SHARE: f32 = 1.6e-2;

/// A pass's fit is off the reference's past this many pixels at some
/// tile, or with other decisions.
const FIT_OFF_PIXELS: f64 = 1e-3;

/// Of a run's passes, the share whose fit may be off the reference's.
/// Not a bound on any one fit, which the votes' rounding can move by
/// pixels (see the sweep's doc), but a backstop for what moves every
/// fit a little: a fault in the votes' weights, which the fit takes
/// and no single vote's shift shows, put half to all of a run's passes
/// off. Over the tuning seeds, 6 of 44 passes (13.6%, seed 140, RADV;
/// 11.1% on NVIDIA); 28%.
const FIT_OFF_SHARE_RUN: f64 = 0.28;

/// What a run's checks left loose, summed over its cases.
#[derive(Default)]
struct Loose {
    samples: usize,
    reached: usize,
    ties: usize,
    votes: usize,
    counted: usize,
    passes: usize,
    fits_off: usize,
    edge: [usize; 2],
    edge_probed: [usize; 2],
}

/// `mosaic` with every sample moved by `ulps` parts in 2^23, up or
/// down by a fixed pattern of signs times `sign`.
fn nudged(mosaic: &[f32], ulps: f32, sign: f32) -> Vec<f32> {
    let mut r = Rng::new(0x6e75_6467, 0);
    mosaic
        .iter()
        .map(|&v| {
            let s = if r.chance(0.5) { sign } else { -sign };
            v * (1.0 + s * ulps * f32::EPSILON)
        })
        .collect()
}

/// What the reference's own nudged runs say of a tile's vote in a
/// color and direction.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Class {
    /// Neither of the below: the vote is held to a bound.
    Steady,
    /// The nudges move the vote by [`CHAOTIC_PER_ULP`] or more.
    Chaotic,
    /// The vote is at the gate (see [`RESIDUE_MOVE`]).
    Gate,
    /// Neither under the nudges, but one or the other under the edge
    /// probes (see [`EDGE_ULPS`]).
    Edge,
}

/// The reference's runs on a pass's input with something moved, each
/// with the ulps it moved and its votes and scaled sums.
type Runs = Vec<(f32, Vec<BlockVote>, Vec<Coefficients>)>;

/// What `runs` say of tile `i`'s vote in color `c` and direction `dir`
/// against the reference's (`sa`, of energy `e`): at the gate, chaotic
/// or steady; and how far an ulp moves it, how far its energy moves.
fn class_under(runs: &Runs, i: usize, c: usize, dir: usize, sa: f32, e: f32) -> (Class, f32, f32) {
    let across = runs
        .iter()
        .any(|(_, n, _)| (n[i].shift[c][dir] == 17.0) != (sa == 17.0));
    let moved = runs
        .iter()
        .map(|(_, _, n)| {
            let f = n[i][dir][2][c];
            if e == 0.0 {
                1.0
            } else {
                f.max(e) / f.min(e).max(f32::MIN_POSITIVE)
            }
        })
        .fold(1f32, f32::max);
    let per_ulp = runs
        .iter()
        .map(|(ulps, n, _)| {
            let s = n[i].shift[c][dir];
            if s == 17.0 || sa == 17.0 {
                0.0
            } else {
                (s - sa).abs() / ulps
            }
        })
        .fold(0f32, f32::max);
    let class = if across || moved > RESIDUE_MOVE {
        Class::Gate
    } else if per_ulp >= CHAOTIC_PER_ULP {
        Class::Chaotic
    } else {
        Class::Steady
    };
    (class, per_ulp, moved)
}

/// Whether tile `i` of a `width` by `height` mosaic is at the
/// picture's edge: its interior within a border of an edge, where its
/// votes read greens past it.
fn at_edge(i: usize, width: usize, height: usize) -> bool {
    let (tops, lefts) = (reference::origins(height), reference::origins(width));
    let border = reference::BORDER as isize;
    let near = |o: isize, n: usize| {
        let (start, end) = (
            o + border,
            (o + border + reference::STEP as isize).min(n as isize),
        );
        start < border || end > n as isize - border
    };
    near(tops[i / lefts.len()], height) || near(lefts[i % lefts.len()], width)
}

/// One pass's votes: the reference's, how many, how many counted, and
/// what the edge probes classified.
struct PassVotes {
    reference: Vec<BlockVote>,
    votes: usize,
    counted: usize,
    /// The reference's votes on tiles at the edge and inside, and of
    /// those the ones the edge probes alone classify.
    edge: [usize; 2],
    edge_probed: [usize; 2],
}

/// One pass's votes, tile by tile, against the reference's on the same
/// input. A vote within [`VOTE_FLOOR`] is the reference's. Past that,
/// on a chaotic tile, one at the gate or one at the edge it is
/// counted, capped at [`COUNTED_SHARE`]; on any other tile it must be
/// cast on both paths and within [`VOTE_PIXELS`] and [`VOTE_SHARE`] of
/// the reference's. A tile's weight is held to [`WEIGHT_SHARE`] where
/// the vote it is taken from is steady.
#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
fn check_pass_votes(
    what: &str,
    input: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    coefficients: &[Coefficients],
    gpu: &[BlockVote],
) -> PassVotes {
    assert_eq!(coefficients.len(), gpu.len(), "{what}: the tile grids");
    let cpu: Vec<BlockVote> = coefficients.iter().map(|&c| vote_of(c)).collect();
    // The graded nudges' votes and gradient energies.
    let graded: Runs = NUDGE_ULPS
        .iter()
        .flat_map(|&ulps| [(ulps, 1.0f32), (ulps, -1.0)])
        .map(|(ulps, sign)| {
            let sums = measure_coefficients(&nudged(input, ulps, sign), width, height, pattern)
                .expect("measures");
            let votes = sums.iter().map(|&c| vote_of(c)).collect();
            (ulps, votes, sums.into_iter().map(scaled).collect())
        })
        .collect();
    let edged: Runs = [EDGE_ULPS, -EDGE_ULPS]
        .into_iter()
        .map(|ulps| {
            let sums =
                measure_coefficients_edged(input, width, height, pattern, ulps * f32::EPSILON)
                    .expect("measures");
            let votes = sums.iter().map(|&c| vote_of(c)).collect();
            (ulps.abs(), votes, sums.into_iter().map(scaled).collect())
        })
        .collect();
    let class = |i: usize, c: usize, dir: usize| {
        let sa = cpu[i].shift[c][dir];
        let e = scaled(coefficients[i])[dir][2][c];
        let (class, per_ulp, moved) = class_under(&graded, i, c, dir, sa, e);
        if class != Class::Steady {
            return (class, per_ulp, moved, e / EPS2);
        }
        let (edge, edge_per_ulp, edge_moved) = class_under(&edged, i, c, dir, sa, e);
        let class = if edge == Class::Steady {
            Class::Steady
        } else {
            Class::Edge
        };
        (
            class,
            per_ulp.max(edge_per_ulp),
            moved.max(edge_moved),
            e / EPS2,
        )
    };
    // How many of the reference's votes the probes alone classify, on
    // tiles at the edge and inside.
    let (mut edge, mut edge_probed) = ([0usize; 2], [0usize; 2]);
    for (i, a) in cpu.iter().enumerate() {
        let k = usize::from(!at_edge(i, width, height));
        let energy = scaled(coefficients[i]);
        for c in 0..2 {
            for dir in 0..2 {
                let sa = a.shift[c][dir];
                if sa != 17.0 {
                    edge[k] += 1;
                    let (probed, _, _) = class_under(&edged, i, c, dir, sa, energy[dir][2][c]);
                    edge_probed[k] += usize::from(probed != Class::Steady);
                }
            }
        }
    }
    let (mut votes, mut counted) = (0usize, 0usize);
    // The steady votes' largest distance from the reference's, as a
    // share of their bound, and in pixels.
    let (mut worst, mut worst_pixels) = (0f32, 0f32);
    // And their tiles' weights' largest, relative.
    let mut worst_weight = 0f32;
    for (i, (a, b)) in cpu.iter().zip(gpu).enumerate() {
        // The tile's weight, the fit's for its votes, is its last color
        // and direction's energy over its color difference: held where
        // that vote is steady, as its shift is.
        let (wa, wb) = (a.weight, b.weight);
        if wa != wb && class(i, 1, 1).0 == Class::Steady {
            let off = (wa - wb).abs() / wa.abs().max(f32::MIN_POSITIVE);
            assert!(
                off <= WEIGHT_SHARE,
                "{what}: tile {i}'s weight is {wb} on the GPU, {wa} on the reference, {off:.3e} \
                 of it off, over {WEIGHT_SHARE:.0e}"
            );
            worst_weight = worst_weight.max(off);
        }
        for c in 0..2 {
            for dir in 0..2 {
                votes += 1;
                let (sa, sb) = (a.shift[c][dir], b.shift[c][dir]);
                let cast = (sa != 17.0, sb != 17.0);
                if cast == (false, false) {
                    continue;
                }
                let d = (sa - sb).abs();
                if cast == (true, true) && d <= VOTE_FLOOR * sa.abs().max(1.0) {
                    continue;
                }
                let (class, per_ulp, moved, energy) = class(i, c, dir);
                if class == Class::Steady {
                    assert!(
                        cast == (true, true),
                        "{what}: tile {i}'s vote ({c}, {dir}) is {sb} on the GPU, {sa} on the \
                         reference, which the nudges move {per_ulp:.3e} px an ulp, its energy \
                         {energy:.3e} of the gate and {moved:.3e} times"
                    );
                    let bound = VOTE_PIXELS + VOTE_SHARE * sa.abs();
                    assert!(
                        d <= bound,
                        "{what}: tile {i}'s vote ({c}, {dir}) is {sb} on the GPU, {sa} on the \
                         reference, {d:.3e} px off, over {bound:.3e}; the nudges move it \
                         {per_ulp:.3e} px an ulp"
                    );
                    worst = worst.max(d / bound);
                    worst_pixels = worst_pixels.max(d);
                    continue;
                }
                counted += 1;
            }
        }
    }
    let share = counted as f64 / votes.max(1) as f64;
    println!(
        "{what} votes: {votes}, {counted} ({:.2}%) counted; the steady within {worst_pixels:.3e} \
         px, {worst:.3} of their bound, their weights within {worst_weight:.3e}; the edge probes \
         alone classify {} of {} cast at the edge, {} of {} inside",
        100.0 * share,
        edge_probed[0],
        edge[0],
        edge_probed[1],
        edge[1]
    );
    assert!(
        share <= COUNTED_SHARE,
        "{what}: {:.2}% of the votes chaotic or at the gate and off, over {:.2}%",
        100.0 * share,
        100.0 * COUNTED_SHARE
    );
    PassVotes {
        reference: cpu,
        votes,
        counted,
        edge,
        edge_probed,
    }
}

/// The fit's decisions: whether it was made, of how many tiles, of
/// which order, or why not.
fn decisions(fit: &Result<Fit, NoFit>) -> Result<(usize, usize), NoFit> {
    fit.as_ref().map(|f| (f.blocks, f.order)).map_err(|e| *e)
}

/// How far apart two fits' shifts are, in pixels, over every tile.
fn fits_apart(a: &Fit, b: &Fit, down: usize, across: usize) -> f64 {
    let mut most = 0f64;
    for v in 1..=down {
        for h in 1..=across {
            let (p, q) = (a.shift_at(v, h), b.shift_at(v, h));
            for c in 0..2 {
                for dir in 0..2 {
                    most = most.max((p[c][dir] - q[c][dir]).abs());
                }
            }
        }
    }
    most
}

/// One pass's fit against the reference's, printed: whether the GPU's
/// is off it (past [`FIT_OFF_PIXELS`], or with other decisions).
fn report_fit(
    what: &str,
    gpu: &Result<Fit, NoFit>,
    votes: &PassVotes,
    down: usize,
    across: usize,
) -> bool {
    let reference = fit_votes(&votes.reference, down, across);
    let apart = match (gpu, &reference) {
        (Ok(a), Ok(b)) => fits_apart(a, b, down, across),
        _ => 0.0,
    };
    let (want, own) = (decisions(&reference), decisions(gpu));
    println!(
        "{what} fit: {want:?} on the reference, {own:?} on the GPU within {apart:.3e} px, with \
         {} votes counted",
        votes.counted
    );
    own != want || apart > FIT_OFF_PIXELS
}

/// How far the GPU's sample may be from the reference's `a`, whose
/// gain is `gain` on values of `scale`: the tolerance, and the gain's
/// reach, capped.
fn sample_reach(a: f32, gain: f32, scale: f32) -> (f32, f32) {
    let base = TOLERANCE * a.abs().max(FLOOR);
    let reach = (2.0 * gain * VALUE_ROUNDING * scale).min(REACH_SCALE * scale);
    (base, reach)
}

/// One pass's resample: the GPU's against the reference's on the same
/// input and the same fit. A sample is within [`TOLERANCE`] of the
/// reference's (relative, floored at [`FLOOR`]), or past it by up to
/// twice its [`Conditioning::gain`] times [`VALUE_ROUNDING`] of its
/// scale where the resample amplifies its inputs' rounding (the
/// weighted branch's weights have the reference's EPS in their
/// denominators), that reach capped at [`REACH_SCALE`] of the scale
/// (and the samples using it at [`REACH_SHARE_RUN`] of a run); past
/// both, one of its guards must be at a tie (its margin within twice
/// that rounding) and
/// the sample the reference's with that guard taken the other way, to
/// the same tolerance, and such samples are capped at [`TIE_SHARE`].
/// Green is exact. Returns the samples resampled, those that used the
/// reach, and the ties.
fn check_resample(
    what: &str,
    reference: &[f32],
    cond: &[Conditioning],
    gpu: &[f32],
) -> (usize, usize, usize) {
    let (mut reached, mut ties, mut moved, mut bad) = (0usize, 0usize, 0usize, 0usize);
    // What the reached samples used: ulps of their scale against their
    // gain, and the most as a share of the scale.
    let (mut ulps, mut of_scale) = (0f32, 0f32);
    // The least [`VALUE_ROUNDING`], in ulps, that would pass every
    // sample: by the reach, or at a tie.
    let mut needed = 0f32;
    let mut worst: Option<(usize, f32, f32, f32, Conditioning)> = None;
    for (i, ((&a, &b), c)) in reference.iter().zip(gpu).zip(cond).enumerate() {
        if c.scale > 0.0 {
            moved += 1;
        }
        let d = (a - b).abs();
        let (base, reach) = sample_reach(a, c.gain, c.scale);
        if d <= base {
            continue;
        }
        let rounding = VALUE_ROUNDING * c.scale;
        let unit = f32::EPSILON * c.scale;
        let by_reach = |d: f32, base: f32, gain: f32| {
            if d - base > REACH_SCALE * c.scale {
                f32::INFINITY
            } else {
                ((d - base) / (gain * unit)).max(0.0)
            }
        };
        let need = c
            .flips
            .iter()
            .map(|f| {
                let base = TOLERANCE * f.value.abs().max(FLOOR);
                (f.margin / unit).max(by_reach((f.value.max(0.0) - b).abs(), base, f.gain))
            })
            .fold(by_reach(d, base, c.gain), f32::min);
        needed = needed.max(need);
        if d <= base + reach {
            reached += 1;
            ulps = ulps.max((d - base) / (2.0 * c.gain * c.scale * f32::EPSILON / 2.0));
            of_scale = of_scale.max((d - base) / c.scale);
        } else if c.flips.iter().any(|f| {
            let (base, reach) = sample_reach(f.value, f.gain, c.scale);
            f.margin < 2.0 * rounding && (f.value.max(0.0) - b).abs() <= base + reach
        }) {
            ties += 1;
        } else {
            bad += 1;
            if worst.is_none_or(|w| d / base > w.1) {
                worst = Some((i, d / base, a, b, *c));
            }
        }
    }
    let (reach_share, tie_share) = (
        reached as f64 / moved.max(1) as f64,
        ties as f64 / moved.max(1) as f64,
    );
    println!(
        "{what} resample: {moved} samples, {reached} ({:.4}%) using the reach (up to {ulps:.2} \
         ulps, {of_scale:.3e} of the scale), {ties} ({:.4}%) at a tie, {bad} past both; the \
         rounding needed {needed:.2} ulps",
        100.0 * reach_share,
        100.0 * tie_share
    );
    assert!(
        bad == 0,
        "{what}: {bad} samples off the reference on the same fit, at no tie; the worst {worst:?}"
    );
    assert!(
        tie_share <= TIE_SHARE,
        "{what}: {:.4}% of the samples took the other side of a tie, over {:.4}%",
        100.0 * tie_share,
        100.0 * TIE_SHARE
    );
    (moved, reached, ties)
}

/// The correction taken apart, pass by pass on the GPU's own
/// trajectory: the GPU's single pass against the reference's resample
/// on the fit the GPU's votes give (the stats that fit's), then the
/// votes on the pass's input and the fit made of them (after the
/// resample, so that a vote off does not hide what the pass did); then
/// the GPU's whole run against its passes chained, and with the guard
/// against the reference's guard on that chain.
#[allow(clippy::too_many_arguments)]
fn check_decomposed(
    ctx: &Context,
    what: &str,
    mosaic: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    options: &CaOptions,
    loose: &mut Loose,
) {
    let (across, down) = (
        reference::origins(width).len(),
        reference::origins(height).len(),
    );
    let one = CaOptions {
        iterations: 1,
        avoid_color_shift: false,
    };
    let mut x = mosaic.to_vec();
    let mut chain = CaStats::default();
    for pass in 0..options.iterations.max(1) {
        let what = format!("{what} pass {pass}");
        let gpu_votes = ctx
            .ca_votes(&x, width, height, pattern)
            .expect("the GPU measures");
        let (out, stats) = ctx
            .correct_ca(&x, width, height, pattern, &one)
            .expect("the GPU pass runs");
        let fit = fit_votes(&gpu_votes, down, across);
        if let Ok(fit) = &fit {
            assert_eq!(
                (stats.corrected, stats.blocks, stats.order, stats.max_shift),
                (true, fit.blocks, fit.order, fit.max_shift),
                "{what}: the stats of the GPU's own fit"
            );
            let (cpu, cond) =
                resample_with(&x, width, height, pattern, fit).expect("the reference resamples");
            let (moved, reached, ties) = check_resample(&what, &cpu, &cond, &out);
            loose.samples += moved;
            loose.reached += reached;
            loose.ties += ties;
        }
        let coefficients =
            measure_coefficients(&x, width, height, pattern).expect("the reference measures");
        let votes = check_pass_votes(&what, &x, width, height, pattern, &coefficients, &gpu_votes);
        loose.votes += votes.votes;
        loose.counted += votes.counted;
        for k in 0..2 {
            loose.edge[k] += votes.edge[k];
            loose.edge_probed[k] += votes.edge_probed[k];
        }
        loose.passes += 1;
        loose.fits_off += usize::from(report_fit(&what, &fit, &votes, down, across));
        match fit {
            Err(no) => {
                assert_eq!(out, x, "{what}: no fit, and the mosaic moved");
                assert!(
                    !stats.corrected && stats.blocks == no.blocks(),
                    "{what}: {stats:?} for {no:?}"
                );
                chain.corrected = false;
                chain.blocks = no.blocks();
                chain.order = 0;
                break;
            }
            Ok(fit) => {
                if pass == 0 {
                    chain.max_shift = fit.max_shift;
                }
                chain.corrected = true;
                chain.blocks = fit.blocks;
                chain.order = fit.order;
                x = out;
            }
        }
    }
    let (full, stats) = ctx
        .correct_ca(mosaic, width, height, pattern, options)
        .expect("the GPU correction runs");
    assert_eq!(
        stats, chain,
        "{what}: the whole run's stats against its passes'"
    );
    if !(options.avoid_color_shift && chain.corrected) {
        assert!(
            full == x,
            "{what}: the whole run against its passes chained"
        );
        return;
    }
    let mut guarded = x;
    avoid_color_shift(mosaic, &mut guarded, width, height, pattern);
    let mut worst = 0f32;
    let mut past = 0usize;
    for (&a, &b) in guarded.iter().zip(&full) {
        let r = (a - b).abs() / a.abs().max(FLOOR);
        worst = worst.max(r);
        past += usize::from(r > TOLERANCE);
    }
    println!("{what} guard: within {worst:.3e} relative, {past} past {TOLERANCE:.0e}");
    assert!(
        past == 0,
        "{what}: the guard is {worst:.3e} from the reference's on the same passes"
    );
}

/// The GPU correction against the reference at random settings, on
/// random mosaics: every Bayer arrangement, aberration of each color
/// either way (none, a little, past the shift limit), the mosaic's
/// size (under the smallest, on the tile grid's steps, many tiles) and
/// kind, and the passes (0 to 4) and the guard, which go beyond the
/// editor's, which always takes two passes and the guard. Taken apart
/// as [`check_decomposed`] does.
#[test]
fn the_gpu_correction_is_the_reference_over_random_settings() {
    let Some(ctx) = context() else {
        return;
    };
    let device = ctx.device().adapter_info().name;
    let mut loose = Loose::default();
    Sweep::from_env(24).run("ca", &device, draw, |what, d| {
        let pattern = CfaPattern::new(2, 2, PATTERNS[d.pattern].to_vec()).unwrap();
        let tex = drawn_texture(d.scene, d.scene_seed, d.width, d.height);
        let mosaic = aberrate(&tex, d.width, d.height, &pattern, d.red, d.blue);
        if d.width < reference::MIN_SIDE || d.height < reference::MIN_SIDE {
            let (cpu, cpu_stats) =
                correct_ca(&mosaic, d.width, d.height, &pattern, &d.options).unwrap();
            let (gpu, gpu_stats) = ctx
                .correct_ca(&mosaic, d.width, d.height, &pattern, &d.options)
                .expect("the GPU correction runs");
            assert_eq!(cpu_stats, gpu_stats, "{what}");
            assert!(!cpu_stats.corrected, "{what}");
            assert!(
                cpu == mosaic && gpu == mosaic,
                "{what}: a small mosaic is left alone"
            );
            return;
        }
        check_decomposed(
            &ctx, what, &mosaic, d.width, d.height, &pattern, &d.options, &mut loose,
        );
    });
    let share = |n: usize, of: usize| n as f64 / of.max(1) as f64;
    let (reached, ties, counted, fits_off) = (
        share(loose.reached, loose.samples),
        share(loose.ties, loose.samples),
        share(loose.counted, loose.votes),
        share(loose.fits_off, loose.passes),
    );
    println!(
        "ca: the edge probes alone classify {:.3}% of the reference's votes on tiles at the edge \
         ({} of {}) and {:.3}% inside ({} of {})",
        100.0 * share(loose.edge_probed[0], loose.edge[0]),
        loose.edge_probed[0],
        loose.edge[0],
        100.0 * share(loose.edge_probed[1], loose.edge[1]),
        loose.edge_probed[1],
        loose.edge[1]
    );
    println!(
        "ca: over the run, of the resampled samples {:.4}% used the reach and {:.4}% were at a \
         tie; of the votes {:.3}% were counted; of the {} passes {} ({:.2}%) fitted off the \
         reference",
        100.0 * reached,
        100.0 * ties,
        100.0 * counted,
        loose.passes,
        loose.fits_off,
        100.0 * fits_off
    );
    for (name, got, cap) in [
        ("samples used the reach", reached, REACH_SHARE_RUN),
        ("samples were at a tie", ties, TIE_SHARE_RUN),
        ("votes were counted", counted, COUNTED_SHARE_RUN),
        (
            "passes fitted off the reference",
            fits_off,
            FIT_OFF_SHARE_RUN,
        ),
    ] {
        assert!(
            got <= cap,
            "ca: over the run {:.4}% of the {name}, over {:.4}%",
            100.0 * got,
            100.0 * cap
        );
    }
}
