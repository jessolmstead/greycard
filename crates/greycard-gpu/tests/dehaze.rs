//! The GPU dehaze against its CPU reference, half by half. The block
//! sums are held to the reference's reduced copy to the bit on Vulkan
//! (an add is correctly rounded there and the order is the
//! reference's), so the fit, which is the reference's own code, makes
//! the same model on both paths; the apply is held to an f64
//! evaluation of that model by a bound worked out from each pixel's
//! own magnitudes ([`f64_apply`]), which the CPU's own f32 apply
//! must also meet; and the stats to the reference's on the same model.
//!
//! Without a GPU adapter each test says so and returns; it does not
//! pass in silence. (`cargo test` shows the line with `--nocapture`.)

mod common;

use common::{Rng, Sweep};
use greycard_core::develop::dehaze::{
    self as reference, DehazeOptions, DehazeStats, LUMA, Model, Reduced, dehaze, dehaze_with_model,
    reduce_factor,
};
use greycard_core::develop::local_contrast::LocalContrastOptions;
use greycard_core::image::WorkingImage;
use greycard_gpu::{Context, wgpu};

fn context() -> Option<Context> {
    match Context::own() {
        Ok(c) => Some(c),
        Err(e) => {
            require_gpu(&format!("the GPU dehaze has nothing to run on ({e})"));
            eprintln!("SKIPPED: the GPU dehaze has nothing to run on ({e})");
            println!("SKIPPED: the GPU dehaze has nothing to run on ({e})");
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

/// Whether the device adds as the CPU does: Vulkan requires a float
/// add to be correctly rounded and a compiler to keep the order of a
/// chain of them. Metal compiles with fast math on, under which it may
/// reassociate, so there the block sums are held to the reassociation's
/// reach instead (see [`check_reduced`]).
fn adds_exactly(ctx: &Context) -> bool {
    ctx.device().adapter_info().backend == wgpu::Backend::Vulkan
}

/// A hazy scene: blocks of colored texture, each with one channel
/// near zero as an outdoor scene's patches have, mixed with an
/// airlight by a transmission that falls across the frame, under a
/// band of pure airlight for sky.
fn hazy(w: usize, h: usize, airlight: [f32; 3], seed: u64) -> WorkingImage {
    let mut r = Rng::new(seed, 7);
    let block = 8;
    let bw = w.div_ceil(block);
    let colors: Vec<[f32; 3]> = (0..bw * h.div_ceil(block))
        .map(|_| {
            let mut c = [r.range(0.2, 0.9), r.range(0.2, 0.9), r.range(0.0, 0.02)];
            c.swap(2, r.below(3));
            c
        })
        .collect();
    let mut image = WorkingImage::new(w, h);
    let sky = h / 8;
    for y in 0..h {
        for x in 0..w {
            let c = colors[(y / block) * bw + x / block];
            let t = if y < sky {
                0.0
            } else {
                0.2 + 0.5 * (x as f32 / w as f32)
            };
            let noise = 0.9 + 0.2 * r.unit();
            let px = &mut image.data[(y * w + x) * 3..][..3];
            for i in 0..3 {
                px[i] = c[i] * noise * t + airlight[i] * (1.0 - t);
            }
        }
    }
    image
}

/// How the GPU's reduced copy stands to the reference's on the same
/// picture: to the bit where the device adds exactly; else each block
/// mean within what reassociating its sum could move it, its count
/// times an ulp of the sum of its pixels' magnitudes (divided by the
/// count, as the mean is).
fn check_reduced(ctx: &Context, what: &str, image: &WorkingImage, gpu: &Reduced, cpu: &Reduced) {
    assert_eq!(gpu.grid(), cpu.grid(), "{what}: the grids");
    let (rw, _, factor) = cpu.grid();
    let exact = adds_exactly(ctx);
    let w = image.width;
    for (i, (g, c)) in gpu.means().iter().zip(cpu.means()).enumerate() {
        if exact {
            // Equal as values: a device may drop the sign of a zero.
            assert!(
                g == c,
                "{what}: block mean {i} is {g:e} on the GPU, {c:e} on the CPU"
            );
        } else {
            let (block, channel) = (i / 3, i % 3);
            let (bx, by) = (block % rw, block / rw);
            let mut magnitude = 0f64;
            let mut count = 0usize;
            for y in by * factor..((by + 1) * factor).min(image.height) {
                for x in bx * factor..((bx + 1) * factor).min(w) {
                    magnitude += f64::from(image.data[(y * w + x) * 3 + channel].abs());
                    count += 1;
                }
            }
            let reach = 2.0 * count as f64 * f64::from(f32::EPSILON) * magnitude / count as f64
                + f64::from(f32::EPSILON) * f64::from(c.abs());
            assert!(
                f64::from((g - c).abs()) <= reach,
                "{what}: block mean {i} is {g:e} on the GPU, {c:e} on the CPU, past {reach:e}"
            );
        }
    }
    if exact {
        assert_eq!(gpu.airlight(), cpu.airlight(), "{what}: the airlight");
    }
}

/// The apply in f64 from the same f32 model and taps: what the two f32
/// applies round from. With each sample's bound beside it: the f32
/// apply's error counted operation by operation from that sample's
/// own magnitudes, `e` (half an ulp) for an add, a subtraction or a
/// multiply, which Vulkan requires correctly rounded, and `5 e` (2.5
/// ulps, Vulkan's precision) for a division. A fused multiply and add
/// rounds once where the count has two, so it stays inside. The CPU's
/// divisions are correctly rounded, so it has room over the GPU here.
fn f64_apply(image: &WorkingImage, model: &Model) -> (Vec<f64>, Vec<f64>) {
    let (w, h) = (image.width, image.height);
    let (gw, gh, factor) = model.grid();
    let columns = reference::taps(w, gw, factor);
    let rows = reference::taps(h, gh, factor);
    let (slope, intercept) = (model.slope(), model.intercept());
    let a = model.airlight().map(f64::from);
    let s = f64::from(model.strength());
    let floor = f64::from(reference::MIN_TRANSMISSION);
    let mut exact = vec![0f64; w * h * 3];
    let mut bound = vec![0f64; w * h * 3];
    for (y, &(y0, y1, fy)) in rows.iter().enumerate() {
        let fy = f64::from(fy);
        for (x, &(x0, x1, fx)) in columns.iter().enumerate() {
            let fx = f64::from(fx);
            let corners = [y0 * gw + x0, y0 * gw + x1, y1 * gw + x0, y1 * gw + x1];
            let lerp = |p: &[f32]| {
                let [c00, c01, c10, c11] = corners.map(|i| f64::from(p[i]));
                let top = c00 + (c01 - c00) * fx;
                let bottom = c10 + (c11 - c10) * fx;
                top + (bottom - top) * fy
            };
            let most = |p: &[f32]| {
                corners
                    .iter()
                    .map(|&i| f64::from(p[i].abs()))
                    .fold(0.0, f64::max)
            };
            let i = y * w + x;
            let px: [f64; 3] = std::array::from_fn(|c| f64::from(image.data[i * 3 + c]));
            let lum: f64 = (0..3).map(|c| f64::from(LUMA[c]) * px[c]).sum();
            let lum_mag: f64 = (0..3).map(|c| f64::from(LUMA[c]) * px[c].abs()).sum();
            let line = lerp(slope) * lum + lerp(intercept);
            let t = if s > 0.0 {
                let dark = (0..3)
                    .map(|c| px[c] / a[c])
                    .fold(f64::INFINITY, f64::min)
                    .clamp(0.0, 1.0);
                line.max(1.0 - s * dark).clamp(floor, 1.0)
            } else {
                line.clamp(1.0, 1.0 - s)
            };
            // The transmission's error. A bilinear read of a plane of
            // magnitude `M` is good to `10 e M`: each of the two rows'
            // lerps rounds three times on values up to `2 M` (`5 e M`),
            // and the lerp between them carries a convex share of that
            // plus three more. The luminance, three products and two
            // sums, to `5 e L` of its magnitude `L`; the product of the
            // slope and it to `M L (10 + 5 + 1) e`; the intercept's
            // read and the sum after it to `10 e M' + e |t|`. Counted
            // separately since a steep slope and its intercept can
            // cancel. The bound by the dark channel: a quotient (`5 e`)
            // times the strength (under one), its product and the
            // subtraction from one, under `8 e`. The clamps and the
            // least add none.
            let e = f64::from(f32::EPSILON) / 2.0;
            let err_t = e * (17.0 * most(slope) * lum_mag + 11.0 * most(intercept) + 8.0);
            for c in 0..3 {
                let j = (px[c] - a[c]) / t + a[c];
                exact[i * 3 + c] = j;
                // The recovery's own rounding: the subtraction (`e` of
                // `|I - A|`, at most `|I| + |A|`) and the division
                // (`5 e`) taken through `1 / t`, the sum with `A` (`e`
                // of `|J|`, doubled for the room); and the transmission's
                // error carried through the division, `|I - A| / t^2`
                // times it.
                bound[i * 3 + c] = e * (6.0 * (px[c].abs() + a[c].abs()) / t + 2.0 * j.abs())
                    + (px[c] - a[c]).abs() / (t * t) * err_t;
            }
        }
    }
    (exact, bound)
}

/// How many times its bound the farthest sample of `got` is from the
/// f64 apply, and where.
fn worst_ratio(exact: &[f64], bound: &[f64], got: &[f32]) -> (f64, usize) {
    exact
        .iter()
        .zip(bound)
        .zip(got)
        .map(|((&x, &b), &g)| (f64::from(g) - x).abs() / b.max(f64::MIN_POSITIVE))
        .enumerate()
        .fold(
            (0.0, 0),
            |(m, at), (i, r)| if r > m { (r, i) } else { (m, at) },
        )
}

/// The bound's multiple a sample may reach, on either path: the bound
/// is a worst case counted for the GPU's precisions ([`f64_apply`]),
/// so both are held to it whole. Measured over 1660 cases on NVIDIA
/// (the default seed, seed 1 and 0xdeadbeef) and 1060 on lavapipe: the
/// GPU reaches 0.172 of it on NVIDIA and 0.180 on lavapipe, which
/// rounds as the CPU does, and the CPU 0.180. A shader that read a
/// tap, a weight or a channel wrongly would be thousands of times
/// over.
const GPU_MULTIPLE: f64 = 1.0;
const CPU_MULTIPLE: f64 = 1.0;

/// The GPU's apply of `model` on `image`, read back, and its stats.
fn gpu_dehaze(ctx: &Context, image: &WorkingImage, model: &Model) -> (WorkingImage, DehazeStats) {
    let uploaded = ctx.upload(image).expect("uploads");
    let (out, stats) = ctx.dehaze(&uploaded, model).expect("the GPU dehaze runs");
    (ctx.download(&out).expect("reads back"), stats)
}

/// Run every half on `image` at `options` and hold each to the
/// reference: the reduced copy, the model, the apply and the stats.
/// Returns how far the dehaze moved the picture, at most, and the
/// worst ratio to the bound the GPU reached.
fn compare(ctx: &Context, what: &str, image: &WorkingImage, options: &DehazeOptions) -> (f32, f64) {
    let uploaded = ctx.upload(image).expect("uploads");
    let gpu_reduced = ctx.dehaze_reduced(&uploaded).expect("the block sums run");
    let cpu_reduced = Reduced::new(image);
    check_reduced(ctx, what, image, &gpu_reduced, &cpu_reduced);
    let (Some(model), cpu_model) = (gpu_reduced.model(options), cpu_reduced.model(options)) else {
        // Nothing to do at this amount: the whole op is the identity.
        assert!(cpu_reduced.model(options).is_none(), "{what}");
        let mut gpu = image.clone();
        let stats = ctx.dehaze_image(&mut gpu, options).expect("runs");
        assert_eq!(gpu.data, image.data, "{what}: zero is the identity");
        assert_eq!(stats, reference::identity_stats(options), "{what}");
        return (0.0, 0.0);
    };
    let cpu_model = cpu_model.expect("both fit, or neither");
    if adds_exactly(ctx) {
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(
            bits(model.slope()),
            bits(cpu_model.slope()),
            "{what}: slope"
        );
        assert_eq!(
            bits(model.intercept()),
            bits(cpu_model.intercept()),
            "{what}: intercept"
        );
    }
    let (exact, bound) = f64_apply(image, &model);
    let mut cpu = image.clone();
    let cpu_stats = dehaze_with_model(&mut cpu, &model);
    let (gpu, gpu_stats) = gpu_dehaze(ctx, image, &model);
    let (cpu_ratio, cpu_at) = worst_ratio(&exact, &bound, &cpu.data);
    let (gpu_ratio, gpu_at) = worst_ratio(&exact, &bound, &gpu.data);
    let moved = image
        .data
        .iter()
        .zip(&cpu.data)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    println!(
        "{what}: moved {moved:.3e}; to the bound, gpu {gpu_ratio:.3} at {gpu_at}, cpu {cpu_ratio:.3} at {cpu_at}; \
         airlight {:?}, t mean {} / {}, min {} / {}",
        model.airlight(),
        cpu_stats.transmission_mean,
        gpu_stats.transmission_mean,
        cpu_stats.transmission_min,
        gpu_stats.transmission_min,
    );
    let sample = |at: usize, got: &[f32]| {
        format!(
            "sample {at} (pixel {}, channel {}): input {:e}, f64 {:e}, got {:e}, bound {:e}",
            at / 3,
            at % 3,
            image.data[at],
            exact[at],
            got[at],
            bound[at]
        )
    };
    assert!(
        cpu_ratio <= CPU_MULTIPLE,
        "{what}: the CPU's apply is {cpu_ratio:.3} times its bound: {}",
        sample(cpu_at, &cpu.data)
    );
    assert!(
        gpu_ratio <= GPU_MULTIPLE,
        "{what}: the GPU's apply is {gpu_ratio:.3} times its bound: {}",
        sample(gpu_at, &gpu.data)
    );
    // The stats: the model's own, and the transmission's mean and
    // least within its rounding (the GPU sums a workgroup in f32).
    assert_eq!(gpu_stats.airlight, cpu_stats.airlight, "{what}");
    assert_eq!(gpu_stats.factor, cpu_stats.factor, "{what}");
    assert_eq!(gpu_stats.strength, cpu_stats.strength, "{what}");
    assert_eq!(gpu_stats.amount, cpu_stats.amount, "{what}");
    let near = |a: f32, b: f32| (a - b).abs() <= 1e-5 * a.abs().max(1.0);
    assert!(
        near(gpu_stats.transmission_mean, cpu_stats.transmission_mean),
        "{what}: mean transmission {} on the GPU, {} on the CPU",
        gpu_stats.transmission_mean,
        cpu_stats.transmission_mean
    );
    assert!(
        near(gpu_stats.transmission_min, cpu_stats.transmission_min),
        "{what}: least transmission {} on the GPU, {} on the CPU",
        gpu_stats.transmission_min,
        cpu_stats.transmission_min
    );
    // And the whole op, the reference's signature, where the device
    // adds exactly: the reference's dehaze run on the CPU alone.
    if adds_exactly(ctx) {
        let mut whole = image.clone();
        let whole_stats = ctx.dehaze_image(&mut whole, options).expect("runs");
        let mut reference_whole = image.clone();
        let reference_stats = dehaze(&mut reference_whole, options);
        assert_eq!(whole.data, gpu.data, "{what}: the whole op is its halves");
        assert_eq!(whole_stats.airlight, reference_stats.airlight, "{what}");
        let (whole_ratio, _) = worst_ratio(&exact, &bound, &reference_whole.data);
        assert!(whole_ratio <= CPU_MULTIPLE, "{what}: {whole_ratio}");
    }
    (moved, gpu_ratio)
}

/// A texture's texels, rows packed, `bytes` a texel.
fn read_texture(ctx: &Context, texture: &wgpu::Texture, bytes: u32) -> Vec<u8> {
    let (w, h) = (texture.width(), texture.height());
    let row = (w * bytes).div_ceil(256) * 256;
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
    let mut out = Vec::with_capacity((w * h * bytes) as usize);
    for y in 0..h as usize {
        out.extend_from_slice(&data[y * row as usize..][..(w * bytes) as usize]);
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

#[test]
fn a_hazy_picture_matches_the_reference() {
    let Some(ctx) = context() else {
        return;
    };
    // Factor one, and two with neither side a multiple of it.
    for (w, h) in [(256, 160), (1601, 401)] {
        let image = hazy(w, h, [0.95, 0.85, 0.70], 1);
        for amount in [1.0, 0.5, -0.5, -1.0] {
            let what = format!("hazy {w}x{h} at {amount}");
            let (moved, _) = compare(&ctx, &what, &image, &DehazeOptions { amount });
            assert!(moved > 1e-2, "{what}: the op did nothing worth checking");
        }
    }
}

/// The sizes at the edges of the reduction: a single pixel, sides
/// shorter than a block, a long edge either side of the first
/// reduction (1536 at factor one, 1537 at two), a long edge just past
/// a multiple of the factor, and the longest side a device is asked
/// for, at factor eleven.
#[test]
fn the_edge_sizes_match_the_reference() {
    let Some(ctx) = context() else {
        return;
    };
    for (w, h) in [
        (1, 1),
        (2, 3),
        (5, 17),
        (1536, 7),
        (1537, 7),
        (9, 3073),
        (4609, 5),
        (16384, 3),
        (3, 16384),
    ] {
        let image = hazy(w, h, [0.9, 0.9, 0.85], 2);
        for amount in [0.8, -0.6] {
            compare(
                &ctx,
                &format!("{w}x{h} (factor {}) at {amount}", reduce_factor(w, h)),
                &image,
                &DehazeOptions { amount },
            );
        }
    }
}

#[test]
fn zero_is_the_identity() {
    let Some(ctx) = context() else {
        return;
    };
    let image = hazy(96, 64, [0.9, 0.9, 0.9], 3);
    let mut gpu = image.clone();
    let stats = ctx
        .dehaze_image(&mut gpu, &DehazeOptions { amount: 0.0 })
        .expect("runs");
    assert_eq!(gpu.data, image.data);
    assert_eq!(stats.transmission_mean, 1.0);
}

/// The viewport's texture holds the full-float result to its half
/// float's rounding, its alpha zero, and the stats are the same.
#[test]
fn the_viewport_texture_holds_the_picture_with_alpha_zero() {
    let Some(ctx) = context() else {
        return;
    };
    let image = hazy(300, 200, [0.95, 0.85, 0.70], 4);
    let uploaded = ctx.upload(&image).expect("uploads");
    let model = ctx
        .dehaze_reduced(&uploaded)
        .expect("sums")
        .model(&DehazeOptions { amount: 0.7 })
        .expect("a model");
    let (full, full_stats) = ctx.dehaze(&uploaded, &model).expect("runs");
    let full = ctx.download(&full).expect("reads back");
    let out = ctx.viewport_texture(300, 200);
    let stats = ctx
        .dehaze_to_viewport(&uploaded, &model, &out)
        .expect("runs");
    assert_eq!(stats, full_stats);
    let halves = read_texture(&ctx, &out, 8);
    for (i, px) in halves.as_chunks::<8>().0.iter().enumerate() {
        let h: [f32; 4] =
            std::array::from_fn(|c| half_to_f32(u16::from_le_bytes([px[c * 2], px[c * 2 + 1]])));
        for (&half, &f) in h.iter().zip(&full.data[i * 3..][..3]) {
            // Half an ulp of a half float, either way of rounding.
            assert!(
                (half - f).abs() <= f.abs() * 2f32.powi(-10) + 6e-8,
                "pixel {i}: {half} against {f}"
            );
        }
        assert_eq!(h[3], 0.0, "pixel {i}'s alpha");
    }
}

/// An output texture the op cannot write to fails the bind group's
/// validation, which the op's error scope catches; a texture or a
/// model of another size is declined before anything runs. The
/// context is still usable afterwards.
#[test]
fn a_device_error_is_an_error_not_a_panic() {
    let Some(ctx) = context() else {
        return;
    };
    let image = hazy(96, 64, [0.9, 0.9, 0.9], 5);
    let uploaded = ctx.upload(&image).expect("uploads");
    let model = ctx
        .dehaze_reduced(&uploaded)
        .expect("sums")
        .model(&DehazeOptions { amount: 0.5 })
        .expect("a model");
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
        .dehaze_to_viewport(&uploaded, &model, &wrong)
        .expect_err("a texture without storage usage is refused");
    println!("caught: {err}");
    assert!(matches!(err, greycard_gpu::Error::Gpu { .. }), "{err}");
    let small = ctx.viewport_texture(10, 10);
    let err = ctx
        .dehaze_to_viewport(&uploaded, &model, &small)
        .expect_err("a texture of another size is refused");
    assert!(matches!(err, greycard_gpu::Error::Unsupported(_)), "{err}");
    let other = ctx.upload(&hazy(1700, 20, [0.9; 3], 5)).expect("uploads");
    let err = ctx
        .dehaze(&other, &model)
        .err()
        .expect("a model of another size is refused");
    assert!(matches!(err, greycard_gpu::Error::Unsupported(_)), "{err}");
    let right = ctx.viewport_texture(96, 64);
    ctx.dehaze_to_viewport(&uploaded, &model, &right)
        .expect("the context works after a caught error");
}

/// The kinds of picture the random sweep draws.
#[derive(Debug, Clone, Copy)]
enum Scene {
    /// [`hazy`] under a drawn airlight, from near neutral to colored,
    /// through and under the neutrality floor.
    Hazy,
    /// Blocks of color each with a channel near zero, no haze: what
    /// the prior reads as clear.
    Clear,
    /// Greys with a little color: what the prior reads as haze.
    Neutral,
    /// Hard steps of a few stops across the picture.
    Steps,
    /// Noise about one level, from deep shadow to past white.
    Noise,
    /// One value everywhere: black, a speck, mid grey, white, clipped.
    Flat,
    /// Blocks of pure primaries, black and bright.
    Saturated,
    /// [`Scene::Hazy`] with clipped patches at 1 to 8 laid over it.
    Clipped,
}

/// A picture of `kind`, its numbers from `seed`.
fn drawn_scene(kind: Scene, seed: u64, w: usize, h: usize) -> WorkingImage {
    let mut r = Rng::new(seed, 0);
    let (fw, fh) = (w as f32, h as f32);
    let rect = move |r: &mut Rng| {
        let (x0, y0) = (r.range(0.0, 0.9), r.range(0.0, 0.9));
        let (x1, y1) = (r.range(x0, 1.0), r.range(y0, 1.0));
        move |x: f32, y: f32| x >= x0 * fw && x < x1 * fw && y >= y0 * fh && y < y1 * fh
    };
    let mut image = WorkingImage::new(w, h);
    match kind {
        Scene::Hazy | Scene::Clipped => {
            let grey = r.range(0.3, 1.2);
            let tint = [r.range(0.6, 1.1), r.range(0.6, 1.1), r.range(0.1, 1.1)];
            image = hazy(w, h, tint.map(|t| t * grey), r.next());
            if let Scene::Clipped = kind {
                for _ in 0..r.between(1, 4) {
                    let at = rect(&mut r);
                    let level = r.range(1.0, 8.0);
                    let (pixels, _) = image.data.as_chunks_mut::<3>();
                    for (i, px) in pixels.iter_mut().enumerate() {
                        if at((i % w) as f32, (i / w) as f32) {
                            *px = [level; 3];
                        }
                    }
                }
            }
        }
        Scene::Clear | Scene::Neutral => {
            let block = r.between(1, 24);
            let bw = w.div_ceil(block);
            let colors: Vec<[f32; 3]> = (0..bw * h.div_ceil(block))
                .map(|_| match kind {
                    Scene::Clear => {
                        let mut c = [r.range(0.2, 0.9), r.range(0.2, 0.9), r.range(0.0, 0.02)];
                        c.swap(2, r.below(3));
                        c
                    }
                    _ => {
                        let g = r.range(0.3, 0.8);
                        [0; 3].map(|_| g * r.range(0.9, 1.1))
                    }
                })
                .collect();
            let (pixels, _) = image.data.as_chunks_mut::<3>();
            for (i, px) in pixels.iter_mut().enumerate() {
                let (x, y) = (i % w, i / w);
                let shade = 0.5 + 0.5 * (x + y) as f32 / (w + h) as f32;
                *px = colors[(y / block) * bw + x / block].map(|c| c * shade);
            }
        }
        Scene::Steps => {
            let level = r.range(0.01, 1.0);
            let (across, down) = (r.between(1, 6), r.between(1, 6));
            let stops: Vec<f32> = (0..across + down).map(|_| r.range(-8.0, 4.0)).collect();
            let cuts: Vec<f32> = (0..across + down).map(|_| r.range(0.0, 1.0)).collect();
            let cast = [r.range(0.5, 1.0), 1.0, r.range(0.0, 1.0)];
            let (pixels, _) = image.data.as_chunks_mut::<3>();
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
                *px = cast.map(|c| c * v);
            }
        }
        Scene::Noise => {
            let level = 10f32.powf(r.range(-4.0, 0.7));
            let amount = r.range(0.0, 0.5);
            let mut noise = Rng::new(seed, 1);
            let (pixels, _) = image.data.as_chunks_mut::<3>();
            for px in pixels.iter_mut() {
                let v = level * (1.0 + amount * (noise.unit() - 0.5));
                *px = [v, v * 0.9, v * 1.1];
            }
        }
        Scene::Flat => {
            let v = r.pick(&[0.0, 1e-7, 0.18, 1.0, 8.0]);
            image.data.fill(v);
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
                *px = colors[choice[(y / block) * across + x / block]].map(|c| c * level);
            }
        }
    }
    image
}

/// One random case: the picture, the dehaze's amount, and the local
/// contrast run on the GPU before it, as the viewport runs them, or
/// none.
#[derive(Debug)]
struct Drawn {
    width: usize,
    height: usize,
    scene: Scene,
    scene_seed: u64,
    amount: f32,
    local_contrast: Option<(LocalContrastOptions, f32)>,
}

fn draw(r: &mut Rng) -> Drawn {
    let side = |r: &mut Rng| {
        if r.chance(0.1) {
            r.between(1, 12)
        } else {
            r.between(13, 500)
        }
    };
    // Now and then a long edge past the first reduction, at factors
    // two to eleven, on a short side to keep the run quick: anywhere,
    // just either side of a multiple of the factor, or 105 to 111 past
    // a multiple of 112, the CA's tiles' edge, which a picture
    // that reaches the dehaze has been through.
    let (long, short) = match r.below(20) {
        0..=3 => (r.between(1530, 6200), r.between(1, 64)),
        4 | 5 => {
            let long = r.between(1537, 9000);
            let factor = long.div_ceil(1536);
            let near = (long / factor) * factor;
            (
                (near + r.pick(&[0, 1, factor - 1])).max(1537),
                r.between(1, 48),
            )
        }
        6 => (
            112 * r.between(14, 70) + r.between(105, 111),
            r.between(1, 40),
        ),
        7 => (r.between(8000, 16384), r.between(1, 12)),
        _ => (side(r), side(r)),
    };
    let (width, height) = if r.chance(0.5) {
        (long, short)
    } else {
        (short, long)
    };
    let scene = r.pick(&[
        Scene::Hazy,
        Scene::Hazy,
        Scene::Clear,
        Scene::Neutral,
        Scene::Steps,
        Scene::Noise,
        Scene::Flat,
        Scene::Saturated,
        Scene::Clipped,
    ]);
    let scene_seed = r.next();
    let amount = r.signed(-1.0, 1.0);
    let local_contrast = r.chance(0.4).then(|| {
        let options = LocalContrastOptions {
            texture: r.signed(-1.0, 1.0),
            clarity: r.signed(-1.0, 1.0),
        };
        let clip = match r.below(4) {
            0 => f32::INFINITY,
            _ => r.range(0.98, 8.0),
        };
        (options, clip)
    });
    Drawn {
        width,
        height,
        scene,
        scene_seed,
        amount,
        local_contrast,
    }
}

/// The GPU dehaze against the reference at random settings, on random
/// pictures: the amount over the panel's range with its ends, zero and
/// small values weighted, negative too (haze put in); the local
/// contrast on the GPU before it or not, as the viewport runs them;
/// the picture's size (factors one to eleven, sides that are not a
/// multiple of the factor, tiny pictures, widths 105 to 111 past a
/// multiple of 112) and kind (hazy, clear, neutral, steps, noise,
/// flat from black to clipped, saturated, clipped patches). Held to
/// [`compare`]. With the local contrast on, the dehaze is checked on
/// the GPU's local contrast output, the pixels it reads in the
/// viewport; how far the CPU's local contrast would have moved its
/// fit is printed beside, measured, not held.
#[test]
fn the_gpu_dehaze_is_the_reference_over_random_settings() {
    let Some(ctx) = context() else {
        return;
    };
    let device = ctx.device().adapter_info().name;
    let mut worst = 0f64;
    let mut drift = (0f32, 0f32);
    Sweep::from_env(60).run("dehaze", &device, draw, |what, d| {
        let mut image = drawn_scene(d.scene, d.scene_seed, d.width, d.height);
        let options = DehazeOptions { amount: d.amount };
        if let Some((lc, clip)) = &d.local_contrast {
            let mut cpu = image.clone();
            greycard_core::develop::local_contrast::local_contrast(&mut cpu, lc, *clip);
            ctx.local_contrast_image(&mut image, lc, *clip)
                .expect("the GPU local contrast runs");
            // The fit the export would make, from the CPU's local
            // contrast, against the viewport's, from the GPU's.
            if let (Some(a), Some(b)) = (
                Reduced::new(&cpu).model(&options),
                Reduced::new(&image).model(&options),
            ) {
                let (mut x, mut y) = (cpu.clone(), image.clone());
                let (sx, sy) = (dehaze_with_model(&mut x, &a), dehaze_with_model(&mut y, &b));
                let da = (0..3)
                    .map(|c| (sx.airlight[c] - sy.airlight[c]).abs())
                    .fold(0.0, f32::max);
                let dt = (sx.transmission_mean - sy.transmission_mean).abs();
                println!("{what}: the CPU's local contrast moves the airlight {da:.3e}, the mean transmission {dt:.3e}");
                drift.0 = drift.0.max(da);
                drift.1 = drift.1.max(dt);
            }
        }
        let (_, ratio) = compare(&ctx, what, &image, &options);
        worst = worst.max(ratio);
    });
    println!(
        "dehaze sweep: the GPU's worst apply {worst:.3} of its bound; the CPU's local contrast moved \
         the airlight {:.3e} and the mean transmission {:.3e} at most",
        drift.0, drift.1
    );
}
