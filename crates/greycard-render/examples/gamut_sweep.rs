//! The gamut op over a frame's display-referred pixels, as the
//! finish hands them to the output: linear Rec.2020, little-endian f32
//! triples, read from a file. Writes beside the prefix the op's output
//! in the output's linear values (`-n.f32`), and per pixel its reach
//! before the bound (`-nreach.f32`), whether the rail moved it
//! (`-nrailed.f32`), and what the final clamp took (`-nresidue.f32`):
//! what round 3's measures read, so a sweep renders each frame once and
//! applies the op offline.
//!
//! `cargo run --release -p greycard-render --example gamut_sweep -- IN OUT_PREFIX [SPACE]`,
//! the space by its export name, sRGB when left out. With `time` for
//! the prefix it writes nothing and says what the op and the clamp
//! cost a pixel of the frame, on one thread.

use greycard_render::export::Space;
use greycard_render::gamut::{Op, Params};
use rayon::prelude::*;

fn write(path: String, values: impl Iterator<Item = f32>) {
    let bytes: Vec<u8> = values.flat_map(f32::to_le_bytes).collect();
    std::fs::write(path, bytes).expect("the output writes");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, prefix) = (&args[1], &args[2]);
    let space = args.get(3).map_or(Space::Srgb, |n| {
        Space::from_name(n).expect("a space's name")
    });
    let bytes = std::fs::read(input).expect("the input reads");
    let px: Vec<[f32; 3]> = bytes
        .as_chunks::<12>()
        .0
        .iter()
        .map(|c| {
            [0, 1, 2]
                .map(|k| f32::from_le_bytes([c[4 * k], c[4 * k + 1], c[4 * k + 2], c[4 * k + 3]]))
        })
        .collect();
    let op = Op::for_output(&space.matrix(), Params::SCENE).expect("the output inverts");
    if prefix == "time" {
        let m = space.matrix();
        let time = |f: &dyn Fn([f32; 3]) -> [f32; 3]| {
            let t = std::time::Instant::now();
            let mut sum = 0.0f32;
            for c in &px {
                sum += std::hint::black_box(f(*c))[1];
            }
            (t.elapsed().as_nanos() as f64 / px.len() as f64, sum)
        };
        let clamp = time(&|c| greycard_core::color::apply3(&m, c).map(|v| v.clamp(0.0, 1.0)));
        let ours = time(&|c| op.apply(c));
        println!(
            "{} pixels: the clamp {:.1} ns a pixel, the op {:.1} ({:.0} {:.0})",
            px.len(),
            clamp.0,
            ours.0,
            clamp.1,
            ours.1
        );
        return;
    }
    let out: Vec<([f32; 3], f32, f32, f32)> = px
        .par_iter()
        .map(|c| {
            let (railed, moved) = op.rail(*c);
            let u = op.unclamped(*c);
            let residue = u
                .iter()
                .map(|v| (v - v.clamp(0.0, 1.0)).abs())
                .fold(0.0, f32::max);
            (
                u.map(|v| v.clamp(0.0, 1.0)),
                op.reach(railed),
                if moved { 1.0 } else { 0.0 },
                residue,
            )
        })
        .collect();
    write(format!("{prefix}-n.f32"), out.iter().flat_map(|o| o.0));
    write(format!("{prefix}-nreach.f32"), out.iter().map(|o| o.1));
    write(format!("{prefix}-nrailed.f32"), out.iter().map(|o| o.2));
    write(format!("{prefix}-nresidue.f32"), out.iter().map(|o| o.3));
}
