//! The GPU sharpen against its CPU reference, on the same pictures.
//!
//! Without a GPU adapter each test says so and returns; it does not
//! pass in silence. (`cargo test` shows the line with `--nocapture`.)

mod common;

use common::{Rng, Sweep};
use greycard_core::develop::sharpen::{
    self as reference, Radius, SharpenOptions, SharpenStats, Threshold, sharpen_with_mask,
};
use greycard_core::image::WorkingImage;
use greycard_gpu::Context;

fn context() -> Option<Context> {
    match Context::own() {
        Ok(c) => Some(c),
        Err(e) => {
            require_gpu(&format!("the GPU sharpen has nothing to run on ({e})"));
            eprintln!("SKIPPED: the GPU sharpen has nothing to run on ({e})");
            println!("SKIPPED: the GPU sharpen has nothing to run on ({e})");
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
    apart_from(reference, reference, other)
}

/// With the early stop off, where the deconvolution darkens a pixel
/// to under this share of its input, the pixel is measured against
/// this share of its input instead. (Nothing ships without the early
/// stop, so the shipped path keeps the plain measure.) Without it,
/// fifty iterations take a pure blue
/// beside bright primaries from 2.66 to 0.0016; its luminance is then
/// a small difference of its neighbors' bright ones and carries their
/// rounding, about 1e-7 of the input, which is 1.6e-4 of the output.
/// The reference itself moves by 1.3e-4 of such an output when its
/// input moves by an ulp.
const DARKENED: f32 = 0.01;

/// [`apart`], relative to the larger of the reference's value and
/// [`DARKENED`] of the input's.
fn apart_from(input: &[f32], reference: &[f32], other: &[f32]) -> Apart {
    assert_eq!(reference.len(), other.len());
    let mut max = 0f32;
    let mut relative = 0f32;
    let mut sum = 0f64;
    for ((&a, &b), &i) in reference.iter().zip(other).zip(input) {
        let d = (a - b).abs();
        max = max.max(d);
        relative = relative.max(d / a.abs().max(DARKENED * i.abs()).max(1e-3));
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
/// value at each pixel (or, with the early stop off, to [`DARKENED`]
/// of the input's, if more).
/// The GPU's sums run in another order and its compiler may fuse a
/// multiply and an add, so the two are not bit-identical; twenty
/// multiplicative iterations amplify that to the order of 1e-5. The
/// mask where it is steep is [`check_steep_mask`]'s.
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
    let picture = if options.stop_early {
        apart(&cpu.data, &gpu.data)
    } else {
        apart_from(&image.data, &cpu.data, &gpu.data)
    };
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
    if mask.max > tolerance {
        check_steep_mask(
            what,
            image,
            options,
            measured,
            clip_level,
            cpu_stats.threshold,
            (&cpu_mask, &gpu_mask),
            tolerance,
        );
    }
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

/// How far apart the local contrast the blend mask reads can be on the
/// two paths, in the contrast's own units, with room. The GPU's L* is
/// a power and a Newton step where the reference's is `cbrt`, and its
/// luminance may be a fused sum: the two agree to a few ulps of L*,
/// about 3e-5 at most near 100. The contrast is a sixteenth of the
/// root of four squared differences of L*, so it moves by a quarter of
/// that, 7.5e-6. The blend reads the contrast only as its ratio to
/// the threshold, so moving the threshold by this moves the sigmoid
/// as moving a pixel's contrast by this times that ratio: it covers
/// the rounding wherever the contrast is over 0.375 of the threshold,
/// and under that the sigmoid is flat to 5e-4 a unit of its argument.
const CONTRAST_ROUNDING: f32 = 2e-5;

/// Of the mask's pixels, the share that may lie past the plain
/// tolerance and inside the bracket. Seeds 1 to 10 of the random
/// sweep, on NVIDIA, RADV and lavapipe, reach 0.3%; a broad break
/// cannot pass as steepness.
const STEEP_SHARE: f64 = 0.01;

/// The blend mask where the plain tolerance is not met. The mask is
/// a steep sigmoid of the local contrast, half at the threshold and
/// sloped there at eight over it, and the contrast is a difference of
/// neighboring L*: in a nearly flat patch at a small threshold one
/// ulp of the picture moves the reference's own mask by more than the
/// tolerance (2e-4 at a threshold of 0.016, 7e-4 at 0.001, measured
/// with each sample moved by an ulp of either sign). So each pixel is
/// held instead to the reference's masks at the threshold moved by
/// [`CONTRAST_ROUNDING`] either way (down to half of itself at most),
/// within the tolerance; the GPU
/// value must lie inside that range, so a NaN or a black there still
/// fails, and the pixels that needed it are capped at
/// [`STEEP_SHARE`].
#[allow(clippy::too_many_arguments)]
fn check_steep_mask(
    what: &str,
    image: &WorkingImage,
    options: &SharpenOptions,
    measured: Option<f32>,
    clip_level: f32,
    threshold: f32,
    (cpu, gpu): (&[f32], &[f32]),
    tolerance: f32,
) {
    assert!(
        threshold > 0.0,
        "{what}: the masks are {:.3e} apart with no threshold, where the mask is the clip mask",
        apart(cpu, gpu).max
    );
    let at = |t: f32| {
        let moved = SharpenOptions {
            contrast: Threshold::Fixed(t),
            ..*options
        };
        sharpen_with_mask(&mut image.clone(), &moved, measured, clip_level).1
    };
    // Under twice the rounding the threshold would move by more than
    // half of itself, which is no longer its rounding; it stops there.
    let (lower, upper) = (
        at(threshold + CONTRAST_ROUNDING),
        at((threshold - CONTRAST_ROUNDING).max(0.5 * threshold)),
    );
    let mut steep = 0usize;
    let mut outside = 0usize;
    let mut widest = 0f32;
    let mut worst = 0f32;
    for i in 0..cpu.len() {
        let lo = cpu[i].min(lower[i]).min(upper[i]);
        let hi = cpu[i].max(lower[i]).max(upper[i]);
        let g = gpu[i];
        widest = widest.max(hi - lo);
        if (g - cpu[i]).abs() > tolerance {
            steep += 1;
        }
        let past = (lo - g).max(g - hi);
        if past.is_nan() || past > tolerance {
            outside += 1;
            worst = worst.max(if past.is_nan() { f32::INFINITY } else { past });
        }
    }
    let share = steep as f64 / cpu.len() as f64;
    println!(
        "{what}: the mask is steep at threshold {threshold}: {steep} pixels ({:.3}%) past the \
         tolerance, the widest bracket {widest:.3e}, {outside} outside it",
        100.0 * share
    );
    assert!(
        outside == 0,
        "{what}: {outside} mask pixels lie outside the reference's bracket at threshold \
         {threshold} +- {CONTRAST_ROUNDING:.0e}, the worst by {worst:.3e}"
    );
    assert!(
        share <= STEEP_SHARE,
        "{what}: {:.3}% of the mask needed the bracket, over {:.1}%",
        100.0 * share,
        100.0 * STEEP_SHARE
    );
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

/// The kinds of picture the random sweep draws, each with its numbers
/// drawn from the case's own seed.
#[derive(Debug, Clone, Copy)]
enum Scene {
    /// Bars, spots, hot patches, grain and a color cast, blurred:
    /// [`textured`] with its numbers drawn.
    Textured,
    /// Noise about one grey, where the automatic threshold finds a
    /// flat tile, or none when the grey is out of the search's range.
    Noise,
    /// Textured, with a flat noisy patch somewhere in it for the fine
    /// search to find.
    Busy,
    /// Textured, but deep in the shadows: luminance near the factor's
    /// floor and L* under the tile search's range.
    Dark,
    /// Blocks of pure primaries, black and bright: a channel at zero,
    /// a luminance at zero.
    Saturated,
}

/// A picture of `scene`'s kind, its numbers from `seed`.
fn drawn_picture(scene: Scene, seed: u64, w: usize, h: usize) -> WorkingImage {
    let mut r = Rng::new(seed, 0);
    let mut noise = Rng::new(seed, 1);
    let mut rand = move || noise.unit() - 0.5;
    let mut image = WorkingImage::new(w, h);
    match scene {
        Scene::Textured | Scene::Busy | Scene::Dark => {
            let (across, down) = (r.between(2, 12), r.between(2, 10));
            let (dark, light) = (r.range(0.0, 0.3), r.range(0.2, 0.9));
            let modulus = r.between(17, 151);
            let spots = r.between(1, modulus / 4);
            let spot = r.range(0.0, 0.5);
            let (hot_across, hot_down) = (r.between(20, 90), r.between(20, 90));
            let hot = if r.chance(0.7) {
                r.range(1.0, 4.0)
            } else {
                0.0
            };
            let corner = r.between(0, 60);
            let flat = r.range(0.0, 0.5);
            let grain = r.range(0.0, 0.03);
            let sigma = r.range(0.5, 1.6);
            let cast = r.range(0.0, 0.6);
            let mut lum = vec![0f32; w * h];
            for y in 0..h {
                for x in 0..w {
                    let bars = if (x / across + y / down) % 2 == 0 {
                        light
                    } else {
                        dark
                    };
                    let spotted = if (x * x + y * y) % modulus < spots {
                        spot
                    } else {
                        0.0
                    };
                    let base = if x + corner >= w && y + corner >= h {
                        flat
                    } else {
                        bars + spotted
                    };
                    let patch = if (x / hot_across) % 5 == 2 && (y / hot_down) % 4 == 1 {
                        hot
                    } else {
                        0.0
                    };
                    lum[y * w + x] = (base + patch) * (1.0 + grain * rand());
                }
            }
            blur(&mut lum, w, h, sigma);
            if let Scene::Busy = scene {
                let (pw, ph) = (r.between(30, 200).min(w), r.between(30, 200).min(h));
                let (px, py) = (r.between(0, w - pw), r.between(0, h - ph));
                let (level, amount) = (r.range(0.03, 0.35), r.range(0.03, 0.15));
                for y in py..py + ph {
                    for x in px..px + pw {
                        lum[y * w + x] = level * (1.0 + amount * rand());
                    }
                }
            }
            let scale = if let Scene::Dark = scene {
                r.range(1e-4, 0.01)
            } else {
                1.0
            };
            let (pixels, _) = image.data.as_chunks_mut::<3>();
            for (i, px) in pixels.iter_mut().enumerate() {
                let (x, y) = (i % w, i / w);
                let v = lum[i] * scale;
                let tint = 1.0 - cast / 2.0 + cast * (x as f32 / w as f32);
                px[0] = v * tint;
                px[1] = v;
                px[2] = v * (2.0 - tint) * (0.5 + 0.5 * (y as f32 / h as f32));
            }
        }
        Scene::Noise => {
            // Mostly inside the tile search's L* range (about 0.007 to
            // 0.3), at a noise it can call flat or nearly (about 0.04
            // to 0.17 of the level, peak to peak, at mid grey; under
            // it a tile is suspiciously flat); now and then outside
            // either.
            let level = if r.chance(0.8) {
                r.range(0.02, 0.35)
            } else {
                r.pick(&[0.003, 0.7])
            };
            let amount = r.range(0.02, 0.2);
            let cast = [r.range(0.8, 1.2), 1.0, r.range(0.8, 1.2)];
            for px in image.data.chunks_mut(3) {
                let v = level * (1.0 + amount * rand());
                for c in 0..3 {
                    px[c] = v * cast[c];
                }
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
            let (pixels, _) = image.data.as_chunks_mut::<3>();
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
    options: SharpenOptions,
    measured: Option<f32>,
    clip_level: f32,
}

/// A side for the sweep: mostly modest, sometimes tiny, sometimes on
/// either side of a block, the fine search's tile or the coarse one.
fn drawn_side(r: &mut Rng) -> usize {
    let u = r.unit();
    if u < 0.08 {
        r.between(1, 12)
    } else if u < 0.25 {
        r.pick(&[
            31, 32, 33, 39, 40, 41, 49, 50, 51, 79, 80, 81, 159, 160, 161,
        ])
    } else {
        r.between(13, 360)
    }
}

fn draw(r: &mut Rng) -> Drawn {
    // A tall picture now and then takes the wider tiles (`tile_for`).
    let (width, height) = match r.below(50) {
        0..=3 => {
            let h = if r.chance(0.3) {
                r.pick(&[1984, 1985])
            } else {
                r.between(1985, 2100)
            };
            (r.between(8, 64), h)
        }
        4 => {
            let tall = r.between(3969, 4100);
            (r.between(8, 32), r.pick(&[3968, 3969, tall]))
        }
        _ => (drawn_side(r), drawn_side(r)),
    };
    let scene = r.pick(&[
        Scene::Textured,
        Scene::Noise,
        Scene::Noise,
        Scene::Busy,
        Scene::Busy,
        Scene::Dark,
        Scene::Saturated,
    ]);
    let scene_seed = r.next();
    // The panel's ranges: radius 0.4 to 2, iterations 5 to 50, a
    // threshold of 0 to 0.5, each automatic or fixed. A fixed radius
    // sits on one of the kernel's width steps now and then, and the
    // iterations on the border's.
    let radius = if r.chance(0.5) {
        Radius::Auto
    } else if r.chance(0.15) {
        Radius::Fixed(r.pick(&[0.6f32, 0.84, 1.15, 1.5]) + r.pick(&[-1e-3f32, 0.0, 1e-3]))
    } else {
        Radius::Fixed(r.slider(reference::MIN_RADIUS, reference::MAX_RADIUS))
    };
    let measured = (!r.chance(0.3)).then(|| r.slider(0.3, 2.4));
    let iterations = if r.chance(0.1) {
        r.pick(&[30, 31])
    } else {
        r.whole(5, 50)
    };
    let contrast = if r.chance(0.5) {
        Threshold::Auto
    } else if r.chance(0.2) {
        Threshold::Fixed(r.range(0.0, 0.05))
    } else {
        Threshold::Fixed(r.slider(0.0, 0.5))
    };
    let stop_early = r.chance(0.8);
    // The clip level is the frame's ceiling's, at least 0.98.
    let clip_level = match r.below(10) {
        0 => f32::INFINITY,
        1 | 2 => 0.98,
        3 | 4 => 8.0,
        _ => r.range(0.98, 8.0),
    };
    Drawn {
        width,
        height,
        scene,
        scene_seed,
        options: SharpenOptions {
            radius,
            iterations,
            contrast,
            stop_early,
        },
        measured,
        clip_level,
    }
}

/// The GPU sharpen against the reference at random settings, on
/// random pictures: the radius, iterations and threshold over the
/// panel's ranges, ends and steps weighted, and the early stop on or
/// off, which goes beyond the editor, which always stops early; the
/// measured radius or none, the clip level, the picture's size and
/// kind. Held to the fixed tests' `check`.
#[test]
fn the_gpu_sharpen_is_the_reference_over_random_settings() {
    let Some(ctx) = context() else {
        return;
    };
    let device = ctx.device().adapter_info().name;
    Sweep::from_env(30).run("sharpen", &device, draw, |what, d| {
        let image = drawn_picture(d.scene, d.scene_seed, d.width, d.height);
        check(
            &ctx,
            what,
            &image,
            &d.options,
            d.measured,
            d.clip_level,
            TOLERANCE,
        );
    });
}
