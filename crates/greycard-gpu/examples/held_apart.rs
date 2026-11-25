//! How far a sharpen of fewer iterations lands from the full one as
//! the viewport shows it below 1:1.
//!
//!     cargo run --release -p greycard-gpu --example held_apart -- FILE [zoom ...]
//!     cargo run --release -p greycard-gpu --example held_apart -- FILE reorder
//!
//! The raw is developed as the bench develops it, sharpened at twenty
//! iterations and at fewer, and each shown at every zoom as the
//! viewport's minify shows it (a mean of up to four by four bilinear
//! taps over a view pixel's footprint), brought to a mid grey at its
//! median and encoded sRGB in 8 bits. The differences from the full
//! sharpen are in those levels; the unsharpened picture is the scale.
//!
//! With `reorder`, how far a sharpen moved ahead of the Detail section
//! lands from today's order, at 1:1 and at the fits: Clarity and
//! Texture at +50 (today's sharpen guided by the picture before them)
//! and Dehaze at +30 (today's unguided).

use greycard_core::develop::dehaze::{DehazeOptions, dehaze};
use greycard_core::develop::local_contrast::{LocalContrastOptions, local_contrast};
use greycard_core::develop::sharpen::{SharpenOptions, sharpen_with_mask};
use greycard_core::develop::{DevelopSettings, develop};
use greycard_core::image::WorkingImage;

/// The viewport's minify: `n` by `n` bilinear taps over each target
/// pixel's footprint at `zoom`, the picture's center at the view's.
fn shown(image: &WorkingImage, zoom: f32) -> (Vec<[f32; 3]>, usize, usize) {
    let (w, h) = (image.width, image.height);
    let (tw, th) = (
        (w as f32 * zoom).floor() as usize,
        (h as f32 * zoom).floor() as usize,
    );
    let n = ((1.0 / zoom).ceil() as usize).clamp(1, 4);
    let bilinear = |x: f32, y: f32| -> [f32; 3] {
        let (gx, gy) = (x - 0.5, y - 0.5);
        let (lx, ly) = (gx.floor(), gy.floor());
        let (fx, fy) = (gx - lx, gy - ly);
        let at = |i: i64, j: i64| {
            image.pixel(
                i.clamp(0, w as i64 - 1) as usize,
                j.clamp(0, h as i64 - 1) as usize,
            )
        };
        let (x0, y0) = (lx as i64, ly as i64);
        let (a, b, c, d) = (
            at(x0, y0),
            at(x0 + 1, y0),
            at(x0, y0 + 1),
            at(x0 + 1, y0 + 1),
        );
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    };
    let mut out = vec![[0f32; 3]; tw * th];
    use rayon::prelude::*;
    out.par_chunks_mut(tw).enumerate().for_each(|(ty, row)| {
        for (tx, o) in row.iter_mut().enumerate() {
            let mut acc = [0f32; 3];
            for j in 0..n {
                for i in 0..n {
                    let ox = (i as f32 + 0.5) / n as f32 - 0.5;
                    let oy = (j as f32 + 0.5) / n as f32 - 0.5;
                    let sx = (tx as f32 + 0.5 + ox) / zoom;
                    let sy = (ty as f32 + 0.5 + oy) / zoom;
                    let v = bilinear(sx, sy);
                    for k in 0..3 {
                        acc[k] += v[k];
                    }
                }
            }
            *o = acc.map(|v| v / (n * n) as f32);
        }
    });
    (out, tw, th)
}

fn encode(v: f32, gain: f32) -> f32 {
    let v = (v * gain).clamp(0.0, 1.0);
    let e = if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    e * 255.0
}

/// Mean, 99th and 99.9th percentile, and largest difference in 8-bit
/// levels, and the share of samples a level or more apart.
fn apart(a: &[[f32; 3]], b: &[[f32; 3]], gain: f32) -> String {
    let mut d: Vec<f32> = a
        .iter()
        .zip(b)
        .flat_map(|(p, q)| (0..3).map(move |k| (encode(p[k], gain) - encode(q[k], gain)).abs()))
        .collect();
    let mean = d.iter().map(|&v| f64::from(v)).sum::<f64>() / d.len() as f64;
    let over = d.iter().filter(|&&v| v >= 1.0).count() as f64 / d.len() as f64;
    d.sort_by(f32::total_cmp);
    let q = |p: f64| d[((d.len() - 1) as f64 * p) as usize];
    format!(
        "mean {mean:.3}, p99 {:.2}, p99.9 {:.2}, max {:.1}, {:.3} % a level or more",
        q(0.99),
        q(0.999),
        d[d.len() - 1],
        over * 100.0
    )
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or_else(|| anyhow::anyhow!("a raw"))?;
    let rest: Vec<String> = args.collect();
    let reorder = rest.first().is_some_and(|a| a == "reorder");
    let zooms: Vec<f32> = if reorder {
        Vec::new()
    } else {
        rest.iter().map(|s| s.parse()).collect::<Result<_, _>>()?
    };
    let frame = greycard_core::decode::decode_path(std::path::Path::new(&path))?;
    let developed = develop(&frame, &DevelopSettings::default())?;
    let image = developed.image;
    let (measured, clip) = (developed.sharpen_radius, developed.clip_level);
    let (w, h) = (image.width, image.height);
    // The gain that puts the median luminance at a mid grey.
    let mut lum: Vec<f32> = image
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .step_by(97)
        .map(|p| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2])
        .collect();
    lum.sort_by(f32::total_cmp);
    let gain = 0.18 / lum[lum.len() / 2].max(1e-4);
    println!("{w}x{h}, radius {measured:?}, gain {gain:.2}");
    let sharpened = |iterations: usize| {
        let mut p = image.clone();
        let options = SharpenOptions {
            iterations,
            ..Default::default()
        };
        let (stats, _) = sharpen_with_mask(&mut p, None, &options, measured, clip);
        (p, stats)
    };
    let fit = |vw: f32, vh: f32| (vw / w as f32).min(vh / h as f32);
    if reorder {
        let sharpen = |p: &mut WorkingImage, guide: Option<&WorkingImage>| {
            sharpen_with_mask(p, guide, &SharpenOptions::default(), measured, clip);
        };
        let detail = |p: &mut WorkingImage, slider: &str| match slider {
            "Dehaze +30" => {
                dehaze(p, &DehazeOptions { amount: 0.3 });
            }
            s => {
                let half = LocalContrastOptions {
                    texture: if s.starts_with("Texture") { 0.5 } else { 0.0 },
                    clarity: if s.starts_with("Clarity") { 0.5 } else { 0.0 },
                    structure: 0.0,
                };
                local_contrast(p, &half, clip);
            }
        };
        for slider in ["Clarity +50", "Texture +50", "Dehaze +30"] {
            let mut today = image.clone();
            detail(&mut today, slider);
            let guided = !slider.starts_with("Dehaze");
            sharpen(&mut today, guided.then_some(&image));
            let mut ahead = image.clone();
            sharpen(&mut ahead, None);
            detail(&mut ahead, slider);
            for zoom in [1.0, fit(3840.0, 2160.0), fit(2560.0, 1440.0)] {
                let (a, _, _) = shown(&today, zoom);
                let (b, _, _) = shown(&ahead, zoom);
                println!("{slider} at {zoom:.3}: {}", apart(&a, &b, gain));
            }
        }
        return Ok(());
    }
    let (full, stats) = sharpened(20);
    println!("full: {stats:?}");
    let fewer: Vec<(usize, WorkingImage)> = [3, 4, 5, 6, 8, 10, 12, 14, 16, 18]
        .into_iter()
        .map(|k| (k, sharpened(k).0))
        .collect();
    let zooms = if zooms.is_empty() {
        vec![fit(2560.0, 1440.0), fit(3840.0, 2160.0), 0.5]
    } else {
        zooms
    };
    for zoom in zooms {
        let (rest, tw, th) = shown(&full, zoom);
        println!("zoom {zoom:.3} ({tw}x{th}):");
        let (none, _, _) = shown(&image, zoom);
        println!("  unsharpened: {}", apart(&rest, &none, gain));
        for (k, p) in &fewer {
            let (held, _, _) = shown(p, zoom);
            println!("  {k:>2} iterations: {}", apart(&rest, &held, gain));
        }
    }
    Ok(())
}
