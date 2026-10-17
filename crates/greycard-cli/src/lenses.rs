//! `greycard lenses`: the lens database, and what it finds for a file
//! or a lens named.

use anyhow::{Context, Result};
use greycard_core::raw::{Orientation, SensorLayout, aperture_text, camera_name, focal_text};

use crate::common::load;

/// The `lenses` command.
pub(crate) fn lenses(
    file: Option<&std::path::Path>,
    fetch: bool,
    lens_name: Option<&str>,
    camera: Option<&[String]>,
) -> Result<()> {
    let store = greycard_lens::Store::user().context("finding the lens database")?;
    if fetch {
        let start = std::time::Instant::now();
        store.fetch(|_| {}).context("fetching the lens database")?;
        println!(
            "fetched into {} in {:.1} s",
            store.dir().display(),
            start.elapsed().as_secs_f32()
        );
    }
    let Some((db, from)) = store.load() else {
        println!(
            "no lens database in {} or lensfun's own places; run with --fetch",
            store.dir().display()
        );
        return Ok(());
    };
    println!(
        "database      {} ({} lenses, {} cameras, {} mounts)",
        from.display(),
        db.lenses.len(),
        db.cameras.len(),
        db.mounts.len()
    );
    let frame = match (file, lens_name) {
        (Some(file), _) => load(file)?,
        (None, Some(name)) => {
            // A frame that is only its tags.
            let mut frame = greycard_core::RawFrame {
                make: String::new(),
                model: String::new(),
                width: 6000,
                height: 4000,
                channels: 1,
                layout: SensorLayout::Cfa(greycard_core::raw::CfaPattern::rggb()),
                samples: greycard_core::raw::Samples::U16(Vec::new()),
                levels: greycard_core::raw::Levels {
                    black: greycard_core::raw::LevelPattern::uniform(0.0, 1),
                    white: greycard_core::raw::LevelPattern::uniform(1.0, 1),
                },
                as_shot_coefficients: None,
                calibrations: Vec::new(),
                crop: None,
                orientation: Orientation::Normal,
                shot: greycard_core::raw::Shot {
                    lens_model: Some(name.to_string()),
                    ..Default::default()
                },
            };
            if let Some([make, model]) = camera {
                frame.make = make.clone();
                frame.model = model.clone();
            }
            frame
        }
        (None, None) => return Ok(()),
    };
    let shot = &frame.shot;
    println!("camera        {}", camera_name(&frame.make, &frame.model));
    println!(
        "lens          {}{}{}",
        shot.lens_model.as_deref().unwrap_or("not recorded"),
        shot.focal_length
            .map_or(String::new(), |f| format!(", {}", focal_text(f))),
        shot.f_number
            .map_or(String::new(), |f| format!(", {}", aperture_text(f))),
    );
    let l = db.lookup(&frame);
    match l.camera {
        Some(c) => println!(
            "body          {} (crop {}, {})",
            c.models[0], c.crop_factor, c.mount
        ),
        None => println!("body          not in the database"),
    }
    match l.lens {
        Some(lens) => {
            println!(
                "profile       {} (crop {}, {}; {} distortion, {} CA, {} vignetting entries)",
                lens.name(),
                lens.crop_factor,
                lens.mounts.join("/"),
                lens.distortion.len(),
                lens.tca.len(),
                lens.vignetting.len()
            );
            let p = greycard_lens::Profile {
                lens,
                camera: l.camera,
            };
            let c = p.correction(frame.width, frame.height, shot, greycard_lens::Wanted::ALL);
            println!("correction    {}", correction_words(&c));
            if let Some(line) = coverage_words(&p, &c) {
                println!("covers        {line}");
            }
        }
        None => println!("profile       none"),
    }
    Ok(())
}

/// What a profile would do to this picture, in words: the engine's own
/// struct printed with `{:?}` is not for anybody to read.
fn correction_words(c: &greycard_core::develop::lens::LensCorrection) -> String {
    use greycard_core::develop::lens::{Distortion, Scale};
    let distortion = match c.distortion {
        Some(Distortion::Poly3 { .. }) => "distortion (poly3)",
        Some(Distortion::Poly5 { .. }) => "distortion (poly5)",
        Some(Distortion::Ptlens { .. }) => "distortion (ptlens)",
        Some(Distortion::Acm { .. }) => "distortion (acm)",
        None => "no distortion",
    };
    let ca = match c.chromatic_aberration {
        Some(_) => "chromatic aberration",
        None => "no chromatic aberration",
    };
    let vignetting = match c.vignetting {
        Some(_) => "vignetting",
        None => "no vignetting",
    };
    let magnification = match c.scale {
        Scale::Auto => "auto".to_string(),
        Scale::Fixed(s) => format!("{s:.4}"),
    };
    format!(
        "{distortion}, {ca}, {vignetting}; radius scale {:.4}, vignetting scale {:.4}, magnification {magnification}",
        c.radius_scale, c.vignetting_scale
    )
}

/// What a profile measured on a smaller sensor leaves out of this
/// picture, in words; nothing when it covers it.
fn coverage_words(
    p: &greycard_lens::Profile,
    c: &greycard_core::develop::lens::LensCorrection,
) -> Option<String> {
    if !p.measured_on_smaller_sensor() {
        return None;
    }
    Some(match c.vignetting_edge() {
        Some(edge) => format!(
            "the profile was measured on a smaller sensor; the corners reach r={:.2}, and the vignetting past r=1 ({:.0}% of the way to the corner) is held at its edge",
            p.reach(),
            edge * 100.0
        ),
        None => format!(
            "the profile was measured on a smaller sensor; the corners reach r={:.2}",
            p.reach()
        ),
    })
}
