//! greycard-denoise-data: pick raws for the AI denoiser's training set,
//! and write them out as the training script reads them.
//!
//! `survey` reads the metadata of every raw under the given folders and
//! tallies them by camera and ISO, so a training set can be chosen. It
//! decodes nothing.
//!
//! `export` decodes each file and writes, beside a JSON of what the
//! training needs to know, two `.npy` arrays: the mosaic in sensor
//! units (levels normalized, no white balance, cropped so it reads RGGB
//! from its top left corner and has even sides), and the target the
//! network learns to reach, the engine's default demosaic of that mosaic
//! divided back by the gains it was demosaiced under, so it too is in
//! sensor units. The frame's own measured noise model is in the JSON:
//! the training adds it to the noise it synthesizes when it stabilizes
//! the variance, since a "clean" low-ISO frame is not silent. See the
//! notes, §37.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use greycard_core::decode::{decode_path, probe_path};
use greycard_core::develop::dual::DualContrast;
use greycard_core::develop::noise::{MID_GREY, estimate_noise};
use greycard_core::develop::{
    DemosaicMethod, apply_gains_cfa, crop_samples, demosaic_cfa_with, normalize_levels,
};
use greycard_core::raw::{CfaColor, CfaPattern, SensorLayout};
use rayon::prelude::*;

#[derive(Parser)]
#[command(
    name = "greycard-denoise-data",
    version,
    about = "Survey raws by camera and ISO, and export training data for the AI denoiser"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Tally every raw under the folders by camera and ISO, without
    /// decoding any; with --list, one line per file
    Survey {
        inputs: Vec<PathBuf>,
        #[arg(long)]
        list: bool,
    },
    /// Decode each file and write its mosaic, target and JSON to --out
    Export {
        files: Vec<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        /// Skip a frame whose mean level is below this: too dark to be
        /// a clean target
        #[arg(long, default_value_t = 0.02)]
        min_mean: f32,
        /// Overwrite files already exported
        #[arg(long)]
        force: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Survey { inputs, list } => survey(&inputs, list),
        Command::Export {
            files,
            out,
            min_mean,
            force,
        } => export(&files, &out, min_mean, force),
    }
}

fn is_raw(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("cr2" | "cr3" | "arw" | "nef" | "dng" | "raf" | "orf" | "rw2" | "pef")
    )
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if is_raw(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Make, model and ISO of a file that could be read.
type Probed = (String, String, Option<u32>);

fn survey(inputs: &[PathBuf], list: bool) -> Result<()> {
    let mut files = Vec::new();
    for input in inputs {
        if input.is_dir() {
            walk(input, &mut files)?;
        } else {
            files.push(input.clone());
        }
    }
    files.sort();
    if files.is_empty() {
        bail!("no raws found");
    }
    let probed: Vec<(PathBuf, Option<Probed>)> = files
        .par_iter()
        .map(|path| {
            let probe = probe_path(path).ok().map(|p| (p.make, p.model, p.iso));
            (path.clone(), probe)
        })
        .collect();

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if list {
        for (path, probe) in &probed {
            match probe {
                Some((make, model, iso)) => writeln!(
                    out,
                    "{}\t{} {}\t{}",
                    path.display(),
                    make,
                    model,
                    iso.map(|i| i.to_string()).unwrap_or_else(|| "?".into())
                )?,
                None => writeln!(out, "{}\t?\t?", path.display())?,
            }
        }
        return Ok(());
    }

    // camera -> ISO -> count
    let mut tally: BTreeMap<String, BTreeMap<u32, usize>> = BTreeMap::new();
    let mut unreadable = 0usize;
    for (_, probe) in &probed {
        match probe {
            Some((make, model, iso)) => {
                *tally
                    .entry(format!("{make} {model}"))
                    .or_default()
                    .entry(iso.unwrap_or(0))
                    .or_default() += 1;
            }
            None => unreadable += 1,
        }
    }
    for (camera, isos) in &tally {
        let total: usize = isos.values().sum();
        writeln!(out, "{camera}: {total} files")?;
        let line: Vec<String> = isos.iter().map(|(iso, n)| format!("{iso}:{n}")).collect();
        writeln!(out, "  ISO {}", line.join("  "))?;
    }
    if unreadable > 0 {
        writeln!(out, "{unreadable} files could not be read")?;
    }
    Ok(())
}

/// The offset within the 2x2 block at which `pattern` reads RGGB.
fn rggb_offset(pattern: &CfaPattern) -> Option<(usize, usize)> {
    let rggb = CfaPattern::rggb();
    (0..2)
        .flat_map(|dy| (0..2).map(move |dx| (dx, dy)))
        .find(|&(dx, dy)| pattern.shifted(dx, dy) == rggb)
}

fn export(files: &[PathBuf], out: &Path, min_mean: f32, force: bool) -> Result<()> {
    std::fs::create_dir_all(out)?;
    for file in files {
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let json_path = out.join(format!("{stem}.json"));
        if json_path.exists() && !force {
            println!("{stem}: already exported");
            continue;
        }
        let start = Instant::now();
        match export_one(file, out, &stem, min_mean) {
            Ok(Some(line)) => println!("{stem}: {line} ({:.1} s)", start.elapsed().as_secs_f32()),
            Ok(None) => println!("{stem}: skipped, too dark"),
            Err(e) => println!("{stem}: skipped: {e:#}"),
        }
    }
    Ok(())
}

fn export_one(file: &Path, out: &Path, stem: &str, min_mean: f32) -> Result<Option<String>> {
    let frame = decode_path(file)?;
    let SensorLayout::Cfa(pattern) = &frame.layout else {
        bail!("not a CFA sensor");
    };
    let Some(coefficients) = frame.as_shot_coefficients else {
        bail!("no as-shot white balance");
    };
    let gains = coefficients.map(|g| g as f32);
    if gains.iter().any(|g| !g.is_finite() || *g <= 0.0) {
        bail!("white balance gains {gains:?}");
    }

    let normalized = normalize_levels(&frame);
    let crop = frame.effective_crop();
    let (samples, width, height) = crop_samples(&normalized, frame.width, frame.channels, crop);
    drop(normalized);
    let pattern = pattern.shifted(crop.x, crop.y);
    let Some((dx, dy)) = rggb_offset(&pattern) else {
        bail!("not a 2x2 Bayer pattern: {pattern}");
    };
    // Cut to an RGGB origin and even sides.
    let w = (width - dx) & !1;
    let h = (height - dy) & !1;
    let mosaic: Vec<f32> = (0..h)
        .flat_map(|y| {
            let row = &samples[(y + dy) * width + dx..(y + dy) * width + dx + w];
            row.iter().copied()
        })
        .collect();
    drop(samples);
    let pattern = CfaPattern::rggb();

    let mean = mosaic.iter().map(|&v| v as f64).sum::<f64>() / mosaic.len() as f64;
    if (mean as f32) < min_mean {
        return Ok(None);
    }

    let noise = estimate_noise(&mosaic, w, h, &pattern)?;

    let mut balanced = mosaic.clone();
    apply_gains_cfa(&mut balanced, w, &pattern, gains);
    let (mut target, _) = demosaic_cfa_with(
        &balanced,
        w,
        h,
        &pattern,
        DemosaicMethod::AmazeVng4,
        DualContrast::Noise(noise.model.after_gains(gains)),
    )?;
    drop(balanced);
    target.par_chunks_mut(3).for_each(|px| {
        for (v, g) in px.iter_mut().zip(gains) {
            *v /= g;
        }
    });

    write_npy(&out.join(format!("{stem}.mosaic.npy")), &[h, w], &mosaic)?;
    write_npy(&out.join(format!("{stem}.target.npy")), &[h, w, 3], &target)?;

    let colors: String = pattern
        .colors
        .iter()
        .map(|c| match c {
            CfaColor::Red => 'R',
            CfaColor::Green => 'G',
            CfaColor::Blue => 'B',
            CfaColor::Other(_) => '?',
        })
        .collect();
    let meta = serde_json::json!({
        "file": file.display().to_string(),
        "make": frame.make,
        "model": frame.model,
        "iso": frame.shot.iso,
        "width": w,
        "height": h,
        "pattern": colors,
        "gains": gains,
        "mean": mean,
        "noise": { "a": noise.model.a, "b": noise.model.b, "blocks": noise.blocks },
    });
    std::fs::write(
        out.join(format!("{stem}.json")),
        serde_json::to_string_pretty(&meta)?,
    )?;

    Ok(Some(format!(
        "{} {} ISO {}, {w}x{h}, mean {mean:.3}, sigma at mid grey {:.4}",
        frame.make,
        frame.model,
        frame
            .shot
            .iso
            .map(|i| i.to_string())
            .unwrap_or_else(|| "?".into()),
        noise.model.sigma(1, MID_GREY),
    )))
}

/// Write `data` as a little-endian float16 `.npy` of `shape`, C order.
fn write_npy(path: &Path, shape: &[usize], data: &[f32]) -> Result<()> {
    assert_eq!(shape.iter().product::<usize>(), data.len());
    let dims: Vec<String> = shape.iter().map(|d| d.to_string()).collect();
    let mut header = format!(
        "{{'descr': '<f2', 'fortran_order': False, 'shape': ({},), }}",
        dims.join(", ")
    );
    // The magic, version and length take 10 bytes; the header is padded
    // with spaces to a multiple of 64 and ends with a newline.
    let pad = (64 - (10 + header.len() + 1) % 64) % 64;
    header.extend(std::iter::repeat_n(' ', pad));
    header.push('\n');
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    file.write_all(b"\x93NUMPY\x01\x00")?;
    file.write_all(&(header.len() as u16).to_le_bytes())?;
    file.write_all(header.as_bytes())?;
    let bytes: Vec<u8> = data
        .par_iter()
        .map(|&v| half::f16::from_f32(v).to_le_bytes())
        .flatten_iter()
        .collect();
    file.write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rggb_offset_finds_each_bayer_phase() {
        let rggb = CfaPattern::rggb();
        assert_eq!(rggb_offset(&rggb), Some((0, 0)));
        assert_eq!(rggb_offset(&rggb.shifted(1, 0)), Some((1, 0)));
        assert_eq!(rggb_offset(&rggb.shifted(0, 1)), Some((0, 1)));
        assert_eq!(rggb_offset(&rggb.shifted(1, 1)), Some((1, 1)));
    }

    #[test]
    fn npy_header_is_aligned_and_readable() {
        let dir = std::env::temp_dir().join(format!("greycard-npy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.npy");
        write_npy(&path, &[2, 3], &[0.0, 0.5, 1.0, 1.5, 2.0, 2.5]).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..6], b"\x93NUMPY");
        let len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        assert_eq!((10 + len) % 64, 0);
        let header = std::str::from_utf8(&bytes[10..10 + len]).unwrap();
        assert!(header.contains("'shape': (2, 3,)"));
        assert!(header.ends_with('\n'));
        assert_eq!(bytes.len(), 10 + len + 6 * 2);
        let last = half::f16::from_le_bytes([bytes[bytes.len() - 2], bytes[bytes.len() - 1]]);
        assert_eq!(last.to_f32(), 2.5);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
