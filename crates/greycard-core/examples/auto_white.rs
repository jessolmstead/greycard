//! The Auto white balance on real frames, against the camera's own
//! white, so the numbers in the notes can be taken again from the tree.
//!
//! Each file is developed at its as-shot white with the file's own
//! profile (RCD, no chromatic aberration correction, for now; the
//! estimate is made in camera space and barely sees the demosaic), a
//! grid about 512 points across is read off it as the editor's
//! `sample_grid` reads the GPU's texture, and [`auto_neutral`] is run
//! on that grid. Prints one line a frame: the as-shot temperature and
//! tint, Auto's, and how much warmer Auto renders the picture than as
//! shot, in mireds: the as-shot white's less Auto's, so a higher white
//! chosen, a warmer picture, reads positive.
//!
//! ```text
//! cargo run --release -p greycard-core --example auto_white -- FILE...
//! ```

use greycard_core::color::{WhitePoint, auto_neutral, neutral_gains, resolve_white_balance};
use greycard_core::decode::decode_path;
use greycard_core::develop::{DemosaicMethod, DevelopSettings, develop};

/// How wide the grid is, as the editor's.
const ACROSS: usize = 512;

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    println!("frame,as_shot_k,as_shot_duv,auto_k,auto_duv,warmer_mired,grid");
    let (mut total, mut count) = (0.0f64, 0usize);
    for file in &files {
        let name = std::path::Path::new(file)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let frame = match decode_path(file) {
            Ok(frame) => frame,
            Err(e) => {
                eprintln!("{name}: {e}");
                continue;
            }
        };
        let settings = DevelopSettings {
            demosaic: DemosaicMethod::Rcd,
            chromatic_aberration: None,
            ..Default::default()
        };
        let developed = match develop(&frame, &settings) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{name}: {e}");
                continue;
            }
        };
        let wb = &developed.white_balance;
        let gains = wb.coefficients_f32();
        let cols = wb.matrix_f32();
        let matrix: [[f32; 3]; 3] = std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r]));
        let image = &developed.image;
        let (w, h) = (image.width, image.height);
        let step = (w / ACROSS).max(1);
        let mut grid = Vec::new();
        for y in (step / 2..h).step_by(step) {
            for x in (step / 2..w).step_by(step) {
                let i = (y * w + x) * 3;
                // The editor's texture is half floats.
                grid.push([0, 1, 2].map(|k| half(image.data[i + k])));
            }
        }
        let across = (step / 2..w).step_by(step).count();
        let Some(px) = auto_neutral(&grid, across, gains, matrix, developed.clip_level, gains)
        else {
            println!(
                "{name},{:.0},{:+.4},,,,few",
                wb.temp_tint.cct, wb.temp_tint.duv
            );
            continue;
        };
        let profile = greycard_core::color::profile_from_frame(&frame).expect("profile");
        let found = neutral_gains(px, gains, matrix).and_then(|g| {
            resolve_white_balance(&frame, &profile, WhitePoint::Coefficients(g)).ok()
        });
        let Some(found) = found else {
            println!(
                "{name},{:.0},{:+.4},,,,unresolved",
                wb.temp_tint.cct, wb.temp_tint.duv
            );
            continue;
        };
        let (a, b) = (wb.temp_tint, found.temp_tint);
        let auto_k = b.cct.clamp(2000.0, 12000.0);
        let shift = 1.0e6 / a.cct - 1.0e6 / auto_k;
        total += shift;
        count += 1;
        println!(
            "{name},{:.0},{:+.4},{:.0},{:+.4},{:+.1},{}",
            a.cct,
            a.duv,
            auto_k,
            b.duv.clamp(-0.05, 0.05),
            shift,
            grid.len()
        );
    }
    if count > 0 {
        eprintln!(
            "mean shift {:+.1} mired over {count} frames",
            total / count as f64
        );
    }
}

/// A value through a half float and back, as the GPU holds it.
fn half(v: f32) -> f32 {
    // Round to 11 significant bits; the range here is well inside a
    // half's.
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let bits = v.to_bits();
    let rounded = (bits + 0x1000) & !0x1fff;
    f32::from_bits(rounded)
}
