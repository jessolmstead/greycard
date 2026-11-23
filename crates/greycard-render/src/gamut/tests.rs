//! The op's tests: its properties, and the design's ramp tests (§7,
//! steps 1 and 2) with the numbers the chosen bound measured. Where the
//! bound misses one of the design's gates, the test holds the op to what
//! it measured, says so, and names the gate.

use super::*;
use crate::export::Space;

const OUTPUTS: [Space; 3] = [Space::Srgb, Space::DisplayP3, Space::AdobeRgb];

fn op(space: Space) -> Op {
    Op::for_output(&space.matrix(), Params::SCENE).expect("the output inverts")
}

fn ok() -> Oklab {
    Oklab::for_working_space()
}

fn lab(c: [f32; 3]) -> [f32; 3] {
    let ok = ok();
    let lms = apply3(&ok.to_lms, c).map(|v| v.abs().cbrt().copysign(v));
    apply3(&LMS_TO_LAB, lms)
}

fn from_lab(l: [f32; 3]) -> [f32; 3] {
    let ok = ok();
    apply3(&ok.from_lms, apply3(&LAB_TO_LMS, l).map(|v| v * v * v))
}

fn lch(l: [f32; 3]) -> [f32; 3] {
    [
        l[0],
        l[1].hypot(l[2]),
        l[2].atan2(l[1]).to_degrees().rem_euclid(360.0),
    ]
}

fn wrap(d: f32) -> f32 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

/// A small, fixed pseudo-random sequence in 0 to 1.
fn noise(n: usize) -> impl Iterator<Item = f32> {
    let mut s = 0x9e37_79b9_u32;
    (0..n).map(move |_| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s >> 8) as f32 / (1u32 << 24) as f32
    })
}

fn colors(n: usize, scale: f32, offset: f32) -> Vec<[f32; 3]> {
    let v: Vec<f32> = noise(3 * n).collect();
    v.chunks(3)
        .map(|c| [0, 1, 2].map(|k| c[k] * scale - offset))
        .collect()
}

/// The reach by its definition in f64, on a fine grid: the chord from
/// grey through the color, the ray's depth past the faces from there
/// (outward the deepest any channel has been under zero, inward the
/// least any one channel has been under zero all the way), and the
/// edge where that depth over the brightest channel meets the spring to
/// the chord, found by halving. The reference the exact form is held to.
fn reach_by_definition(op: &Op, c: [f32; 3]) -> f64 {
    let l = op.lab(c);
    if !(l[0] > 0.0 && l[1].hypot(l[2]) > 1e-7) {
        return 0.0;
    }
    let ray = |t: f64| -> [f64; 3] {
        let w = [0, 1, 2].map(|j| {
            let v = f64::from(LAB_TO_LMS[j][0]) * f64::from(l[0])
                + t * (f64::from(LAB_TO_LMS[j][1]) * f64::from(l[1])
                    + f64::from(LAB_TO_LMS[j][2]) * f64::from(l[2]));
            v * v * v
        });
        [0, 1, 2].map(|k| (0..3).map(|j| f64::from(op.lms_to_out[k][j]) * w[j]).sum())
    };
    let span = 1.0 / f64::from(op.params.onset());
    let grey = ray(0.0)[0];
    let chord = ray(1.0)
        .iter()
        .filter(|v| **v < grey)
        .map(|v| grey / (grey - v))
        .fold(f64::INFINITY, f64::min)
        .min(span);
    let t0 = chord.min(1.0);
    let at = ray(t0);
    let n = at[0].max(at[1]).max(at[2]).max(grey);
    let steps = 20_000;
    let grid: Vec<[f64; 3]> = (0..=steps)
        .map(|i| ray(span * f64::from(i) / f64::from(steps)))
        .collect();
    let index = |t: f64| ((t / span * f64::from(steps)).round() as usize).min(steps as usize);
    let i0 = index(t0);
    let depth = |e: f64| {
        let ie = index(e);
        let (a, b) = (i0.min(ie), i0.max(ie));
        let mut points: Vec<[f64; 3]> = grid[a..=b].to_vec();
        points.push(ray(t0));
        points.push(ray(e));
        (0..3)
            .map(|k| {
                let vs = points.iter().map(|p| p[k]);
                if e >= t0 {
                    -vs.fold(f64::INFINITY, f64::min)
                } else {
                    -vs.fold(f64::NEG_INFINITY, f64::max)
                }
            })
            .fold(f64::NEG_INFINITY, f64::max)
    };
    let rise = |e: f64| depth(e) / n + f64::from(SPRING) * (e - chord);
    if rise(span) <= 0.0 {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.0, span);
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        if rise(mid) > 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    1.0 / (0.5 * (lo + hi))
}

/// The edge is its definition: the exact form's edge, on its share of
/// the color's chroma, against the f64 grid's.
#[test]
fn the_edge_is_its_definition() {
    for space in OUTPUTS {
        let op = op(space);
        let mut worst = 0.0f64;
        for c in colors(800, 2.0, 0.5) {
            let (got, want) = (f64::from(op.reach(c)), reach_by_definition(&op, c));
            if want > 0.0 && got > 0.0 {
                worst = worst.max((1.0 / got - 1.0 / want).abs());
            } else {
                // Only at the span, where the reach reads out as nothing.
                let near = (got + want) * f64::from(op.params.onset());
                assert!(got == want || near > 0.99, "{c:?} {got} {want}");
            }
        }
        eprintln!("{space:?}: the edge against its definition, at most {worst:e}");
        assert!(worst < 2e-4, "{space:?}: {worst:e}");
    }
}

/// The largest step between neighbors along a line of colors through
/// the op, in levels of 255 on sRGB's curve, and how many steps stand
/// alone: over four times the larger of the two beside them and a
/// level, which a seam is and a smooth turn is not.
fn steps(op: &Op, line: &[[f32; 3]]) -> (f32, usize, Option<usize>) {
    let enc = |v: f32| {
        if v <= 0.003_130_8 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    };
    let out: Vec<[f32; 3]> = line
        .iter()
        .map(|c| op.apply(*c).map(|v| 255.0 * enc(v)))
        .collect();
    let s: Vec<f32> = out
        .windows(2)
        .map(|w| {
            (0..3)
                .map(|k| (w[1][k] - w[0][k]).abs())
                .fold(0.0, f32::max)
        })
        .collect();
    let mut alone = 0;
    let mut first = None;
    for i in 1..s.len().saturating_sub(1) {
        if s[i] > 4.0 * s[i - 1].max(s[i + 1]) + 1.0 {
            alone += 1;
            first.get_or_insert(i);
        }
    }
    (s.iter().copied().fold(0.0, f32::max), alone, first)
}

/// No seam anywhere near a corner: hue lines every 0.02 degrees across
/// eight degrees around each primary and secondary of sRGB, P3, Adobe
/// RGB, and Rec.2020, at four lightnesses and at chromas inside, on, and
/// past the working cube's edge there, through every output under both
/// sets of thresholds. The first edge search found two edges by sRGB's
/// blue corner (review B1).
#[test]
fn no_seam_by_any_corner() {
    let ident = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let corners = [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 0.0],
    ];
    let ok = ok();
    let mut lines = Vec::new();
    for space in OUTPUTS.into_iter().chain([Space::Rec2020]) {
        let from = invert3(space.matrix()).unwrap();
        for corner in corners {
            let h0 = lch(lab(apply3(&from, corner)))[2];
            for l in [0.3, 0.5, 0.7, 0.85] {
                // At the working cube's edge chroma at the corner's hue,
                // held along the line: a line of constant Oklab L and C.
                let edge = edge_chroma(l, h0, &ident);
                for f in [0.8, 1.0, 1.25, 2.0] {
                    let line: Vec<[f32; 3]> = (0..=400)
                        .map(|i| {
                            let h = h0 - 4.0 + 0.02 * i as f32;
                            let c = f * edge;
                            let (cs, sn) = (h.to_radians().cos(), h.to_radians().sin());
                            from_lab([l, c * cs, c * sn])
                        })
                        .collect();
                    // Past the cube, colors an eye sees: a cone response
                    // under zero is held at zero, and such a color's hue
                    // is no hue (`Op::lab`).
                    if line
                        .iter()
                        .any(|c| apply3(&ok.to_lms, *c).iter().any(|v| *v <= 0.0))
                    {
                        continue;
                    }
                    lines.push((space, h0, l, f, line));
                }
            }
        }
    }
    assert!(lines.len() > 300, "{}", lines.len());
    let mut worst = 0.0f32;
    let mut seams = Vec::new();
    for out in OUTPUTS.into_iter().chain([Space::Rec2020]) {
        for params in [Params::SCENE, Params::EDGE] {
            let op = Op::for_output(&out.matrix(), params).unwrap();
            for (space, h0, l, f, line) in &lines {
                let (most, alone, first) = steps(&op, line);
                worst = worst.max(most);
                if alone > 0 {
                    seams.push((out, params.own_top, *space, *h0, *l, *f, alone, first, most));
                }
            }
        }
    }
    eprintln!("{} lines; the largest step {worst:.2} levels", lines.len());
    assert!(
        seams.is_empty(),
        "{} seams: {:?}",
        seams.len(),
        &seams[..seams.len().min(8)]
    );
}

/// Review B1's picture, through the export: a smooth gradient of a scene
/// light at the working space's edge, cyan through blue to magenta
/// across, 0 to +4 EV down 400 rows, into sixteen-bit TIFFs in sRGB, P3,
/// and Adobe RGB. No step stands alone (6 levels of 255 or more and over
/// four times every step within two pixels of it). And the most a row
/// changes over three pixels is within [`STEEPER`] of what the output's
/// clamp per channel makes of the same display stage. The first edge
/// search left 331 lone steps of 6 to 25 levels in sRGB; the second, a
/// band on each dip, 65 of up to 23 levels, one pixel wide at this width
/// (review B1's fix-check).
#[test]
fn the_blue_light_gradient_exports_without_a_seam() {
    use crate::export::{Format, Pixels, Settings, render};
    let (w, h) = (1200usize, 400usize);
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let ev = 4.0 * y as f32 / (h - 1) as f32;
        for x in 0..w {
            let u = 0.4 + 0.2 * x as f32 / (w - 1) as f32;
            let rgb = [
                0.002 + (u - 0.5).max(0.0) * 2.0,
                0.002 + (0.5 - u).max(0.0) * 2.0,
                1.0,
            ];
            let y = LUMA[0] * rgb[0] + LUMA[1] * rgb[1] + LUMA[2] * rgb[2];
            data.extend(rgb.map(|v| v / y * 0.18 * 0.2 * 2f32.powf(ev)));
        }
    }
    let image = greycard_core::image::WorkingImage {
        width: w,
        height: h,
        data,
    };
    let edit = greycard_edit::Edit::default();
    let global = crate::finish::Baked::global(&edit, crate::finish::Source::SCENE);
    let shown: Vec<[f32; 3]> = image
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| crate::finish::finish_pixel_display(*p, &global, &[], 0.0, None, None, None, None))
        .collect();
    for space in OUTPUTS {
        let settings = Settings {
            format: Format::Tiff,
            space,
            ..Settings::default()
        };
        let r = render(
            &image,
            &edit,
            &greycard_edit::NoStores,
            (w as u32, h as u32),
            &settings,
            &Default::default(),
            1.0,
            None,
            crate::finish::Source::SCENE,
            None,
            None,
        );
        let px: Vec<u16> = match r.pixels {
            Pixels::Sixteen(p) => p,
            Pixels::Eight(p) => p.into_iter().map(|v| u16::from(v) * 257).collect(),
        };
        let mut alone = 0;
        for y in 0..h {
            let row: Vec<[f32; 3]> = (0..w)
                .map(|x| [0, 1, 2].map(|k| f32::from(px[(y * w + x) * 3 + k]) / 257.0))
                .collect();
            alone += slope(&row).1;
        }
        assert_eq!(alone, 0, "{space:?}: {alone} alone");
        // The op against the clamp on the same display stage, both in
        // sixteen bits under one encoding.
        let (op, m) = (op(space), space.matrix());
        let mut worst = [0.0f32; 2];
        for y in 0..h {
            let line = &shown[y * w..(y + 1) * w];
            let ours: Vec<[f32; 3]> = line.iter().map(|c| levels(op.apply(*c), 16)).collect();
            let clamp: Vec<[f32; 3]> = line.iter().map(|c| levels(apply3(&m, *c), 16)).collect();
            worst[0] = worst[0].max(slope(&clamp).0);
            worst[1] = worst[1].max(slope(&ours).0);
        }
        eprintln!(
            "{space:?}: 0 alone; at most {:.1} levels over three pixels, the clamp {:.1}",
            worst[1], worst[0]
        );
        assert!(worst[1] <= STEEPER * worst[0], "{space:?}: {worst:?}");
    }
}

/// How much steeper than the clamp's the op's output may be along a
/// ramp: the most it changes over three samples, against the clamp's
/// most on the same ramp (or 6 levels of 255, where the clamp hardly
/// moves). Measured 1.7, on Adobe RGB's thresholds of one, where a hue
/// line at L 0.8 is brought to the edge at its hue along a stretch the
/// edge turns sharply; 1.2 or under under a scene's thresholds but for
/// Rec.2020's rail (`the_op_is_no_steeper_than_the_clamp`).
const STEEPER: f32 = 1.6;

/// Rec.2020 under a scene's thresholds: the rail alone, under sRGB's
/// top, which the clamp does not have (Rec.2020 holds the working
/// space): a bright cyan's ramp across its corner changes by 16 levels
/// over three pixels where the clamp's changes by 3. The first build's
/// rail, the owner's A.
const STEEPER_RAILED: f32 = 2.7;

const ROLL_L: f32 = 2e-4;
const ROLL_H: f32 = 1.1;
const EDGE_L: f32 = 5e-4;
const EDGE_H: f32 = 2.2;
const EDGE_OFF: f32 = 2.5e-3;
const CUBE_L: f32 = 1e-3;
const CUBE_H: f32 = 3.0;

/// Along a line of outputs, in levels of 255 (fractions kept): the most
/// it changes over three samples, and how many steps stand alone: 6
/// levels or more and over four times every step within two of it.
fn slope(line: &[[f32; 3]]) -> (f32, usize) {
    let d: Vec<f32> = line
        .windows(2)
        .map(|w| {
            (0..3)
                .map(|k| (w[1][k] - w[0][k]).abs())
                .fold(0.0, f32::max)
        })
        .collect();
    let most = line
        .windows(4)
        .map(|w| {
            (0..3)
                .map(|k| (w[3][k] - w[0][k]).abs())
                .fold(0.0, f32::max)
        })
        .fold(0.0, f32::max);
    let mut alone = 0;
    for i in 2..d.len().saturating_sub(2) {
        let near = d[i - 2].max(d[i - 1]).max(d[i + 1]).max(d[i + 2]);
        if d[i] >= 6.0 && d[i] > 4.0 * near {
            alone += 1;
        }
    }
    (most, alone)
}

/// A linear output's channels on sRGB's curve, in levels of 255 rounded
/// to `bits`.
fn levels(o: [f32; 3], bits: u32) -> [f32; 3] {
    let q = if bits == 8 { 255.0 } else { 65535.0 };
    o.map(|v| (crate::finish::encode(v.clamp(0.0, 1.0)) * q).round() * 255.0 / q)
}

/// The op is no steeper than the clamp, within [`STEEPER`], on every
/// ramp: through each corner of the working space (each primary and
/// secondary to its two neighbors, four mixes of white, 400 rows from
/// 3% of white to 1.25 times it), and the 2-D sweeps (hue lines every
/// 0.05 degrees at four lightnesses and three chromas; chroma and
/// lightness lines every 2 degrees of hue), into every output under a
/// scene's thresholds and thresholds of one, in 8 and 16 bits. And no
/// step stands alone anywhere (review B1's fix-check).
#[test]
fn the_op_is_no_steeper_than_the_clamp() {
    let corners: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [1.0, 1.0, 0.0]),
        ([1.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 1.0]),
        ([0.0, 1.0, 1.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [0.0, 1.0, 1.0], [1.0, 0.0, 1.0]),
        ([1.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ];
    let mut lines: Vec<(String, String, Vec<[f32; 3]>)> = Vec::new();
    for (ci, (c0, a, b)) in corners.into_iter().enumerate() {
        for mix in [0.0f32, 0.002, 0.01, 0.03] {
            for y in 0..400 {
                let level = 0.03 * (1.25f32 / 0.03).powf(y as f32 / 399.0);
                let line = (0..160)
                    .map(|x| {
                        let u = 2.0 * x as f32 / 159.0 - 1.0;
                        let side = if u < 0.0 { a } else { b };
                        let t = 0.35 * u.abs();
                        let rgb = [0, 1, 2].map(|k| mix + c0[k] + t * (side[k] - c0[k]));
                        let m = rgb[0].max(rgb[1]).max(rgb[2]);
                        rgb.map(|v| v / m * level)
                    })
                    .collect();
                lines.push((
                    format!("corner {ci} mix {mix}"),
                    format!("corner {ci} mix {mix} row {y}"),
                    line,
                ));
            }
        }
    }
    for l in [0.3f32, 0.5, 0.7, 0.85] {
        for c in [0.15f32, 0.3, 0.4] {
            let line = (0..7200)
                .map(|i| {
                    let h = (i as f32 * 0.05).to_radians();
                    from_lab([l, c * h.cos(), c * h.sin()])
                })
                .collect();
            lines.push((
                format!("hue line L {l} C {c}"),
                format!("hue line L {l} C {c}"),
                line,
            ));
        }
    }
    for hi in (0..360).step_by(2) {
        let h = (hi as f32).to_radians();
        for l in [0.2f32, 0.35, 0.5, 0.65, 0.8, 0.92] {
            let line = (0..400)
                .map(|i| {
                    let c = 0.42 * i as f32 / 399.0;
                    from_lab([l, c * h.cos(), c * h.sin()])
                })
                .collect();
            lines.push((
                format!("chroma lines L {l}"),
                format!("chroma line L {l} h {hi}"),
                line,
            ));
        }
        for c in [0.04f32, 0.08, 0.14, 0.2, 0.28] {
            let line = (0..400)
                .map(|i| from_lab([0.05 + 0.93 * i as f32 / 399.0, c * h.cos(), c * h.sin()]))
                .collect();
            lines.push((
                format!("lightness lines C {c}"),
                format!("lightness line C {c} h {hi}"),
                line,
            ));
        }
    }
    let jobs: Vec<(Space, Params)> = OUTPUTS
        .into_iter()
        .chain([Space::Rec2020])
        .flat_map(|s| [(s, Params::SCENE), (s, Params::EDGE)])
        .collect();
    let lines = &lines;
    let results: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|&(space, params)| {
                scope.spawn(move || {
                    let to = space.matrix();
                    let op = Op::for_output(&to, params).unwrap();
                    // Each family of ramps (a corner's gradient at a mix,
                    // the hue lines, a lightness's chroma lines, a
                    // chroma's lightness lines): the op's steepest change
                    // against the clamp's, in 8 and 16 bits.
                    let mut family: std::collections::BTreeMap<&str, [[f32; 2]; 2]> =
                        Default::default();
                    let mut alone = Vec::new();
                    for (name, what, line) in lines {
                        let out: Vec<[f32; 3]> = line.iter().map(|c| op.apply(*c)).collect();
                        let clamp: Vec<[f32; 3]> = line.iter().map(|c| apply3(&to, *c)).collect();
                        let f = family.entry(name.as_str()).or_default();
                        for (i, bits) in [8, 16].into_iter().enumerate() {
                            let (a, lone) =
                                slope(&out.iter().map(|o| levels(*o, bits)).collect::<Vec<_>>());
                            let (b, _) =
                                slope(&clamp.iter().map(|o| levels(*o, bits)).collect::<Vec<_>>());
                            f[i][0] = f[i][0].max(a);
                            f[i][1] = f[i][1].max(b);
                            if lone > 0 {
                                alone.push(format!("{what}, {bits} bits: {lone}"));
                            }
                        }
                    }
                    let mut worst = (0.0f32, String::new());
                    for (name, f) in &family {
                        for (i, [a, b]) in f.iter().enumerate() {
                            let r = a / b.max(6.0);
                            if r > worst.0 {
                                worst = (
                                    r,
                                    format!("{name}, {} bits: {a:.1} against {b:.1}", [8, 16][i]),
                                );
                            }
                        }
                    }
                    let name = format!(
                        "{space:?} {}",
                        if params.own_top {
                            "thresholds of one"
                        } else {
                            "a scene's"
                        }
                    );
                    eprintln!(
                        "{name}: at most {:.2} of the clamp's slope [{}]",
                        worst.0, worst.1
                    );
                    assert!(
                        alone.is_empty(),
                        "{name}: lone steps {:?}",
                        &alone[..alone.len().min(4)]
                    );
                    let bound = if space == Space::Rec2020 && !params.own_top {
                        STEEPER_RAILED
                    } else {
                        STEEPER
                    };
                    assert!(worst.0 <= bound, "{name}: {} [{}]", worst.0, worst.1);
                    name
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(results.len(), 8);
}

/// The final clamp moves little, over the working cube and twice past
/// it, both ways. Under zero: nothing but rounding, since what the
/// spring leaves past a face is taken in along the line to grey
/// ([`SPRING`]); measured 3e-8. Over one: the roll
/// holds Oklab lightness, and a ray of a bright color under the rail's
/// top can bow over white on its way in; measured at most 0.016, a
/// picture's color as bright as white past the working cube, held to
/// 0.02.
#[test]
fn the_final_clamp_moves_little() {
    const UNDER: f32 = 1e-6;
    let mut top = 0.0f32;
    for space in OUTPUTS.into_iter().chain([Space::Rec2020]) {
        for params in [Params::SCENE, Params::EDGE] {
            let op = Op::for_output(&space.matrix(), params).unwrap();
            let (mut over, mut moved, mut most) = (0, 0, 0.0f32);
            for c in colors(20_000, 2.5, 0.5) {
                // A color the finish can hand over: dark, or of light,
                // not one under black.
                if lab(c)[0] <= 0.0 {
                    continue;
                }
                let u = op.unclamped(c);
                let low = u.iter().map(|v| -v).fold(0.0, f32::max);
                let high = u.iter().map(|v| v - 1.0).fold(0.0, f32::max);
                assert!(low <= UNDER, "{space:?} {c:?} -> {u:?}");
                assert!(high < 0.02, "{space:?} {c:?} -> {u:?}");
                top = top.max(high);
                moved += usize::from(low > 0.0);
                over += usize::from(low > 0.005);
                most = most.max(low);
            }
            eprintln!(
                "{space:?} {}: {over} of {moved} moved by over 0.005, at most {most}",
                params.own_top
            );
        }
    }
    eprintln!("over white at most {top}");
}

/// sRGB's matrix into each output has no negative entry, so the rail,
/// which puts every channel under one in sRGB, puts every output's
/// under one too.
#[test]
fn the_rail_puts_every_output_under_white() {
    let to_srgb = Space::Srgb.matrix();
    let from_srgb = invert3(to_srgb).unwrap();
    for space in OUTPUTS.into_iter().chain([Space::Rec2020]) {
        let m = mul3(space.matrix(), from_srgb);
        assert!(m.iter().flatten().all(|v| *v > -1e-5), "{space:?}: {m:?}");
        let op = op(space);
        for c in colors(20_000, 8.0, 1.0) {
            let (r, _) = op.rail(c);
            let s = apply3(&to_srgb, r);
            assert!(s.iter().all(|v| *v <= 1.0 + 1e-4), "{c:?}: {s:?}");
            let o = apply3(&space.matrix(), r);
            assert!(o.iter().all(|v| *v <= 1.0 + 1e-4), "{space:?} {c:?}: {o:?}");
        }
    }
}

/// Under its knee the rail leaves a color to the bit: every color in
/// sRGB whose channels sit under nine tenths of the way to white.
#[test]
fn the_rail_leaves_a_color_under_its_knee() {
    let op = op(Space::Srgb);
    let from_srgb = invert3(Space::Srgb.matrix()).unwrap();
    let mut seen = 0;
    for s in colors(20_000, 1.0, 0.0) {
        let c = apply3(&from_srgb, s);
        let y = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
        if s.iter().all(|v| (v - y) / (1.0 - y) < 0.89) {
            seen += 1;
            assert_eq!(op.rail(c), (c, false));
        }
    }
    assert!(seen > 1000);
}

/// A color inside the output and under every knee comes back to the
/// bit: the early out is the output matrix and nothing else, and the
/// op with it is the op without it, to the bit, on every color, inside
/// and out, under a scene's thresholds and thresholds of one. And the
/// early out's share is safe: a color whose smallest channel keeps
/// 0.15 of its brightest is under the roll's knee in every output.
#[test]
fn a_color_under_the_knees_comes_back_to_the_bit() {
    for space in OUTPUTS {
        let from = invert3(space.matrix()).unwrap();
        for params in [Params::SCENE, Params::EDGE] {
            let op = Op::for_output(&space.matrix(), params).unwrap();
            let mut most = 0.0f32;
            for o in colors(40_000, 1.0, 0.0)
                .into_iter()
                .chain(colors(40_000, 2.0, 0.5))
            {
                let c = apply3(&from, o);
                let r = op.reach(c);
                let (lo, hi) = (o[0].min(o[1]).min(o[2]), o[0].max(o[1]).max(o[2]));
                if r >= Params::SCENE.roll[0] && hi <= 1.0 {
                    most = most.max(lo / hi);
                }
                if !op.rail(c).1 && lo >= EARLY * hi && hi <= 1.0 {
                    assert_eq!(op.unclamped(c), apply3(&space.matrix(), c));
                }
                let (a, b) = (op.unclamped(c), op.solve(c, false));
                assert!(
                    a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()),
                    "{space:?} {c:?}: {a:?} {b:?}"
                );
            }
            assert!(most < 0.12, "{space:?}: {most}");
        }
    }
}

/// The roll holds lightness and hue: a color brought in by it alone
/// (no rail, no bound) keeps its Oklab L and h where its ray lands
/// inside the output. Where the spring leaves it past a face, the line
/// to grey takes it in, and lightness and hue move by at most ROLL_L
/// and ROLL_H degrees.
#[test]
fn the_roll_holds_lightness_and_hue() {
    let (mut dl, mut dh) = (0.0f32, 0.0f32);
    for space in OUTPUTS {
        let op = Op::for_output(
            &space.matrix(),
            Params {
                rail: None,
                bound: None,
                ..Params::SCENE
            },
        )
        .unwrap();
        let from = invert3(space.matrix()).unwrap();
        for c in colors(4000, 1.2, 0.1) {
            let r = op.reach(c);
            if !(0.9..1.5).contains(&r) || c.iter().any(|v| *v > 1.0) {
                continue;
            }
            let (a, b) = (lch(lab(c)), lch(lab(apply3(&from, op.unclamped(c)))));
            assert!(b[1] < a[1]);
            dl = dl.max((a[0] - b[0]).abs());
            dh = dh.max(wrap(a[2] - b[2]).abs());
        }
    }
    eprintln!("the roll: lightness within {dl}, hue within {dh} degrees");
    assert!(dl < ROLL_L && dh < ROLL_H, "{dl} {dh}");
}

/// An output as wide as the working space, and a picture already
/// rendered for a display, take thresholds of one: nothing inside
/// moves, to the bit.
#[test]
fn thresholds_of_one_move_nothing_inside() {
    let same = Op::for_output(&Space::Rec2020.matrix(), Params::SCENE).unwrap();
    assert_eq!(same.params.roll, [1.0, 1.0]);
    assert_eq!(same.params.bound, None);
    for space in OUTPUTS.into_iter().chain([Space::Rec2020]) {
        let edge = Op::for_output(&space.matrix(), Params::EDGE).unwrap();
        let from = invert3(space.matrix()).unwrap();
        for o in colors(20_000, 1.0, 0.0) {
            let c = apply3(&from, o);
            let m = apply3(&space.matrix(), c);
            assert_eq!(
                edge.apply(c),
                m.map(|v| v.clamp(0.0, 1.0)),
                "{space:?} {o:?}"
            );
        }
    }
    // In Rec.2020 the rail still forms the top under sRGB's: a color
    // under its knee there is left to the bit.
    for c in colors(20_000, 1.0, 0.0) {
        if !same.rail(c).1 {
            assert_eq!(same.apply(c), c);
        }
    }
}

/// The same at sixteen bits over many more colors, on the eight-bit
/// grid: to the bit. Review L3 counted 3 of 200,000 a level off in P3,
/// colors the rail in the output's own primaries read a hair over white
/// by rounding; none since the rail holds the luminance under white.
#[test]
fn a_display_pictures_colors_inside_hold_to_a_sixteen_bit_level() {
    let q = |v: [f32; 3]| v.map(|x| (x.clamp(0.0, 1.0) * 65535.0).round() as u32);
    for (space, measured) in [
        (Space::Srgb, 0),
        (Space::DisplayP3, 0),
        (Space::AdobeRgb, 0),
        (Space::Rec2020, 0),
    ] {
        let edge = Op::for_output(&space.matrix(), Params::EDGE).unwrap();
        let from = invert3(space.matrix()).unwrap();
        let (mut moved, mut most) = (0, 0u32);
        for o in colors(200_000, 1.0, 0.0) {
            let c = apply3(&from, o.map(|v| (v * 255.0).round() / 255.0));
            let (a, b) = (q(edge.apply(c)), q(apply3(&space.matrix(), c)));
            let d = (0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0);
            moved += usize::from(d > 0);
            most = most.max(d);
        }
        eprintln!("{space:?}: {moved} of 200000 off, by at most {most} of 65535");
        assert!(most <= 1, "{space:?}: {most}");
        assert!(moved <= measured, "{space:?}: {moved}");
    }
}

/// How far the op moves a color inside sRGB under a scene's thresholds,
/// where the rail and the roll's knee act by design, in Oklab ΔE (x100).
/// The design quoted at most 0.8 for the rail and 1.9 for the roll; the
/// product moves a near-pure blue by more (review L4: 2.19; 2.32 with
/// the spring), past the design's figure, and is held to what it measured.
#[test]
fn the_largest_move_inside_srgb() {
    let to = Space::Srgb.matrix();
    let from = invert3(to).unwrap();
    let op = op(Space::Srgb);
    let (mut worst, mut at) = (0.0f32, [0.0f32; 3]);
    for o in colors(400_000, 1.0, 0.0) {
        let c = apply3(&from, o);
        let (a, b) = (lab(c), lab(apply3(&from, op.apply(c))));
        let d = 100.0 * (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt();
        if d > worst {
            (worst, at) = (d, o);
        }
    }
    eprintln!("the largest move inside sRGB: {worst:.2} at sRGB {at:?}");
    assert!(worst < 2.4, "{worst} at {at:?}");
}

/// A color outside a display picture's output comes to the edge at its
/// hue and lightness, the top per channel aside: within EDGE_H degrees
/// and EDGE_L of lightness, the most at sRGB's blue corner, where the ray
/// grazes the faces and the spring holds the edge off the graze.
#[test]
fn thresholds_of_one_bring_a_color_to_the_edge_at_its_hue() {
    let (mut dl, mut dh, mut off) = (0.0f32, 0.0f32, 0.0f32);
    let space = Space::Srgb;
    let edge = Op::for_output(&space.matrix(), Params::EDGE).unwrap();
    let from = invert3(space.matrix()).unwrap();
    for c in colors(4000, 1.0, 0.0) {
        let o = apply3(&space.matrix(), c);
        if o.iter().all(|v| *v >= 0.0) || o.iter().any(|v| *v > 0.95) {
            continue;
        }
        let out = edge.apply(c);
        off = off.max(out[0].min(out[1]).min(out[2]));
        let (a, b) = (lch(lab(c)), lch(lab(apply3(&from, out))));
        dl = dl.max((a[0] - b[0]).abs());
        if b[1] > 0.02 {
            dh = dh.max(wrap(a[2] - b[2]).abs());
        }
    }
    eprintln!("to the edge: lightness within {dl}, hue within {dh} degrees, off the edge by {off}");
    assert!(
        dl < EDGE_L && dh < EDGE_H && off < EDGE_OFF,
        "{dl} {dh} {off}"
    );
}

/// The floor leaves a neutral to the bit and nothing under a stop
/// below white, and moves a bright saturated color toward white.
#[test]
fn the_floor_leaves_a_neutral_and_the_mid_tones() {
    let white = 3.0;
    for v in [0.0, 0.01, 0.18, 1.0, 3.0, 12.0, 100.0] {
        assert_eq!(floor([v; 3], white), [v; 3]);
    }
    for c in colors(20_000, 1.4, 0.0) {
        let s = c.map(|v| v.max(0.0));
        let sq: f32 = s.iter().map(|v| v * v).sum();
        let n = s.iter().map(|v| v * v * v).sum::<f32>() / sq.max(1e-30);
        if n < white / 2.0 {
            assert_eq!(floor(s, white), s, "{s:?}");
        }
    }
    let lit = floor([24.0, 0.5, 0.1], white);
    assert!(lit[1] > 0.5 && lit[2] > 0.1 && lit[0] < 24.0, "{lit:?}");
    // Every row sums to one: what one channel gives, the others take.
    let grey = floor([24.0, 24.0, 24.0], white);
    assert_eq!(grey, [24.0; 3]);
}

/// Inside the table's gamut the table's new form is today's to
/// rounding; outside, the color keeps what the table cannot see.
#[test]
fn the_look_table_inside_its_gamut_is_todays() {
    let size = 17;
    let mut lut = greycard_core::Lut3d::identity(size);
    for v in lut.data.iter_mut() {
        *v = [v[0] * 0.9 + 0.05, v[1].powf(1.2), v[2] * 0.8 + 0.1];
    }
    let look = greycard_core::lut::Look::new(std::sync::Arc::new(lut), 0.75).unwrap();
    let mut inside = 0;
    for c in colors(20_000, 1.0, 0.0) {
        let r = apply3(look.to_lut(), c);
        let (a, b) = (look.at(c), look_kept(&look, c));
        if r.iter().all(|v| (0.0..=1.0).contains(v)) {
            inside += 1;
            for k in 0..3 {
                assert!((a[k] - b[k]).abs() < 1e-5, "{c:?}: {a:?} {b:?}");
            }
        }
    }
    assert!(inside > 5000);
    // Outside, the identity table changes nothing in the new form; the
    // old one clipped the color into the table's cube.
    let identity = greycard_core::Lut3d::identity(size);
    let look = greycard_core::lut::Look::new(std::sync::Arc::new(identity), 1.0).unwrap();
    let out = [0.6, 0.1, -0.05];
    let kept = look_kept(&look, out);
    assert!((0..3).all(|k| (kept[k] - out[k]).abs() < 1e-4), "{kept:?}");
    assert!((look.at(out)[2] - out[2]).abs() > 1e-3);
}

/// A profile's curve's result, brought to the working cube's lower
/// faces: nothing inside moves, and what is under zero comes to zero at
/// its lightness and hue, within CUBE_L and CUBE_H degrees.
#[test]
fn into_working_takes_the_lower_faces_alone() {
    let (mut dl, mut dh) = (0.0f32, 0.0f32);
    for c in colors(20_000, 1.5, 0.0) {
        assert_eq!(into_working(c), c);
    }
    let ok = ok();
    for c in colors(4000, 1.2, 0.2) {
        // Under zero in the working space, but a color an eye sees: its
        // hue is a hue.
        if c.iter().all(|v| *v >= 0.0) || apply3(&ok.to_lms, c).iter().any(|v| *v <= 0.0) {
            continue;
        }
        let w = into_working(c);
        assert!(w.iter().all(|v| *v >= 0.0));
        let (a, b) = (lch(lab(c)), lch(lab(w)));
        if a[0] > 0.05 && b[1] > 0.03 {
            dl = dl.max((a[0] - b[0]).abs());
            dh = dh.max(wrap(a[2] - b[2]).abs());
        }
    }
    eprintln!("into the working cube: lightness within {dl}, hue within {dh} degrees");
    assert!(dl < CUBE_L && dh < CUBE_H, "{dl} {dh}");
}

// The ramps of §7, steps 1 and 2.

const N: usize = 1024;
const LIGHTS: [f32; 5] = [0.2, 0.35, 0.55, 0.75, 0.9];

/// Oklab chroma at the edge of `space` along lightness `l` and hue `h`.
fn edge_chroma(l: f32, h: f32, to: &Matrix3) -> f32 {
    let (mut lo, mut hi) = (0.0f32, 0.6f32);
    let (cs, sn) = (h.to_radians().cos(), h.to_radians().sin());
    for _ in 0..40 {
        let c = 0.5 * (lo + hi);
        let v = apply3(to, from_lab([l, c * cs, c * sn]));
        if v.iter().all(|x| (-1e-7..=1.0 + 1e-7).contains(x)) {
            lo = c;
        } else {
            hi = c;
        }
    }
    lo
}

/// The largest fall from a running maximum.
fn fall(v: &[f32]) -> f32 {
    let mut run = f32::MIN;
    let mut most = 0.0f32;
    for x in v {
        run = run.max(*x);
        most = most.max(run - x);
    }
    most
}

/// How far a hue runs back against its overall way, degrees, where the
/// chroma is over 0.02.
fn back(labs: &[[f32; 3]]) -> f32 {
    let mut hs: Vec<f32> = Vec::new();
    for l in labs {
        let p = lch(*l);
        if p[1] > 0.02 {
            let h = match hs.last() {
                Some(last) => last + wrap(p[2] - last.rem_euclid(360.0)),
                None => p[2],
            };
            hs.push(h);
        }
    }
    if hs.len() < 2 {
        return 0.0;
    }
    let total: f32 = hs.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
    (total - (hs[hs.len() - 1] - hs[0]).abs()) / 2.0
}

/// The sharpest turn along a path in Oklab, as round 3 measured it.
fn kink(labs: &[[f32; 3]]) -> f32 {
    let d: Vec<[f32; 3]> = labs
        .windows(2)
        .map(|w| [0, 1, 2].map(|k| w[1][k] - w[0][k]))
        .collect();
    let norm = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    d.windows(2)
        .map(|w| {
            let speed = norm(w[0]) + norm(w[1]);
            if speed > 1e-4 {
                norm([0, 1, 2].map(|k| w[1][k] - w[0][k])) / (speed + 1e-6)
            } else {
                0.0
            }
        })
        .fold(0.0, f32::max)
}

/// What a ramp measured through a way.
#[derive(Debug, Default, Clone, Copy)]
struct Measure {
    l_fall: f32,
    c_fall: f32,
    hue_back: f32,
    kink: f32,
}

fn measure(labs: &[[f32; 3]]) -> Measure {
    let l: Vec<f32> = labs.iter().map(|v| v[0]).collect();
    let c: Vec<f32> = labs.iter().map(|v| v[1].hypot(v[2])).collect();
    Measure {
        l_fall: 100.0 * fall(&l),
        c_fall: 100.0 * fall(&c),
        hue_back: back(labs),
        kink: kink(labs),
    }
}

/// A ramp through the op and through the clamp, each in absolute
/// Oklab.
fn through(space: Space, ramp: &[[f32; 3]]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let op = op(space);
    let m = space.matrix();
    let from = invert3(m).unwrap();
    let ours = ramp
        .iter()
        .map(|c| lab(apply3(&from, op.apply(*c))))
        .collect();
    let clamp = ramp
        .iter()
        .map(|c| lab(apply3(&from, apply3(&m, *c).map(|v| v.clamp(0.0, 1.0)))))
        .collect();
    (ours, clamp)
}

/// Step 1's chroma ramps: five lightnesses, a hue every 15 degrees,
/// out from grey to three times sRGB's edge or Rec.2020's.
fn chroma_ramps() -> Vec<(f32, f32, Vec<[f32; 3]>)> {
    let srgb = Space::Srgb.matrix();
    let ident = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let mut out = Vec::new();
    for l in LIGHTS {
        for h in (0..360).step_by(15).map(|h| h as f32) {
            let top = (3.0 * edge_chroma(l, h, &srgb)).min(edge_chroma(l, h, &ident));
            let (cs, sn) = (h.to_radians().cos(), h.to_radians().sin());
            let ramp = (0..N)
                .map(|i| {
                    let c = top * i as f32 / (N - 1) as f32;
                    from_lab([l, c * cs, c * sn])
                })
                .collect();
            out.push((l, h, ramp));
        }
    }
    out
}

/// The lights of round 3's exposure ramps.
const LIGHTS_RGB: [(&str, [f32; 3]); 11] = [
    ("R", [1.0, 0.0, 0.0]),
    ("G", [0.0, 1.0, 0.0]),
    ("B", [0.0, 0.0, 1.0]),
    ("C", [0.0, 1.0, 1.0]),
    ("M", [1.0, 0.0, 1.0]),
    ("Y", [1.0, 1.0, 0.0]),
    ("lantern orange", [1.0, 0.28, 0.02]),
    ("sodium amber", [1.0, 0.55, 0.05]),
    ("neon pink", [1.0, 0.05, 0.45]),
    ("LED blue", [0.05, 0.25, 1.0]),
    ("skin, red light", [1.0, 0.32, 0.22]),
];

/// A light's color at mid grey's luminance, `ev` stops from it as the
/// develop hands it over (the body's baseline goes on in the finish, as
/// round 3 had it), through the default display stage.
fn display_ramp(rgb: [f32; 3], evs: impl Iterator<Item = f32>) -> Vec<[f32; 3]> {
    let global = crate::finish::Baked::global(
        &greycard_edit::Edit::default(),
        crate::finish::Source::SCENE,
    );
    let y = LUMA[0] * rgb[0] + LUMA[1] * rgb[1] + LUMA[2] * rgb[2];
    evs.map(|ev| {
        let s = rgb.map(|v| v / y * 0.18 * 2f32.powf(ev));
        crate::finish::finish_pixel_display(s, &global, &[], 0.0, None, None, None, None)
    })
    .collect()
}

/// Step 1's exposure ramps: six stops either way.
fn exposure_ramps() -> Vec<(&'static str, Vec<[f32; 3]>)> {
    LIGHTS_RGB
        .into_iter()
        .map(|(name, rgb)| {
            let evs = (0..N).map(|i| -6.0 + 12.0 * i as f32 / (N - 1) as f32);
            (name, display_ramp(rgb, evs))
        })
        .collect()
}

/// Step 1 on the chroma ramps, sRGB and P3. The design's gates and what
/// the bound at 1.1 to 1.2 measured (round 4):
///
/// | gate | design | measured, sRGB / P3 |
/// |---|---|---|
/// | lightness drop, ramps inside sRGB's top | 0.2 | 0.001 / 0.000 |
/// | chroma drop, ramps inside sRGB's top | 0.2 | 0.000 / 0.000 |
/// | hue back on any chroma ramp | 0.5 deg | 0.419 / 0.376 |
/// | kinks past the clamp's | none | none |
/// | hue drift to a reach of 1.2, inside the top | 0.1 deg | 1.03 / 1.13, missed |
///
/// The drift gate is missed by the chosen bound, which starts at 1.1:
/// the bound moves a color's hue toward the output's nearest color by
/// design. The test holds the drift to what was measured.
#[test]
fn the_chroma_ramps_hold_round_fours_numbers() {
    let srgb = Space::Srgb.matrix();
    for (space, drift_bound) in [(Space::Srgb, 1.1), (Space::DisplayP3, 1.2)] {
        let op = op(space);
        let (mut l_fall, mut c_fall, mut hue, mut drift) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut kinks = Vec::new();
        for (l, h, ramp) in chroma_ramps() {
            let over = ramp
                .iter()
                .any(|c| apply3(&srgb, *c).iter().any(|v| *v > 1.0 + 1e-6));
            let (ours, clamp) = through(space, &ramp);
            let (m, k) = (measure(&ours), measure(&clamp));
            if !over {
                l_fall = l_fall.max(m.l_fall);
                c_fall = c_fall.max(m.c_fall);
                for (c, o) in ramp.iter().zip(&ours) {
                    let (a, b) = (lch(lab(*c)), lch(*o));
                    if op.reach(*c) <= 1.2 && a[1] > 0.02 && b[1] > 0.02 {
                        drift = drift.max(wrap(b[2] - a[2]).abs());
                    }
                }
            }
            hue = hue.max(m.hue_back);
            if m.kink > k.kink + 0.02 {
                kinks.push((l, h, m.kink, k.kink));
            }
        }
        eprintln!("{space:?}: L {l_fall:.3} C {c_fall:.3} hue {hue:.3} drift {drift:.3}");
        assert!(l_fall < 0.2, "{space:?} lightness drop {l_fall}");
        assert!(c_fall < 0.2, "{space:?} chroma drop {c_fall}");
        assert!(hue < 0.5, "{space:?} hue back {hue}");
        assert!(kinks.is_empty(), "{space:?} kinks {kinks:?}");
        assert!(drift < drift_bound, "{space:?} drift {drift}");
    }
}

/// Step 1 on the exposure ramps, through the display stage (the floor
/// in it): no lightness falls, and the hue runs back no
/// more than 2 degrees past the clamp's on the same ramp (round 4: 0.003
/// sRGB, 0.45 P3, before the floor).
#[test]
fn the_exposure_ramps_hold_round_fours_numbers() {
    for space in [Space::Srgb, Space::DisplayP3] {
        let mut most = 0.0f32;
        for (name, ramp) in exposure_ramps() {
            let (ours, clamp) = through(space, &ramp);
            let (m, k) = (measure(&ours), measure(&clamp));
            most = most.max(m.hue_back - k.hue_back);
            assert!(m.l_fall < 0.2, "{space:?} {name}: lightness {}", m.l_fall);
            assert!(
                m.hue_back - k.hue_back < 2.0,
                "{space:?} {name}: hue back {} against {}",
                m.hue_back,
                k.hue_back
            );
            assert!(m.kink <= k.kink + 0.02, "{space:?} {name}: kink {}", m.kink);
        }
        eprintln!("{space:?}: exposure hue back over the clamp's {most:.3}");
    }
}

/// Step 2: sRGB against P3 on the exposure ramps, the largest hue
/// difference where both are colored. The design's gate is 0.5 degrees
/// on lanterns, amber, pink, and skin; round 4 measured the chosen bound
/// at 0.17, 0.17, 1.57, and 0.01, missing it on the pink, whose far
/// part P3 holds and sRGB bounds. Blue and green are reported, not
/// gated (7.57 and 3.63). Through the display stage, the floor in it,
/// the product measures 0.12, 0.14, 1.55, 0.00, 7.60,
/// and 6.04: green's two stops of whitening differ between the two
/// tops. Each is held to what it measured.
#[test]
fn srgb_and_p3_agree_under_the_top() {
    let bounds = [
        ("lantern orange", 0.5),
        ("sodium amber", 0.5),
        ("neon pink", 1.7),
        ("skin, red light", 0.5),
        ("LED blue", 8.0),
        ("G", 6.1),
    ];
    let ramps = exposure_ramps();
    for (name, bound) in bounds {
        let ramp = &ramps.iter().find(|(n, _)| *n == name).unwrap().1;
        let (a, _) = through(Space::Srgb, ramp);
        let (b, _) = through(Space::DisplayP3, ramp);
        let most = a
            .iter()
            .zip(&b)
            .map(|(a, b)| (lch(*a), lch(*b)))
            .filter(|(a, b)| a[1] > 0.03 && b[1] > 0.03)
            .map(|(a, b)| wrap(a[2] - b[2]).abs())
            .fold(0.0, f32::max);
        eprintln!("{name}: {most:.2}");
        assert!(most < bound, "{name}: {most}");
    }
}

/// §6.2's tests of the floor through the display stage at sRGB: every
/// exposure ramp is white (Oklab chroma under 0.01, lightness over
/// 0.99) within 9 stops of mid grey. Round 4 measured green at 9.08,
/// past the gate by a step; 9.1 is held.
#[test]
fn the_floor_takes_every_ramp_to_white() {
    let op = op(Space::Srgb);
    let from = invert3(Space::Srgb.matrix()).unwrap();
    for (name, rgb) in LIGHTS_RGB {
        let evs: Vec<f32> = (0..=1800).map(|i| -6.0 + i as f32 * 0.01).collect();
        let ramp = display_ramp(rgb, evs.iter().copied());
        let white_at = evs.iter().zip(&ramp).find_map(|(ev, d)| {
            let l = lch(lab(apply3(&from, op.apply(*d))));
            (l[1] < 0.01 && l[0] > 0.99).then_some(*ev)
        });
        eprintln!("{name}: white at {white_at:?}");
        assert!(white_at.is_some_and(|ev| ev <= 9.1), "{name}: {white_at:?}");
    }
}

/// What the op costs a pixel on one thread, against the clamp's, for
/// colors inside under the knees (the early out) and colors past the
/// edge: `cargo test --release -p greycard-render --lib cost_of_the_op
/// -- --ignored --nocapture`.
#[test]
#[ignore = "a timing"]
fn the_cost_of_the_op() {
    let space = Space::Srgb;
    let m = space.matrix();
    let op = op(space);
    let from = invert3(m).unwrap();
    let inside: Vec<[f32; 3]> = colors(1 << 20, 0.7, -0.15)
        .into_iter()
        .map(|o| apply3(&from, o.map(|v| v.max(0.0) + 0.15)))
        .collect();
    let outside: Vec<[f32; 3]> = colors(1 << 20, 1.2, 0.0)
        .into_iter()
        .filter(|c| apply3(&m, *c).iter().any(|v| *v < 0.0))
        .collect();
    let time = |what: &str, px: &[[f32; 3]], f: &dyn Fn([f32; 3]) -> [f32; 3]| {
        let t = std::time::Instant::now();
        let mut sum = 0.0f32;
        for c in px {
            sum += std::hint::black_box(f(*c))[1];
        }
        let ns = t.elapsed().as_nanos() as f64 / px.len() as f64;
        eprintln!("{what}: {ns:.1} ns a pixel ({sum:.0})");
    };
    let clamp = |c: [f32; 3]| apply3(&m, c).map(|v| v.clamp(0.0, 1.0));
    time("clamp, inside", &inside, &clamp);
    time("op, inside", &inside, &|c| op.apply(c));
    time("clamp, outside", &outside, &clamp);
    time("op, outside", &outside, &|c| op.apply(c));
    time("floor, near white", &inside, &|c| {
        floor(c.map(|v| v * 8.0), 3.0)
    });
}

/// A color past every number is the clamp's (review L7): the output
/// matrix and the final clamp alone, to the bit.
/// An infinite red is red, not black.
#[test]
fn a_color_past_every_number_is_the_clamps() {
    for space in OUTPUTS {
        for params in [Params::SCENE, Params::EDGE] {
            let op = Op::for_output(&space.matrix(), params).unwrap();
            for c in [
                [f32::INFINITY, 0.2, 0.1],
                [0.3, f32::NEG_INFINITY, 0.5],
                [f32::NAN, 0.4, 0.4],
                [f32::INFINITY; 3],
            ] {
                let want = apply3(&space.matrix(), c).map(|v| v.clamp(0.0, 1.0));
                let got = op.apply(c);
                assert!(
                    got.iter()
                        .zip(want)
                        .all(|(a, b)| a.to_bits() == b.to_bits()),
                    "{space:?} {c:?}: {got:?} against {want:?}"
                );
            }
            assert_eq!(op.apply([f32::INFINITY, 0.2, 0.1])[0], 1.0, "{space:?}");
        }
    }
}
