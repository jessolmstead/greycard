//! `--measure-lightroom CATALOG --crops`: how close the Lightroom
//! import's crop lands on Lightroom's own exports, frame by frame, with
//! no window and nothing written but the report.
//!
//! The color measurement ([`crate::measure_lightroom`]) cuts
//! Lightroom's crop out of the render itself, so that its numbers stand
//! apart from the crop. This one is the crop's own proof: each cropped
//! pair's raw developed under the edit the import writes, its crop with
//! it ([`greycard_library::lightroom_catalog::imported_edit`] given the
//! catalog's shape of the file), finished at the export's long edge
//! through the export's own geometry, and the export registered onto
//! that render with nothing cut or turned by hand. Where the crop is
//! right, the two are one picture: the same size, a scale of one, no
//! shift. What a frame gives: the crop's size at full size as ours
//! cuts it beside the one the catalog's crop has, the two pictures'
//! sizes, and the registration (refined to a fraction of a pixel) as a
//! scale, a shift and the farthest any corner of the export lands from
//! the render's, in the export's pixels.
//!
//! **The develop's own scale.** Two lens corrections fill the frame by
//! slightly different amounts, so a frame comes out a few tenths of a
//! percent larger in one than in the other, about the frame's center
//! wherever the crop is. That is the develop's, not the crop's: it
//! shows on uncropped frames too, which the run measures beside the
//! cropped ones as controls. So the shift is also given with that
//! magnification taken out: a scale `s` about the frame's center `z`
//! (in the render, from its own center) moves the picture by
//! `(1 - s) z`, and what is left of the shift after it is where the
//! crop itself stands off Lightroom's.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Serialize;

use greycard_library::lightroom_exports::{self as lx, Confidence, Crop};
use greycard_match::Picture;
use greycard_match::register::{Plane, Registration, ncc_shifted, register};

use crate::camera_match as cm;

/// The refinement's working width: the export halved, at most this.
const FINE_WIDTH: usize = 2560;

/// What the command line asked for.
pub(crate) struct Options {
    pub(crate) catalog: PathBuf,
    pub(crate) roots: Vec<(String, PathBuf)>,
    pub(crate) folders: Vec<PathBuf>,
    pub(crate) limit: Option<usize>,
    pub(crate) out: Option<PathBuf>,
}

/// One cropped frame against its export.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct CropLine {
    pub(crate) name: String,
    pub(crate) export: String,
    pub(crate) confidence: String,
    /// Lightroom's crop: its angle and its orientation code.
    pub(crate) angle: f32,
    pub(crate) orientation: Option<String>,
    /// The crop at full size, as ours cuts it and as Lightroom's
    /// corners make it on the catalog's frame, each the way up it is
    /// shown.
    pub(crate) ours: (f32, f32),
    pub(crate) lightroom: Option<(f32, f32)>,
    /// Drawn in by the import to stay on the turned picture.
    pub(crate) drawn_in: bool,
    /// The export's size, and ours finished at its long edge.
    pub(crate) export_size: (usize, usize),
    pub(crate) render_size: (usize, usize),
    /// The registration laying the export onto ours: its scale over
    /// the fit, its shift in the export's pixels, its correlation.
    pub(crate) scale: f32,
    pub(crate) dx: f32,
    pub(crate) dy: f32,
    pub(crate) ncc: f32,
    /// The farthest a corner of the export lands from ours, in the
    /// export's pixels.
    pub(crate) corner: f32,
    /// The frame's center in the render, from the render's own center,
    /// in the export's pixels; and the shift left once the scale is
    /// taken as a magnification about it: the crop's own residual.
    pub(crate) frame_center: (f32, f32),
    pub(crate) crop_dx: f32,
    pub(crate) crop_dy: f32,
    /// An uncropped frame, measured for the develop's own scale.
    pub(crate) control: bool,
    pub(crate) seconds: f32,
    pub(crate) failed: Option<String>,
}

/// The run.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct CropReport {
    pub(crate) pairs: usize,
    pub(crate) cropped: usize,
    pub(crate) frames: Vec<CropLine>,
    /// Over the cropped frames measured: the mean and the largest
    /// corner residual, and the mean and largest of the crop's own
    /// shift once the develop's scale is taken out, export pixels.
    pub(crate) corner_mean: f32,
    pub(crate) corner_max: f32,
    pub(crate) crop_mean: f32,
    pub(crate) crop_max: f32,
    pub(crate) seconds: f32,
}

/// `--measure-lightroom --crops`, from the command line to the exit
/// code.
pub(crate) fn headless(cli: &crate::Cli, catalog: &Path) -> Result<std::process::ExitCode> {
    let opts = Options {
        catalog: catalog.to_path_buf(),
        roots: cli.root.clone(),
        folders: cli.exports.clone(),
        limit: cli.limit,
        out: cli.out.clone(),
    };
    let report = run(&opts)?;
    print!("{}", text(&report));
    if let Some(out) = &opts.out {
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(out, json).with_context(|| format!("writing {}", out.display()))?;
        eprintln!("measure crops: {}", out.display());
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// Pair, develop each cropped pair with its crop, register.
pub(crate) fn run(opts: &Options) -> Result<CropReport> {
    let start = Instant::now();
    let pairing = lx::pair(
        &opts.catalog,
        &lx::Options {
            roots: opts.roots.clone(),
            folders: opts.folders.clone(),
        },
    )
    .with_context(|| format!("reading {}", opts.catalog.display()))?;
    // The cropped pairs, those by the export history first: their
    // settings are the export's own.
    let mut cropped: Vec<&lx::Pair> = pairing
        .pairs
        .iter()
        .filter(|p| Crop::of(&lx::words(&p.settings)).is_some())
        .collect();
    cropped.sort_by_key(|p| p.confidence != Confidence::History);
    let mut uncropped: Vec<&lx::Pair> = pairing
        .pairs
        .iter()
        .filter(|p| p.confidence == Confidence::History)
        .filter(|p| Crop::of(&lx::words(&p.settings)).is_none())
        .collect();
    let n_cropped = cropped.len();
    let take: Vec<usize> = match opts.limit {
        Some(n) => {
            // A control for every three cropped, a few at least.
            let controls = cm::evenly(uncropped.len(), n.div_ceil(3).max(2));
            let picked: Vec<&lx::Pair> = controls.iter().map(|&i| uncropped[i]).collect();
            uncropped = picked;
            cm::evenly(n_cropped, n)
                .into_iter()
                .chain(n_cropped..n_cropped + uncropped.len())
                .collect()
        }
        None => (0..n_cropped + uncropped.len()).collect(),
    };
    cropped.extend(uncropped);
    let lenses = greycard_lens::Store::user()
        .ok()
        .and_then(|s| s.load())
        .map(|(db, _)| db);
    if lenses.is_none() {
        eprintln!("measure crops: no lens database; the develop runs without lens corrections");
    }
    let mut frames = Vec::new();
    for (k, &i) in take.iter().enumerate() {
        let pair = cropped[i];
        let shot = &pairing.shots[pair.shot];
        let mut line = frame(pair, shot, lenses.as_ref());
        line.control = i >= n_cropped;
        eprintln!(
            "measure crops: {}/{} {}{}: {}",
            k + 1,
            take.len(),
            line.name,
            if line.control { " (control)" } else { "" },
            match &line.failed {
                None => format!(
                    "corner {:.2} px, shift ({:+.2}, {:+.2}), crop's own ({:+.2}, {:+.2}), \
                     scale {:.5}, ncc {:.3}, {:.0} s",
                    line.corner,
                    line.dx,
                    line.dy,
                    line.crop_dx,
                    line.crop_dy,
                    line.scale,
                    line.ncc,
                    line.seconds
                ),
                Some(why) => format!("not measured: {why}"),
            }
        );
        frames.push(line);
    }
    let measured: Vec<&CropLine> = frames
        .iter()
        .filter(|f| f.failed.is_none() && !f.control)
        .collect();
    let n = measured.len().max(1) as f32;
    let own = |f: &CropLine| f.crop_dx.hypot(f.crop_dy);
    Ok(CropReport {
        pairs: pairing.pairs.len(),
        cropped: n_cropped,
        corner_mean: measured.iter().map(|f| f.corner).sum::<f32>() / n,
        corner_max: measured.iter().map(|f| f.corner).fold(0.0, f32::max),
        crop_mean: measured.iter().map(|f| own(f)).sum::<f32>() / n,
        crop_max: measured.iter().map(|f| own(f)).fold(0.0, f32::max),
        frames,
        seconds: start.elapsed().as_secs_f32(),
    })
}

/// One pair measured.
fn frame(pair: &lx::Pair, shot: &lx::Shot, lenses: Option<&greycard_lens::Database>) -> CropLine {
    let start = Instant::now();
    let crop = Crop::of(&lx::words(&pair.settings));
    let mut line = CropLine {
        name: match &shot.copy {
            Some(copy) => format!("{} ({copy})", shot.name),
            None => shot.name.clone(),
        },
        export: pair
            .export
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        confidence: pair.confidence.word().to_string(),
        angle: crop.map_or(0.0, |c| c.angle as f32),
        orientation: shot.orientation.clone(),
        ..CropLine::default()
    };
    let Some(raw) = shot.raw.as_deref() else {
        line.failed = Some("the raw is not here".into());
        return line;
    };
    let shape = shot.shape();
    let edit =
        match greycard_library::lightroom_catalog::imported_edit(raw, &pair.settings, Some(&shape))
        {
            Ok(f) => f.edit,
            Err(e) => {
                line.failed = Some(format!("the settings: {e}"));
                return line;
            }
        };
    if crop.is_some() && edit.geometry.crop.is_none() {
        line.failed = Some("the import carried no crop".into());
        return line;
    }
    // Ours at full size, against Lightroom's corners on its frame; and
    // where the frame's center stands from the crop's, in plane pixels.
    let stance = greycard_core::decode::stance_path(raw).ok();
    let mut center_off = (0.0f32, 0.0f32);
    if let Some((w, h)) = stance.and_then(|s| s.shown_size(0)) {
        let (w, h) = (w as f32, h as f32);
        let f = edit.geometry.frame(w, h);
        line.ours = f.size;
        let (pw, ph) = edit.geometry.plane_size(w, h);
        center_off = (
            pw / 2.0 - (f.origin.0 + f.size.0 / 2.0),
            ph / 2.0 - (f.origin.1 + f.size.1 / 2.0),
        );
    }
    if let (Some(c), Some((w, h))) = (crop, shot.size) {
        let (cw, ch) = c.size(w, h);
        let on_side = shot
            .orientation
            .as_deref()
            .and_then(greycard_edit::lightroom::crop::orientation_of)
            .is_some_and(|o| {
                matches!(
                    o,
                    greycard_core::raw::Orientation::Rotate90
                        | greycard_core::raw::Orientation::Rotate270
                )
            });
        line.lightroom = Some(if on_side {
            (ch.round() as f32, cw.round() as f32)
        } else {
            (cw.round() as f32, ch.round() as f32)
        });
    }
    // Drawn in by the import to stay on the turned picture: smaller
    // than Lightroom's by more than its rounding.
    line.drawn_in = line
        .lightroom
        .is_some_and(|(w, h)| line.ours.0 < w - 1.5 || line.ours.1 < h - 1.5);
    let export = match crate::measure_lightroom::read_export(&pair.export) {
        Ok(p) => p,
        Err(e) => {
            line.failed = Some(format!("the export: {e}"));
            return line;
        }
    };
    line.export_size = (export.width, export.height);
    let settings = crate::export::Settings {
        long_edge: Some(export.width.max(export.height) as u32),
        ..cm::settings()
    };
    let render = match crate::worker::FrameDevelop::develop(raw, &edit, lenses) {
        Ok(mut d) => cm::picture_of(&d.finish(&edit, &settings)),
        Err(e) => {
            line.failed = Some(format!("the develop: {e}"));
            return line;
        }
    };
    line.render_size = (render.width, render.height);
    let landscape = |w: usize, h: usize| w >= h;
    if landscape(render.width, render.height) != landscape(export.width, export.height) {
        line.failed = Some(format!(
            "the export stands the other way: {} by {} against our {} by {}",
            export.width, export.height, render.width, render.height
        ));
        return line;
    }
    let coarse = register(&render, &export);
    let fine = refine(&render, &export, coarse);
    // Render pixels an export pixel, as the fit lays one on the other.
    let fit = (render.height as f32 / export.height as f32)
        .min(render.width as f32 / export.width as f32);
    line.scale = fine.scale;
    line.dx = fine.dx / fit;
    line.dy = fine.dy / fit;
    line.ncc = fine.ncc;
    line.corner = corner(&render, &export, &fine) / fit;
    // The frame's center in the export's pixels from its center, and
    // the shift left once the scale is a magnification about it.
    if line.ours.0 > 0.0 {
        let k = export.width as f32 / line.ours.0;
        line.frame_center = (center_off.0 * k, center_off.1 * k);
    }
    let lens = 1.0 - fine.scale;
    line.crop_dx = line.dx - lens * line.frame_center.0;
    line.crop_dy = line.dy - lens * line.frame_center.1;
    line.seconds = start.elapsed().as_secs_f32();
    line
}

/// The farthest a corner of `export`, laid onto `render` by `reg`,
/// lands from the render's own corner, in render pixels.
fn corner(render: &Picture, export: &Picture, reg: &Registration) -> f32 {
    let fit = (render.height as f32 / export.height as f32)
        .min(render.width as f32 / export.width as f32)
        * reg.scale;
    let (ew, eh) = (export.width as f32 / 2.0, export.height as f32 / 2.0);
    let (rw, rh) = (render.width as f32 / 2.0, render.height as f32 / 2.0);
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .iter()
        .map(|(sx, sy)| {
            let x = sx * ew * fit + reg.dx - sx * rw;
            let y = sy * eh * fit + reg.dy - sy * rh;
            (x * x + y * y).sqrt()
        })
        .fold(0.0, f32::max)
}

/// The registration refined about `coarse`: on the luminance at up to
/// [`FINE_WIDTH`] across, scales within a third of a percent in steps
/// of a fortieth of one, every whole pixel within three of the coarse
/// shift, then the peak's shift to a fraction of a pixel by a parabola
/// through it and its neighbors each way. The shift comes back in
/// render pixels, as `register`'s does.
pub(crate) fn refine(render: &Picture, export: &Picture, coarse: Registration) -> Registration {
    let width = render.width.clamp(64, FINE_WIDTH);
    let f = render.width as f32 / width as f32;
    let r = render.luminance().resized_to_width(width);
    let j_width =
        ((width as f32 * export.width as f32 / render.width as f32).round() as usize).max(1);
    let j = export.luminance().resized_to_width(j_width);
    let (cy, cx) = (
        (coarse.dy / f).round() as i32,
        (coarse.dx / f).round() as i32,
    );
    let margin = 16;
    let mut best = (coarse.scale, cy, cx, f32::MIN);
    let mut best_warp: Option<Plane> = None;
    for i in 0..=24 {
        let s = coarse.scale - 0.003 + 0.006 * i as f32 / 24.0;
        let warped = j.warp_to(r.width, r.height, s, 0.0, 0.0);
        let mut improved = false;
        for dy in -3..=3 {
            for dx in -3..=3 {
                let c = ncc_shifted(&warped, &r, cy + dy, cx + dx, margin, 2);
                if c > best.3 {
                    best = (s, cy + dy, cx + dx, c);
                    improved = true;
                }
            }
        }
        if improved {
            best_warp = Some(warped);
        }
    }
    let (s, by, bx, peak) = best;
    let warped = best_warp.expect("one scale at least");
    let at = |dy: i32, dx: i32| ncc_shifted(&warped, &r, dy, dx, margin, 1);
    let center = at(by, bx);
    let vertex = |minus: f32, plus: f32| {
        let d = minus - 2.0 * center + plus;
        if d.abs() < 1e-9 {
            0.0
        } else {
            (0.5 * (minus - plus) / d).clamp(-0.5, 0.5)
        }
    };
    let sub_y = vertex(at(by - 1, bx), at(by + 1, bx));
    let sub_x = vertex(at(by, bx - 1), at(by, bx + 1));
    Registration {
        scale: s,
        dy: (by as f32 + sub_y) * f,
        dx: (bx as f32 + sub_x) * f,
        ncc: peak.max(center),
    }
}

/// The report as the command line prints it.
pub(crate) fn text(r: &CropReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "pairs         {}, {} of them cropped; {} measured, {} of them uncropped controls",
        r.pairs,
        r.cropped,
        r.frames.len(),
        r.frames.iter().filter(|f| f.control).count()
    );
    for f in &r.frames {
        match &f.failed {
            Some(why) => {
                let _ = writeln!(out, "frame         {}: not measured, {why}", f.name);
            }
            None => {
                let lr = f
                    .lightroom
                    .map(|(w, h)| format!("{w:.0} x {h:.0}"))
                    .unwrap_or_else(|| "?".into());
                let _ = writeln!(
                    out,
                    "{}{} ({}, {:+.2}°{}): ours {:.0} x {:.0}, Lightroom's {lr}; \
                     export {} x {}, render {} x {}; scale {:.5}, shift ({:+.2}, {:+.2}) px, \
                     the crop's own ({:+.2}, {:+.2}) px, corner {:.2} px, ncc {:.3}",
                    if f.control {
                        "control       "
                    } else {
                        "frame         "
                    },
                    f.name,
                    f.confidence,
                    f.angle,
                    if f.drawn_in { ", drawn in" } else { "" },
                    f.ours.0,
                    f.ours.1,
                    f.export_size.0,
                    f.export_size.1,
                    f.render_size.0,
                    f.render_size.1,
                    f.scale,
                    f.dx,
                    f.dy,
                    f.crop_dx,
                    f.crop_dy,
                    f.corner,
                    f.ncc
                );
            }
        }
    }
    let _ = writeln!(
        out,
        "corners       mean {:.2} px, largest {:.2} px, in the export's pixels",
        r.corner_mean, r.corner_max
    );
    let _ = writeln!(
        out,
        "the crop's    mean {:.2} px, largest {:.2} px, the develop's scale taken out",
        r.crop_mean, r.crop_max
    );
    let _ = writeln!(out, "took          {:.0} s", r.seconds);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture with structure everywhere for the registration to
    /// hold on to.
    fn scene(w: usize, h: usize, shift: (f32, f32)) -> Picture {
        let mut data = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = ((x as f32 - shift.0) / 97.0, (y as f32 - shift.1) / 71.0);
                let v = 0.5
                    + 0.2 * (fx * 3.1).sin() * (fy * 2.3).cos()
                    + 0.1 * (fx * 11.0 + fy * 7.0).sin()
                    + 0.1 * (fx * 23.0).cos() * (fy * 19.0).sin();
                data.push([v, v, v]);
            }
        }
        Picture::new(w, h, data)
    }

    #[test]
    fn the_same_picture_registers_at_no_shift_and_a_shift_is_found_to_a_fraction() {
        let a = scene(800, 600, (0.0, 0.0));
        let reg = refine(&a, &a, register(&a, &a));
        assert!((reg.scale - 1.0).abs() < 5e-4, "{reg:?}");
        assert!(reg.dx.abs() < 0.2 && reg.dy.abs() < 0.2, "{reg:?}");
        assert!(corner(&a, &a, &reg) < 0.5);
        // Ours moved 1.5 pixels right of the export's: the export lays
        // over it that far right.
        let ours = scene(800, 600, (1.5, 0.0));
        let reg = refine(&ours, &a, register(&ours, &a));
        assert!((reg.dx - 1.5).abs() < 0.35, "{reg:?}");
        assert!(reg.dy.abs() < 0.35, "{reg:?}");
        let c = corner(&ours, &a, &reg);
        assert!((c - 1.5).abs() < 0.5, "{c}");
    }
}
