//! Step 6 of the gamut op's design: a fitted look's error over its match
//! records under today's table form and the new one, tables as they
//! are. The records are the fit's pairs (`frame, render rgb, jpeg rgb`,
//! encoded sRGB); the error is the mean over frames of each frame's
//! mean Oklab distance from the camera's picture.
//!
//! `cargo run --release -p greycard-render --example gamut_look -- CUBE PAIRS`

use std::collections::BTreeMap;
use std::sync::Arc;

use greycard_core::color::{Oklab, apply3};
use greycard_core::lut::Look;
use greycard_render::export::Space;
use greycard_render::gamut::look_kept;

fn decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let lut = greycard_core::Lut3d::load(std::path::Path::new(&args[1])).expect("the table loads");
    let look = Look::new(Arc::new(lut), 1.0).expect("the table's primaries");
    let ok = Oklab::for_working_space();
    let to_srgb = Space::Srgb.matrix();
    let from_srgb = greycard_core::color::invert3(to_srgb).expect("sRGB inverts");
    let lab = |w: [f32; 3]| {
        let lms = apply3(&ok.to_lms, w).map(|v| v.abs().cbrt().copysign(v));
        apply3(&greycard_core::color::LMS_TO_LAB, lms)
    };
    let text = std::fs::read_to_string(&args[2]).expect("the pairs read");
    let mut per_frame: BTreeMap<String, [f64; 3]> = BTreeMap::new();
    let (mut outside, mut moved) = (0usize, 0.0f32);
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let v: Vec<&str> = line.split(',').map(str::trim).collect();
        let num = |i: usize| v[i].parse::<f32>().expect("a number");
        let x = apply3(&from_srgb, [num(1), num(2), num(3)].map(decode));
        let y = apply3(&from_srgb, [num(4), num(5), num(6)].map(decode));
        let (a, b) = (look.at(x), look_kept(&look, x));
        let r = apply3(look.to_lut(), x);
        if r.iter().any(|v| !(0.0..=1.0).contains(v)) {
            outside += 1;
        }
        moved = moved.max((0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max));
        let dist = |p: [f32; 3]| {
            let (p, q) = (lab(p), lab(y));
            f64::from(
                ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt(),
            )
        };
        let e = per_frame.entry(v[0].to_owned()).or_insert([0.0; 3]);
        e[0] += dist(a);
        e[1] += dist(b);
        e[2] += 1.0;
    }
    let n = per_frame.len() as f64;
    let today: f64 = per_frame.values().map(|e| e[0] / e[2]).sum::<f64>() / n;
    let new: f64 = per_frame.values().map(|e| e[1] / e[2]).sum::<f64>() / n;
    let records: f64 = per_frame.values().map(|e| e[2]).sum();
    println!(
        "{records} records over {n} frames, {outside} outside the table's cube; error today {today:.6}, new form {new:.6}, rise {:+.6}; largest move {moved:.2e}",
        new - today
    );
}
