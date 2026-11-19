//! The analysis against the oracle's fixtures, stage by stage, with
//! SPEC.md's tolerances; its independence of the thread count; and
//! its timing on large synthetic frames (ignored, reported).

use std::time::Instant;

use super::analysis::{Analysis, analyze, analyze_traced};
use super::blur::{blur, blur_normalized};
use super::fixtures::{Fixture, Inputs};
use super::half::{self, Cell, CodePlane};
use super::net::FEATURES;
use super::{HARM_ITERS, OFF, REACH_HALF, RING, SIGMA_MIX, THUMB_LONG};

/// hl-1's look's `harmonic_bright`.
pub(super) const HARM_BRIGHT: f32 = 0.25;
use super::{harmonic, regions, thumbnail};

/// SPEC's tolerance for the thumbnail: numpy's float32 log2 is up to 2
/// ulp from the correctly rounded one (log2 of 0.92012054 is
/// −0.12010523; numpy 2.5 gives −0.120105214), and ours is the
/// correctly rounded one.
const THUMB_ULPS: u32 = 2;
/// SPEC's tolerance for dd, dc, depth, and cdepth: relative to
/// max(|v|, 1), since depths reach 512.
pub(super) const DEPTH_REL: f32 = 1e-6;

/// The largest difference relative to max(|want|, 1), asserted within
/// `rel`.
pub(super) fn close_rel(scene: &str, what: &str, got: &[f32], want: &[f32], rel: f32) {
    assert_eq!(got.len(), want.len(), "{scene}/{what}: length");
    let worst = got
        .iter()
        .zip(want)
        .map(|(&g, &w)| (g - w).abs() / w.abs().max(1.0))
        .fold(0.0f32, f32::max);
    assert!(
        worst <= rel,
        "{scene}/{what}: {worst:e} relative, over {rel:e}"
    );
    eprintln!("{scene:>16} {what:<12} max {worst:.2e} relative (tol {rel:.0e})");
}

/// The largest absolute difference, asserted within `tol`; NaN-free.
pub(super) fn close(scene: &str, what: &str, got: &[f32], want: &[f32], tol: f32) -> f32 {
    assert_eq!(got.len(), want.len(), "{scene}/{what}: length");
    let mut worst = 0.0f32;
    let mut at = 0;
    for (i, (&g, &w)) in got.iter().zip(want).enumerate() {
        assert!(
            g.is_finite() && w.is_finite(),
            "{scene}/{what}[{i}]: {g} against {w}"
        );
        let e = (g - w).abs();
        if e > worst {
            worst = e;
            at = i;
        }
    }
    assert!(
        worst <= tol,
        "{scene}/{what}: {worst:e} over {tol:e} at {at} ({} against {})",
        got[at],
        want[at]
    );
    eprintln!("{scene:>16} {what:<12} max {worst:.2e} (tol {tol:.0e})");
    worst
}

pub(super) fn exact(scene: &str, what: &str, got: &[f32], want: &[f32]) {
    assert_eq!(got.len(), want.len(), "{scene}/{what}: length");
    let bad = got
        .iter()
        .zip(want)
        .position(|(g, w)| g.to_bits() != w.to_bits());
    assert!(
        bad.is_none(),
        "{scene}/{what}: differs at {} ({} against {})",
        bad.unwrap(),
        got[bad.unwrap()],
        want[bad.unwrap()]
    );
    eprintln!("{scene:>16} {what:<12} exact");
}

pub(super) fn ulps(scene: &str, what: &str, got: &[f32], want: &[f32], tol: u32) {
    assert_eq!(got.len(), want.len());
    let ord = |v: f32| {
        let b = v.to_bits() as i32;
        if b < 0 { i32::MIN - b } else { b }
    };
    let (at, worst) = got
        .iter()
        .zip(want)
        .map(|(&g, &w)| ord(g).abs_diff(ord(w)))
        .enumerate()
        .max_by_key(|p| p.1)
        .unwrap_or((0, 0));
    assert!(
        worst <= tol,
        "{scene}/{what}: {worst} ulp at {at} ({:e} against {:e})",
        got[at],
        want[at]
    );
    eprintln!("{scene:>16} {what:<12} max {worst} ulp (tol {tol})");
}

fn as_f32(v: impl IntoIterator<Item = impl Into<f64>>) -> Vec<f32> {
    v.into_iter().map(|v| v.into() as f32).collect()
}

/// A channels-first (2, h, w) plane from per-cell pairs.
pub(super) fn planar(v: &[[f32; 2]]) -> Vec<f32> {
    v.iter()
        .map(|p| p[0])
        .chain(v.iter().map(|p| p[1]))
        .collect()
}

/// The analysis, the ring owners (rows), and yh.
fn run(inp: &Inputs) -> (Analysis, Vec<u32>, Vec<f32>) {
    let codes = CodePlane {
        codes: &inp.codes,
        width: inp.width,
    };
    analyze_traced(
        &inp.x,
        inp.width,
        inp.height,
        &codes,
        inp.clips,
        HARM_BRIGHT,
    )
}

/// The synthesis's normalized depth field from a distance plane and its
/// mask (dd from d0 and fa, dc from dc0 and fc), and each row's largest
/// value clamped to at least 1: exercises the blur against `dd`, `dc`,
/// `depth`, and `cdepth`.
fn depth_field(a: &Analysis, d: &[u16], mask: &[bool]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let off: Vec<f32> = a.regions.iter().map(|r| OFF.min(0.5 * r.depth0)).collect();
    let offs: Vec<f32> = a.labels.inv.iter().map(|&r| off[r as usize]).collect();
    let v: Vec<f32> = d
        .iter()
        .zip(&offs)
        .zip(mask)
        .map(|((&d, &o), &m)| (d as f32 - o).max(0.0) * m as u32 as f32)
        .collect();
    let f = blur_normalized(&v, mask, a.half_width, a.half_height);
    let mut top = vec![0.0f32; a.regions.len()];
    for (&r, &v) in a.labels.inv.iter().zip(&f) {
        top[r as usize] = top[r as usize].max(v);
    }
    (offs, f, top.into_iter().map(|v| v.max(1.0)).collect())
}

fn mix(a: &Analysis, sigma: f64) -> Vec<f32> {
    let fc: Vec<f32> = a.cells.iter().map(|c| c.fc() as u32 as f32).collect();
    let b = blur(&fc, a.half_width, a.half_height, sigma);
    b.iter()
        .zip(&a.cells)
        .map(|(&v, c)| {
            let m = (v * c.fa() as u32 as f32).clamp(0.0, 1.0);
            if c.fc() { m.max(0.5) } else { m }
        })
        .collect()
}

#[test]
fn every_scene_matches_the_oracle() {
    let scenes = Fixture::scenes();
    assert_eq!(scenes.len(), 17, "{scenes:?}");
    for name in &scenes {
        let fx = Fixture::load(name);
        let inp = fx.inputs();
        let (a, ring, yh) = run(&inp);
        let n = a.half_width * a.half_height;
        let s = name.as_str();
        if fx.has("ids") {
            let ids = as_f32(fx.i32("ids"));
            exact(
                s,
                "ids",
                &as_f32(a.labels.ids.iter().map(|&v| v as f64)),
                &ids,
            );
            close(s, "feats", &a.features.concat(), &fx.f32("feats"), 2e-5);
            assert_eq!(a.features[0].len(), FEATURES);
            let fa: Vec<bool> = a.cells.iter().map(|c| c.fa()).collect();
            let fc: Vec<bool> = a.cells.iter().map(|c| c.fc()).collect();
            let (_, _, depth) = depth_field(&a, &a.d0, &fa);
            close_rel(s, "depth", &depth, &fx.f32("depth"), DEPTH_REL);
            let (_, _, cdepth) = depth_field(&a, &a.dc0, &fc);
            close_rel(s, "cdepth", &cdepth, &fx.f32("cdepth"), DEPTH_REL);
        } else {
            assert!(
                a.regions.is_empty() && a.features.is_empty(),
                "{s}: no region"
            );
        }
        let plane = |f: &dyn Fn(usize) -> f64| as_f32((0..n).map(f));
        if fx.has("regions") {
            exact(
                s,
                "regions",
                &plane(&|i| a.region_id(i) as f64),
                &as_f32(fx.i32("regions")),
            );
        }
        if fx.has("ring") {
            exact(
                s,
                "ring",
                &plane(&|i| match ring[i] {
                    0 => 0.0,
                    r => a.labels.ids[r as usize] as f64,
                }),
                &as_f32(fx.i32("ring")),
            );
        }
        if fx.has("d0") {
            exact(s, "d0", &as_f32(a.d0.iter().copied()), &fx.f32("d0"));
        }
        if fx.has("dcore") {
            exact(s, "dcore", &plane(&|i| a.dcore(i) as f64), &fx.f32("dcore"));
        }
        if fx.has("dc0") {
            exact(s, "dc0", &as_f32(a.dc0.iter().copied()), &fx.f32("dc0"));
        }
        if fx.has("harm") {
            let harm: Vec<[f32; 2]> = (0..n).map(|i| a.harm(i)).collect();
            close(s, "harm", &planar(&harm), &fx.f32("harm"), 2e-6);
        }
        if fx.has("thumb") {
            let codes = CodePlane {
                codes: &inp.codes,
                width: inp.width,
            };
            let t = thumbnail::thumbnail(&inp.x, inp.width, inp.height, &codes, THUMB_LONG);
            assert_eq!(fx.dims("thumb"), [t.height, t.width, 5]);
            ulps(s, "thumb", &t.data, &fx.f32("thumb"), THUMB_ULPS);
        }
        if fx.has("xh") {
            every_stage(&fx, &inp, &a, &ring, &yh);
        }
    }
}

/// `lamp_f3` stores every stage.
fn every_stage(fx: &Fixture, inp: &Inputs, a: &Analysis, ring: &[u32], yh: &[f32]) {
    let s = fx.name.as_str();
    let (wh, hh) = (a.half_width, a.half_height);
    let n = wh * hh;
    let codes = CodePlane {
        codes: &inp.codes,
        width: inp.width,
    };
    let xh = half::half_picture(&inp.x, inp.width, inp.height, &codes);
    close(s, "xh", &xh.concat(), &fx.f32("xh"), 2e-6);
    let blocks: Vec<_> = (0..n)
        .map(|i| half::half_cell(&inp.x, inp.width, inp.height, &codes, i % wh, i / wh))
        .collect();
    exact(
        s,
        "any_share",
        &as_f32(blocks.iter().map(|b| b.1 as f32 / 4.0)),
        &fx.f32("any_share"),
    );
    exact(
        s,
        "core_share",
        &as_f32(blocks.iter().map(|b| b.2 as f32 / 4.0)),
        &fx.f32("core_share"),
    );
    exact(
        s,
        "kh",
        &as_f32(a.cells.iter().map(|c| c.kh())),
        &fx.f32("kh"),
    );
    exact(
        s,
        "fa",
        &as_f32(a.cells.iter().map(|c| c.fa() as u8)),
        &fx.f32("fa"),
    );
    exact(
        s,
        "fc",
        &as_f32(a.cells.iter().map(|c| c.fc() as u8)),
        &fx.f32("fc"),
    );
    close(s, "abh", &planar(&a.abh), &fx.f32("abh"), 2e-6);
    close(s, "yh", yh, &fx.f32("yh"), 2e-6);
    let ly: Vec<f32> = yh.iter().map(|&y| half::ly(y)).collect();
    close(s, "ly", &ly, &fx.f32("ly"), 2e-6);
    let fixed = harmonic::fixed(&a.cells, yh, HARM_BRIGHT);
    exact(
        s,
        "harm_fixed",
        &as_f32(fixed.iter().map(|&f| f as u8)),
        &fx.f32("harm_fixed"),
    );
    let mut h = harmonic::start(&a.abh, &fixed, wh, hh);
    exact(
        s,
        "harm_known",
        &as_f32(h.known.iter().map(|&k| k as u8)),
        &as_f32(fx.i32("harm_known")),
    );
    close(s, "harm_val", &planar(&h.val), &fx.f32("harm_val"), 2e-6);
    close(
        s,
        "harm_start",
        &planar(&h.start),
        &fx.f32("harm_start"),
        2e-6,
    );
    harmonic::iterate(&mut h, HARM_ITERS);
    close(
        s,
        "harm_eighth",
        &planar(&h.cur),
        &fx.f32("harm_eighth"),
        2e-6,
    );
    exact(
        s,
        "inv",
        &as_f32(a.labels.inv.iter().copied()),
        &as_f32(fx.i32("inv")),
    );
    exact(
        s,
        "ring_owner",
        &as_f32(ring.iter().copied()),
        &as_f32(fx.i32("ring_owner")),
    );
    let centroid: Vec<f32> = a.regions.iter().flat_map(|r| r.centroid).collect();
    close(s, "centroid", &centroid, &fx.f32("centroid"), 2e-6);
    exact(s, "where", &a.extent, &fx.f32("where"));
    close(s, "grid", &a.grid.concat(), &fx.f32("grid"), 2e-6);
    // The blur, through the synthesis's fields that are made with it.
    let fa: Vec<bool> = a.cells.iter().map(|c| c.fa()).collect();
    let fc: Vec<bool> = a.cells.iter().map(|c| c.fc()).collect();
    let (off, dd, _) = depth_field(a, &a.d0, &fa);
    exact(s, "off", &off, &fx.f32("off"));
    close_rel(s, "dd", &dd, &fx.f32("dd"), DEPTH_REL);
    let (_, dc, _) = depth_field(a, &a.dc0, &fc);
    close_rel(s, "dc", &dc, &fx.f32("dc"), DEPTH_REL);
    close(s, "mix", &mix(a, SIGMA_MIX), &fx.f32("mix"), 5e-6);
    // f = 3: σ_g = max(6/3, 1) = 2, the hand-over's own.
    assert_eq!(inp.f, 3.0);
    close(
        s,
        "mixg",
        &mix(a, (6.0 / inp.f as f64).max(1.0)),
        &fx.f32("mixg"),
        5e-6,
    );
}

#[test]
fn a_ragged_thumbnail_matches_the_oracle() {
    let fx = Fixture::load("lamp");
    let inp = fx.inputs();
    let codes = CodePlane {
        codes: &inp.codes,
        width: inp.width,
    };
    let t = thumbnail::thumbnail(&inp.x, inp.width, inp.height, &codes, 40);
    assert_eq!(fx.dims("thumb_long40"), [t.height, t.width, 5]);
    ulps(
        "lamp",
        "thumb_long40",
        &t.data,
        &fx.f32("thumb_long40"),
        THUMB_ULPS,
    );
}

#[test]
fn clip_codes_read_through_their_cells() {
    use crate::develop::clipcodes::ClipCodes;
    let fx = Fixture::load("lamp");
    let inp = fx.inputs();
    // The lamp's codes are a cell's: every 2×2 block holds one value.
    let (w, h) = (inp.width, inp.height);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let cells: Vec<u8> = (0..cw * ch)
        .map(|i| inp.codes[(i / cw) * 2 * w + (i % cw) * 2])
        .collect();
    let cc = ClipCodes {
        width: w,
        height: h,
        cell: 2,
        cells_wide: cw,
        cells_high: ch,
        codes: cells,
        clips: inp.clips,
    };
    if (0..w * h).all(|i| cc.at(i % w, i / w) == inp.codes[i]) {
        assert_eq!(
            analyze(&inp.x, w, h, &cc, inp.clips, HARM_BRIGHT),
            run(&inp).0
        );
    } else {
        // Not cell-aligned: the same codes through a plane and through
        // the trait give the same analysis all the same.
        let plane = CodePlane {
            codes: &inp.codes,
            width: w,
        };
        assert_eq!(
            analyze(&inp.x, w, h, &plane, inp.clips, HARM_BRIGHT),
            run(&inp).0
        );
    }
}

/// A synthetic frame: a gradient, `small` clipped disks with bands of
/// one and two colors scattered over it, and one large one.
pub(super) fn synthetic(w: usize, h: usize, small: usize, seed: u64) -> (Vec<f32>, Vec<u8>) {
    let mut x = vec![0.0f32; w * h * 3];
    let mut codes = vec![0u8; w * h];
    for y in 0..h {
        for xx in 0..w {
            let p = 3 * (y * w + xx);
            let t = (xx + y) as f32 / (w + h) as f32;
            x[p] = 0.05 + 0.5 * t;
            x[p + 1] = 0.08 + 0.4 * t;
            x[p + 2] = 0.3 - 0.2 * t;
        }
    }
    let mut state = seed;
    let mut rnd = |m: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % m as u64) as usize
    };
    let mut disks: Vec<(usize, usize, usize)> = (0..small)
        .map(|_| (rnd(w), rnd(h), 2 + rnd(h / 40 + 1)))
        .collect();
    disks.push((w / 3, h / 2, h / 4));
    for (cx, cy, r) in disks {
        let warm = [1.0f32, 0.92, 0.7];
        for y in cy.saturating_sub(r + 4)..(cy + r + 5).min(h) {
            for xx in cx.saturating_sub(r + 4)..(cx + r + 5).min(w) {
                let d = ((xx as f32 - cx as f32).powi(2) + (y as f32 - cy as f32).powi(2)).sqrt();
                let p = 3 * (y * w + xx);
                let (code, v) = if d < 0.6 * r as f32 {
                    (3u8, 1.0)
                } else if d < 0.8 * r as f32 {
                    (2, 0.97)
                } else if d < r as f32 {
                    (1, 0.9)
                } else {
                    continue;
                };
                codes[y * w + xx] = codes[y * w + xx].max(code);
                for c in 0..3 {
                    x[p + c] = x[p + c].max(v * warm[c]);
                }
            }
        }
    }
    (x, codes)
}

#[test]
fn the_analysis_is_the_same_on_any_thread_count() {
    let (w, h) = (403, 251);
    let (x, codes) = synthetic(w, h, 60, 7);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let on = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| analyze_traced(&x, w, h, &plane, [1.0, 0.98, 0.95], HARM_BRIGHT))
    };
    let one = on(1);
    assert!(one.0.regions.len() > 20, "{} rows", one.0.regions.len());
    assert_eq!(one, on(3));
    assert_eq!(one, on(8));
    let t = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| thumbnail::thumbnail(&x, w, h, &plane, THUMB_LONG))
    };
    assert_eq!(t(1), t(5));
}

#[test]
fn a_frame_too_small_for_the_quarter_grid_has_no_harmonic() {
    // 6x5 px: a 3x3 half grid, no 4x4 block for the harmonic's pool
    // (the oracle's pool refuses it); the continuation reads 0.
    let (w, h) = (6, 5);
    let x = vec![0.9f32; w * h * 3];
    let codes = vec![3u8; w * h];
    let a = analyze(
        &x,
        w,
        h,
        &CodePlane {
            codes: &codes,
            width: w,
        },
        [1.0; 3],
        HARM_BRIGHT,
    );
    assert_eq!((a.harmonic.width, a.harmonic.height), (0, 0));
    assert_eq!(a.harm(0), [0.0, 0.0]);
    assert_eq!(a.regions.len(), 1);
    assert!(!a.labels.has_outside());
}

/// A hand-made analysis of a `w` × `h` half grid for the region table:
/// cells, yh, and abh given, distances 0, no harmonic.
fn hand_made(
    w: usize,
    h: usize,
    cells: Vec<Cell>,
    yh: &[f32],
    ab: [f32; 2],
) -> (Analysis, Vec<u32>) {
    let fa: Vec<bool> = cells.iter().map(|c| c.fa()).collect();
    let labels = regions::labels(&fa, w, h);
    let ring = regions::ring(&fa, &labels, w, h, RING);
    let mut a = Analysis {
        width: 2 * w,
        height: 2 * h,
        half_width: w,
        half_height: h,
        cells,
        abh: vec![ab; w * h],
        labels,
        d0: vec![0; w * h],
        dc0: vec![0; w * h],
        harmonic: harmonic::start(&[], &[], 0, 0),
        regions: Vec::new(),
        features: Vec::new(),
        extent: [0.0, 0.0, 1.0, 1.0],
        grid: Vec::new(),
    };
    a.regions = super::analysis::region_table(&a, &ring, yh);
    (a, ring)
}

/// hl-1's per-region sums are f64 with integer counts: a region of
/// 3000 × 2000 half-size cells of one value reads that value, and its
/// centroid the middle, to 1e-6. (f32 sums in raster order, the trial's,
/// read the mean ly some 2% off on a region this size.)
#[test]
fn a_large_region_reads_its_means_and_centroid() {
    let (w, h) = (3002, 2002);
    let inside = |i: usize| {
        let (x, y) = (i % w, i / w);
        x > 0 && y > 0 && x < w - 1 && y < h - 1
    };
    let cells: Vec<Cell> = (0..w * h)
        .map(|i| {
            if inside(i) {
                Cell::new(4, 0, 1)
            } else {
                Cell::new(0, 0, 0)
            }
        })
        .collect();
    let yh = vec![0.9f32; w * h];
    let (a, _) = hand_made(w, h, cells, &yh, [0.03, -0.02]);
    assert_eq!(a.regions.len(), 2);
    let r = a.regions[1];
    assert_eq!((r.area, r.cb1, r.cbn), (6_000_000, 6_000_000, 6_000_000));
    let ly = half::ly(0.9);
    assert!((r.yb1 - ly).abs() <= 1e-6, "{} against {ly}", r.yb1);
    assert!((r.an - 0.03).abs() <= 1e-6 && (r.bn + 0.02).abs() <= 1e-6);
    for c in r.centroid {
        assert!((c - 0.5).abs() <= 1e-6, "{:?}", r.centroid);
    }
    // The f32 sum the trial took, for the record of why.
    let drift = (0..6_000_000).fold(0.0f32, |s, _| s + ly) / 6e6;
    assert!((drift - ly).abs() > 1e-3, "{drift} against {ly}");
}

/// The harmonic runs exactly HARM_ITERS (300) rounds: on a frame whose
/// quarter grid is mostly unknown, far from converged, the analysis
/// matches 300 rounds of a literal reference loop, and not 299.
#[test]
fn the_harmonic_runs_three_hundred_rounds() {
    // A bright colored strip down the left, the rest clipped: 320 × 320
    // px, a 40 × 40 quarter grid known only in its first columns.
    let (w, h) = (320, 320);
    let mut x = vec![0.0f32; w * h * 3];
    let mut codes = vec![3u8; w * h];
    for y in 0..h {
        for xx in 0..24 {
            let p = 3 * (y * w + xx);
            x[p..p + 3].copy_from_slice(&[0.9, 0.5 + 0.3 * (y as f32 / h as f32), 0.2]);
            codes[y * w + xx] = 0;
        }
    }
    let a = analyze(
        &x,
        w,
        h,
        &CodePlane {
            codes: &codes,
            width: w,
        },
        [1.0; 3],
        HARM_BRIGHT,
    );
    let hm = &a.harmonic;
    assert_eq!((hm.width, hm.height), (40, 40));
    assert!(hm.known.iter().filter(|&&k| k).count() <= 3 * 40);
    let (qw, qh) = (hm.width, hm.height);
    let step = |cur: &[[f32; 2]]| -> Vec<[f32; 2]> {
        let mut next = cur.to_vec();
        for y in 0..qh {
            for x in 0..qw {
                let i = y * qw + x;
                if hm.known[i] {
                    next[i] = hm.val[i];
                    continue;
                }
                let mut s = [0.0f32; 2];
                for dy in [-1isize, 0, 1] {
                    for dx in [-1isize, 0, 1] {
                        let yy = (y as isize + dy).clamp(0, qh as isize - 1) as usize;
                        let xx = (x as isize + dx).clamp(0, qw as isize - 1) as usize;
                        s[0] += cur[yy * qw + xx][0];
                        s[1] += cur[yy * qw + xx][1];
                    }
                }
                next[i] = [s[0] / 9.0, s[1] / 9.0];
            }
        }
        next
    };
    let mut cur = hm.start.clone();
    for _ in 0..299 {
        cur = step(&cur);
    }
    let at_299 = cur.clone();
    cur = step(&cur);
    assert_eq!(cur, hm.cur, "300 rounds");
    let moved = at_299
        .iter()
        .zip(&cur)
        .map(|(p, q)| (p[1] - q[1]).abs())
        .fold(0.0f32, f32::max);
    assert!(moved > 1e-6, "round 300 moves the field by {moved}");
}

/// The harmonic judges brightness by the raw yh, the features by
/// exp2(ly) (SPEC's trial behavior 1). With correctly rounded log2 and
/// exp2 (glibc's, and torch's and numpy's on the oracle's side) the two
/// readings agree on every f32 within 2²⁰ ulp of 0.25, the only place
/// they could part, so which one each stage takes cannot move a cell
/// there; a platform whose libm parts them fails here first. Both
/// thresholds are strict.
#[test]
fn brightness_is_judged_two_ways() {
    let quarter = 0.25f32.to_bits();
    let split = (0..1u32 << 20)
        .flat_map(|d| [quarter + d, quarter - d])
        .map(f32::from_bits)
        .find(|&y| (y > HARM_BRIGHT) != (half::ly(y).exp2() > 0.25));
    assert_eq!(split, None, "the readings part at {split:?}");
    // A region, its rim cell, and a farther rim cell, at 0.25 and the
    // next f32 up.
    let above = f32::from_bits(quarter + 1);
    let cells = vec![Cell::new(4, 0, 1), Cell::new(0, 0, 0), Cell::new(0, 0, 0)];
    for (rim, bright) in [(0.25f32, false), (above, true)] {
        let yh = [0.9, rim, rim];
        assert_eq!(
            harmonic::fixed(&cells, &yh, HARM_BRIGHT),
            [false, bright, bright]
        );
        let (a, ring) = hand_made(3, 1, cells.clone(), &yh, [0.01, 0.02]);
        assert_eq!(ring, [0, 1, 1]);
        assert_eq!(a.regions[1].cbr, 2 * bright as u64);
    }
}

#[test]
#[should_panic(expected = "a 2x2 block's counts")]
fn cells_refuse_counts_no_block_has() {
    Cell::new(2, 3, 3);
}

pub(super) fn peak_rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(0.0, |kb| kb / 1024.0)
}

/// The analysis's stages timed on synthetic 24 and 100 MP frames, one
/// thread and all of them. Reported, not asserted:
/// `cargo test -p greycard-core --release highlight_fill::tests::timing -- --ignored --nocapture`.
#[test]
#[ignore]
fn timing() {
    for (w, h) in [(6000usize, 4000usize), (11648, 8736)] {
        let (x, codes) = synthetic(w, h, 400, 11);
        let plane = CodePlane {
            codes: &codes,
            width: w,
        };
        eprintln!("{w}x{h}: input built, peak RSS {:.0} MB", peak_rss_mb());
        for threads in [1, rayon::current_num_threads()] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            pool.install(|| {
                // The whole analysis first, so the high-water mark it
                // leaves is its own over the input's.
                let before = peak_rss_mb();
                let all = Instant::now();
                let a = analyze(&x, w, h, &plane, [1.0, 0.98, 0.95], HARM_BRIGHT);
                eprintln!(
                    "  {threads:>2} threads analyze    {:>8.1} ms, {} rows, peak RSS {:.0} MB (+{:.0})",
                    all.elapsed().as_secs_f64() * 1e3,
                    a.regions.len(),
                    peak_rss_mb(),
                    peak_rss_mb() - before
                );
                drop(a);
                let mut last = Instant::now();
                let mut lap = |what: &str| {
                    let now = Instant::now();
                    eprintln!("  {threads:>2} threads {what:<10} {:>8.1} ms", (now - last).as_secs_f64() * 1e3);
                    last = now;
                };
                let (wh, hh) = half::half_size(w, h);
                let (cells, abh, yh) = half::planes(&x, w, h, &plane);
                lap("half");
                let fa: Vec<bool> = cells.iter().map(|c: &Cell| c.fa()).collect();
                let labels = regions::labels(&fa, wh, hh);
                lap("labels");
                let (_d0, _dc0) = rayon::join(
                    || regions::distance(&fa, wh, hh, REACH_HALF),
                    || {
                        let fc: Vec<bool> = cells.iter().map(|c| c.fc()).collect();
                        regions::distance(&fc, wh, hh, REACH_HALF)
                    },
                );
                lap("distances");
                let _ring = regions::ring(&fa, &labels, wh, hh, RING);
                lap("ring");
                let fixed = harmonic::fixed(&cells, &yh, HARM_BRIGHT);
                let mut hm = harmonic::start(&abh, &fixed, wh, hh);
                harmonic::iterate(&mut hm, HARM_ITERS);
                lap("harmonic");
                let t = thumbnail::thumbnail(&x, w, h, &plane, THUMB_LONG);
                lap("thumbnail");
                assert!(t.width <= THUMB_LONG && t.height <= THUMB_LONG);
                let fc: Vec<f32> = cells.iter().map(|c| c.fc() as u32 as f32).collect();
                let _ = blur(&fc, wh, hh, 6.0);
                lap("blur σ6");
            });
        }
    }
}
