//! The X-Trans denoiser's data tool, which `tools/denoise/xtrans_pool.py`
//! runs to cut its pool, and its evaluation's classical baseline.
//!
//! `export FILE STEM OUT`: decode an X-Trans raw, normalize its levels,
//! crop it, cut it to the canonical X-Trans phase (the layout of
//! `CfaPattern::xtrans()` from its top left) with sides a multiple of
//! six, measure its noise, and write `OUT/STEM.mosaic.npy` (float16,
//! sensor units) and `OUT/STEM.json`.
//!
//! `develop LIST`: each line of LIST is `IN OUT A0 A1 A2 B0 B1 B2 G0 G1 G2
//! DENOISE`: IN a float32 or float16 mosaic `.npy` at the canonical
//! phase, in sensor units; the noise model and gains; DENOISE 0 or 1.
//! Writes OUT, float32 `[H, W, 3]` in sensor units: Markesteijn's three
//! passes on the balanced mosaic, then (with DENOISE) the engine's
//! default profiled denoise, divided back by the gains.

use std::io::{BufRead, Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use greycard_core::decode::decode_path;
use greycard_core::develop::denoise::{DenoiseOptions, denoise_picture};
use greycard_core::develop::dual::DualContrast;
use greycard_core::develop::noise::{NoiseModel, estimate_noise};
use greycard_core::develop::{
    DemosaicMethod, apply_gains_cfa, crop_samples, demosaic_cfa_with, normalize_levels,
};
use greycard_core::raw::{CfaPattern, SensorLayout};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("export") => export(Path::new(&args[2]), &args[3], Path::new(&args[4])),
        Some("develop") => develop(Path::new(&args[2])),
        _ => bail!("export FILE STEM OUT | develop LIST"),
    }
}

fn export(file: &Path, stem: &str, out: &Path) -> Result<()> {
    let frame = decode_path(file)?;
    let SensorLayout::Cfa(pattern) = &frame.layout else {
        bail!("not a CFA sensor");
    };
    let gains = frame
        .as_shot_coefficients
        .context("no as-shot white balance")?
        .map(|g| g as f32);
    let normalized = normalize_levels(&frame);
    let crop = frame.effective_crop();
    let (samples, width, height) = crop_samples(&normalized, frame.width, frame.channels, crop);
    drop(normalized);
    let pattern = pattern.shifted(crop.x, crop.y);
    // X-Trans is cut to its canonical phase, Bayer to an RGGB origin.
    let (canon, period) = if pattern.is_xtrans() {
        (CfaPattern::xtrans(), 6)
    } else {
        (CfaPattern::rggb(), 2)
    };
    let (dx, dy) = (0..period)
        .flat_map(|dy| (0..period).map(move |dx| (dx, dy)))
        .find(|&(dx, dy)| pattern.shifted(dx, dy) == canon)
        .with_context(|| format!("not X-Trans or Bayer: {pattern}"))?;
    let w = (width - dx) / 6 * 6;
    let h = (height - dy) / 6 * 6;
    let mosaic: Vec<f32> = (0..h)
        .flat_map(|y| {
            samples[(y + dy) * width + dx..(y + dy) * width + dx + w]
                .iter()
                .copied()
        })
        .collect();
    drop(samples);
    let noise = estimate_noise(&mosaic, w, h, &canon)?;
    write_npy(
        &out.join(format!("{stem}.mosaic.npy")),
        &[h, w],
        &mosaic,
        true,
    )?;
    let d65 = frame
        .calibrations
        .iter()
        .find(|c| c.illuminant == 21)
        .or(frame.calibrations.first())
        .map(|c| c.color_matrix.clone())
        .unwrap_or_default();
    let mean = mosaic.iter().map(|&v| v as f64).sum::<f64>() / mosaic.len() as f64;
    let meta = serde_json::json!({
        "model": frame.model,
        "iso": frame.shot.iso,
        "width": w,
        "height": h,
        "phase": [dx, dy],
        "pattern": if period == 6 { "xtrans" } else { "rggb" },
        "orientation": format!("{:?}", frame.orientation),
        "gains": gains,
        "mean": mean,
        "white": frame.levels.headroom(),
        "noise": { "a": noise.model.a, "b": noise.model.b, "blocks": noise.blocks, "bins": noise.bins },
        "color_matrix": d65,
    });
    std::fs::write(
        out.join(format!("{stem}.json")),
        serde_json::to_string_pretty(&meta)?,
    )?;
    println!(
        "{stem}: {} ISO {:?} {w}x{h} sigma@0.18 {:.4}",
        frame.model,
        frame.shot.iso,
        noise.model.sigma(1, 0.18)
    );
    Ok(())
}

fn develop(list: &Path) -> Result<()> {
    let xtrans = CfaPattern::xtrans();
    for line in std::io::BufReader::new(std::fs::File::open(list)?).lines() {
        let line = line?;
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 12 && f.len() != 13 {
            continue;
        }
        // An optional 13th field: `rggb` develops a Bayer mosaic with the
        // engine's Bayer default, AMaZE+VNG4 with the noise threshold.
        let bayer = f.get(12) == Some(&"rggb");
        let pattern = if bayer {
            CfaPattern::rggb()
        } else {
            xtrans.clone()
        };
        let num = |i: usize| -> Result<f32> { Ok(f[i].parse::<f32>()?) };
        let model = NoiseModel {
            a: [num(2)?, num(3)?, num(4)?],
            b: [num(5)?, num(6)?, num(7)?],
        };
        let gains = [num(8)?, num(9)?, num(10)?];
        let denoise = f[11] == "1";
        let (shape, mut samples) = read_npy(Path::new(f[0]))?;
        let (h, w) = (shape[0], shape[1]);
        for s in samples.iter_mut() {
            *s = s.max(0.0);
        }
        apply_gains_cfa(&mut samples, w, &pattern, gains);
        let (method, contrast) = if bayer {
            (
                DemosaicMethod::AmazeVng4,
                DualContrast::Noise(model.after_gains(gains)),
            )
        } else {
            (DemosaicMethod::Markesteijn3, DualContrast::Auto)
        };
        let (mut rgb, _) = demosaic_cfa_with(&samples, w, h, &pattern, method, contrast)?;
        if denoise {
            denoise_picture(
                &mut rgb,
                w,
                h,
                &model.after_gains(gains),
                &DenoiseOptions::default(),
                false,
            )?;
        }
        for px in rgb.chunks_mut(3) {
            for (v, g) in px.iter_mut().zip(gains) {
                *v /= g;
            }
        }
        write_npy(Path::new(f[1]), &[h, w, 3], &rgb, false)?;
    }
    Ok(())
}

fn read_npy(path: &Path) -> Result<(Vec<usize>, Vec<f32>)> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    if &bytes[..6] != b"\x93NUMPY" {
        bail!("not an npy");
    }
    let hlen = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    let header = std::str::from_utf8(&bytes[10..10 + hlen])?;
    let body = &bytes[10 + hlen..];
    let shape_str = header
        .split("'shape': (")
        .nth(1)
        .and_then(|s| s.split(')').next())
        .context("shape")?;
    let shape: Vec<usize> = shape_str
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let data = if header.contains("'<f4'") {
        body.as_chunks::<4>()
            .0
            .iter()
            .map(|&c| f32::from_le_bytes(c))
            .collect()
    } else if header.contains("'<f2'") {
        body.as_chunks::<2>()
            .0
            .iter()
            .map(|&c| half::f16::from_le_bytes(c).to_f32())
            .collect()
    } else {
        bail!("dtype in {header}");
    };
    Ok((shape, data))
}

fn write_npy(path: &Path, shape: &[usize], data: &[f32], f16: bool) -> Result<()> {
    assert_eq!(shape.iter().product::<usize>(), data.len());
    let dims: Vec<String> = shape.iter().map(|d| d.to_string()).collect();
    let descr = if f16 { "<f2" } else { "<f4" };
    let mut header = format!(
        "{{'descr': '{descr}', 'fortran_order': False, 'shape': ({},), }}",
        dims.join(", ")
    );
    let pad = (64 - (10 + header.len() + 1) % 64) % 64;
    header.extend(std::iter::repeat_n(' ', pad));
    header.push('\n');
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    file.write_all(b"\x93NUMPY\x01\x00")?;
    file.write_all(&(header.len() as u16).to_le_bytes())?;
    file.write_all(header.as_bytes())?;
    if f16 {
        for &v in data {
            file.write_all(&half::f16::from_f32(v).to_le_bytes())?;
        }
    } else {
        for &v in data {
            file.write_all(&v.to_le_bytes())?;
        }
    }
    Ok(())
}
