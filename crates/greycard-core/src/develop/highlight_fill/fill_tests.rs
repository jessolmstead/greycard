//! The synthesis, the upsample, the apply (SPEC.md stages 10 and 11),
//! and the whole fill against the oracle's fixtures with SPEC's
//! tolerances; their properties; that each mutation the tolerances must
//! catch is caught; and the whole fill's timing (ignored, reported).

use std::time::Instant;

use super::analysis::{Analysis, analyze};
use super::apply::{Fields, TILE, pixel};
use super::fixtures::{Fixture, Inputs, random_weights};
use super::half::{CodePlane, PixelCodes};
use super::harmonic::source;
use super::net::{FillLook, OUTPUTS, Reach};
use super::synthesis::{self, CellOut, Scaled, Synthesis};
use super::tests::{DEPTH_REL, HARM_BRIGHT, close, close_rel, exact, peak_rss_mb, synthetic, ulps};
use super::{down, down_codes, fill, fill_fields, oklab, region_outputs};

/// hl-1's look (`oracle.py`'s constants block).
pub(super) const LOOK: FillLook = FillLook {
    core_whiten: 0.5,
    core_boost: 1.4,
    harmonic_bright: 0.25,
    depth_tie: [12.0, 48.0],
    gain_sigma: 6.0,
    reach: None,
};

/// hl-1's look with a reach small enough for these scenes to meet
/// every part of it: regions under the gate, on its shoulder, and past
/// it, cores both shorter and longer than the ramp, and neighbors
/// sharing a gate.
pub(super) const REACH_LOOK: FillLook = FillLook {
    reach: Some(Reach {
        gate: [16.0, 40.0],
        ramp: Some(12.0),
        near: Some(24.0),
    }),
    ..LOOK
};

/// SPEC.md's tolerances with the random weights.
const TOL_OUTPUTS: f32 = 5e-6;
/// Absolute, except L and L2: relative to max(|v|, 1), as the depths
/// they scale (they reach 4096).
const TOL_PARAMS: f32 = 5e-6;
/// s, mix, mixg, s2, tie, keep, and dab_h (and the color's parts).
const TOL_FIELD: f32 = 5e-6;
const TOL_GAIN: f32 = 1e-5;
/// The final picture, relative to max(|v|, 1).
const TOL_PICTURE: f32 = 5e-6;

fn plane_codes(inp: &Inputs) -> CodePlane<'_> {
    CodePlane {
        codes: &inp.codes,
        width: inp.width,
    }
}

fn analysis_of(inp: &Inputs) -> Analysis {
    analyze(
        &inp.x,
        inp.width,
        inp.height,
        &plane_codes(inp),
        inp.clips,
        HARM_BRIGHT,
    )
}

/// The analysis and the network's outputs (None without a region).
fn outputs_of(inp: &Inputs) -> (Analysis, Option<Vec<[f32; OUTPUTS]>>) {
    let a = analysis_of(inp);
    let out = region_outputs(&a, &inp.x, &plane_codes(inp), &random_weights());
    (a, out)
}

/// A half-size plane of one value of every cell's synthesis.
fn plane(syn: &Synthesis, n: usize, f: impl Fn(&CellOut) -> f32) -> Vec<f32> {
    (0..n).map(|i| f(&syn.cell(i))).collect()
}

/// The same for a pair, channels first.
fn plane2(syn: &Synthesis, n: usize, f: impl Fn(&CellOut) -> [f32; 2]) -> Vec<f32> {
    let a = plane(syn, n, |c| f(c)[0]);
    let b = plane(syn, n, |c| f(c)[1]);
    [a, b].concat()
}

/// The fields at a pixel, zeros where none reaches.
fn up3(fields: &Fields, x: usize, y: usize) -> [f32; 3] {
    fields
        .upsampled(x, y)
        .map_or([0.0; 3], |(g, d)| [g, d[0], d[1]])
}

/// The pixels with k > 0, raster order: the rows of `out_clipped`.
fn clipped(inp: &Inputs) -> Vec<usize> {
    (0..inp.width * inp.height)
        .filter(|&p| inp.codes[p] & 0x7F > 0)
        .collect()
}

/// The largest error of a filled picture against the oracle's, relative
/// to max(|v|, 1): at the clipped pixels the oracle leaves as x (bit for
/// bit), any difference in bits counts as infinite; elsewhere among the
/// clipped pixels the difference, a non-finite value infinite; every
/// unclipped pixel must keep its bits, or infinite.
fn picture_error(inp: &Inputs, out: &[f32], want: &[f32]) -> f32 {
    let sel = clipped(inp);
    assert_eq!(want.len(), 3 * sel.len());
    let mut worst = 0.0f32;
    let mut is_clipped = vec![false; inp.width * inp.height];
    for (k, &p) in sel.iter().enumerate() {
        is_clipped[p] = true;
        let (x, o, w) = (
            &inp.x[3 * p..3 * p + 3],
            &out[3 * p..3 * p + 3],
            &want[3 * k..3 * k + 3],
        );
        let left = x.iter().zip(w).all(|(a, b)| a.to_bits() == b.to_bits());
        for c in 0..3 {
            let e = if left {
                if o[c].to_bits() == x[c].to_bits() {
                    0.0
                } else {
                    f32::INFINITY
                }
            } else if o[c].is_finite() && w[c].is_finite() {
                (o[c] - w[c]).abs() / w[c].abs().max(1.0)
            } else {
                f32::INFINITY
            };
            worst = worst.max(e);
        }
    }
    for (p, &c) in is_clipped.iter().enumerate() {
        if !c && (0..3).any(|k| out[3 * p + k].to_bits() != inp.x[3 * p + k].to_bits()) {
            return f32::INFINITY;
        }
    }
    worst
}

/// The parameters against the oracle's: L and L2 (columns 1 and 11)
/// relative to max(|v|, 1), the rest absolute.
fn params_close(s: &str, got: &[f32], want: &[f32]) {
    assert_eq!(got.len(), want.len());
    let mut worst = [0.0f32; 2];
    for (k, (&g, &w)) in got.iter().zip(want).enumerate() {
        let col = k % OUTPUTS;
        let (e, slot) = if col == 1 || col == 11 {
            ((g - w).abs() / w.abs().max(1.0), 1)
        } else {
            ((g - w).abs(), 0)
        };
        assert!(e <= TOL_PARAMS, "{s}/params[{k}]: {g} against {w}");
        worst[slot] = worst[slot].max(e);
    }
    eprintln!(
        "{s:>16} {:<12} max {:.2e}, L and L2 {:.2e} relative (tol {TOL_PARAMS:.0e})",
        "params", worst[0], worst[1]
    );
}

#[test]
fn every_scene_fills_as_the_oracle() {
    let weights = random_weights();
    let scenes = Fixture::scenes();
    assert_eq!(scenes.len(), 17);
    for name in &scenes {
        let fx = Fixture::load(name);
        let inp = fx.inputs();
        let s = name.as_str();
        let f = inp.f as f64;
        let codes = plane_codes(&inp);
        let (a, outputs) = outputs_of(&inp);
        let n = a.half_width * a.half_height;
        let fields = match outputs {
            None => {
                assert!(!fx.has("ids"), "{s}: no region");
                None
            }
            Some(outputs) => {
                let ids = fx.i32("ids");
                let mut want = fx.f32("outputs");
                if ids[0] == 0 {
                    want[..OUTPUTS].fill(0.0);
                }
                close(s, "outputs", &outputs.concat(), &want, TOL_OUTPUTS);
                let syn = Synthesis::new(&a, &outputs, Scaled::new(&LOOK, f));
                let params: Vec<f32> = syn.tables.params.iter().flat_map(|p| p.row()).collect();
                params_close(s, &params, &fx.f32("params"));
                let (depth, cdepth) = syn.depth();
                close_rel(s, "depth", &depth, &fx.f32("depth"), DEPTH_REL);
                close_rel(s, "cdepth", &cdepth, &fx.f32("cdepth"), DEPTH_REL);
                if fx.has("s") {
                    every_cell(&fx, &a, &syn);
                }
                let fields = syn.fields();
                assert_eq!(
                    fields,
                    synthesis::fields(&a, &outputs, Scaled::new(&LOOK, f))
                );
                // The fields as kept, by tile, are the synthesis's.
                let wh = a.half_width;
                let kept: Vec<[f32; 3]> = (0..n).map(|i| fields.value(i % wh, i / wh)).collect();
                for (i, k) in kept.iter().enumerate() {
                    let c = syn.cell(i);
                    assert_eq!(*k, [c.gain, c.dab[0], c.dab[1]], "{s}: cell {i}");
                }
                if fx.has("gain_h") {
                    let gain: Vec<f32> = kept.iter().map(|v| v[0]).collect();
                    let dab: Vec<f32> = [1, 2]
                        .iter()
                        .flat_map(|&c| kept.iter().map(move |v| v[c]))
                        .collect();
                    close(s, "gain_h", &gain, &fx.f32("gain_h"), TOL_GAIN);
                    close(s, "dab_h", &dab, &fx.f32("dab_h"), TOL_FIELD);
                }
                assert_eq!((fields.width(), fields.height()), (inp.width, inp.height));
                Some(fields)
            }
        };
        drop(a);
        if fx.has("gain_up") {
            let fl = fields.as_ref().unwrap();
            let up: Vec<[f32; 3]> = (0..inp.width * inp.height)
                .map(|p| up3(fl, p % inp.width, p / inp.width))
                .collect();
            let gain: Vec<f32> = up.iter().map(|v| v[0]).collect();
            let dab: Vec<f32> = [1, 2]
                .iter()
                .flat_map(|&c| up.iter().map(move |v| v[c]))
                .collect();
            close(s, "gain_up", &gain, &fx.f32("gain_up"), TOL_GAIN);
            close(s, "dab_up", &dab, &fx.f32("dab_up"), TOL_FIELD);
        }
        let mut out = inp.x.clone();
        if let Some(fl) = &fields {
            fl.apply(&mut out, &codes);
        }
        let e = picture_error(&inp, &out, &fx.f32("out_clipped"));
        assert!(e <= TOL_PICTURE, "{s}: picture {e:e}");
        eprintln!(
            "{s:>16} {:<12} max {e:.2e} relative (tol {TOL_PICTURE:.0e})",
            "picture"
        );
        // The entry point gives the same fields.
        let whole = fill_fields(
            &inp.x, inp.width, inp.height, &codes, inp.clips, &weights, &LOOK, f,
        );
        assert_eq!(whole, fields, "{s}: fill_fields");
    }
}

/// lamp_f3 stores every stage of the synthesis.
fn every_cell(fx: &Fixture, a: &Analysis, syn: &Synthesis) {
    let s = fx.name.as_str();
    let n = a.half_width * a.half_height;
    let inv = |i: usize| a.labels.inv[i] as usize;
    let off: Vec<f32> = (0..n).map(|i| syn.tables.off[inv(i)]).collect();
    exact(s, "off", &off, &fx.f32("off"));
    let (dd, dc): (Vec<f32>, Vec<f32>) = (0..n).map(|i| syn.depths(i)).unzip();
    close_rel(s, "dd", &dd, &fx.f32("dd"), DEPTH_REL);
    close_rel(s, "dc", &dc, &fx.f32("dc"), DEPTH_REL);
    for (what, f, tol) in [
        ("s", (|c: &CellOut| c.s) as fn(&CellOut) -> f32, TOL_FIELD),
        ("mix", |c| c.mix, TOL_FIELD),
        ("mixg", |c| c.mixg, TOL_FIELD),
        ("gain_band", |c| c.gain_band, TOL_GAIN),
        ("s2", |c| c.s2, TOL_FIELD),
        ("tie", |c| c.tie, TOL_FIELD),
        ("keep", |c| c.keep, TOL_FIELD),
    ] {
        close(s, what, &plane(syn, n, f), &fx.f32(what), tol);
    }
    for (what, f) in [
        (
            "core_dab",
            (|c: &CellOut| c.core_dab) as fn(&CellOut) -> [f32; 2],
        ),
        ("band_dab", |c| c.band_dab),
        ("dab_mixed", |c| c.dab_mixed),
    ] {
        close(s, what, &plane2(syn, n, f), &fx.f32(what), TOL_FIELD);
    }
}

/// A frame of the synthetic gradient with clipped disks (cx, cy, r) in
/// px, centers anywhere (off the frame too), each with a core and
/// bands of one and two colors.
fn spots(w: usize, h: usize, disks: &[(f32, f32, f32)]) -> (Vec<f32>, Vec<u8>) {
    let (mut x, mut codes) = synthetic(w, h, 0, 1);
    // synthetic always lays one large disk: start from its gradient.
    codes.fill(0);
    for y in 0..h {
        for xx in 0..w {
            let p = 3 * (y * w + xx);
            let t = (xx + y) as f32 / (w + h) as f32;
            x[p..p + 3].copy_from_slice(&[0.05 + 0.5 * t, 0.08 + 0.4 * t, 0.3 - 0.2 * t]);
        }
    }
    for &(cx, cy, r) in disks {
        let warm = [1.0f32, 0.92, 0.7];
        let ys = (cy - r - 1.0).max(0.0) as usize..((cy + r + 2.0).max(0.0) as usize).min(h);
        for y in ys {
            let xs = (cx - r - 1.0).max(0.0) as usize..((cx + r + 2.0).max(0.0) as usize).min(w);
            for xx in xs {
                let d = (xx as f32 - cx).hypot(y as f32 - cy);
                let (code, v) = if d < 0.6 * r {
                    (3u8, 1.0)
                } else if d < 0.8 * r {
                    (2, 0.97)
                } else if d < r {
                    (1, 0.9)
                } else {
                    continue;
                };
                codes[y * w + xx] = codes[y * w + xx].max(code);
                let p = 3 * (y * w + xx);
                for c in 0..3 {
                    x[p + c] = x[p + c].max(v * warm[c]);
                }
            }
        }
    }
    (x, codes)
}

/// The windows change no bit: on scattered regions, two in opposite
/// corners, pairs whose boxes sit either side of the merging reach, and
/// regions cut by the frame, every cell's values, every row's
/// parameters, and the fields equal one window over the whole grid's,
/// at f = 1, 2.5, and 3.
#[test]
fn the_windows_are_the_whole_grid_s() {
    let weights = random_weights();
    // Pairs of 4 px disks whose gaps run 26 to 46 px, the boxes some 13
    // to 23 cells apart, either side of the reach at f = 1 (18 cells, σ
    // 6's radius); at f = 2.5 and 3 the reach is 9 (σ 3's) and none
    // merge. The pairs sit 30 cells apart down the frame.
    let mut pairs: Vec<(f32, f32, f32)> = Vec::new();
    for (k, gap) in (26..=46).step_by(2).enumerate() {
        let y = 20.0 + 60.0 * k as f32;
        pairs.push((30.0, y, 4.0));
        pairs.push((30.0 + 8.0 + gap as f32, y, 4.0));
    }
    type Frame = (&'static str, usize, usize, Vec<f32>, Vec<u8>);
    let frames: Vec<Frame> = vec![
        {
            let (x, c) = synthetic(403, 251, 6, 5);
            ("scattered", 403, 251, x, c)
        },
        {
            let (x, c) = synthetic(403, 251, 40, 3);
            ("many", 403, 251, x, c)
        },
        {
            let (x, c) = spots(301, 207, &[(9.0, 9.0, 6.0), (291.0, 197.0, 6.0)]);
            ("corners", 301, 207, x, c)
        },
        {
            let (x, c) = spots(160, 680, &pairs);
            ("pairs", 160, 680, x, c)
        },
        {
            let edge = [
                (0.0, 0.0, 9.0),
                (179.0, 60.0, 12.0),
                (90.0, 140.0, 20.0),
                (-30.0, 80.0, 40.0),
            ];
            let (x, c) = spots(181, 141, &edge);
            ("edges", 181, 141, x, c)
        },
    ];
    for (name, w, h, x, codes) in &frames {
        let plane = CodePlane { codes, width: *w };
        let a = analyze(x, *w, *h, &plane, [1.0; 3], HARM_BRIGHT);
        let outputs = region_outputs(&a, x, &plane, &weights).unwrap();
        for (f, base) in [1.0, 2.5, 3.0]
            .into_iter()
            .flat_map(|f| [(f, &LOOK), (f, &REACH_LOOK)])
        {
            let look = Scaled::new(base, f);
            let part = Synthesis::new(&a, &outputs, look);
            let whole = Synthesis::whole(&a, &outputs, look);
            let ws = part.windows();
            let cells: usize = ws.iter().map(|w| w.width * w.height).sum();
            eprintln!(
                "{name:>9} f = {f}: {} rows, {} windows over {:.0}% of the grid",
                a.regions.len(),
                ws.len(),
                100.0 * cells as f64 / (a.half_width * a.half_height) as f64
            );
            assert_eq!(part.depth(), whole.depth(), "{name}");
            assert_eq!(part.tables.params, whole.tables.params, "{name}");
            for i in 0..a.half_width * a.half_height {
                assert_eq!(part.cell(i), whole.cell(i), "{name} f = {f}: cell {i}");
            }
            assert_eq!(part.fields(), whole.fields(), "{name}");
            assert_eq!(
                synthesis::fields(&a, &outputs, look),
                whole.fields(),
                "{name}"
            );
            match *name {
                "corners" => assert_eq!(ws.len(), 2),
                "pairs" if f == 1.0 => {
                    assert!(ws.len() > 11 && ws.len() < 22, "{} windows", ws.len())
                }
                "pairs" => assert_eq!(ws.len(), 22),
                "edges" => assert!(ws.iter().any(|w| w.x0 == 0 && w.y0 == 0)),
                _ => {}
            }
        }
    }
}

#[test]
fn reduced_inputs_are_the_fixtures() {
    for (scene, src, n) in [
        ("lamp_f3", "lamp", 3),
        ("depth_tie_f3", "depth_tie", 3),
        ("depth_tie_f2_5", "depth_tie", 2),
    ] {
        let full = Fixture::load(src).inputs();
        let want = Fixture::load(scene).inputs();
        let (x, w, h) = down(&full.x, full.width, full.height, n);
        assert_eq!((w, h), (want.width, want.height), "{scene}");
        exact(scene, "down", &x, &want.x);
        let codes = down_codes(&full.codes, full.width, full.height, n);
        assert_eq!(codes, want.codes, "{scene}: down_codes");
        // Bit 7 is not carried, and the codes are capped below 3.
        assert!(codes.iter().all(|&k| k <= 3));
    }
    // A block of 9: five fully clipped make a 3, four a 2.
    let mut c = vec![0u8; 9];
    c[..4].fill(3 | 0x80);
    assert_eq!(down_codes(&c, 3, 3, 3), [2]);
    c[4] = 3;
    assert_eq!(down_codes(&c, 3, 3, 3), [3]);
    // Exactly half fully clipped is not more than half (the oracle's
    // `down_codes` gives 2 and 3 for these two).
    assert_eq!(down_codes(&[3, 3, 0, 0], 2, 2, 2), [2]);
    assert_eq!(down_codes(&[3, 3, 3, 0], 2, 2, 2), [3]);
    assert_eq!(down_codes(&[0x81, 0, 0, 0], 2, 2, 2), [1]);
    assert_eq!(down(&[1.0; 2 * 3 * 3], 3, 2, 2), (vec![1.0; 3], 1, 1));
}

/// The `tiles` scene shows what its fixture is for: a region on cells of
/// four tiles, and filled pixels whose upsample reads a tile not stored
/// beside a stored one (the oracle's dense fields, compared in
/// [`every_scene_fills_as_the_oracle`], then hold there too).
#[test]
fn the_tiles_scene_crosses_tiles() {
    let weights = random_weights();
    let inp = Fixture::load("tiles").inputs();
    let codes = plane_codes(&inp);
    let a = analysis_of(&inp);
    let fields = fill_fields(
        &inp.x, inp.width, inp.height, &codes, inp.clips, &weights, &LOOK, 1.0,
    )
    .unwrap();
    let wh = a.half_width;
    let tile = |cx: usize, cy: usize| (cx / TILE, cy / TILE);
    let stored = |cx: usize, cy: usize| {
        (0..TILE * TILE).any(|k| {
            let (x, y) = (cx / TILE * TILE + k % TILE, cy / TILE * TILE + k / TILE);
            x < wh && y < a.half_height && fields.value(x, y) != [0.0; 3]
        })
    };
    let mut four = false;
    for r in 1..a.regions.len() {
        let mut tiles: Vec<(usize, usize)> = (0..a.cells.len())
            .filter(|&i| a.labels.inv[i] as usize == r)
            .map(|i| tile(i % wh, i / wh))
            .collect();
        tiles.sort_unstable();
        tiles.dedup();
        four |= tiles.len() == 4;
    }
    assert!(four, "a region on four tiles");
    let mut across = 0;
    let mut out = inp.x.clone();
    fields.apply(&mut out, &codes);
    for p in 0..inp.width * inp.height {
        let (x, y) = (p % inp.width, p / inp.width);
        if inp.codes[p] & 0x7F == 0 {
            continue;
        }
        let (y0, y1, _, _) = source(y, a.half_height, 0.5);
        let (x0, x1, _, _) = source(x, wh, 0.5);
        let cells = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)];
        let empty = cells.iter().any(|&(cx, cy)| !stored(cx, cy));
        let full = cells.iter().any(|&(cx, cy)| stored(cx, cy));
        let changed = (0..3).any(|c| out[3 * p + c] != inp.x[3 * p + c]);
        across += (empty && full && changed) as usize;
    }
    assert!(across > 10, "{across} filled pixels read an empty tile");
}

/// The upsample and the apply on a frame of many tiles against a dense
/// reference written here: the synthesis's cells in a plain plane,
/// torch's bilinear, and the pixel fill.
#[test]
fn the_tiles_read_as_a_dense_plane() {
    let weights = random_weights();
    let (w, h) = (403, 251);
    let (x, codes) = synthetic(w, h, 12, 21);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let a = analyze(&x, w, h, &plane, [1.0; 3], HARM_BRIGHT);
    let outputs = region_outputs(&a, &x, &plane, &weights).unwrap();
    let syn = Synthesis::new(&a, &outputs, Scaled::new(&LOOK, 1.0));
    let fields = syn.fields();
    let (wh, hh) = (a.half_width, a.half_height);
    let total = wh.div_ceil(TILE) * hh.div_ceil(TILE);
    assert!(
        fields.tiles() > 4 && fields.tiles() < total,
        "{} of {total} tiles",
        fields.tiles()
    );
    let dense: Vec<[f32; 3]> = (0..wh * hh)
        .map(|i| {
            let c = syn.cell(i);
            if a.cells[i].fa() {
                [c.gain, c.dab[0], c.dab[1]]
            } else {
                [0.0; 3]
            }
        })
        .collect();
    let mut want = x.clone();
    for py in 0..h {
        for px in 0..w {
            let fy = ((py as f32 + 0.5) * 0.5 - 0.5).max(0.0);
            let fx = ((px as f32 + 0.5) * 0.5 - 0.5).max(0.0);
            let (y0, x0) = (fy as usize, fx as usize);
            let (y1, x1) = ((y0 + 1).min(hh - 1), (x0 + 1).min(wh - 1));
            let (ly, lx) = (fy - y0 as f32, fx - x0 as f32);
            let v = |xx: usize, yy: usize| dense[yy * wh + xx];
            let up: [f32; 3] = std::array::from_fn(|c| {
                (v(x0, y0)[c] * (1.0 - lx) + v(x1, y0)[c] * lx) * (1.0 - ly)
                    + (v(x0, y1)[c] * (1.0 - lx) + v(x1, y1)[c] * lx) * ly
            });
            assert_eq!(up3(&fields, px, py), up, "pixel {px}, {py}");
            let p = py * w + px;
            if codes[p] & 0x7F > 0
                && let Some(o) = pixel(
                    [x[3 * p], x[3 * p + 1], x[3 * p + 2]],
                    up[0],
                    [up[1], up[2]],
                )
            {
                want[3 * p..3 * p + 3].copy_from_slice(&o);
            }
        }
    }
    let mut out = x.clone();
    fields.apply(&mut out, &plane);
    assert!(
        out.iter()
            .zip(&want)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
}

/// A field with no gain and an offset reaches its pixel: the apply
/// fills where any of the three is nonzero.
#[test]
fn an_offset_alone_fills() {
    let mut fields = Fields::empty(4, 4, 2, 2);
    let mut tile = vec![[0.0f32; 3]; TILE * TILE];
    tile[0] = [0.0, 0.05, 0.0];
    tile[1] = [-0.0, 0.0, -0.0];
    fields.merge(0, 0, tile);
    assert_eq!(fields.upsampled(0, 0), Some((0.0, [0.05, 0.0])));
    assert_eq!(fields.upsampled(3, 0), None);
    let codes = vec![1u8; 16];
    let plane = CodePlane {
        codes: &codes,
        width: 4,
    };
    let x = vec![0.6f32; 48];
    let mut out = x.clone();
    fields.apply(&mut out, &plane);
    assert_ne!(out[..3], x[..3], "the offset moves the pixel");
    let left = pixel([0.6; 3], 0.0, [0.05, 0.0]).unwrap();
    assert_eq!(out[..3], left);
    // Pixel 3 of row 0 reads only the -0 cell: left as it is.
    assert_eq!(out[9..12], x[9..12]);
}

/// Applying one set of fields to two copies gives one picture; and at
/// f = 1, `fill` is `fill_fields` and `apply`.
#[test]
fn fields_apply_alike_to_any_picture() {
    let weights = random_weights();
    for scene in ["lamp", "mixed", "sky"] {
        let inp = Fixture::load(scene).inputs();
        let codes = plane_codes(&inp);
        let fields = fill_fields(
            &inp.x, inp.width, inp.height, &codes, inp.clips, &weights, &LOOK, 1.0,
        )
        .unwrap();
        let (mut one, mut two) = (inp.x.clone(), inp.x.clone());
        fields.apply(&mut one, &codes);
        fields.apply(&mut two, &codes);
        let bits = |v: &[f32]| v.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&one), bits(&two), "{scene}");
        assert_ne!(bits(&one), bits(&inp.x), "{scene}: something is filled");
        let mut whole = inp.x.clone();
        assert!(fill(
            &mut whole, inp.width, inp.height, &codes, inp.clips, &weights, &LOOK, 1.0
        ));
        assert_eq!(bits(&whole), bits(&one), "{scene}: fill");
    }
}

#[test]
fn nothing_clipped_is_untouched() {
    let weights = random_weights();
    let inp = Fixture::load("no_clip").inputs();
    let codes = plane_codes(&inp);
    let mut x = inp.x.clone();
    assert!(!fill(
        &mut x, inp.width, inp.height, &codes, inp.clips, &weights, &LOOK, 1.0
    ));
    assert!(
        x.iter()
            .zip(&inp.x)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
    // Fields applied where no code is set change nothing.
    let lamp = Fixture::load("lamp").inputs();
    let fields = fill_fields(
        &lamp.x,
        lamp.width,
        lamp.height,
        &plane_codes(&lamp),
        lamp.clips,
        &weights,
        &LOOK,
        1.0,
    )
    .unwrap();
    let none = vec![0x80u8; lamp.width * lamp.height];
    let mut x = lamp.x.clone();
    fields.apply(
        &mut x,
        &CodePlane {
            codes: &none,
            width: lamp.width,
        },
    );
    assert!(
        x.iter()
            .zip(&lamp.x)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
    // An empty picture has no fields.
    let empty = CodePlane {
        codes: &[],
        width: 0,
    };
    assert_eq!(
        fill_fields(&[], 0, 0, &empty, [1.0; 3], &weights, &LOOK, 1.0),
        None
    );
}

#[test]
#[should_panic(expected = "the codes are the fill's size")]
fn codes_of_another_size_are_refused() {
    let inp = Fixture::load("lamp").inputs();
    let fields = fill_fields(
        &inp.x,
        inp.width,
        inp.height,
        &plane_codes(&inp),
        inp.clips,
        &random_weights(),
        &LOOK,
        1.0,
    )
    .unwrap();
    let mut x = inp.x.clone();
    // The same pixel count, another shape: a turned picture's codes.
    let turned = CodePlane {
        codes: &inp.codes,
        width: inp.height,
    };
    fields.apply(&mut x, &turned);
}

#[test]
fn non_finite_pixels_pass_through() {
    let weights = random_weights();
    let inp = Fixture::load("nonfinite").inputs();
    let codes = plane_codes(&inp);
    let mut x = inp.x.clone();
    assert!(fill(
        &mut x, inp.width, inp.height, &codes, inp.clips, &weights, &LOOK, 1.0
    ));
    let mut odd = 0;
    for (p, px) in inp.x.as_chunks::<3>().0.iter().enumerate() {
        if px.iter().any(|v| !v.is_finite()) {
            odd += 1;
            for c in 0..3 {
                assert_eq!(x[3 * p + c].to_bits(), px[c].to_bits(), "pixel {p}");
            }
        } else {
            assert!(
                x[3 * p..3 * p + 3].iter().all(|v| v.is_finite()),
                "pixel {p}"
            );
        }
    }
    assert!(odd >= 2, "{odd} non-finite pixels");
    assert_eq!(pixel([f32::NAN, 1.0, 1.0], 1.0, [0.1, 0.0]), None);
    assert_eq!(pixel([f32::INFINITY, 1.0, 1.0], 1.0, [0.0; 2]), None);
    assert_eq!(pixel([1e-7, 1e-7, 1e-7], 1.0, [0.0; 2]), None);
    assert_eq!(pixel([0.9, 0.9, 0.9], -0.0, [0.0, -0.0]), None);
}

#[test]
fn the_fill_is_the_same_on_any_thread_count() {
    let weights = random_weights();
    let (w, h) = (403, 251);
    let (x, codes) = synthetic(w, h, 60, 7);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let on = |threads: usize, f: f64, look: &FillLook| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                let fields =
                    fill_fields(&x, w, h, &plane, [1.0, 0.98, 0.95], &weights, look, f).unwrap();
                let mut out = x.clone();
                fields.apply(&mut out, &plane);
                (fields, out.iter().map(|v| v.to_bits()).collect::<Vec<_>>())
            })
    };
    for f in [1.0, 2.5] {
        for look in [&LOOK, &REACH_LOOK] {
            let one = on(1, f, look);
            assert!(one.0.tiles() > 0);
            assert_eq!(one, on(3, f, look));
            assert_eq!(one, on(8, f, look));
        }
    }
}

/// An unclipped cell beside a region takes no field, and an unclipped
/// pixel beside clipped ones keeps its bits though the upsampled field
/// reaches it.
#[test]
fn the_rim_takes_no_fill() {
    let weights = random_weights();
    let inp = Fixture::load("lamp").inputs();
    let codes = plane_codes(&inp);
    let a = analysis_of(&inp);
    let fields = fill_fields(
        &inp.x, inp.width, inp.height, &codes, inp.clips, &weights, &LOOK, 1.0,
    )
    .unwrap();
    let (wh, hh) = (a.half_width, a.half_height);
    let mut rim = 0;
    for y in 0..hh {
        for x in 0..wh {
            let near = (y.saturating_sub(1)..(y + 2).min(hh)).any(|yy| {
                (x.saturating_sub(1)..(x + 2).min(wh)).any(|xx| a.cells[yy * wh + xx].fa())
            });
            if !a.cells[y * wh + x].fa() && near {
                rim += 1;
                assert!(
                    fields.value(x, y).iter().all(|&v| v == 0.0),
                    "cell {x}, {y}"
                );
            }
        }
    }
    assert!(rim > 10, "{rim} rim cells");
    let mut out = inp.x.clone();
    fields.apply(&mut out, &codes);
    let mut reached = 0;
    for p in 0..inp.width * inp.height {
        if codes.code(p % inp.width, p / inp.width) & 0x7F == 0 {
            if fields.upsampled(p % inp.width, p / inp.width).is_some() {
                reached += 1;
            }
            for c in 0..3 {
                assert_eq!(
                    out[3 * p + c].to_bits(),
                    inp.x[3 * p + c].to_bits(),
                    "pixel {p}"
                );
            }
        }
    }
    assert!(reached > 0, "the upsample reaches past the clipped pixels");
}

/// The picture with the fields upsampled by `up` and filled by `fill`,
/// at look `look`, against the scene's `out_clipped`.
fn mutated(
    scene: &str,
    look: Scaled,
    up: impl Fn(&Fields, usize, usize) -> [f32; 3],
    fill_px: impl Fn([f32; 3], f32, [f32; 2]) -> Option<[f32; 3]>,
) -> f32 {
    let fx = Fixture::load(scene);
    let inp = fx.inputs();
    let (a, outputs) = outputs_of(&inp);
    let fields = synthesis::fields(&a, &outputs.unwrap(), look);
    let mut out = inp.x.clone();
    for p in 0..inp.width * inp.height {
        if inp.codes[p] & 0x7F == 0 {
            continue;
        }
        let [g, da, db] = up(&fields, p % inp.width, p / inp.width);
        if let Some(v) = fill_px([out[3 * p], out[3 * p + 1], out[3 * p + 2]], g, [da, db]) {
            out[3 * p..3 * p + 3].copy_from_slice(&v);
        }
    }
    picture_error(&inp, &out, &fx.f32("out_clipped"))
}

/// align_corners true: src = dst·(n_in − 1)/(n_out − 1).
fn corners(fields: &Fields, x: usize, y: usize, wh: usize, hh: usize) -> [f32; 3] {
    let axis = |d: usize, n_in: usize, n_out: usize| {
        let src = if n_out > 1 {
            d as f32 * (n_in - 1) as f32 / (n_out - 1) as f32
        } else {
            0.0
        };
        let i0 = (src.floor() as usize).min(n_in - 1);
        let l = src - i0 as f32;
        (i0, (i0 + 1).min(n_in - 1), 1.0 - l, l)
    };
    let (x0, x1, w0, w1) = axis(x, wh, 2 * wh);
    let (y0, y1, h0, h1) = axis(y, hh, 2 * hh);
    let (v00, v01, v10, v11) = (
        fields.value(x0, y0),
        fields.value(x1, y0),
        fields.value(x0, y1),
        fields.value(x1, y1),
    );
    std::array::from_fn(|c| (v00[c] * w0 + v01[c] * w1) * h0 + (v10[c] * w0 + v11[c] * w1) * h1)
}

/// The trial's apply, without hl-1's luminance guard.
fn unguarded(x: [f32; 3], gain: f32, dab: [f32; 2]) -> Option<[f32; 3]> {
    if gain == 0.0 && dab == [0.0; 2] {
        return None;
    }
    let y = oklab::luma(x).max(1e-6);
    let mut u = oklab::of(x.map(|v| v / y));
    u[1] += dab[0];
    u[2] += dab[1];
    let rgb = oklab::rgb(u);
    let yn = oklab::luma(rgb).max(1e-6);
    Some(rgb.map(|v| v / yn * y * gain.exp2()))
}

/// Each mutation the tests must catch moves some scene's picture past
/// its tolerance, while the fill as written stays within it.
#[test]
fn mutations_are_caught() {
    let look = |f: f64| Scaled::new(&LOOK, f);
    let caught = |what: &str, scene: &str, e: f32| {
        eprintln!("{what:<28} on {scene:<15} {e:.2e}");
        assert!(e > TOL_PICTURE, "{what} on {scene}: {e:e} within tolerance");
    };
    for (scene, f) in [
        ("edge_cut", 1.0),
        ("depth_tie_f2_5", 2.5),
        ("lamp", 1.0),
        ("mixed", 1.0),
    ] {
        let e = mutated(scene, look(f), up3, pixel);
        assert!(e <= TOL_PICTURE, "{scene}: {e:e}");
    }
    let no_boost = Scaled {
        boost: 0.0,
        ..look(1.0)
    };
    caught(
        "boost dropped",
        "edge_cut",
        mutated("edge_cut", no_boost, up3, pixel),
    );
    let no_whiten = Scaled {
        whiten: 0.0,
        ..look(1.0)
    };
    caught(
        "whitening dropped",
        "edge_cut",
        mutated("edge_cut", no_whiten, up3, pixel),
    );
    let tie_in_px = Scaled {
        tie_lo: 12.0,
        tie_den: 36.0,
        ..look(2.5)
    };
    caught(
        "tie not over f",
        "depth_tie_f2_5",
        mutated("depth_tie_f2_5", tie_in_px, up3, pixel),
    );
    let narrow = Scaled {
        gain_sigma: None,
        ..look(1.0)
    };
    caught("mixg at σ 2", "lamp", mutated("lamp", narrow, up3, pixel));
    let inp = Fixture::load("lamp").inputs();
    let (wh, hh) = (inp.width.div_ceil(2), inp.height.div_ceil(2));
    let aligned = move |f: &Fields, x, y| corners(f, x, y, wh, hh);
    caught(
        "align_corners true",
        "lamp",
        mutated("lamp", look(1.0), aligned, pixel),
    );
    caught(
        "luminance guard removed",
        "mixed",
        mutated("mixed", look(1.0), up3, unguarded),
    );
    // A tile not stored read as anything but 0.
    let garbage = |f: &Fields, x: usize, y: usize| {
        let (wh, hh) = (f.width().div_ceil(2), f.height().div_ceil(2));
        let v = |cx: usize, cy: usize| {
            let stored = (0..TILE * TILE).any(|k| {
                let (tx, ty) = (cx / TILE * TILE + k % TILE, cy / TILE * TILE + k / TILE);
                tx < wh && ty < hh && f.value(tx, ty) != [0.0; 3]
            });
            if stored { f.value(cx, cy) } else { [1e-3; 3] }
        };
        let (y0, y1, h0, h1) = source(y, hh, 0.5);
        let (x0, x1, w0, w1) = source(x, wh, 0.5);
        let (v00, v01, v10, v11) = (v(x0, y0), v(x1, y0), v(x0, y1), v(x1, y1));
        std::array::from_fn(|c| (v00[c] * w0 + v01[c] * w1) * h0 + (v10[c] * w0 + v11[c] * w1) * h1)
    };
    caught(
        "a dropped tile misread",
        "tiles",
        mutated("tiles", look(1.0), garbage, pixel),
    );
}

/// The upsample's source rows are torch's: a 2-cell axis maps 4 pixels
/// to (0, 0), (0, 1)·¾¼, (0, 1)·¼¾, and (1, 1).
#[test]
fn the_upsample_takes_torch_s_weights() {
    assert_eq!(source(0, 2, 0.5), (0, 1, 1.0, 0.0));
    assert_eq!(source(1, 2, 0.5), (0, 1, 0.75, 0.25));
    assert_eq!(source(2, 2, 0.5), (0, 1, 0.25, 0.75));
    assert_eq!(source(3, 2, 0.5), (1, 1, 0.75, 0.25));
}

/// The oracle's own half-size fields, upsampled here, are its gain_up
/// and dab_up within 2 ulp (lamp_f3, the one scene storing them).
/// torch takes a separable kernel on frames this small and a generic
/// one on large strips, each compiled with its own contraction; no
/// order of the two lerps, fused or not, gives its bits on every pixel
/// (tried: either axis first, each lerp plain or fused either way, and
/// the four-weight sum), so we keep torch's documented order and its
/// value to 2 ulp, well inside SPEC's field tolerances.
#[test]
fn the_upsample_is_torch_s_within_two_ulp() {
    let fx = Fixture::load("lamp_f3");
    let inp = fx.inputs();
    let (wh, hh) = (inp.width.div_ceil(2), inp.height.div_ceil(2));
    let (gain, dab) = (fx.f32("gain_h"), fx.f32("dab_h"));
    let mut fields = Fields::empty(inp.width, inp.height, wh, hh);
    for ty in 0..hh.div_ceil(TILE) {
        for tx in 0..wh.div_ceil(TILE) {
            let mut tile = vec![[0.0f32; 3]; TILE * TILE];
            for (k, t) in tile.iter_mut().enumerate() {
                let (x, y) = (tx * TILE + k % TILE, ty * TILE + k / TILE);
                if x < wh && y < hh {
                    let i = y * wh + x;
                    *t = [gain[i], dab[i], dab[hh * wh + i]];
                }
            }
            fields.merge(tx, ty, tile);
        }
    }
    let n = inp.width * inp.height;
    let up: Vec<[f32; 3]> = (0..n)
        .map(|p| up3(&fields, p % inp.width, p / inp.width))
        .collect();
    let gain: Vec<f32> = up.iter().map(|v| v[0]).collect();
    let dab: Vec<f32> = [1, 2]
        .iter()
        .flat_map(|&c| up.iter().map(move |v| v[c]))
        .collect();
    ulps("lamp_f3", "gain_up", &gain, &fx.f32("gain_up"), 2);
    ulps("lamp_f3", "dab_up", &dab, &fx.f32("dab_up"), 2);
}

/// The whole fill timed by stage on synthetic 24 and 100 MP frames, one
/// thread and all of them, with the peak RSS over the input: one spot,
/// two spots in opposite corners, 400 scattered disks and a large one,
/// and one region spanning the frame (a clipped sky over its top third).
/// Reported, not asserted:
/// `cargo test -p greycard-core --release highlight_fill::fill_tests::timing -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing() {
    let weights = random_weights();
    for (w, h) in [(6000usize, 4000usize), (11648, 8736)] {
        let (wf, hf) = (w as f32, h as f32);
        for scene in ["one spot", "two corners", "scattered", "sky"] {
            for threads in [1, rayon::current_num_threads()] {
                let (mut x, codes) = match scene {
                    "one spot" => spots(w, h, &[(wf * 0.4, hf * 0.6, 30.0)]),
                    "two corners" => {
                        spots(w, h, &[(40.0, 40.0, 30.0), (wf - 40.0, hf - 40.0, 30.0)])
                    }
                    "scattered" => synthetic(w, h, 400, 11),
                    _ => spots(w, h, &[(wf / 2.0, -10.0 * hf, 10.33 * hf)]),
                };
                let plane = CodePlane {
                    codes: &codes,
                    width: w,
                };
                // Reset the high-water mark to what the input holds.
                let _ = std::fs::write("/proc/self/clear_refs", "5");
                let base = peak_rss_mb();
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap();
                pool.install(|| {
                    let all = Instant::now();
                    let mut last = Instant::now();
                    let mut laps = Vec::new();
                    let mut lap = |what: &str| {
                        let now = Instant::now();
                        laps.push(format!("{what} {:.0}", (now - last).as_secs_f64() * 1e3));
                        last = now;
                    };
                    let a = analyze(&x, w, h, &plane, [1.0, 0.98, 0.95], HARM_BRIGHT);
                    lap("analyze");
                    let outputs = region_outputs(&a, &x, &plane, &weights).unwrap();
                    lap("network");
                    let look = Scaled::new(&LOOK, 1.0);
                    let windows = synthesis::windows(&a, 18).len();
                    let fields = synthesis::fields(&a, &outputs, look);
                    lap("synthesis");
                    let rows = a.regions.len();
                    drop(a);
                    fields.apply(&mut x, &plane);
                    lap("apply");
                    eprintln!(
                        "{w}x{h} {scene:<11} {threads:>2} threads: {:>7.0} ms ({}), {rows} rows, {windows} windows, fields {:.1} MB, peak +{:.0} MB",
                        all.elapsed().as_secs_f64() * 1e3,
                        laps.join(", "),
                        fields.bytes() as f64 / 1048576.0,
                        peak_rss_mb() - base
                    );
                });
            }
        }
    }
}
