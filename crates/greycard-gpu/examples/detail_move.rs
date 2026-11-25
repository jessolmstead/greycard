//! Time what a Clarity or a Dehaze move runs again, stage by stage, on
//! a picture made here at a camera's size, no raw needed.
//!
//!     cargo run --release -p greycard-gpu --example detail_move -- [megapixels] [runs] [cpu]
//!
//! The picture is a working image of a sky over textured ground with
//! grain, 102 MP (11648 x 8736) unless `megapixels` says 45 (8256 x
//! 5504) or 24 (6000 x 4000). Each stage is waited for before the next
//! is timed, so the times add up to a move; the medians of `runs` runs
//! after one to warm up. With `cpu`, the CPU's references are timed
//! too.

use std::time::Instant;

use greycard_core::develop::dehaze::{DehazeOptions, dehaze};
use greycard_core::develop::local_contrast::{LocalContrastOptions, local_contrast};
use greycard_core::develop::sharpen::{
    Part, SharpenOptions, iterations_seen_at, sharpen_with_mask,
};
use greycard_core::image::WorkingImage;
use greycard_gpu::Context;

/// A sky over textured ground, with grain: flat enough at the top for
/// the automatic threshold to find a patch, detailed below.
pub fn scene(w: usize, h: usize) -> WorkingImage {
    let mut image = WorkingImage::new(w, h);
    let hash = |x: usize, y: usize, c: usize| {
        let mut v = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
            ^ (c as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
        v ^= v >> 31;
        v = v.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        v ^= v >> 29;
        (v >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };
    let horizon = h * 3 / 10;
    for (y, row) in image.data.chunks_exact_mut(w * 3).enumerate() {
        for x in 0..w {
            let (fx, fy) = (x as f32, y as f32);
            let base = if y < horizon {
                let t = fy / horizon as f32;
                [0.18 + 0.1 * t, 0.25 + 0.1 * t, 0.45 + 0.05 * t]
            } else {
                let v = (fx * 0.11).sin() * (fy * 0.07).cos()
                    + (fx * 0.031 + fy * 0.052).sin()
                    + ((fx * 0.9 + fy * 0.37) * 0.23).sin() * 0.5
                    + ((fx - fy) * 0.017).cos() * 0.7
                    + if (x / 97 + y / 61) % 2 == 0 {
                        0.8
                    } else {
                        -0.8
                    };
                let l = 0.12 + 0.1 * v / 3.0;
                [l * 1.1, l, l * 0.7]
            };
            for c in 0..3 {
                row[x * 3 + c] = (base[c] + 0.006 * hash(x, y, c)).max(0.0);
            }
        }
    }
    image
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mp: u32 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(102);
    let runs: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(3);
    let cpu = args.next().is_some_and(|s| s == "cpu");
    let (w, h) = match mp {
        24 => (6000, 4000),
        45 => (8256, 5504),
        _ => (11648, 8736),
    };
    let t = Instant::now();
    let image = scene(w, h);
    println!("{w}x{h} made in {:.2} s", t.elapsed().as_secs_f64());
    let clip = 1.0;
    let measured = Some(0.75);
    let lc = LocalContrastOptions {
        texture: 0.0,
        clarity: 0.5,
        structure: 0.0,
    };
    let haze = DehazeOptions { amount: 0.3 };
    let sharpen = SharpenOptions::default();
    let planes_only = SharpenOptions {
        iterations: 0,
        ..sharpen
    };

    let ctx = Context::own()?;
    println!("gpu: {}", ctx.name());
    let wait = || {
        ctx.device()
            .poll(greycard_gpu::wgpu::PollType::wait_indefinitely())
    };
    let t = Instant::now();
    let uploaded = ctx.upload(&image)?;
    wait()?;
    println!("upload {:.3} s", t.elapsed().as_secs_f64());
    let out = ctx.viewport_texture(w as u32, h as u32);

    let mut rows: Vec<(&str, Vec<f64>)> = Vec::new();
    let mut time = |name: &'static str, f: &mut dyn FnMut() -> anyhow::Result<()>| {
        f()?;
        let mut times = Vec::new();
        for _ in 0..runs {
            let t = Instant::now();
            f()?;
            times.push(t.elapsed().as_secs_f64());
        }
        println!("{name}: median {:.3} s {times:.3?}", median(times.clone()));
        rows.push((name, times));
        anyhow::Ok(())
    };

    // A Clarity move: the local contrast, then the sharpen guided by
    // the picture before it, whose threshold is remembered.
    let (after, _) = ctx.local_contrast(&uploaded, &lc, clip)?;
    time("gpu local contrast (Clarity 0.5)", &mut || {
        ctx.local_contrast(&uploaded, &lc, clip)?;
        wait()?;
        Ok(())
    })?;
    time("gpu sharpen, guided, 20 iterations", &mut || {
        ctx.sharpen(&after, Some(&uploaded), &sharpen, measured, clip, &out)?;
        wait()?;
        Ok(())
    })?;
    // The picture's bits, for holding a change to the shader that must
    // not move them to the build before it.
    let bits = ctx.read_viewport_rows(&out, 0, h as u32)?;
    let hash = bits.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    println!("the sharpened picture's hash: {hash:016x}");
    time("gpu sharpen, guided, planes and apply alone", &mut || {
        ctx.sharpen(&after, Some(&uploaded), &planes_only, measured, clip, &out)?;
        wait()?;
        Ok(())
    })?;
    // What a move's first round sharpens: the tiles a 2560 by 1440
    // view shows at 1:1 in the middle, with the window's margin; and
    // every tile at the iterations the fit of that view needs.
    let (vw, vh) = (2560usize, 1440usize);
    let part = Part {
        x0: (w - vw) / 2 - 33,
        y0: (h - vh) / 2 - 33,
        x1: (w + vw) / 2 + 33,
        y1: (h + vh) / 2 + 33,
    };
    let seen_close = SharpenOptions {
        part: Some(part),
        ..sharpen
    };
    time(
        "gpu sharpen, guided, a 2560x1440 view's tiles at 1:1",
        &mut || {
            ctx.sharpen(&after, Some(&uploaded), &seen_close, measured, clip, &out)?;
            wait()?;
            Ok(())
        },
    )?;
    let fit = (vw as f32 / w as f32).min(vh as f32 / h as f32);
    let seen_fit = SharpenOptions {
        iterations: iterations_seen_at(fit, 0.75, sharpen.iterations),
        ..sharpen
    };
    println!(
        "the fit of a 2560x1440 view: zoom {fit:.3}, {} iterations",
        seen_fit.iterations
    );
    time("gpu sharpen, guided, the fit's iterations", &mut || {
        ctx.sharpen(&after, Some(&uploaded), &seen_fit, measured, clip, &out)?;
        wait()?;
        Ok(())
    })?;
    // A Dehaze move: the reduced copy, its fit, the dehaze, and the
    // sharpen on a new picture, whose threshold is searched for.
    time("gpu dehaze (reduced copy, fit, apply)", &mut || {
        let reduced = ctx.dehaze_reduced(&uploaded)?;
        let model = reduced.model(&haze).expect("a strength");
        ctx.dehaze(&uploaded, &model)?;
        wait()?;
        Ok(())
    })?;
    time(
        "gpu sharpen on a new picture (threshold searched)",
        &mut || {
            let reduced = ctx.dehaze_reduced(&uploaded)?;
            let model = reduced.model(&haze).expect("a strength");
            let (dehazed, _) = ctx.dehaze(&uploaded, &model)?;
            wait()?;
            let t = Instant::now();
            ctx.sharpen(&dehazed, None, &sharpen, measured, clip, &out)?;
            wait()?;
            println!("  sharpen alone {:.3} s", t.elapsed().as_secs_f64());
            Ok(())
        },
    )?;

    if cpu {
        time("cpu local contrast (Clarity 0.5)", &mut || {
            let mut p = image.clone();
            local_contrast(&mut p, &lc, clip);
            Ok(())
        })?;
        let mut after = image.clone();
        local_contrast(&mut after, &lc, clip);
        time("cpu sharpen, guided, 20 iterations", &mut || {
            let mut p = after.clone();
            sharpen_with_mask(&mut p, Some(&image), &sharpen, measured, clip);
            Ok(())
        })?;
        time("cpu sharpen, guided, planes alone", &mut || {
            let mut p = after.clone();
            sharpen_with_mask(&mut p, Some(&image), &planes_only, measured, clip);
            Ok(())
        })?;
        time("cpu dehaze", &mut || {
            let mut p = image.clone();
            dehaze(&mut p, &haze);
            Ok(())
        })?;
    }
    println!("| {w}x{h} | median of {runs} |\n|---|---|");
    for (name, times) in rows {
        println!("| {name} | {:.3} s |", median(times));
    }
    Ok(())
}
