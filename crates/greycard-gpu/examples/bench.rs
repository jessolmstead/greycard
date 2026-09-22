//! Time the sharpen and the CA correction both ways on a raw, and say
//! how far apart they land.
//!
//!     cargo run --release -p greycard-gpu --example bench -- FILE.CR3 [runs]
//!
//! The develop is the engine's default without its sharpen, as the
//! editor makes its base; the sharpen is the editor's default: the
//! measured radius, the automatic threshold, twenty iterations. The
//! CA correction runs on the balanced mosaic as `prepare` hands it
//! to the correction, under the engine's defaults.

use std::time::Instant;

use greycard_core::develop::ca::{CaOptions, correct_ca};
use greycard_core::develop::sharpen::{SharpenOptions, sharpen_with_mask};
use greycard_core::develop::{DevelopSettings, develop, prepare};
use greycard_gpu::Context;

/// How far two planes are apart: the largest absolute and relative
/// differences, the mean, and how many samples are over `step`
/// relative.
fn apart(reference: &[f32], other: &[f32], step: f32) -> String {
    let mut max = 0f32;
    let mut relative = 0f32;
    let mut sum = 0f64;
    let mut steps = 0usize;
    for (&a, &b) in reference.iter().zip(other) {
        let d = (a - b).abs();
        let r = d / a.abs().max(1e-3);
        max = max.max(d);
        relative = relative.max(r);
        sum += f64::from(d);
        if r > step {
            steps += 1;
        }
    }
    format!(
        "max {max:.3e}, relative {relative:.3e}, mean {:.3e}, {steps} of {} over {step:.0e}",
        sum / reference.len() as f64,
        reference.len()
    )
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("a raw file's path"))?;
    let runs: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(3);
    let frame = greycard_core::decode::decode_path(std::path::Path::new(&path))?;
    let settings = DevelopSettings {
        sharpen: None,
        ..Default::default()
    };
    let t = Instant::now();
    let developed = develop(&frame, &settings)?;
    let image = developed.image;
    println!(
        "{}x{} developed in {:.2} s; measured radius {:?}, clip level {}",
        image.width,
        image.height,
        t.elapsed().as_secs_f64(),
        developed.sharpen_radius,
        developed.clip_level
    );
    let options = SharpenOptions::default();
    let (measured, clip) = (developed.sharpen_radius, developed.clip_level);

    let mut cpu = image.clone();
    let mut cpu_stats = None;
    for run in 0..runs {
        cpu = image.clone();
        let t = Instant::now();
        let (stats, _mask) = sharpen_with_mask(&mut cpu, &options, measured, clip);
        println!("cpu run {run}: {:.3} s", t.elapsed().as_secs_f64());
        cpu_stats = Some(stats);
    }

    let t = Instant::now();
    let ctx = Context::own()?;
    println!(
        "gpu: {} (context in {:.3} s)",
        ctx.name(),
        t.elapsed().as_secs_f64()
    );
    let t = Instant::now();
    let uploaded = ctx.upload(&image)?;
    println!("upload: {:.3} s", t.elapsed().as_secs_f64());
    let out = ctx.viewport_texture(uploaded.width(), uploaded.height());
    let mut gpu_stats = None;
    for run in 0..runs {
        let t = Instant::now();
        let stats = ctx.sharpen(&uploaded, &options, measured, clip, &out)?;
        println!(
            "gpu run {run}: {:.3} s{}",
            t.elapsed().as_secs_f64(),
            if run == 0 {
                " (with the threshold search)"
            } else {
                " (threshold remembered)"
            }
        );
        gpu_stats = Some(stats);
    }
    println!("cpu stats {cpu_stats:?}\ngpu stats {gpu_stats:?}");

    // The full-float read back, against the CPU's picture.
    let mut gpu = image.clone();
    let t = Instant::now();
    let (_, _) = ctx.sharpen_image(&mut gpu, &options, measured, clip)?;
    println!("gpu with read back: {:.3} s", t.elapsed().as_secs_f64());
    println!("apart: {}", apart(&cpu.data, &gpu.data, 1e-4));

    // The CA correction, on the mosaic as `prepare` hands it over.
    let settings = DevelopSettings {
        chromatic_aberration: None,
        ..Default::default()
    };
    let prepared = prepare(&frame, &settings, false)?;
    let Some(pattern) = prepared.pattern.as_ref() else {
        println!("no CA correction: not a Bayer mosaic");
        return Ok(());
    };
    let (w, h) = (prepared.width, prepared.height);
    let options = CaOptions::default();
    println!("ca on the {w}x{h} mosaic:");
    let mut cpu = None;
    for run in 0..runs {
        let t = Instant::now();
        let out = correct_ca(&prepared.samples, w, h, pattern, &options)?;
        println!(
            "cpu run {run}: {:.3} s {:?}",
            t.elapsed().as_secs_f64(),
            out.1
        );
        cpu = Some(out);
    }
    let mut gpu = None;
    for run in 0..runs {
        let t = Instant::now();
        let out = ctx.correct_ca(&prepared.samples, w, h, pattern, &options)?;
        println!(
            "gpu run {run}: {:.3} s {:?}{}",
            t.elapsed().as_secs_f64(),
            out.1,
            if run == 0 { " (planes made)" } else { "" }
        );
        gpu = Some(out);
    }
    if let (Some((cpu, _)), Some((gpu, _))) = (cpu, gpu) {
        println!("apart: {}", apart(&cpu, &gpu, 1e-5));
    }
    Ok(())
}
