//! `greycard register`: the transform that puts one frame on top of
//! another.

use anyhow::{Context, Result};
use greycard_core::register::{self, Model, Options};
use greycard_core::{DemosaicMethod, DevelopSettings, HighlightMode, develop};

use crate::common::load;

/// A file's luminance, for [`register`]: a picture as it stands, a raw
/// developed the short way.
///
/// White balance and the camera matrix still run — they are what makes
/// a working image, and `develop` does not offer to skip them. What is
/// skipped is everything the fit cannot use: the good demosaic
/// (bilinear puts the luminance where it belongs, which is all a fit
/// on the luminance asks of it), the chromatic aberration correction,
/// and highlight reconstruction.
///
/// Highlights are clipped rather than reconstructed on purpose.
/// Reconstruction invents detail above the clip from whichever channel
/// is still reading, and two frames of a bracket clip in different
/// places, so it would invent differently in each and put a difference
/// where the scene has none. Clipped, both frames go flat over the
/// same scene luminance: no gradient there, which costs the fit
/// nothing it had, and no disagreement either.
///
/// Hot photosite repair is on, as the develop's default has it: a
/// stuck photosite is a bright speck in every frame at the same sensor
/// position, which is to say a feature that does not move, and the fit
/// would rather not be shown one.
fn luminance_of(file: &std::path::Path) -> Result<(Vec<f32>, usize, usize)> {
    let image = if greycard_core::picture::is_picture_path(file) {
        greycard_core::picture::decode_picture_path(file)
            .with_context(|| format!("reading {}", file.display()))?
            .image
    } else {
        let frame = load(file)?;
        let settings = DevelopSettings {
            demosaic: DemosaicMethod::Bilinear,
            highlights: HighlightMode::Clip,
            chromatic_aberration: None,
            ..Default::default()
        };
        develop(&frame, &settings)
            .with_context(|| format!("developing {}", file.display()))?
            .image
    };
    let (width, height) = (image.width, image.height);
    Ok((register::luminance(&image), width, height))
}

/// Fit one frame onto another and say what came of it.
pub(crate) fn register(
    reference: &std::path::Path,
    moving: &std::path::Path,
    model: Model,
    finest_level: usize,
) -> Result<()> {
    let (a, w, h) = luminance_of(reference)?;
    let (b, mw, mh) = luminance_of(moving)?;
    if (w, h) != (mw, mh) {
        anyhow::bail!("the frames are {w}x{h} and {mw}x{mh}: registration wants one size");
    }
    let options = Options {
        model,
        finest_level,
        ..Options::default()
    };
    let started = std::time::Instant::now();
    let fit = register::fit(&a, &b, w, h, &options)?;
    let took = started.elapsed();

    println!("reference     {} ({w}x{h})", reference.display());
    println!("moving        {}", moving.display());
    println!(
        "pyramid       {} levels down to {}x{}{}, {model}, {} iterations in {:.0} ms",
        fit.levels,
        fit.coarsest.0,
        fit.coarsest.1,
        match fit.finest {
            0 => String::new(),
            n => format!(", stopping at level {n}"),
        },
        fit.iterations,
        took.as_secs_f64() * 1000.0
    );
    let m = fit.transform.m;
    println!(
        "transform     [{:>12.7} {:>12.7} {:>10.4}]",
        m[0], m[1], m[2]
    );
    println!(
        "              [{:>12.7} {:>12.7} {:>10.4}]",
        m[3], m[4], m[5]
    );
    // Which way round the shift reads: the transform takes reference
    // coordinates to moving ones, so this is where the middle of the
    // reference frame is to be found in the moving frame.
    let (dx, dy) = fit
        .transform
        .displacement((w - 1) as f64 / 2.0, (h - 1) as f64 / 2.0);
    println!(
        "shift         the reference's middle is at {dx:+.3}, {dy:+.3} px in the moving frame"
    );
    println!(
        "              scale {:.6}, rotation {:.4} degrees{}",
        fit.transform.scale(),
        fit.transform.rotation().to_degrees(),
        match model {
            // A scale and an angle are the whole of a similarity's
            // linear part. An affine's also has a shear and a second
            // scale, which these two numbers cannot hold: they are the
            // square root of the area factor and the angle the x axis
            // turned through. The matrix above is the answer.
            Model::Affine => " (an affine's linear part does not reduce to these; see the matrix)",
            _ => "",
        }
    );
    println!(
        "residual      {:.4} over {:.1}% of the frame{}",
        fit.residual,
        fit.overlap * 100.0,
        if fit.converged {
            ""
        } else {
            " (ran out of iterations)"
        }
    );
    // The engine's own threshold, so this agrees with what a caller
    // asking `Fit::aligned` would be told. Between aligned and plainly
    // hopeless there is a band worth naming: the fit found the
    // picture, but something in the frame did not hold still.
    let verdict = if fit.aligned() {
        "aligned"
    } else if fit.residual < 0.2 {
        "close, but something moved or changed: check before merging"
    } else {
        "no alignment found: do not use this transform"
    };
    println!("verdict       {verdict}");
    Ok(())
}
