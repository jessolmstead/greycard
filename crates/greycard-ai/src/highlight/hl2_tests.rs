//! hl-2's size gate with the built-in weights, on scenes made here, and
//! the core-only ramp `Reach` also offers, on a look of the tests' own
//! (notes §719 §7).

use greycard_core::develop::highlight_fill::half::CodePlane;
use greycard_core::develop::highlight_fill::net::{FillLook, Reach};
use greycard_core::develop::highlight_fill::{fill_fields, oklab};

use super::*;

/// A light: its center, the radius its core ends at, and the radius
/// its clipping ends at, px.
#[derive(Clone, Copy)]
struct Light {
    cx: f32,
    cy: f32,
    core: f32,
    edge: f32,
}

/// The warm light's color: red clips first, then green, then blue.
const WARM: [f32; 3] = [1.0, 0.92, 0.7];

/// A dim gradient with smooth lights on it: each light's luminance
/// falls linearly from 1/0.7 (blue's clip, so the core's edge) at
/// `core` to 1 (red's clip, the clipping's edge) at `edge` and on past
/// it, every channel cut at 1, the clip. A pixel's code is the count of
/// its channels at the clip. The picture without the fill is smooth
/// everywhere: what a step in the filled one is measured against.
fn scene(w: usize, h: usize, lights: &[Light]) -> (Vec<f32>, Vec<u8>) {
    let mut x = vec![0.0f32; w * h * 3];
    let mut codes = vec![0u8; w * h];
    for y in 0..h {
        for xx in 0..w {
            let t = (xx + y) as f32 / (w + h) as f32;
            let mut rgb = [0.05 + 0.2 * t, 0.06 + 0.15 * t, 0.1 - 0.05 * t];
            for l in lights {
                let d = (xx as f32 - l.cx).hypot(y as f32 - l.cy);
                let top = 1.0 / WARM[2];
                let v = top + (1.0 - top) * (d - l.core) / (l.edge - l.core);
                for c in 0..3 {
                    rgb[c] = rgb[c].max(v * WARM[c]);
                }
            }
            let p = y * w + xx;
            codes[p] = rgb.iter().filter(|&&v| v >= 1.0).count() as u8;
            for c in 0..3 {
                x[3 * p + c] = rgb[c].min(1.0);
            }
        }
    }
    (x, codes)
}

const CLIPS: [f32; 3] = [1.0, 1.0, 1.0];

/// The scene filled by `look` with the built-in weights.
fn filled(x: &[f32], codes: &[u8], w: usize, h: usize, look: &FillLook) -> Vec<f32> {
    let plane = CodePlane { codes, width: w };
    let mut out = x.to_vec();
    if let Some(fields) = fill_fields(x, w, h, &plane, CLIPS, HL_1.weights(), look, 1.0) {
        fields.apply(&mut out, &plane);
    }
    out
}

/// Each pixel's lift, stops: log2 of its luminance filled over before.
fn lift(off: &[f32], on: &[f32]) -> Vec<f32> {
    off.as_chunks::<3>()
        .0
        .iter()
        .zip(on.as_chunks::<3>().0)
        .map(|(&a, &b)| (oklab::luma(b) / oklab::luma(a)).log2())
        .collect()
}

fn same_bits(a: &[f32], b: &[f32]) -> bool {
    a.iter().zip(b).all(|(p, q)| p.to_bits() == q.to_bits())
}

/// hl-1's look with hl-2's gate and a core-only ramp of 32 px: the
/// ramp's rules, which no version uses yet.
const CORE_ONLY: FillLook = FillLook {
    reach: Some(Reach {
        gate: [168.0, 224.0],
        ramp: Some(32.0),
        near: None,
    }),
    ..super::HL_1_LOOK
};

/// hl-2's look with its gate moved past any region.
fn ungated() -> FillLook {
    let reach = HL_2.look.reach.unwrap();
    FillLook {
        reach: Some(Reach {
            gate: [4000.0, 5000.0],
            ..reach
        }),
        ..HL_2.look
    }
}

/// A region deeper than the gate's end is the picture without the fill
/// to the bit, while a small one beside it fills; hl-1 fills both. A
/// frame of such regions alone makes no fields: the network is not
/// asked. Catches a gate left out, or one that scales instead of
/// stopping at its end.
#[test]
fn a_region_past_the_gate_is_the_classical_fill_to_the_bit() {
    // A round region's depth0 is its chessboard depth, about its radius
    // over √2: this one's is about 240 px.
    let (w, h) = (900, 760);
    let big = Light {
        cx: 380.0,
        cy: 380.0,
        core: 240.0,
        edge: 340.0,
    };
    let small = Light {
        cx: 840.0,
        cy: 70.0,
        core: 24.0,
        edge: 44.0,
    };
    let (x, codes) = scene(w, h, &[big, small]);
    let on = filled(&x, &codes, w, h, &HL_2.look);
    let one = filled(&x, &codes, w, h, &HL_1.look);
    let inside =
        |l: Light, p: usize| ((p % w) as f32 - l.cx).hypot((p / w) as f32 - l.cy) < l.edge + 4.0;
    let (mut big_moved, mut big_moved_1, mut small_moved) = (0, 0, 0);
    for p in 0..w * h {
        let (a, b, c) = (
            &x[3 * p..3 * p + 3],
            &on[3 * p..3 * p + 3],
            &one[3 * p..3 * p + 3],
        );
        if inside(big, p) {
            big_moved += !same_bits(a, b) as usize;
            big_moved_1 += !same_bits(a, c) as usize;
        } else if inside(small, p) {
            small_moved += !same_bits(a, b) as usize;
        }
    }
    assert_eq!(big_moved, 0, "the gated region is the classical fill's");
    assert!(big_moved_1 > 10_000, "hl-1 fills it: {big_moved_1}");
    assert!(small_moved > 200, "the small one fills: {small_moved}");

    let (x, codes) = scene(w, h, &[big]);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    assert!(fill_fields(&x, w, h, &plane, CLIPS, HL_1.weights(), &HL_2.look, 1.0).is_none());
}

/// Under the gate, the core-only ramp changes only the core: every partly clipped
/// pixel keeps its bits, every core pixel beside one is lifted by
/// under 0.01 stop (its cells are a layer or two from the core's edge,
/// where the ramp starts from zero), and the core fills. Catches the
/// fields left on the band, and a ramp that does not start at zero.
#[test]
fn under_the_gate_only_the_core_changes() {
    let (w, h) = (300, 260);
    let l = Light {
        cx: 150.0,
        cy: 130.0,
        core: 60.0,
        edge: 100.0,
    };
    let (x, codes) = scene(w, h, &[l]);
    let on = filled(&x, &codes, w, h, &CORE_ONLY);
    let one = filled(&x, &codes, w, h, &HL_1.look);
    let lifted = lift(&x, &on);
    let beside_band = |p: usize| {
        let (px, py) = ((p % w) as i64, (p / w) as i64);
        (-1..=1).any(|dy| {
            (-1..=1).any(|dx| {
                let (qx, qy) = (px + dx, py + dy);
                (0..w as i64).contains(&qx)
                    && (0..h as i64).contains(&qy)
                    && codes[(qy * w as i64 + qx) as usize] < 3
            })
        })
    };
    let (mut core_moved, mut band_1, mut beside) = (0, 0, 0.0f32);
    for p in 0..w * h {
        let (a, b) = (&x[3 * p..3 * p + 3], &on[3 * p..3 * p + 3]);
        if codes[p] < 3 {
            assert!(same_bits(a, b), "pixel {p} (code {})", codes[p]);
            band_1 += (codes[p] > 0 && !same_bits(a, &one[3 * p..3 * p + 3])) as usize;
        } else if beside_band(p) {
            beside = beside.max(lifted[p].abs());
        } else {
            core_moved += !same_bits(a, b) as usize;
        }
    }
    eprintln!("beside the band, the core lifts {beside:.5} stop at most");
    assert!(beside < 0.01, "{beside}");
    assert!(core_moved > 5_000, "{core_moved} core pixels filled");
    assert!(band_1 > 5_000, "hl-1 fills the band: {band_1}");
}

/// The largest change of lift between two neighboring pixels, stops,
/// and where.
fn worst_step(l: &[f32], w: usize, h: usize) -> (f32, usize) {
    let mut worst = (0.0f32, 0);
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            for q in [(x + 1 < w).then(|| p + 1), (y + 1 < h).then(|| p + w)]
                .into_iter()
                .flatten()
            {
                let d = (l[p] - l[q]).abs();
                if d > worst.0 {
                    worst = (d, p);
                }
            }
        }
    }
    worst
}

/// The largest change of lift between neighbors where either is not
/// fully clipped: across the core's edge, through the band, and across
/// the clipping's outer edge.
fn edge_step(l: &[f32], codes: &[u8], w: usize, h: usize) -> f32 {
    let mut worst = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            for q in [(x + 1 < w).then(|| p + 1), (y + 1 < h).then(|| p + w)]
                .into_iter()
                .flatten()
            {
                if codes[p] < 3 || codes[q] < 3 {
                    worst = worst.max((l[p] - l[q]).abs());
                }
            }
        }
    }
    worst
}

/// On a smooth light the core-only ramp's picture is continuous. Across the
/// core's edge, through the band, and across the clipping's outer edge
/// no neighbors' lifts differ by more than 0.01 stop (the band keeps
/// its bits), where hl-1's band lift steps by more than 0.02; and
/// inside the core
/// no step exceeds the ramp's steepest, 1.5 times the peak lift over
/// the ramp's 32 px, and 0.01 for the network's own field. Three depths
/// under the gate. Catches the ramp left out (the core's edge steps by
/// the whole lift), and the band's fields left on.
#[test]
fn the_fill_is_continuous_across_both_edges() {
    for (core, edge) in [(40.0, 70.0), (90.0, 130.0), (130.0, 190.0)] {
        let side = (2.0 * edge + 40.0) as usize;
        let c = side as f32 / 2.0;
        let l = Light {
            cx: c,
            cy: c,
            core,
            edge,
        };
        let (x, codes) = scene(side, side, &[l]);
        let two = lift(&x, &filled(&x, &codes, side, side, &CORE_ONLY));
        let one = lift(&x, &filled(&x, &codes, side, side, &HL_1.look));
        let (s2, at2) = worst_step(&two, side, side);
        let (e2, e1) = (
            edge_step(&two, &codes, side, side),
            edge_step(&one, &codes, side, side),
        );
        let peak = two.iter().fold(0.0f32, |m, &v| m.max(v));
        let bound = 1.5 * peak / 32.0 + 0.01;
        eprintln!(
            "core {core} edge {edge}: core-only step {s2:.4} at ({}, {}) of {bound:.4}, peak {peak:.3}; at the edges core-only {e2}, hl-1 {e1:.4}",
            at2 % side,
            at2 / side,
        );
        assert!(peak > 0.05, "the core fills: {peak}");
        assert!(e2 < 0.01, "no step at either edge: {e2}");
        assert!(e1 > 0.02, "hl-1 steps {e1}: the scene shows a step");
        assert!(s2 <= bound, "core-only steps {s2} at {at2}, over {bound}");
    }
}

/// Under the core-only ramp a 20 px specular keeps its fill at its
/// center: the ramp spans the
/// core's own depth where that is less than the ramp's length, so its
/// deepest pixel takes hl-1's whole lift. Catches a ramp of the full
/// length on every core, which leaves a small one about a fifth.
#[test]
fn a_small_specular_still_gains() {
    let side = 80;
    let l = Light {
        cx: 40.0,
        cy: 40.0,
        core: 10.0,
        edge: 16.0,
    };
    let (x, codes) = scene(side, side, &[l]);
    let peak = |look: &FillLook| {
        lift(&x, &filled(&x, &codes, side, side, look))
            .iter()
            .fold(0.0f32, |m, &v| m.max(v))
    };
    let (one, two) = (peak(&HL_1.look), peak(&CORE_ONLY));
    eprintln!("a 20 px specular's peak lift: hl-1 {one:.4}, core-only {two:.4}");
    assert!(one > 0.1, "hl-1 lifts it: {one}");
    assert!(
        two >= 0.9 * one,
        "the ramp keeps its center: {two} of {one}"
    );
}

/// A region on the gate's shoulder takes the gate's share of its fill:
/// its peak lift is the ungated hl-2's times the smoothstep at its
/// depth, so a region a little past one depth differs a little from
/// one a little short of it, and nothing flips.
#[test]
fn a_region_on_the_shoulder_fades() {
    let reach = HL_2.look.reach.unwrap();
    let mid = (reach.gate[0] + reach.gate[1]) as f32 / 2.0;
    let mut last = f32::INFINITY;
    // A round region's depth0 is about its radius over √2.
    for depth in [mid - 18.0, mid, mid + 18.0] {
        let edge = depth * std::f32::consts::SQRT_2;
        let side = (2.0 * edge + 40.0) as usize;
        let c = side as f32 / 2.0;
        let l = Light {
            cx: c,
            cy: c,
            core: edge - 50.0,
            edge,
        };
        let (x, codes) = scene(side, side, &[l]);
        let peak = |look: &FillLook| {
            lift(&x, &filled(&x, &codes, side, side, look))
                .iter()
                .fold(0.0f32, |m, &v| m.max(v))
        };
        let (gated, whole) = (peak(&HL_2.look), peak(&ungated()));
        let share = gated / whole;
        eprintln!("depth {depth}: peak {gated:.4} of {whole:.4}, {share:.3}");
        assert!(whole > 0.05);
        assert!(share > 0.02 && share < 0.98, "on the shoulder: {share}");
        assert!(share < last, "deeper fades more");
        last = share;
    }
}

/// Under the gate hl-2 is hl-1 to the bit: the same fields and the same
/// picture, band and core, on lights whose regions are all shallower
/// than the gate's start. Catches a ramp or a gate share left on a
/// region the gate keeps whole.
#[test]
fn under_the_gate_hl_2_is_hl_1() {
    let (w, h) = (420, 300);
    let lights = [
        Light {
            cx: 120.0,
            cy: 150.0,
            core: 60.0,
            edge: 100.0,
        },
        Light {
            cx: 330.0,
            cy: 90.0,
            core: 10.0,
            edge: 16.0,
        },
        Light {
            cx: 320.0,
            cy: 230.0,
            core: 25.0,
            edge: 45.0,
        },
    ];
    let (x, codes) = scene(w, h, &lights);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let fields = |look: &FillLook| {
        fill_fields(&x, w, h, &plane, CLIPS, HL_1.weights(), look, 1.0).expect("fields")
    };
    assert_eq!(fields(&HL_2.look), fields(&HL_1.look));
    let (two, one) = (
        filled(&x, &codes, w, h, &HL_2.look),
        filled(&x, &codes, w, h, &HL_1.look),
    );
    assert!(same_bits(&two, &one));
    assert!(!same_bits(&two, &x), "it fills");
}

/// A small region within hl-2's neighbors' reach of a gated one takes
/// its gate and is the classical fill's to the bit, as the pieces of
/// one blown sky broken by branches; the same region farther off keeps
/// hl-1's fill to the bit. hl-1 fills both. Catches the reach left out,
/// and a reach that gates the whole frame.
#[test]
fn a_fragment_beside_a_gated_region_shares_its_gate() {
    let near = HL_2.look.reach.unwrap().near.expect("hl-2 shares gates");
    let (w, h) = (900, 760);
    let big = Light {
        cx: 380.0,
        cy: 380.0,
        core: 240.0,
        edge: 340.0,
    };
    // Its clipping's edge 40 px from the big one's, well within reach.
    let gap = 40.0;
    assert!(gap < near as f32);
    let beside = Light {
        cx: 380.0 + 340.0 + gap + 20.0,
        cy: 380.0,
        core: 10.0,
        edge: 20.0,
    };
    // Farther than the reach from both: its edge over 150 px away.
    let apart = Light {
        cx: 860.0,
        cy: 40.0,
        core: 10.0,
        edge: 20.0,
    };
    let (x, codes) = scene(w, h, &[big, beside, apart]);
    let two = filled(&x, &codes, w, h, &HL_2.look);
    let one = filled(&x, &codes, w, h, &HL_1.look);
    let alone = {
        let reach = HL_2.look.reach.unwrap();
        let look = FillLook {
            reach: Some(Reach {
                near: None,
                ..reach
            }),
            ..HL_2.look
        };
        filled(&x, &codes, w, h, &look)
    };
    let inside =
        |l: Light, p: usize| ((p % w) as f32 - l.cx).hypot((p / w) as f32 - l.cy) < l.edge + 3.0;
    let (mut beside_1, mut beside_alone, mut apart_1) = (0, 0, 0);
    for p in 0..w * h {
        let px = |v: &[f32]| [v[3 * p], v[3 * p + 1], v[3 * p + 2]];
        let (o, b, c, d) = (px(&x), px(&two), px(&one), px(&alone));
        if inside(beside, p) {
            assert!(same_bits(&o, &b), "beside: pixel {p} keeps its bits");
            beside_1 += !same_bits(&o, &c) as usize;
            beside_alone += !same_bits(&o, &d) as usize;
        } else if inside(apart, p) {
            assert!(same_bits(&b, &c), "apart: pixel {p} is hl-1's");
            apart_1 += !same_bits(&o, &c) as usize;
        }
    }
    assert!(beside_1 > 100, "hl-1 fills the piece beside: {beside_1}");
    assert!(
        beside_alone > 100,
        "its own gate alone fills it: {beside_alone}"
    );
    assert!(apart_1 > 100, "hl-1 fills the piece apart: {apart_1}");
}
