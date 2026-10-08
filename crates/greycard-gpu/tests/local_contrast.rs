//! The GPU local contrast against its CPU reference and against an
//! f64 port of that reference, on the same pictures. The f64 port is
//! the truth the two f32 paths round from: the GPU, reading each box
//! window whole, stays within a few 1e-6 of it, where the CPU's
//! running sums drift up to about 1e-4 on a large plane, so the GPU
//! is held to the f64 port tightly and to the CPU loosely.
//!
//! Without a GPU adapter each test says so and returns; it does not
//! pass in silence. (`cargo test` shows the line with `--nocapture`.)

mod common;

use common::{Rng, Sweep};
use greycard_core::develop::local_contrast::{
    self as reference, COARSE_FROM_RADIUS, LocalContrastOptions, LocalContrastStats,
    local_contrast, radii,
};
use greycard_core::develop::sharpen::{SharpenOptions, Threshold, sharpen_with_mask};
use greycard_core::image::WorkingImage;
use greycard_gpu::{Context, wgpu};

fn context() -> Option<Context> {
    match Context::own() {
        Ok(c) => Some(c),
        Err(e) => {
            require_gpu(&format!(
                "the GPU local contrast has nothing to run on ({e})"
            ));
            eprintln!("SKIPPED: the GPU local contrast has nothing to run on ({e})");
            println!("SKIPPED: the GPU local contrast has nothing to run on ({e})");
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

/// A picture with structure at every scale Clarity and Texture read:
/// a slow gradient, broad soft shapes the size of Clarity's window, a
/// hard four-stop edge, fine bars, grain, a color cast, a deep shadow
/// and a clipped patch.
fn scene(w: usize, h: usize) -> WorkingImage {
    let mut image = WorkingImage::new(w, h);
    let (pixels, _) = image.data.as_chunks_mut::<3>();
    let mut seed = 0x9E3779B9u32;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) - 0.5
    };
    let scale = w.max(h) as f32;
    for (i, px) in pixels.iter_mut().enumerate() {
        let (x, y) = ((i % w) as f32, (i / w) as f32);
        let gradient = 0.08 + 0.3 * (x / w as f32);
        let soft = 0.25 * ((x / scale * 40.0).sin() * (y / scale * 55.0).cos() + 1.0);
        let edge = if x > w as f32 * 0.6 && y < h as f32 * 0.5 {
            2.0
        } else {
            0.0
        };
        let bars = if ((x / 3.0) as usize).is_multiple_of(2) {
            0.04
        } else {
            0.0
        };
        let shadow = if x < w as f32 * 0.1 && y > h as f32 * 0.7 {
            0.002
        } else {
            1.0
        };
        let clipped = if x > w as f32 * 0.85 && y > h as f32 * 0.8 {
            4.0
        } else {
            0.0
        };
        let v = ((gradient + soft + edge + bars) * shadow + clipped) * (1.0 + 0.02 * rand());
        let tint = 0.85 + 0.3 * (y / h as f32);
        px[0] = v * tint;
        px[1] = v;
        px[2] = v * (1.7 - tint);
    }
    image
}

/// How far two pictures are apart: the largest absolute difference,
/// the largest relative to the reference's value, and the mean.
struct Apart {
    max: f32,
    relative: f32,
    mean: f64,
}

fn apart(reference: &[f32], other: &[f32]) -> Apart {
    assert_eq!(reference.len(), other.len());
    let mut max = 0f32;
    let mut relative = 0f32;
    let mut sum = 0f64;
    for (&a, &b) in reference.iter().zip(other) {
        let d = (a - b).abs();
        max = max.max(d);
        relative = relative.max(d / a.abs().max(1e-3));
        sum += f64::from(d);
    }
    Apart {
        max,
        relative,
        mean: sum / reference.len() as f64,
    }
}

/// The GPU against the CPU, relative at each pixel; and where the
/// CPU's own drift from the f64 port is more than this, that drift
/// plus [`F64_TOLERANCE`].
const TOLERANCE: f32 = 1e-4;

/// The GPU against the f64 port, relative at each pixel. Measured on
/// NVIDIA and lavapipe at 1.04e-5 at most on these pictures, and
/// under 2e-5 on a 24 MP frame; the CPU drifts up to 1.01e-4 from it
/// here (Texture on the 16384-pixel row).
const F64_TOLERANCE: f64 = 5e-5;

/// How far a picture is from the f64 port: the largest difference
/// relative to the port's value, as [`apart`] takes it.
fn apart_from_f64(exact: &[f64], other: &[f32]) -> f64 {
    assert_eq!(exact.len(), other.len());
    exact
        .iter()
        .zip(other)
        .map(|(&a, &b)| (a - f64::from(b)).abs() / a.abs().max(1e-3))
        .fold(0.0, f64::max)
}

/// Run both paths and the f64 port on `image` and compare: the GPU to
/// the port within [`F64_TOLERANCE`] and to the CPU within
/// [`TOLERANCE`] (or the CPU's own drift), the stats to equality. The
/// GPU's sums run in another order and its `log2` and `exp2` are the
/// device's, so the paths are not bit-identical. Returns how far the
/// op moved the picture, and the stats.
fn compare(
    ctx: &Context,
    what: &str,
    image: &WorkingImage,
    options: &LocalContrastOptions,
    clip_level: f32,
) -> (f32, LocalContrastStats) {
    let exact = f64_local_contrast(image, options, clip_level);
    let mut cpu = image.clone();
    let cpu_stats = local_contrast(&mut cpu, options, clip_level);
    let mut gpu = image.clone();
    let gpu_stats = ctx
        .local_contrast_image(&mut gpu, options, clip_level)
        .expect("the GPU local contrast runs");
    let moved = apart(&image.data, &cpu.data);
    let picture = apart(&cpu.data, &gpu.data);
    let gpu_f64 = apart_from_f64(&exact, &gpu.data);
    let cpu_f64 = apart_from_f64(&exact, &cpu.data);
    println!(
        "{what}: moved {:.3e}; gpu~cpu max {:.3e} (relative {:.3e}) mean {:.3e}; gpu~f64 {gpu_f64:.3e}; cpu~f64 {cpu_f64:.3e}; stats {cpu_stats:?}",
        moved.max, picture.max, picture.relative, picture.mean
    );
    assert!(
        gpu_f64 <= F64_TOLERANCE,
        "{what}: the GPU is {gpu_f64:.3e} from the f64 port relative, over {F64_TOLERANCE:.1e}"
    );
    let cpu_bound = f64::from(TOLERANCE).max(cpu_f64 + F64_TOLERANCE);
    assert!(
        f64::from(picture.relative) <= cpu_bound,
        "{what}: the pictures are {:.3e} apart relative, over {cpu_bound:.1e}",
        picture.relative
    );
    assert_eq!(cpu_stats, gpu_stats, "{what}");
    (moved.max, cpu_stats)
}

/// [`compare`], on a picture the op is known to move.
fn check(
    ctx: &Context,
    what: &str,
    image: &WorkingImage,
    options: &LocalContrastOptions,
    clip_level: f32,
) -> LocalContrastStats {
    let (moved, stats) = compare(ctx, what, image, options, clip_level);
    assert!(moved > 1e-3, "{what}: the op did nothing worth checking");
    stats
}

/// The CPU op in f64, step for step: the box means as exact prefix
/// sums over the window the edge leaves, the guided filter, the coarse
/// grid with its count-weighted short blocks and bilinear taps, and
/// both gains with their fades. Written for these tests from
/// `greycard_core::develop::local_contrast`.
fn f64_local_contrast(
    image: &WorkingImage,
    options: &LocalContrastOptions,
    clip_level: f32,
) -> Vec<f64> {
    let (w, h) = (image.width, image.height);
    let (texture_radius, clarity_radius) = radii(w, h);
    let step = reference::clarity_step(clarity_radius);
    let luma = reference::LUMA.map(f64::from);
    let mut px: Vec<f64> = image.data.iter().map(|&v| f64::from(v)).collect();
    let mut log: Vec<f64> = px
        .chunks(3)
        .map(|p| {
            ((luma[0] * p[0] + luma[1] * p[1] + luma[2] * p[2]).max(f64::from(reference::FLOOR))
                / f64::from(reference::MID_GREY))
            .log2()
        })
        .collect();
    if options.texture != 0.0 {
        let k = f64::from(options.texture.clamp(-1.0, 1.0) * reference::TEXTURE_GAIN);
        let base = f64_guided(
            &log,
            w,
            h,
            texture_radius,
            f64::from(reference::TEXTURE_EPSILON),
        );
        f64_apply(&mut px, &log, &base, clip_level, |_| k);
    }
    if options.clarity != 0.0 {
        f64_box_mean(&mut log, w, h, texture_radius);
        f64_box_mean(&mut log, w, h, texture_radius);
        let k = f64::from(options.clarity.clamp(-1.0, 1.0) * reference::CLARITY_GAIN);
        let epsilon = f64::from(reference::CLARITY_EPSILON);
        let base = if step > 1 {
            f64_coarse(&log, w, h, clarity_radius, epsilon, step)
        } else {
            f64_guided(&log, w, h, clarity_radius, epsilon)
        };
        let (shadow, highlight) = (
            reference::CLARITY_SHADOW_FADE,
            reference::CLARITY_HIGHLIGHT_FADE,
        );
        f64_apply(&mut px, &log, &base, clip_level, |b| {
            k * smoothstep(f64::from(shadow.0), f64::from(shadow.1), b)
                * (1.0 - smoothstep(f64::from(highlight.0), f64::from(highlight.1), b))
        });
    }
    px
}

fn f64_box_mean(data: &mut [f64], w: usize, h: usize, radius: usize) {
    let mut tmp = vec![0.0f64; w * h];
    let mut prefix = vec![0.0f64; w.max(h) + 1];
    for y in 0..h {
        for x in 0..w {
            prefix[x + 1] = prefix[x] + data[y * w + x];
        }
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(radius), (x + radius).min(w - 1));
            tmp[y * w + x] = (prefix[hi + 1] - prefix[lo]) / (hi + 1 - lo) as f64;
        }
    }
    for x in 0..w {
        for y in 0..h {
            prefix[y + 1] = prefix[y] + tmp[y * w + x];
        }
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(radius), (y + radius).min(h - 1));
            data[y * w + x] = (prefix[hi + 1] - prefix[lo]) / (hi + 1 - lo) as f64;
        }
    }
}

fn f64_guided(input: &[f64], w: usize, h: usize, radius: usize, epsilon: f64) -> Vec<f64> {
    let mut a = input.to_vec();
    f64_box_mean(&mut a, w, h, radius);
    let mut b: Vec<f64> = input.iter().map(|v| v * v).collect();
    f64_box_mean(&mut b, w, h, radius);
    for (sq, mean) in b.iter_mut().zip(a.iter_mut()) {
        let variance = (*sq - *mean * *mean).max(0.0);
        let slope = variance / (variance + epsilon);
        *sq = slope;
        *mean -= slope * *mean;
    }
    f64_box_mean(&mut b, w, h, radius);
    f64_box_mean(&mut a, w, h, radius);
    a.iter()
        .zip(&b)
        .zip(input)
        .map(|((intercept, slope), v)| intercept + slope * v)
        .collect()
}

/// For each pixel along an axis, the two blocks whose centers it lies
/// between and how far along it is, clamped to the end blocks'.
fn f64_taps(len: usize, step: usize) -> Vec<(usize, usize, f64)> {
    let blocks = len.div_ceil(step);
    let center = |b: usize| (b * step) as f64 + (step.min(len - b * step) - 1) as f64 / 2.0;
    let mut b = 0;
    (0..len)
        .map(|x| {
            let x = x as f64;
            while b + 1 < blocks && center(b + 1) <= x {
                b += 1;
            }
            if b + 1 == blocks || x <= center(b) {
                (b, b, 0.0)
            } else {
                (b, b + 1, (x - center(b)) / (center(b + 1) - center(b)))
            }
        })
        .collect()
}

fn f64_coarse(
    input: &[f64],
    w: usize,
    h: usize,
    radius: usize,
    epsilon: f64,
    step: usize,
) -> Vec<f64> {
    let (cw, ch) = (w.div_ceil(step), h.div_ceil(step));
    let grid_radius = reference::coarse_radius(radius, step);
    let area = (step * step) as f64;
    let mut mean = vec![0.0; cw * ch];
    let mut sq = vec![0.0; cw * ch];
    let mut share = vec![0.0; cw * ch];
    for by in 0..ch {
        for bx in 0..cw {
            let (y0, y1) = (by * step, ((by + 1) * step).min(h));
            let (x0, x1) = (bx * step, ((bx + 1) * step).min(w));
            let (mut m, mut q) = (0.0, 0.0);
            for y in y0..y1 {
                for x in x0..x1 {
                    let v = input[y * w + x];
                    m += v;
                    q += v * v;
                }
            }
            mean[by * cw + bx] = m / area;
            sq[by * cw + bx] = q / area;
            share[by * cw + bx] = ((y1 - y0) * (x1 - x0)) as f64 / area;
        }
    }
    let mut held = share.clone();
    f64_box_mean(&mut held, cw, ch, grid_radius);
    f64_box_mean(&mut mean, cw, ch, grid_radius);
    f64_box_mean(&mut sq, cw, ch, grid_radius);
    for i in 0..cw * ch {
        let (m, q) = (mean[i] / held[i], sq[i] / held[i]);
        let variance = (q - m * m).max(0.0);
        let slope = variance / (variance + epsilon);
        sq[i] = slope * share[i];
        mean[i] = (m - slope * m) * share[i];
    }
    f64_box_mean(&mut sq, cw, ch, grid_radius);
    f64_box_mean(&mut mean, cw, ch, grid_radius);
    for i in 0..cw * ch {
        sq[i] /= held[i];
        mean[i] /= held[i];
    }
    let (across, down) = (f64_taps(w, step), f64_taps(h, step));
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        let (j0, j1, t) = down[y];
        for x in 0..w {
            let (i0, i1, s) = across[x];
            let at = |p: &[f64], i: usize| p[j0 * cw + i] + t * (p[j1 * cw + i] - p[j0 * cw + i]);
            let slope = at(&sq, i0) + s * (at(&sq, i1) - at(&sq, i0));
            let intercept = at(&mean, i0) + s * (at(&mean, i1) - at(&mean, i0));
            out[y * w + x] = slope * input[y * w + x] + intercept;
        }
    }
    out
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One band's gain on the picture: `2^(fade * k(base) * (log - base))`,
/// the fade the clip guard's on the pixel's brightest channel.
fn f64_apply(px: &mut [f64], log: &[f64], base: &[f64], clip_level: f32, k: impl Fn(f64) -> f64) {
    let from = f64::from(clip_level * reference::CLIP_FADE);
    for (i, p) in px.chunks_mut(3).enumerate() {
        let brightest = p[0].max(p[1]).max(p[2]);
        let fade = if clip_level.is_finite() {
            1.0 - smoothstep(from, f64::from(clip_level), brightest)
        } else {
            1.0
        };
        let gain = (fade * k(base[i]) * (log[i] - base[i])).exp2();
        for v in p.iter_mut() {
            *v *= gain;
        }
    }
}

const NO_CLIP: f32 = f32::INFINITY;

fn texture(amount: f32) -> LocalContrastOptions {
    LocalContrastOptions {
        texture: amount,
        clarity: 0.0,
    }
}

fn clarity(amount: f32) -> LocalContrastOptions {
    LocalContrastOptions {
        texture: 0.0,
        clarity: amount,
    }
}

fn both(texture: f32, clarity: f32) -> LocalContrastOptions {
    LocalContrastOptions { texture, clarity }
}

/// A picture under the grid's radius, so Clarity takes the exact
/// filter at full size: each band alone, both together, each sign,
/// and with a clip level that the clipped patch fades under.
#[test]
fn a_small_picture_matches_the_reference_through_the_exact_filter() {
    let Some(ctx) = context() else {
        return;
    };
    let image = scene(523, 389);
    let (_, r) = radii(523, 389);
    assert!(
        r < COARSE_FROM_RADIUS,
        "the exact filter is the one tried here"
    );
    check(&ctx, "texture +1", &image, &texture(1.0), NO_CLIP);
    check(&ctx, "texture -0.5", &image, &texture(-0.5), NO_CLIP);
    check(&ctx, "clarity +1", &image, &clarity(1.0), NO_CLIP);
    check(&ctx, "clarity -0.7", &image, &clarity(-0.7), NO_CLIP);
    check(&ctx, "both", &image, &both(0.6, 0.8), NO_CLIP);
    check(&ctx, "both, clip at 4", &image, &both(0.6, 0.8), 4.0);
    check(&ctx, "both, clip at 1", &image, &both(-0.4, 1.0), 1.0);
}

/// A picture large enough for Clarity to take the quarter grid, with
/// odd sides so the last row and column of blocks are cut short, and
/// one with even sides.
#[test]
fn a_large_picture_matches_the_reference_through_the_grid() {
    let Some(ctx) = context() else {
        return;
    };
    for (w, h) in [(2601usize, 1001usize), (2560, 1600)] {
        let image = scene(w, h);
        let (_, r) = radii(w, h);
        assert!(r >= COARSE_FROM_RADIUS, "{w}x{h} takes the grid");
        let what = |s: &str| format!("{w}x{h} {s}");
        check(&ctx, &what("clarity +1"), &image, &clarity(1.0), NO_CLIP);
        check(&ctx, &what("texture +1"), &image, &texture(1.0), NO_CLIP);
        check(&ctx, &what("both"), &image, &both(0.5, 0.5), NO_CLIP);
        check(
            &ctx,
            &what("both, clip at 4"),
            &image,
            &both(0.5, -1.0),
            4.0,
        );
    }
}

/// A tall picture, so the grid's last row is the one cut short and
/// the column passes have rows to spare.
#[test]
fn a_tall_picture_matches_the_reference() {
    let Some(ctx) = context() else {
        return;
    };
    let image = scene(401, 2561);
    check(&ctx, "401x2561 both", &image, &both(0.7, 0.7), 3.0);
}

/// The output `Image` is the sharpen's input, and its automatic
/// threshold is this picture's: two Detail moves make two pictures,
/// and the sharpen finds a threshold on each, the reference's for
/// that picture, and not the first's kept for the second. Noise at
/// mid grey, where the reference's search finds a flat tile, and
/// Texture to double that noise and to take some of it away (not so
/// much that the tile turns suspiciously flat and the search gives
/// up).
#[test]
fn each_output_has_a_threshold_of_its_own() {
    let Some(ctx) = context() else {
        return;
    };
    let (w, h) = (256, 192);
    let mut image = WorkingImage::new(w, h);
    let mut seed = 0x9E3779B9u32;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) - 0.5
    };
    for px in image.data.chunks_mut(3) {
        let v = 0.18 + 0.014 * rand();
        px.copy_from_slice(&[v, v, v]);
    }
    let uploaded = ctx.upload(&image).expect("uploads");
    let sharpen = SharpenOptions {
        contrast: Threshold::Auto,
        ..Default::default()
    };
    let mut thresholds = Vec::new();
    for options in [texture(1.0), texture(-0.3)] {
        let (after, _) = ctx
            .local_contrast(&uploaded, &options, NO_CLIP)
            .expect("the GPU local contrast runs");
        let out = ctx.viewport_texture(after.width(), after.height());
        let gpu = ctx
            .sharpen(&after, &sharpen, None, NO_CLIP, &out)
            .expect("the GPU sharpen runs on the op's output");
        let mut cpu = image.clone();
        local_contrast(&mut cpu, &options, NO_CLIP);
        let (reference, _) = sharpen_with_mask(&mut cpu, &sharpen, None, NO_CLIP);
        println!(
            "{options:?}: threshold gpu {} cpu {}",
            gpu.threshold, reference.threshold
        );
        assert!(
            reference.threshold > 0.0 && (gpu.threshold - reference.threshold).abs() < 1e-6,
            "{options:?}: thresholds {} and {}",
            gpu.threshold,
            reference.threshold
        );
        thresholds.push(gpu.threshold);
    }
    assert!(
        thresholds[0] != thresholds[1],
        "the two pictures must differ in threshold for this test to tell a stale one: {thresholds:?}"
    );
}

/// With the sharpen off the op writes the viewport's half-float
/// texture itself, the alpha zero as the CPU path's `Halves` has it
/// with no mask.
#[test]
fn the_viewport_texture_holds_the_picture_with_alpha_zero() {
    let Some(ctx) = context() else {
        return;
    };
    let image = scene(300, 200);
    let options = both(0.5, 0.5);
    let uploaded = ctx.upload(&image).expect("uploads");
    let out = ctx.viewport_texture(300, 200);
    let stats = ctx
        .local_contrast_to_viewport(&uploaded, &options, 4.0, &out)
        .expect("the GPU local contrast runs to the viewport");
    let mut cpu = image.clone();
    assert_eq!(stats, local_contrast(&mut cpu, &options, 4.0));
    let halves = read_halves(&ctx, &out);
    let mut max = 0f32;
    for (i, px) in cpu.data.chunks(3).enumerate() {
        let got = &halves[i * 4..i * 4 + 4];
        for c in 0..3 {
            let d = (got[c] - px[c]).abs() / px[c].abs().max(1e-2);
            max = max.max(d);
        }
        assert_eq!(got[3], 0.0, "alpha at {i}");
    }
    println!("viewport halves vs cpu: relative {max:.3e}");
    // Half floats carry eleven bits of mantissa.
    assert!(max < 2e-3, "the halves are {max:.3e} from the reference");
}

/// Identity options leave the picture as it is, as the reference does.
#[test]
fn zero_is_the_identity() {
    let Some(ctx) = context() else {
        return;
    };
    let image = scene(128, 96);
    let mut gpu = image.clone();
    ctx.local_contrast_image(&mut gpu, &LocalContrastOptions::default(), 2.0)
        .expect("runs");
    assert_eq!(gpu.data, image.data);
}

/// What the device rejects comes back as an error, not a panic: an
/// output texture the op cannot write to fails the bind group's
/// validation, which the op's error scope catches. The context is
/// still usable afterwards.
#[test]
fn a_device_error_is_an_error_not_a_panic() {
    let Some(ctx) = context() else {
        return;
    };
    let image = scene(96, 64);
    let uploaded = ctx.upload(&image).expect("uploads");
    let wrong = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("not writable by a shader"),
        size: wgpu::Extent3d {
            width: 96,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let err = ctx
        .local_contrast_to_viewport(&uploaded, &clarity(0.5), 2.0, &wrong)
        .expect_err("a texture without storage usage is refused");
    println!("caught: {err}");
    assert!(matches!(err, greycard_gpu::Error::Gpu { .. }), "{err}");
    // A wrong size is declined before anything runs.
    let small = ctx.viewport_texture(10, 10);
    let err = ctx
        .local_contrast_to_viewport(&uploaded, &clarity(0.5), 2.0, &small)
        .expect_err("a texture of another size is refused");
    assert!(matches!(err, greycard_gpu::Error::Unsupported(_)), "{err}");
    // And the same op on a proper texture still runs.
    let right = ctx.viewport_texture(96, 64);
    ctx.local_contrast_to_viewport(&uploaded, &clarity(0.5), 2.0, &right)
        .expect("the context works after a caught error");
}

/// The sizes at the edges of the passes: a single pixel, sides
/// shorter than any window, a row longer than the row pass's tile,
/// the long edge on either side of the grid's start (2539 takes the
/// exact filter, 2540 the grid), and the longest side a device is
/// asked for. Each with the clip guard off and at 1, against the f64
/// port and the CPU; the tiny ones the op barely moves, so only how
/// far apart the paths are is held, not that the op did something.
#[test]
fn the_edge_sizes_match_the_reference() {
    let Some(ctx) = context() else {
        return;
    };
    let sizes = [
        (1usize, 1usize),
        (2, 3),
        (5, 17),
        (257, 33),
        (2539, 101),
        (2540, 101),
        (16384, 64),
    ];
    let options = [
        ("both", both(0.5, 0.5)),
        ("texture +1", texture(1.0)),
        ("clarity -1", clarity(-1.0)),
    ];
    for (w, h) in sizes {
        let image = scene(w, h);
        let (_, r) = radii(w, h);
        match w {
            2539 => assert!(r < COARSE_FROM_RADIUS, "2539 takes the exact filter"),
            2540 => assert!(r >= COARSE_FROM_RADIUS, "2540 takes the grid"),
            _ => {}
        }
        for (name, options) in &options {
            for clip in [NO_CLIP, 1.0] {
                compare(
                    &ctx,
                    &format!("{w}x{h} {name}, clip {clip}"),
                    &image,
                    options,
                    clip,
                );
            }
        }
    }
}

/// The planes kept between runs are the ones the sliders need, and
/// making them for each run instead (as an integrated or
/// unified-memory GPU does) gives the same picture to the bit and
/// keeps nothing.
#[test]
fn the_planes_are_those_the_sliders_need_kept_or_not() {
    let Some(ctx) = context() else {
        return;
    };
    let (w, h) = (2560u32, 400u32);
    let image = scene(w as usize, h as usize);
    let plane = u64::from(w * h) * 4;
    let (gw, gh) = (w.div_ceil(4), h.div_ceil(4));
    let grid = 5 * u64::from(gw * gh) * 4;
    let run = |options: &LocalContrastOptions| {
        let mut out = image.clone();
        ctx.local_contrast_image(&mut out, options, 2.0)
            .expect("the GPU local contrast runs");
        out.data
    };
    let cases = [
        // The log and its scratch, the slope and intercept and the gain.
        (texture(0.5), 5 * plane),
        // The log, its scratch and the grid.
        (clarity(0.5), 2 * plane + grid),
        (both(0.5, 0.5), 5 * plane + grid),
    ];
    ctx.keep_local_contrast_planes(true);
    let kept: Vec<_> = cases
        .iter()
        .map(|(options, bytes)| {
            let out = run(options);
            assert_eq!(
                ctx.kept_bytes().local_contrast,
                *bytes,
                "{options:?}: the planes kept"
            );
            out
        })
        .collect();
    ctx.release_local_contrast();
    assert_eq!(ctx.kept_bytes().local_contrast, 0);
    ctx.keep_local_contrast_planes(false);
    for ((options, _), kept) in cases.iter().zip(kept) {
        assert_eq!(run(options), kept, "{options:?}: per run against kept");
        assert_eq!(
            ctx.kept_bytes().local_contrast,
            0,
            "{options:?}: nothing kept"
        );
    }
}

/// An Rgba16Float texture read back as floats.
fn read_halves(ctx: &Context, texture: &wgpu::Texture) -> Vec<f32> {
    let (w, h) = (texture.width(), texture.height());
    let row = (w * 8).div_ceil(256) * 256;
    let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("read back"),
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue().submit(Some(encoder.finish()));
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    ctx.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the device polls");
    rx.recv().expect("the map callback comes").expect("maps");
    let data = slice.get_mapped_range().expect("the mapped range");
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let line = &data[y * row as usize..][..(w * 8) as usize];
        for pair in line.as_chunks::<2>().0 {
            out.push(half_to_f32(u16::from_le_bytes(*pair)));
        }
    }
    out
}

fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = ((h >> 10) & 0x1f) as i32;
    let mantissa = (h & 0x3ff) as f32;
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        31 => {
            if mantissa == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    }
}

/// The kinds of picture the random sweep draws, each with its numbers
/// drawn from the case's own seed.
#[derive(Debug, Clone, Copy)]
enum Scene {
    /// [`scene`] with its numbers drawn: a gradient, soft shapes, a
    /// hard edge of some stops, bars, grain, a cast, a deep shadow and
    /// a clipped patch, each present or not.
    Soft,
    /// Hard steps of a few stops either way across the picture, in
    /// both directions: the guided filters' edges and Clarity's fades.
    Steps,
    /// Noise about one level, from deep shadow to past white.
    Noise,
    /// One value everywhere, black included.
    Flat,
    /// [`Scene::Soft`] deep in the shadows, through Clarity's shadow
    /// fade and down to the log's floor.
    Dark,
    /// Blocks of pure primaries, black and bright: a channel at zero,
    /// a luminance at zero.
    Saturated,
}

/// A picture of `kind`, its numbers from `seed`.
fn drawn_scene(kind: Scene, seed: u64, w: usize, h: usize) -> WorkingImage {
    let mut r = Rng::new(seed, 0);
    let mut noise = Rng::new(seed, 1);
    let mut rand = move || noise.unit() - 0.5;
    let mut image = WorkingImage::new(w, h);
    let (pixels, _) = image.data.as_chunks_mut::<3>();
    let (fw, fh) = (w as f32, h as f32);
    // A rectangle somewhere in the picture, as fractions of it.
    let rect = move |r: &mut Rng| {
        let (x0, y0) = (r.range(0.0, 0.9), r.range(0.0, 0.9));
        let (x1, y1) = (r.range(x0, 1.0), r.range(y0, 1.0));
        move |x: f32, y: f32| x >= x0 * fw && x < x1 * fw && y >= y0 * fh && y < y1 * fh
    };
    match kind {
        Scene::Soft | Scene::Dark => {
            let slope = r.range(0.0, 0.5);
            let base = r.range(0.0, 0.2);
            let soft = r.range(0.0, 0.4);
            let (fx, fy) = (r.range(5.0, 80.0), r.range(5.0, 80.0));
            let edge = if r.chance(0.7) {
                r.range(0.0, 4.0)
            } else {
                0.0
            };
            let edged = rect(&mut r);
            let bars = r.range(0.0, 0.1);
            let period = r.range(2.0, 8.0);
            let shadow = r.pick(&[0.002, 1e-5, 1.0]);
            let shadowed = rect(&mut r);
            let clipped = if r.chance(0.6) {
                r.range(1.0, 6.0)
            } else {
                0.0
            };
            let clipped_at = rect(&mut r);
            let grain = r.range(0.0, 0.05);
            let cast = r.range(0.0, 0.5);
            let scale = if let Scene::Dark = kind {
                10f32.powf(r.range(-5.0, -2.0))
            } else {
                1.0
            };
            let long = w.max(h) as f32;
            for (i, px) in pixels.iter_mut().enumerate() {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let mut v = base
                    + slope * (x / fw)
                    + soft * ((x / long * fx).sin() * (y / long * fy).cos() + 1.0)
                    + if edged(x, y) { edge } else { 0.0 }
                    + if ((x / period) as usize).is_multiple_of(2) {
                        bars
                    } else {
                        0.0
                    };
                if shadowed(x, y) {
                    v *= shadow;
                }
                if clipped_at(x, y) {
                    v += clipped;
                }
                let v = v * (1.0 + grain * rand()) * scale;
                let tint = 1.0 - cast / 2.0 + cast * (y / fh);
                px[0] = v * tint;
                px[1] = v;
                px[2] = v * (2.0 - tint);
            }
        }
        Scene::Steps => {
            let level = r.range(0.01, 1.0);
            let (across, down) = (r.between(1, 6), r.between(1, 6));
            let stops: Vec<f32> = (0..across + down).map(|_| r.range(-8.0, 4.0)).collect();
            let cuts: Vec<f32> = (0..across + down).map(|_| r.range(0.0, 1.0)).collect();
            for (i, px) in pixels.iter_mut().enumerate() {
                let (x, y) = ((i % w) as f32 / fw, (i / w) as f32 / fh);
                let mut e = 0.0;
                for k in 0..across + down {
                    let t = if k < across { x } else { y };
                    if t >= cuts[k] {
                        e += stops[k] / (across + down) as f32;
                    }
                }
                let v = level * 2f32.powf(e);
                px.copy_from_slice(&[v, v, v]);
            }
        }
        Scene::Noise => {
            let level = 10f32.powf(r.range(-4.0, 0.7));
            let amount = r.range(0.0, 0.5);
            for px in pixels.iter_mut() {
                let v = level * (1.0 + amount * rand());
                px.copy_from_slice(&[v, v * 0.9, v * 1.1]);
            }
        }
        Scene::Flat => {
            let v = r.pick(&[0.0, 1e-7, 0.18, 1.0, 5.0]);
            for px in pixels.iter_mut() {
                px.copy_from_slice(&[v, v, v]);
            }
        }
        Scene::Saturated => {
            let block = r.between(1, 24);
            let level = r.range(0.05, 3.0);
            let colors: [[f32; 3]; 7] = [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 1.0],
                [0.0, 0.6, 0.6],
                [0.8, 0.0, 0.5],
            ];
            let across = w.div_ceil(block);
            let choice: Vec<usize> = (0..across * h.div_ceil(block))
                .map(|_| r.below(colors.len()))
                .collect();
            for (i, px) in pixels.iter_mut().enumerate() {
                let (x, y) = (i % w, i / w);
                let color = colors[choice[(y / block) * across + x / block]];
                for c in 0..3 {
                    px[c] = color[c] * level;
                }
            }
        }
    }
    image
}

/// One random case: the picture and every input the op reads.
#[derive(Debug)]
struct Drawn {
    width: usize,
    height: usize,
    scene: Scene,
    scene_seed: u64,
    options: LocalContrastOptions,
    clip_level: f32,
}

fn draw(r: &mut Rng) -> Drawn {
    let side = |r: &mut Rng| {
        if r.chance(0.08) {
            r.between(1, 12)
        } else {
            r.between(13, 640)
        }
    };
    // Now and then a long edge either side of the grid's start (2539
    // takes the exact filter, 2540 the grid) or of Texture's radius
    // going from two to three (at 5000), or a very long one, each on
    // a short side to keep the run quick.
    let (long, short) = match r.below(50) {
        0..=4 => {
            let long = if r.chance(0.5) {
                r.pick(&[2539, 2540])
            } else {
                r.between(2530, 2560)
            };
            (long, r.between(1, 120))
        }
        5 | 6 => (r.between(4990, 5010), r.between(1, 40)),
        7 => (r.between(8000, 16384), r.between(1, 16)),
        _ => (side(r), side(r)),
    };
    let (width, height) = if r.chance(0.5) {
        (long, short)
    } else {
        (short, long)
    };
    let scene = r.pick(&[
        Scene::Soft,
        Scene::Soft,
        Scene::Steps,
        Scene::Noise,
        Scene::Flat,
        Scene::Dark,
        Scene::Saturated,
    ]);
    let scene_seed = r.next();
    // The panel's ranges, -1 to 1 each.
    let options = LocalContrastOptions {
        texture: r.signed(-1.0, 1.0),
        clarity: r.signed(-1.0, 1.0),
    };
    // The clip level is the frame's ceiling's, at least 0.98, or none.
    let clip_level = match r.below(20) {
        0..=4 => NO_CLIP,
        5..=7 => 0.98,
        8 | 9 => 8.0,
        _ => r.range(0.98, 8.0),
    };
    Drawn {
        width,
        height,
        scene,
        scene_seed,
        options,
        clip_level,
    }
}

/// The GPU local contrast against the reference and the f64 port at
/// random settings, on random pictures: Texture and Clarity over the
/// panel's range with their ends, zero and small values weighted, the
/// clip level or none, and the picture's size (both filters, the
/// grid's start, Texture's radius steps, odd sides) and kind. Held to
/// the fixed tests' `compare`.
#[test]
fn the_gpu_local_contrast_is_the_reference_over_random_settings() {
    let Some(ctx) = context() else {
        return;
    };
    let device = ctx.device().adapter_info().name;
    Sweep::from_env(50).run("local contrast", &device, draw, |what, d| {
        let image = drawn_scene(d.scene, d.scene_seed, d.width, d.height);
        compare(&ctx, what, &image, &d.options, d.clip_level);
    });
}
