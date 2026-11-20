//! hl-2's reach (`net::Reach`): the distance it ramps by, its gate and
//! ramp, and where its fields may be nonzero, with random weights.

use super::fill_tests::{LOOK, REACH_LOOK};
use super::fixtures::random_weights;
use super::half::CodePlane;
use super::net::{FillLook, Reach};
use super::synthesis::{CellOut, ScaledReach, Synthesis, squared_distance};
use super::tests::{HARM_BRIGHT, synthetic};
use super::{analysis, fill_fields, region_outputs, synthesis};

/// The exact squared distance is the brute force's, on random masks,
/// a mask with one source, and one with none (infinite everywhere).
#[test]
fn the_squared_distance_is_exact() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut rnd = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for (w, h, every) in [
        (37, 23, 9),
        (64, 5, 40),
        (1, 30, 7),
        (50, 50, 400),
        (9, 9, 0),
    ] {
        let source: Vec<bool> = (0..w * h)
            .map(|_| every > 0 && rnd() % every == 0)
            .collect();
        let got = squared_distance(&source, w, h);
        for y in 0..h {
            for x in 0..w {
                let want = (0..w * h)
                    .filter(|&j| source[j])
                    .map(|j| {
                        let (dx, dy) = ((j % w) as f64 - x as f64, (j / w) as f64 - y as f64);
                        dx * dx + dy * dy
                    })
                    .fold(f64::INFINITY, f64::min);
                assert_eq!(got[y * w + x], want, "{w}x{h} at ({x}, {y})");
            }
        }
    }
}

/// The gate is whole to its start and none from its end, exactly, and
/// falls without a jump between; the ramp is 0 at the core's edge,
/// whole at the ramp's length or the core's own depth, whichever is
/// less, and 0 on a core with no depth. Both over f.
#[test]
fn the_gate_and_the_ramp_meet_their_ends() {
    let r = ScaledReach::new(
        &Reach {
            gate: [160.0, 224.0],
            ramp: Some(32.0),
            near: None,
        },
        1.0,
    );
    assert_eq!(r.gate(0.0), 1.0);
    assert_eq!(r.gate(160.0), 1.0);
    assert_eq!(r.gate(224.0), 0.0);
    assert_eq!(r.gate(512.0), 0.0);
    assert!((r.gate(192.0) - 0.5).abs() < 1e-6);
    let mut last = 1.0f32;
    for d in (160..=224).map(|d| d as f32) {
        let g = r.gate(d);
        assert!(g <= last && last - g < 0.03, "{d}: {g} after {last}");
        last = g;
    }
    assert_eq!(r.ramp(0.0, 100.0), 0.0);
    assert_eq!(r.ramp(32.0, 100.0), 1.0);
    assert_eq!(r.ramp(80.0, 100.0), 1.0);
    assert!((r.ramp(16.0, 100.0) - 0.5).abs() < 1e-6);
    assert_eq!(r.ramp(9.0, 9.0), 1.0, "a small core: whole at its center");
    assert_eq!(r.ramp(0.0, 0.0), 0.0);
    let half = ScaledReach::new(
        &Reach {
            gate: [160.0, 224.0],
            ramp: Some(32.0),
            near: None,
        },
        2.0,
    );
    assert_eq!(half.gate(112.0), 0.0);
    assert_eq!(half.ramp(16.0, 100.0), 1.0);
}

/// With a reach, a cell's fields are nonzero only on a core cell past
/// the core's first two layers, in a region short of the gate's end;
/// on those, each is hl-1's times the cell's weight, the region's gate
/// times its ramp. And every pixel whose four upsampling cells include
/// one not fully clipped keeps its bits. Random weights, at f = 1 and 2.
#[test]
fn the_fields_keep_to_the_cores_of_regions_under_the_gate() {
    let weights = random_weights();
    let (w, h) = (403, 251);
    let (x, codes) = synthetic(w, h, 40, 3);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let a = analysis::analyze(&x, w, h, &plane, [1.0; 3], HARM_BRIGHT);
    let outputs = region_outputs(&a, &x, &plane, &weights).unwrap();
    for f in [1.0, 2.0] {
        let plain = Synthesis::new(&a, &outputs, synthesis::Scaled::new(&LOOK, f));
        let look = synthesis::Scaled::new(&REACH_LOOK, f);
        let reach = look.reach.unwrap();
        let held = Synthesis::new(&a, &outputs, look);
        let (wh, hh) = (a.half_width, a.half_height);
        let (mut gated, mut full) = (0, 0);
        for i in 0..wh * hh {
            let (c, p) = (held.cell(i), plain.cell(i));
            let cell = a.cells[i];
            let r = a.labels.inv[i] as usize;
            let gate = reach.gate(a.regions[r].depth0);
            if !cell.fc() || gate == 0.0 {
                assert_eq!(c.weight, 0.0, "cell {i}");
            }
            // The first two layers: a cell not fc within one cell.
            let (cx, cy) = (i % wh, i / wh);
            let edge = (cy.saturating_sub(1)..(cy + 2).min(hh)).any(|y| {
                (cx.saturating_sub(1)..(cx + 2).min(wh)).any(|x| !a.cells[y * wh + x].fc())
            });
            if edge {
                assert_eq!(c.weight, 0.0, "cell {i} at the core's edge");
            }
            assert_eq!(c.gain, p.gain * c.weight);
            assert_eq!(c.dab, p.dab.map(|v| v * c.weight));
            if cell.fc() && !edge {
                if gate == 0.0 {
                    gated += 1;
                } else if gate == 1.0 {
                    full += 1;
                }
            }
        }
        assert!(gated > 100 && full > 0, "{gated} gated, {full} whole");

        let fields = fill_fields(&x, w, h, &plane, [1.0; 3], &weights, &REACH_LOOK, f).unwrap();
        let mut out = x.clone();
        fields.apply(&mut out, &plane);
        let mut kept = 0;
        for py in 0..h {
            for px in 0..w {
                // The upsample reads cells ⌊(p − 1)/2⌋ to ⌈p/2⌉.
                let ys = py.saturating_sub(1) / 2..=py.div_ceil(2).min(hh - 1);
                let near = ys.into_iter().any(|cy| {
                    (px.saturating_sub(1) / 2..=px.div_ceil(2).min(wh - 1))
                        .any(|cx| !a.cells[cy * wh + cx].fc())
                });
                if near {
                    let p = 3 * (py * w + px);
                    kept += (codes[py * w + px] > 0) as usize;
                    for c in 0..3 {
                        assert_eq!(out[p + c].to_bits(), x[p + c].to_bits(), "({px}, {py})");
                    }
                }
            }
        }
        assert!(kept > 1000, "{kept} clipped pixels by a band kept");
    }
}

/// A frame whose every region is past the gate's end makes no fields:
/// the network is not asked, and the picture is the classical fill's.
/// One region short of it makes them again.
#[test]
fn a_frame_past_the_gate_makes_no_fields() {
    let weights = random_weights();
    let (w, h) = (403, 251);
    let (x, codes) = synthetic(w, h, 0, 3);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let past = FillLook {
        reach: Some(Reach {
            gate: [4.0, 8.0],
            ramp: Some(12.0),
            near: None,
        }),
        ..LOOK
    };
    assert!(fill_fields(&x, w, h, &plane, [1.0; 3], &weights, &past, 1.0).is_none());
    assert!(fill_fields(&x, w, h, &plane, [1.0; 3], &weights, &LOOK, 1.0).is_some());
    let (x, codes) = synthetic(w, h, 40, 3);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    assert!(fill_fields(&x, w, h, &plane, [1.0; 3], &weights, &past, 1.0).is_some());
}

/// A reach without a ramp is the gate alone: every cell's fields are
/// hl-1's times its region's gate, band and core alike, and a region
/// short of the gate's start keeps hl-1's to the bit.
#[test]
fn the_gate_alone_scales_hl_1_by_region() {
    let weights = random_weights();
    let (w, h) = (403, 251);
    let (x, codes) = synthetic(w, h, 40, 3);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let a = analysis::analyze(&x, w, h, &plane, [1.0; 3], HARM_BRIGHT);
    let outputs = region_outputs(&a, &x, &plane, &weights).unwrap();
    let gated = FillLook {
        reach: Some(Reach {
            gate: [4.0, 44.0],
            ramp: None,
            near: None,
        }),
        ..LOOK
    };
    let look = synthesis::Scaled::new(&gated, 1.0);
    let reach = look.reach.unwrap();
    let plain = Synthesis::new(&a, &outputs, synthesis::Scaled::new(&LOOK, 1.0));
    let held = Synthesis::new(&a, &outputs, look);
    let (mut whole, mut faded, mut none) = (0, 0, 0);
    for i in 0..a.half_width * a.half_height {
        if !a.cells[i].fa() {
            continue;
        }
        let (c, p) = (held.cell(i), plain.cell(i));
        let gate = reach.gate(a.regions[a.labels.inv[i] as usize].depth0);
        assert_eq!(c.weight, gate);
        assert_eq!(c.gain, p.gain * gate);
        assert_eq!(c.dab, p.dab.map(|v| v * gate));
        if gate == 1.0 {
            assert_eq!(c, CellOut { weight: 1.0, ..p });
            whole += 1;
        } else if gate == 0.0 {
            none += 1;
        } else {
            faded += 1;
        }
    }
    assert!(whole > 0 && faded > 0 && none > 0, "{whole} {faded} {none}");
}

/// hl-2's gate, as `greycard_ai::highlight::HL_2` sets it.
const HL_2_GATE: [f64; 2] = [168.0, 224.0];

/// The largest gap between a lone region's gate read at full size and
/// read by a reduced develop (bin 2, look-scaled), across disks,
/// squares, and diamonds whose depth lies on the shoulder, at both
/// phases of the bin. A region's depth0 is quantized to 4 px at either
/// size, so the reduced develop reads it to 8 full-size px: on hl-2's
/// 56 px shoulder, whose steepest slope is 1.5 / 56 a px, that is at
/// most 0.21 of the region's share; measured 0.093. Pinned under 0.15:
/// 0.15 stop of a 1-stop lift between the window's small pictures and
/// the full-size one.
#[test]
fn the_reduced_develop_gates_as_full_size_within_a_bound() {
    use super::reduced::{down, down_codes, look_scale};
    let reach = Reach {
        gate: HL_2_GATE,
        ramp: None,
        near: None,
    };
    let mut worst = 0.0f32;
    let mut on_shoulder = 0;
    let shapes: [(&str, std::iter::StepBy<std::ops::RangeInclusive<usize>>); 3] = [
        ("disk", (230..=330).step_by(14)),
        ("square", (160..=232).step_by(10)),
        ("diamond", (160..=232).step_by(10)),
    ];
    for (shape, sizes) in shapes {
        for size in sizes {
            for phase in [0usize, 1] {
                let side = 2 * size + 64 + phase * 2;
                let (w, h) = (side, side);
                let c = side as f32 / 2.0 + phase as f32 * 0.5;
                let mut x = vec![0.2f32; w * h * 3];
                let mut codes = vec![0u8; w * h];
                for by in 0..h / 2 {
                    for bx in 0..w / 2 {
                        let (dx, dy) = ((2 * bx) as f32 + 1.0 - c, (2 * by) as f32 + 1.0 - c);
                        let d = match shape {
                            "disk" => dx.hypot(dy),
                            "square" => dx.abs().max(dy.abs()),
                            _ => (dx.abs() + dy.abs()) / std::f32::consts::SQRT_2,
                        };
                        let k = if d < size as f32 - 10.0 {
                            3
                        } else if d < size as f32 {
                            1
                        } else {
                            0
                        };
                        let rgb = [
                            1.0,
                            if k == 3 { 1.0 } else { 0.9 },
                            if k == 3 { 1.0 } else { 0.8 },
                        ];
                        for (yy, xx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                            let p = (2 * by + yy) * w + 2 * bx + xx;
                            codes[p] = k;
                            if k > 0 {
                                x[3 * p..3 * p + 3].copy_from_slice(&rgb);
                            }
                        }
                    }
                }
                let deepest = |a: &analysis::Analysis| {
                    a.regions
                        .iter()
                        .filter(|r| r.id != 0)
                        .fold(0.0f32, |m, r| m.max(r.depth0))
                };
                let plane = CodePlane {
                    codes: &codes,
                    width: w,
                };
                let full = analysis::analyze(&x, w, h, &plane, [1.0; 3], HARM_BRIGHT);
                let g1 = ScaledReach::new(&reach, 1.0).gate(deepest(&full));
                let (xr, wr, hr) = down(&x, w, h, 2);
                let cr = down_codes(&codes, w, h, 2);
                let plane = CodePlane {
                    codes: &cr,
                    width: wr,
                };
                let reduced = analysis::analyze(&xr, wr, hr, &plane, [1.0; 3], HARM_BRIGHT);
                let f = look_scale((w, h), (wr, hr));
                let g2 = ScaledReach::new(&reach, f).gate(deepest(&reduced));
                on_shoulder += (g1 > 0.0 && g1 < 1.0) as usize;
                worst = worst.max((g1 - g2).abs());
            }
        }
    }
    eprintln!("worst gate gap {worst:.3} over {on_shoulder} regions on the shoulder");
    assert!(on_shoulder >= 20, "{on_shoulder}");
    assert!(worst < 0.15, "{worst}");
}

/// Each row's shared gate is the brute force's: the smallest gate of
/// any region with an fa cell within the reach (chessboard) of one of
/// its own. Gates all 1 come back as they are.
#[test]
fn neighbors_share_the_smallest_gate_within_reach() {
    let (w, h) = (403, 251);
    let (x, codes) = synthetic(w, h, 60, 7);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let a = analysis::analyze(&x, w, h, &plane, [1.0; 3], HARM_BRIGHT);
    let gate: Vec<f32> = (0..a.regions.len())
        .map(|r| [1.0, 0.0, 0.5, 1.0, 0.25][r % 5])
        .collect();
    let (wh, hh) = (a.half_width, a.half_height);
    let cells: Vec<(i64, i64, usize)> = (0..wh * hh)
        .filter(|&i| a.cells[i].fa())
        .map(|i| ((i % wh) as i64, (i / wh) as i64, a.labels.inv[i] as usize))
        .collect();
    for near in [0usize, 3, 12] {
        let shared = synthesis::shared_gates(&a, &gate, near);
        let mut want = gate.clone();
        for &(x0, y0, r0) in &cells {
            for &(x1, y1, r1) in &cells {
                if (x0 - x1).abs().max((y0 - y1).abs()) <= near as i64 {
                    want[r0] = want[r0].min(gate[r1]);
                }
            }
        }
        assert_eq!(shared, want, "near {near}");
        if near == 12 {
            assert!(
                shared.iter().zip(&gate).any(|(s, g)| s < g),
                "some row shares"
            );
        }
    }
    let ones = vec![1.0f32; gate.len()];
    assert_eq!(synthesis::shared_gates(&a, &ones, 12), ones);
}

/// On a frame of regions open, faded, and gated, with and without the
/// neighbors' reach, the fields made window by window (a window of
/// gated regions alone skipped) are the whole grid's to the bit.
#[test]
fn skipped_windows_leave_the_whole_grid_s_fields() {
    let weights = random_weights();
    let (w, h) = (403, 251);
    let (x, codes) = synthetic(w, h, 40, 3);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let a = analysis::analyze(&x, w, h, &plane, [1.0; 3], HARM_BRIGHT);
    let outputs = region_outputs(&a, &x, &plane, &weights).unwrap();
    for near in [None, Some(24.0)] {
        let look = FillLook {
            reach: Some(Reach {
                gate: [4.0, 44.0],
                ramp: None,
                near,
            }),
            ..LOOK
        };
        let scaled = synthesis::Scaled::new(&look, 1.0);
        let reach = scaled.reach.unwrap();
        let gates: Vec<f32> = a
            .regions
            .iter()
            .filter(|r| r.id != 0)
            .map(|r| reach.gate(r.depth0))
            .collect();
        assert!(gates.contains(&0.0) && gates.contains(&1.0));
        assert!(gates.iter().any(|&g| g > 0.0 && g < 1.0));
        let whole = Synthesis::whole(&a, &outputs, scaled).fields();
        assert!(whole.tiles() > 0);
        assert_eq!(
            synthesis::fields(&a, &outputs, scaled),
            whole,
            "near {near:?}"
        );
    }
}
