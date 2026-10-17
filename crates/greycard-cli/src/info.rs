//! `greycard info`: what a file says about itself and what the engine
//! makes of it.

use anyhow::{Context, Result};
use greycard_core::CalibrationIlluminant;
use greycard_core::color::{as_shot_temp_tint, profile_from_frame};
use greycard_core::raw::{
    SensorLayout, Shot, aperture_text, camera_name, focal_text, shutter_text,
};

use crate::common::load;

pub(crate) fn info(file: &std::path::Path) -> Result<()> {
    let frame = load(file)?;
    println!("camera        {}", camera_name(&frame.make, &frame.model));
    print_exposure(&frame.shot);
    println!(
        "sensor        {}x{} x{} samples",
        frame.width, frame.height, frame.channels
    );
    match &frame.layout {
        SensorLayout::Cfa(p) => {
            println!("layout        CFA {}x{} {:?}", p.width, p.height, p.colors)
        }
        other => println!("layout        {other:?}"),
    }
    let b = &frame.levels.black;
    let w = &frame.levels.white;
    println!(
        "black         {}x{}x{} {:?}",
        b.width, b.height, b.cpp, b.values
    );
    println!(
        "white         {}x{}x{} {:?}",
        w.width, w.height, w.cpp, w.values
    );
    let check = frame.white_check();
    println!(
        "brightest     {:.0}; {} of {} samples above the declared white ({:.3}%)",
        check.brightest,
        check.above,
        check.total,
        check.fraction_above() * 100.0
    );
    match frame.crop {
        Some(c) => println!(
            "crop          {}x{} at ({}, {})",
            c.width, c.height, c.x, c.y
        ),
        None => println!("crop          none"),
    }
    println!("orientation   {:?}", frame.orientation);
    let shot = &frame.shot;
    println!(
        "lens          {}{}{}{}",
        shot.lens_model.as_deref().unwrap_or("not recorded"),
        shot.focal_length
            .map_or(String::new(), |f| format!(", {}", focal_text(f))),
        shot.f_number
            .map_or(String::new(), |f| format!(", {}", aperture_text(f))),
        shot.focus_distance
            .map_or(String::new(), |d| format!(", {d:.1} m")),
    );
    match frame.as_shot_coefficients {
        Some(c) => println!("as-shot gains R {:.4}  G {:.4}  B {:.4}", c[0], c[1], c[2]),
        None => println!("as-shot gains none"),
    }
    for c in &frame.calibrations {
        let name = CalibrationIlluminant::from_exif_code(c.illuminant)
            .map(|i| format!("{i:?}"))
            .unwrap_or_else(|| format!("code {}", c.illuminant));
        println!("calibration   {name}: {:?}", c.color_matrix);
    }
    match profile_from_frame(&frame) {
        Ok(profile) => {
            println!(
                "profile       {:?}{}",
                profile.primary.illuminant,
                profile
                    .secondary
                    .as_ref()
                    .map(|s| format!(" + {:?}", s.illuminant))
                    .unwrap_or_default()
            );
            match as_shot_temp_tint(&frame, &profile) {
                Ok(tt) => println!("as-shot white {:.0} K, Duv {:+.4}", tt.cct, tt.duv),
                Err(e) => println!("as-shot white unavailable ({e})"),
            }
        }
        Err(e) => println!("profile       unavailable ({e})"),
    }
    Ok(())
}

/// What a picture that is not a raw says about itself.
pub(crate) fn info_picture(file: &std::path::Path) -> Result<()> {
    let p = greycard_core::picture::decode_picture_path(file)
        .with_context(|| format!("decoding {}", file.display()))?;
    println!("picture       {}-bit, taken as {}", p.bits, p.space);
    println!("camera        {}", camera_name(&p.make, &p.model));
    print_exposure(&p.shot);
    println!(
        "size          {}x{}, upright",
        p.image.width, p.image.height
    );
    match &p.shot.lens_model {
        Some(l) => println!("lens          {l}"),
        None => println!("lens          not recorded"),
    }
    Ok(())
}

/// What the camera was set to, one tag a line, said the way the
/// editor's panel says it.
fn print_exposure(shot: &Shot) {
    match shot.iso {
        Some(iso) => println!("iso           {iso}"),
        None => println!("iso           not recorded"),
    }
    match shot.exposure_time.and_then(shutter_text) {
        Some(t) => println!("shutter       {t}"),
        None => println!("shutter       not recorded"),
    }
}
