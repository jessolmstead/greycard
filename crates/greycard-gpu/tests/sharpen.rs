//! The GPU sharpen against its CPU reference, on the same pictures.
//!
//! Without a GPU adapter each test says so and returns; it does not
//! pass in silence. (`cargo test` shows the line with `--nocapture`.)

use greycard_core::develop::sharpen::{
    Radius, SharpenOptions, SharpenStats, Threshold, sharpen_with_mask,
};
use greycard_core::image::WorkingImage;
use greycard_gpu::Context;

fn context() -> Option<Context> {
    match Context::own() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("SKIPPED: the GPU sharpen has nothing to run on ({e})");
            println!("SKIPPED: the GPU sharpen has nothing to run on ({e})");
            None
        }
    }
}

/// A picture with detail in every tile: bars, spots and a color
/// cast, blurred by `sigma`, with a clipped patch and a flat corner.
fn textured(w: usize, h: usize, sigma: f32) -> WorkingImage {
    let mut lum = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let bars = if (x / 7 + y / 5) % 2 == 0 { 0.55 } else { 0.08 };
            let spot = if (x * x + y * y) % 97 < 11 { 0.3 } else { 0.0 };
            let flat = if x >= w - 40 && y >= h - 40 {
                0.2
            } else {
                bars + spot
            };
            let hot = if (x / 60) % 5 == 2 && (y / 60) % 4 == 1 {
                3.0
            } else {
                0.0
            };
            lum[y * w + x] = flat + hot;
        }
    }
    blur(&mut lum, w, h, sigma);
    let mut image = WorkingImage::new(w, h);
    let (pixels, _) = image.data.as_chunks_mut::<3>();
    for (i, px) in pixels.iter_mut().enumerate() {
        let (x, y) = (i % w, i / w);
        let v = lum[i];
        let tint = 0.8 + 0.4 * (x as f32 / w as f32);
        px[0] = v * tint;
        px[1] = v;
        px[2] = v * (1.6 - tint) * (0.5 + 0.5 * (y as f32 / h as f32));
    }
    image
}

/// A plain Gaussian blur, for making the pictures.
fn blur(data: &mut [f32], w: usize, h: usize, sigma: f32) {
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    let tmp: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as isize, i / w);
            k.iter()
                .enumerate()
                .map(|(j, kv)| {
                    kv * data[y * w + (x + j as isize - r).clamp(0, w as isize - 1) as usize]
                })
                .sum::<f32>()
                / sum
        })
        .collect();
    for (i, d) in data.iter_mut().enumerate() {
        let (x, y) = (i % w, (i / w) as isize);
        *d = k
            .iter()
            .enumerate()
            .map(|(j, kv)| kv * tmp[(y + j as isize - r).clamp(0, h as isize - 1) as usize * w + x])
            .sum::<f32>()
            / sum;
    }
}

/// How far two pictures are apart: the largest and the mean absolute
/// difference, and the largest relative to the reference's value.
struct Apart {
    max: f32,
    mean: f64,
    relative: f32,
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
        mean: sum / reference.len() as f64,
        relative,
    }
}

/// Run both paths on `image` and compare the pictures, the masks and
/// the stats; the tolerance is `tolerance` relative to the reference
/// value at each pixel. The GPU's sums run in another order and its
/// compiler may fuse a multiply and an add, so the two are not
/// bit-identical; twenty multiplicative iterations amplify that to
/// the order of 1e-5.
fn check(
    ctx: &Context,
    what: &str,
    image: &WorkingImage,
    options: &SharpenOptions,
    measured: Option<f32>,
    clip_level: f32,
    tolerance: f32,
) -> (SharpenStats, SharpenStats) {
    let mut cpu = image.clone();
    let (cpu_stats, cpu_mask) = sharpen_with_mask(&mut cpu, options, measured, clip_level);
    let mut gpu = image.clone();
    let (gpu_stats, gpu_mask) = ctx
        .sharpen_image(&mut gpu, options, measured, clip_level)
        .expect("the GPU sharpen runs");
    let picture = apart(&cpu.data, &gpu.data);
    let mask = apart(&cpu_mask, &gpu_mask);
    println!(
        "{what}: picture max {:.3e} (relative {:.3e}) mean {:.3e}; mask max {:.3e} mean {:.3e}; \
         stats cpu {cpu_stats:?} gpu {gpu_stats:?}",
        picture.max, picture.relative, picture.mean, mask.max, mask.mean
    );
    assert!(
        picture.relative <= tolerance,
        "{what}: the pictures are {:.3e} apart relative, over {tolerance:.1e}",
        picture.relative
    );
    assert!(
        mask.max <= tolerance,
        "{what}: the masks are {:.3e} apart, over {tolerance:.1e}",
        mask.max
    );
    assert_eq!(cpu_stats.radius, gpu_stats.radius);
    assert_eq!(cpu_stats.iterations, gpu_stats.iterations);
    assert!(
        (cpu_stats.threshold - gpu_stats.threshold).abs() < 1e-6,
        "{what}: thresholds {} and {}",
        cpu_stats.threshold,
        gpu_stats.threshold
    );
    assert!(
        (cpu_stats.blend_mean - gpu_stats.blend_mean).abs() <= tolerance,
        "{what}: blend means {} and {}",
        cpu_stats.blend_mean,
        gpu_stats.blend_mean
    );
    assert!(
        (cpu_stats.clipped - gpu_stats.clipped).abs() <= 1e-6,
        "{what}: clipped {} and {}",
        cpu_stats.clipped,
        gpu_stats.clipped
    );
    (cpu_stats, gpu_stats)
}

const TOLERANCE: f32 = 1e-4;

/// A synthetic picture, tiles partly off its edge, a fixed radius and
/// threshold, and a clipped patch the mask must leave alone.
#[test]
fn a_synthetic_picture_matches_the_reference() {
    let Some(ctx) = context() else {
        return;
    };
    let image = textured(523, 389, 1.0);
    let options = SharpenOptions {
        radius: Radius::Fixed(1.0),
        iterations: 20,
        contrast: Threshold::Fixed(0.02),
        stop_early: true,
    };
    let (cpu, _) = check(&ctx, "fixed", &image, &options, None, 2.0, TOLERANCE);
    assert!(cpu.clipped > 0.0 && cpu.blend_mean > 0.1, "{cpu:?}");
    // A wider point spread takes the widest kernel and border.
    let wide = SharpenOptions {
        radius: Radius::Fixed(1.8),
        iterations: 30,
        ..options
    };
    check(&ctx, "wide", &image, &wide, None, 2.0, TOLERANCE);
    // Without the early stop every block goes the distance.
    let steady = SharpenOptions {
        stop_early: false,
        iterations: 12,
        ..options
    };
    check(&ctx, "no early stop", &image, &steady, None, 2.0, TOLERANCE);
    // The measured radius, and a threshold of zero: the clip mask is
    // the blend.
    let measured = SharpenOptions {
        radius: Radius::Auto,
        contrast: Threshold::Fixed(0.0),
        ..options
    };
    check(
        &ctx,
        "measured, no threshold",
        &image,
        &measured,
        Some(0.6),
        2.0,
        TOLERANCE,
    );
}

/// The automatic threshold: found on the GPU's tile statistics from
/// the same flattest patch the reference finds, on noise at mid grey
/// and on a picture where the coarse pass finds nothing flat.
#[test]
fn the_automatic_threshold_is_the_references() {
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
    let (cpu, gpu) = check(
        &ctx,
        "noise",
        &image,
        &SharpenOptions::default(),
        None,
        10.0,
        TOLERANCE,
    );
    assert!(
        cpu.threshold > 0.0 && cpu.threshold == gpu.threshold,
        "{cpu:?} {gpu:?}"
    );
    // Texture everywhere but one flat corner, too small for the coarse
    // grid: the fine pass and the search around its best.
    let mut busy = textured(400, 300, 0.8);
    for y in 200..300 {
        for x in 250..400 {
            let i = (y * 400 + x) * 3;
            let v = 0.2 + 0.02 * rand();
            busy.data[i..i + 3].copy_from_slice(&[v, v, v]);
        }
    }
    let (cpu, gpu) = check(
        &ctx,
        "busy",
        &busy,
        &SharpenOptions::default(),
        None,
        10.0,
        TOLERANCE,
    );
    assert!(
        cpu.threshold > 0.0 && cpu.threshold == gpu.threshold,
        "{cpu:?} {gpu:?}"
    );
}

/// What the device rejects comes back as an error, not a panic: an
/// output texture the op cannot write to (no storage usage) fails the
/// bind group's validation, which the op's error scope catches. The
/// context is still usable afterwards.
#[test]
fn a_device_error_is_an_error_not_a_panic() {
    let Some(ctx) = context() else {
        return;
    };
    let image = textured(96, 64, 1.0);
    let uploaded = ctx.upload(&image).expect("uploads");
    let wrong = ctx
        .device()
        .create_texture(&greycard_gpu::wgpu::TextureDescriptor {
            label: Some("not writable by a shader"),
            size: greycard_gpu::wgpu::Extent3d {
                width: 96,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: greycard_gpu::wgpu::TextureDimension::D2,
            format: greycard_gpu::wgpu::TextureFormat::Rgba16Float,
            usage: greycard_gpu::wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
    let options = SharpenOptions {
        radius: Radius::Fixed(1.0),
        contrast: Threshold::Fixed(0.02),
        ..Default::default()
    };
    let err = ctx
        .sharpen(&uploaded, &options, None, 2.0, &wrong)
        .expect_err("a texture without storage usage is refused");
    println!("caught: {err}");
    assert!(matches!(err, greycard_gpu::Error::Gpu { .. }), "{err}");
    // And the same op on a proper texture still runs.
    let right = ctx.viewport_texture(96, 64);
    ctx.sharpen(&uploaded, &options, None, 2.0, &right)
        .expect("the context works after a caught error");
}

/// A picture smaller than the coarse grid's tile and the fine grid's
/// margin: the threshold search around the fine grid's best is then
/// the largest of the three searches (441 entries), and the search's
/// buffer has to hold it. A 150x150 overran it once.
#[test]
fn a_small_picture_fits_the_threshold_search() {
    let Some(ctx) = context() else {
        return;
    };
    for (w, h) in [(150, 150), (64, 48), (41, 41)] {
        let image = textured(w, h, 0.8);
        let (cpu, gpu) = check(
            &ctx,
            &format!("{w}x{h}"),
            &image,
            &SharpenOptions::default(),
            None,
            2.0,
            TOLERANCE,
        );
        assert_eq!(cpu.threshold, gpu.threshold, "{w}x{h}");
    }
}

/// A region of a real frame, developed by the engine, under the
/// editor's defaults: the measured radius, the automatic threshold.
/// Wants `GREYCARD_SAMPLES`, a directory with `5M0A3976.CR3` in it.
#[test]
#[ignore]
fn a_real_frame_region_matches_the_reference() {
    let Some(dir) = std::env::var_os("GREYCARD_SAMPLES") else {
        println!("SKIPPED: GREYCARD_SAMPLES is not set");
        return;
    };
    let Some(ctx) = context() else {
        return;
    };
    let path = std::path::Path::new(&dir).join("5M0A3976.CR3");
    let frame = greycard_core::decode::decode_path(&path).expect("the sample decodes");
    let settings = greycard_core::develop::DevelopSettings {
        sharpen: None,
        ..Default::default()
    };
    let developed = greycard_core::develop::develop(&frame, &settings).expect("develops");
    // A region from the middle, tall enough for a few bands of tiles.
    let (rw, rh) = (2048usize, 1536usize);
    let (x0, y0) = (
        (developed.image.width - rw) / 2,
        (developed.image.height - rh) / 2,
    );
    let mut region = WorkingImage::new(rw, rh);
    for y in 0..rh {
        let src = ((y0 + y) * developed.image.width + x0) * 3;
        region.data[y * rw * 3..(y + 1) * rw * 3]
            .copy_from_slice(&developed.image.data[src..src + rw * 3]);
    }
    let (cpu, gpu) = check(
        &ctx,
        "5M0A3976 region",
        &region,
        &SharpenOptions::default(),
        developed.sharpen_radius,
        developed.clip_level,
        TOLERANCE,
    );
    assert!(
        cpu.blend_mean > 0.0 && cpu.threshold == gpu.threshold,
        "{cpu:?} {gpu:?}"
    );
}
