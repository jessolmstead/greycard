//! The camera-space fill: its input is the matrix's to the bit, it
//! equals the fill after the matrix to rounding, it writes only what it
//! changes and says so, and a binned picture's codes are its cells.

use std::collections::BTreeSet;

use super::super::fill_tests::LOOK;
use super::super::fixtures::{Fixture, random_weights};
use super::super::half::{CodePlane, PixelCodes, PixelSource};
use super::super::tests::synthetic;
use super::super::{Fields, down_codes, fill, fill_fields, look_scale};
use super::*;
use crate::develop::clipcodes::clip_codes;
use crate::develop::reduced::bin_mosaic;
use crate::develop::tests::{CANON_LEVELS, lit_frame, ramp_light};
use crate::develop::{DevelopSettings, apply_matrix, demosaic_prepared, finish, prepare};
use crate::raw::CfaPattern;

/// A warm camera's matrix (rows, `color.rs`'s test matrix), negative
/// entries off the diagonal, by columns.
fn warm() -> [[f32; 3]; 3] {
    let rows = [[1.7f32, -0.5, -0.2], [-0.2, 1.4, -0.2], [0.0, -0.4, 1.4]];
    std::array::from_fn(|c| std::array::from_fn(|r| rows[r][c]))
}

/// The cameras every equivalence is tried with: identity at a unit
/// scale, and the warm matrix at a ceiling of 2.1 (the largest gain)
/// with no headroom and with Canon's 1.123.
fn cameras() -> Vec<Camera> {
    let id = std::array::from_fn(|c| std::array::from_fn(|r| (r == c) as u8 as f32));
    vec![
        Camera {
            matrix: id,
            ceiling: 1.0,
            headroom: 1.0,
        },
        Camera {
            matrix: warm(),
            ceiling: 2.1,
            headroom: 1.0,
        },
        Camera {
            matrix: warm(),
            ceiling: 2.1,
            headroom: 1.123,
        },
    ]
}

/// The camera picture whose fill input is about `x`: `M⁻¹·(x·scale)`
/// in f64, rounded.
fn camera_picture(x: &[f32], camera: &Camera) -> Vec<f32> {
    let inv = camera.inverse().unwrap();
    let s = camera.scale() as f64;
    x.as_chunks::<3>()
        .0
        .iter()
        .flat_map(|p| {
            let y = p.map(|v| v as f64 * s);
            (0..3).map(move |c| {
                (inv[0][c] as f64 * y[0] + inv[1][c] as f64 * y[1] + inv[2][c] as f64 * y[2]) as f32
            })
        })
        .collect()
}

/// The fill's input made whole: `finish`'s `apply_matrix`, then the
/// division.
fn materialized(rgb: &[f32], camera: &Camera) -> Vec<f32> {
    let mut x = rgb.to_vec();
    apply_matrix(&mut x, camera.matrix);
    let s = camera.scale();
    x.iter_mut().for_each(|v| *v /= s);
    x
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|v| v.to_bits()).collect()
}

/// The pixels whose bits differ.
fn differing(a: &[f32], b: &[f32]) -> BTreeSet<usize> {
    a.as_chunks::<3>()
        .0
        .iter()
        .zip(b.as_chunks::<3>().0)
        .enumerate()
        .filter(|(_, (p, q))| p.map(f32::to_bits) != q.map(f32::to_bits))
        .map(|(i, _)| i)
        .collect()
}

fn changed_set(changes: &Changes, width: usize) -> BTreeSet<usize> {
    changes.pixels().map(|(x, y)| y * width + x).collect()
}

/// The largest difference relative to max(|v|, 1).
fn worst(got: &[f32], want: &[f32]) -> f32 {
    got.iter()
        .zip(want)
        .map(|(g, w)| (g - w).abs() / w.abs().max(1.0))
        .fold(0.0, f32::max)
}

/// The synthetic scene: a gradient, scattered lamps, and one large
/// disk, with a code a pixel.
fn scene(w: usize, h: usize) -> (Vec<f32>, Vec<u8>) {
    synthetic(w, h, 12, 7)
}

#[test]
fn the_source_reads_as_the_matrix_then_a_division() {
    let (x, _) = scene(37, 23);
    for camera in cameras() {
        let mut rgb = camera_picture(&x, &camera);
        rgb[3] = f32::NAN;
        rgb[7] = -0.25;
        rgb[11] = f32::INFINITY;
        let want = materialized(&rgb, &camera);
        let source = Through::new(&rgb, 37, 23, &camera);
        assert_eq!(source.size(), (37, 23));
        let got: Vec<f32> = (0..23)
            .flat_map(|y| (0..37).map(move |x| (x, y)))
            .flat_map(|(px, py)| source.rgb(px, py))
            .collect();
        assert_eq!(bits(&got), bits(&want));
    }
}

/// The fields made through the matrix as the analysis reads are the
/// fields of the input made whole first, to the bit.
#[test]
fn the_fields_are_those_of_the_input_made_whole() {
    let weights = random_weights();
    let (w, h) = (160, 112);
    let (x, codes) = scene(w, h);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let clips = [0.99, 0.95, 0.9];
    for camera in cameras() {
        let rgb = camera_picture(&x, &camera);
        let whole = materialized(&rgb, &camera);
        let want = fill_fields(&whole, w, h, &plane, clips, &weights, &LOOK, 1.0).unwrap();
        let got = camera_fields(
            &rgb,
            w,
            h,
            &camera,
            Some(&plane),
            clips,
            &weights,
            &LOOK,
            1.0,
        )
        .unwrap();
        assert_eq!(*got.fields(), want);
    }
}

/// Filled in camera space and then taken through the matrix, a picture
/// is the one filled after the matrix: bit for bit wherever the
/// working-space fill leaves a pixel, and within float rounding (the
/// matrix meeting its inverse) where it fills: 3.2e-7 at most relative
/// to max(|v|, 1) when measured, held under 1e-6.
#[test]
fn filling_in_camera_space_is_filling_after_the_matrix() {
    let weights = random_weights();
    let (w, h) = (160, 112);
    let (x, codes) = scene(w, h);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let clips = [0.99, 0.95, 0.9];
    for camera in cameras() {
        let rgb = camera_picture(&x, &camera);
        let fill_ = camera_fields(
            &rgb,
            w,
            h,
            &camera,
            Some(&plane),
            clips,
            &weights,
            &LOOK,
            1.0,
        )
        .unwrap();
        let mut filled = rgb.clone();
        let changes = fill_.apply(&mut filled, &plane);
        assert!(changes.count() > 1000, "{}", changes.count());
        let got = materialized(&filled, &camera);

        let before = materialized(&rgb, &camera);
        let mut want = before.clone();
        assert!(fill(&mut want, w, h, &plane, clips, &weights, &LOOK, 1.0));

        let worked = differing(&before, &want);
        assert_eq!(worked, changed_set(&changes, w), "the same pixels change");
        let e = worst(&got, &want);
        eprintln!(
            "camera {:?}: {} pixels filled, worst {e:e}",
            camera.headroom,
            changes.count()
        );
        assert!(e < 1e-6, "{e}");
        for p in 0..w * h {
            if !worked.contains(&p) {
                assert_eq!(
                    bits(&rgb[3 * p..3 * p + 3]),
                    bits(&filled[3 * p..3 * p + 3])
                );
                assert_eq!(bits(&got[3 * p..3 * p + 3]), bits(&want[3 * p..3 * p + 3]));
            }
        }
    }
}

/// A frame prepared with its codes, demosaiced, filled in camera space,
/// and finished is the frame finished and then filled, to rounding: the
/// real matrix and `finish` itself, with and without headroom.
#[test]
fn a_develop_filled_before_the_finish_is_one_filled_after() {
    let weights = random_weights();
    let settings = DevelopSettings {
        hot_pixels: None,
        clip_codes: true,
        ..Default::default()
    };
    let (black, white) = CANON_LEVELS[0];
    let (w, h) = (121, 83);
    let ramp = ramp_light(w);
    let light = |x, y, c| ramp(x, y, 1) * [1.0, 1.1, 0.9][c];
    let mut headrooms = Vec::new();
    for saturated in [false, true] {
        let frame = lit_frame(w, h, black, white, saturated, light);
        let prepared = prepare(&frame, &settings, false).unwrap();
        headrooms.push(prepared.headroom);
        let camera = Camera::of(&prepared);
        let codes = FrameCodes::new(prepared.clip_codes.as_ref().unwrap(), 1).unwrap();
        assert_eq!(codes.look_scale(), 1.0);
        let (rgb, _, _) = demosaic_prepared(&prepared, &settings).unwrap();
        let fill_ = camera_fields(
            &rgb,
            w,
            h,
            &camera,
            Some(&codes),
            codes.clips(),
            &weights,
            &LOOK,
            1.0,
        )
        .unwrap();
        let mut filled = rgb.clone();
        let changes = fill_.apply(&mut filled, &codes);
        assert!(!changes.is_empty());
        // The fill's unit, from the prepared frame and not from the
        // camera, so a wrong scale shows.
        let s = prepared.ceiling * prepared.headroom;
        assert_eq!(camera.scale(), s);
        let finished = |rgb: Vec<f32>| {
            finish(prepared.clone(), rgb, None, None, &settings)
                .unwrap()
                .image
                .data
        };
        let got: Vec<f32> = finished(filled).iter().map(|v| v / s).collect();
        let mut want: Vec<f32> = finished(rgb).iter().map(|v| v / s).collect();
        let before = want.clone();
        assert!(fill(
            &mut want,
            w,
            h,
            &codes,
            codes.clips(),
            &weights,
            &LOOK,
            1.0
        ));
        assert_eq!(differing(&before, &want), changed_set(&changes, w));
        let e = worst(&got, &want);
        eprintln!(
            "develop, headroom {}: {} filled, worst {e:e}",
            prepared.headroom,
            changes.count()
        );
        assert!(e < 1e-6, "{e}");
        let untouched = differing(&before, &got);
        assert_eq!(untouched, changed_set(&changes, w));
    }
    assert!(headrooms[0] == 1.0 && headrooms[1] > 1.1, "{headrooms:?}");
}

/// The oracle's scenes, their x taken back through `M⁻¹` to a camera
/// picture and filled there, are the oracle's filled pictures: within
/// the round trip's own error, which the unreached pixels measure (at
/// most 1.8e-7 when measured): the oracle's 5e-6, though the input
/// itself moved (2.2e-6 at most when measured).
/// Not `nonfinite`: one channel's NaN or infinity spreads through `M⁻¹`
/// to all three, another input than the oracle's (its pixels pass
/// through: `non_finite_and_negative_pixels_pass_through`).
#[test]
fn the_oracle_scenes_fill_through_the_camera() {
    let weights = random_weights();
    let camera = cameras()[2];
    for name in Fixture::scenes().into_iter().filter(|n| n != "nonfinite") {
        let fx = Fixture::load(&name);
        let inp = fx.inputs();
        let (w, h) = (inp.width, inp.height);
        let plane = CodePlane {
            codes: &inp.codes,
            width: w,
        };
        let rgb = camera_picture(&inp.x, &camera);
        let got = camera_fields(
            &rgb,
            w,
            h,
            &camera,
            Some(&plane),
            inp.clips,
            &weights,
            &LOOK,
            inp.f as f64,
        );
        let Some(fill_) = got else {
            assert!(!fx.has("ids"), "{name}: no region");
            continue;
        };
        let mut filled = rgb.clone();
        fill_.apply(&mut filled, &plane);
        let out = materialized(&filled, &camera);
        let want = fx.f32("out_clipped");
        let (mut trip, mut err) = (0.0f32, 0.0f32);
        let mut k = 0;
        for p in 0..w * h {
            let (o, x) = (&out[3 * p..3 * p + 3], &inp.x[3 * p..3 * p + 3]);
            if inp.codes[p] & 0x7F == 0 {
                trip = trip.max(worst(o, x));
                continue;
            }
            let wv = &want[3 * k..3 * k + 3];
            k += 1;
            if wv.iter().all(|v| v.is_finite()) {
                err = err.max(worst(o, wv));
            } else {
                assert_eq!(
                    bits(&rgb[3 * p..3 * p + 3]),
                    bits(&filled[3 * p..3 * p + 3])
                );
            }
        }
        eprintln!("{name}: round trip {trip:e}, filled {err:e}");
        assert!(trip < 5e-7, "{name}: {trip}");
        assert!(err < 5e-6, "{name}: {err}");
    }
}

/// The pair: one fill made from one picture changes another of its
/// size only where the fields reach and its own guards let them, and
/// says which pixels.
#[test]
fn one_fill_applies_to_both_halves_of_a_pair() {
    let weights = random_weights();
    let (w, h) = (160, 112);
    let (x, codes) = scene(w, h);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let camera = cameras()[1];
    let plain = camera_picture(&x, &camera);
    // The network half: the same light a little darker, with grain.
    let network: Vec<f32> = plain
        .iter()
        .enumerate()
        .map(|(i, v)| v * 0.97 + 0.002 * ((i * 7919) % 13) as f32)
        .collect();
    let fill_ = camera_fields(
        &plain,
        w,
        h,
        &camera,
        Some(&plane),
        [1.0; 3],
        &weights,
        &LOOK,
        1.0,
    )
    .unwrap();
    for half in [&plain, &network] {
        let mut out = half.clone();
        let changes = fill_.apply(&mut out, &plane);
        assert_eq!(differing(half, &out), changed_set(&changes, w));
        // Each the working-space apply of the same fields, to rounding.
        let mut want = materialized(half, &camera);
        fill_.fields().apply(&mut want, &plane);
        assert!(worst(&materialized(&out, &camera), &want) < 1e-6);
        for (px, py) in changes.pixels() {
            assert!(codes[py * w + px] > 0);
            assert!(fill_.fields().upsampled(px, py).is_some());
        }
    }
}

/// The changes are the pixels whose bits changed, no more and no less,
/// as maximal runs by row and column.
#[test]
fn the_changes_are_exactly_the_pixels_whose_bits_changed() {
    let weights = random_weights();
    let (w, h) = (96, 80);
    let lamp = Fixture::load("lamp").inputs();
    assert_eq!((lamp.width, lamp.height), (w, h));
    let plane = CodePlane {
        codes: &lamp.codes,
        width: w,
    };
    let camera = cameras()[2];
    let rgb = camera_picture(&lamp.x, &camera);
    let fill_ = camera_fields(
        &rgb,
        w,
        h,
        &camera,
        Some(&plane),
        lamp.clips,
        &weights,
        &LOOK,
        1.0,
    )
    .unwrap();
    let mut out = rgb.clone();
    let changes = fill_.apply(&mut out, &plane);
    assert_eq!(differing(&rgb, &out), changed_set(&changes, w));
    let changed = differing(&rgb, &out);
    assert_eq!(changes.count(), changed.len());
    for p in 0..w * h {
        assert_eq!(changes.contains(p % w, p / w), changed.contains(&p));
    }
    let order: Vec<(usize, usize)> = changes.pixels().map(|(x, y)| (y, x)).collect();
    assert!(order.windows(2).all(|p| p[0] < p[1]));
    // A cell of any grid is marked exactly when it holds a changed
    // pixel, cells crossing word edges and the picture's edge included.
    for side in [1, 5, 7, 13, 64, 70, 200] {
        for cy in (0..h).step_by(side) {
            for cx in (0..w).step_by(side) {
                let any = (cy..cy + side)
                    .flat_map(|y| (cx..cx + side).map(move |x| (x, y)))
                    .any(|(x, y)| x < w && y < h && changed.contains(&(y * w + x)));
                assert_eq!(
                    changes.any_in(cx..cx + side, cy..cy + side),
                    any,
                    "cell {cx},{cy} of {side}"
                );
            }
        }
    }
    assert!(!changes.any_in(5..5, 0..h));
    // Applied again, the fill fills the filled picture: it does not
    // undo itself, and still says only what it changed.
    let mut again = out.clone();
    let second = fill_.apply(&mut again, &plane);
    assert_eq!(differing(&out, &again), changed_set(&second, w));
}

/// No codes, nothing clipped, an empty picture, or a matrix with no
/// inverse: None, and nothing to apply.
#[test]
fn what_cannot_fill_is_none() {
    let weights = random_weights();
    let (w, h) = (64, 48);
    let (x, codes) = scene(w, h);
    let camera = cameras()[1];
    let rgb = camera_picture(&x, &camera);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let call = |rgb: &[f32], w, h, camera: &Camera, codes: Option<&CodePlane>| {
        camera_fields(rgb, w, h, camera, codes, [1.0; 3], &weights, &LOOK, 1.0)
    };
    assert!(call(&rgb, w, h, &camera, None).is_none());
    let none = vec![0u8; w * h];
    let unclipped = CodePlane {
        codes: &none,
        width: w,
    };
    assert!(call(&rgb, w, h, &camera, Some(&unclipped)).is_none());
    let empty = CodePlane {
        codes: &[],
        width: 0,
    };
    assert!(call(&[], 0, 0, &camera, Some(&empty)).is_none());
    // An empty picture is None whatever codes come with it.
    assert!(call(&[], 0, 0, &camera, Some(&plane)).is_none());
    let flat = Camera {
        matrix: [[1.0, 1.0, 1.0]; 3],
        ..camera
    };
    assert!(call(&rgb, w, h, &flat, Some(&plane)).is_none());
    let unscaled = Camera {
        ceiling: 0.0,
        ..camera
    };
    assert!(call(&rgb, w, h, &unscaled, Some(&plane)).is_none());
    assert!(call(&rgb, w, h, &camera, Some(&plane)).is_some());
}

/// Non-finite pixels keep their bits, as does one of negative
/// luminance; a reached pixel with one negative channel is filled as
/// the working-space fill fills it.
#[test]
fn non_finite_and_negative_pixels_pass_through() {
    let weights = random_weights();
    let inp = Fixture::load("nonfinite").inputs();
    let (w, h) = (inp.width, inp.height);
    let plane = CodePlane {
        codes: &inp.codes,
        width: w,
    };
    let camera = cameras()[2];
    let mut rgb = camera_picture(&inp.x, &camera);
    let coded: Vec<usize> = (0..w * h)
        .filter(|&p| inp.codes[p] > 0 && rgb[3 * p..3 * p + 3].iter().all(|v| v.is_finite()))
        .collect();
    // One coded pixel dark and negative.
    let dark = coded[coded.len() / 2];
    rgb[3 * dark..3 * dark + 3].copy_from_slice(&[-0.5, -0.4, -0.6]);
    let fill_ = camera_fields(
        &rgb,
        w,
        h,
        &camera,
        Some(&plane),
        inp.clips,
        &weights,
        &LOOK,
        1.0,
    )
    .unwrap();
    let mut out = rgb.clone();
    let changes = fill_.apply(&mut out, &plane);
    assert!(!changes.is_empty());
    let mut odd = 0;
    for p in 0..w * h {
        let px = &rgb[3 * p..3 * p + 3];
        if px.iter().any(|v| !v.is_finite()) || p == dark {
            odd += 1;
            assert_eq!(bits(px), bits(&out[3 * p..3 * p + 3]), "pixel {p}");
        }
    }
    assert!(odd > 2);
    let mut want = materialized(&rgb, &camera);
    fill_.fields().apply(&mut want, &plane);
    let got = materialized(&out, &camera);
    for p in 0..w * h {
        let (g, wv) = (&got[3 * p..3 * p + 3], &want[3 * p..3 * p + 3]);
        if wv.iter().all(|v| v.is_finite()) {
            assert!(worst(g, wv) < 1e-6, "pixel {p}");
        } else {
            assert_eq!(bits(g), bits(wv));
        }
    }
}

#[test]
fn the_camera_fill_is_the_same_on_any_thread_count() {
    let weights = random_weights();
    let (w, h) = (160, 112);
    let (x, codes) = scene(w, h);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let camera = cameras()[2];
    let rgb = camera_picture(&x, &camera);
    let run = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                let fill_ = camera_fields(
                    &rgb,
                    w,
                    h,
                    &camera,
                    Some(&plane),
                    [1.0; 3],
                    &weights,
                    &LOOK,
                    1.0,
                )
                .unwrap();
                let mut out = rgb.clone();
                let changes = fill_.apply(&mut out, &plane);
                (fill_, bits(&out), changes)
            })
    };
    let one = run(1);
    for threads in [2, 3, 8] {
        assert_eq!(run(threads), one, "{threads} threads");
    }
}

/// A clipped Bayer mosaic of `w` × `h` at `phase`: a gradient under the
/// white, lamps on a grid and one large disk over it, gains applied.
fn mosaic(w: usize, h: usize, pattern: &CfaPattern, gains: [f32; 3]) -> Vec<f32> {
    let (cx, cy, r) = (w as f32 * 0.4, h as f32 * 0.55, h as f32 * 0.22);
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let c = pattern.color_at(y, x).rgb_index().unwrap();
            let (fx, fy) = (x as f32, y as f32);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let lamp = (x % 23 < 3) && (y % 19 < 3);
            let light = if d < r || lamp {
                1.6 - d / r * 0.4
            } else {
                0.1 + 0.6 * fx / w as f32 + 0.05 * ((x * 5 + y * 3) % 7) as f32
            };
            light * [0.8, 1.0, 0.7][c] * gains[c]
        })
        .collect()
}

/// A code a pixel, read through each pixel's cell.
fn per_pixel(codes: &ClipCodes) -> Vec<u8> {
    (0..codes.height)
        .flat_map(|y| (0..codes.width).map(move |x| (x, y)))
        .map(|(x, y)| codes.at(x, y))
        .collect()
}

/// A Bayer picture binned by 2 is one cell a pixel, in every phase and
/// at odd sizes: its codes are the cells', which are `down_codes` of
/// the codes a pixel at n = 2, and its pixels are the cells' bins.
#[test]
fn a_binned_pixel_is_one_cell() {
    let gains = [2.1, 1.0, 1.6];
    for (w, h) in [(40, 30), (41, 29), (37, 52), (2, 3)] {
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let pattern = CfaPattern::rggb().shifted(dx, dy);
            let samples = mosaic(w, h, &pattern, gains);
            let codes = clip_codes(&samples, w, h, &pattern, gains, 1.0);
            let view = BinnedCodes::new(&codes, 2).unwrap();
            let (_, bw, bh) = bin_mosaic(&samples, w, h, &pattern, 2);
            assert_eq!(view.size(), (bw, bh));
            let pooled = down_codes(&per_pixel(&codes), w, h, 2);
            let read: Vec<u8> = (0..bh)
                .flat_map(|y| (0..bw).map(move |x| (x, y)))
                .map(|(x, y)| view.code(x, y))
                .collect();
            assert_eq!(read, pooled, "{w}x{h} phase {dx},{dy}");
            let frame = FrameCodes::new(&codes, 2).unwrap();
            assert_eq!(frame.size(), (bw, bh));
            assert_eq!(frame.look_scale(), look_scale((w, h), (bw, bh)));
            // A cell clipped alone bins into its pixel alone: the bin
            // and the cells are laid from one origin.
            let mut one = vec![0.1f32; w * h];
            let (cx, cy) = (bw - 1, bh - 1);
            for y in 2 * cy..2 * cy + 2 {
                for x in 2 * cx..2 * cx + 2 {
                    one[y * w + x] = 5.0;
                }
            }
            let c1 = clip_codes(&one, w, h, &pattern, gains, 1.0);
            let (b1, _, _) = bin_mosaic(&one, w, h, &pattern, 2);
            let v1 = BinnedCodes::new(&c1, 2).unwrap();
            for y in 0..bh {
                for x in 0..bw {
                    let hit = (x, y) == (cx, cy);
                    assert_eq!(v1.code(x, y), if hit { 3 } else { 0 });
                    assert_eq!(b1[3 * (y * bw + x)] == 5.0, hit);
                }
            }
        }
    }
}

/// The bins a picture's codes can be read at: 1 (the mosaic's), and
/// the cell; not one that splits or merges cells.
#[test]
fn codes_are_offered_only_at_the_cell() {
    let gains = [2.0, 1.0, 1.5];
    let bayer = clip_codes(&[0.0; 30], 6, 5, &CfaPattern::rggb(), gains, 1.0);
    assert!(matches!(
        FrameCodes::new(&bayer, 1),
        Some(FrameCodes::Full(_))
    ));
    assert!(matches!(
        FrameCodes::new(&bayer, 2),
        Some(FrameCodes::Binned(_))
    ));
    for bin in [0, 3, 4, 6] {
        assert!(FrameCodes::new(&bayer, bin).is_none(), "bin {bin}");
    }
    assert_eq!(FrameCodes::new(&bayer, 2).unwrap().size(), (3, 2));
    assert_eq!(FrameCodes::new(&bayer, 1).unwrap().look_scale(), 1.0);
    let xtrans = clip_codes(&[0.0; 42], 7, 6, &CfaPattern::xtrans(), gains, 1.0);
    assert!(FrameCodes::new(&xtrans, 2).is_none());
    assert_eq!(FrameCodes::new(&xtrans, 3).unwrap().size(), (2, 2));
    // Too small to bin into a pixel.
    let thin = clip_codes(&[0.0; 3], 1, 3, &CfaPattern::rggb(), gains, 1.0);
    assert!(FrameCodes::new(&thin, 2).is_none());
}

#[test]
fn the_look_scale_is_the_long_edges_ratio() {
    assert_eq!(look_scale((6000, 4000), (3000, 2000)), 2.0);
    assert_eq!(look_scale((4000, 6000), (2000, 3000)), 2.0);
    assert_eq!(look_scale((4001, 3001), (2000, 1500)), 4001.0 / 2000.0);
    assert_eq!(look_scale((97, 63), (97, 63)), 1.0);
}

/// A binned picture fills at the helper's f as the oracle defines a
/// reduced input's fill: `fill_fields` on the same picture with the
/// pooled codes as a plane, to the bit.
#[test]
fn a_binned_picture_fills_as_the_oracle_s_reduced_input() {
    let weights = random_weights();
    let gains = [2.1, 1.0, 1.6];
    let camera = Camera {
        matrix: warm(),
        ceiling: 2.1,
        headroom: 1.0,
    };
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let (w, h) = (331, 223);
        let pattern = CfaPattern::rggb().shifted(dx, dy);
        let samples = mosaic(w, h, &pattern, gains);
        let codes = clip_codes(&samples, w, h, &pattern, gains, 1.0);
        let (rgb, bw, bh) = bin_mosaic(&samples, w, h, &pattern, 2);
        let frame = FrameCodes::new(&codes, 2).unwrap();
        let f = frame.look_scale();
        assert_eq!(f, 331.0 / 165.0);
        let got = camera_fields(
            &rgb,
            bw,
            bh,
            &camera,
            Some(&frame),
            frame.clips(),
            &weights,
            &LOOK,
            f,
        )
        .unwrap();
        let pooled = down_codes(&per_pixel(&codes), w, h, 2);
        let plane = CodePlane {
            codes: &pooled,
            width: bw,
        };
        let x = materialized(&rgb, &camera);
        let want: Fields =
            fill_fields(&x, bw, bh, &plane, codes.clips, &weights, &LOOK, f).unwrap();
        assert_eq!(*got.fields(), want, "phase {dx},{dy}");
        let (mut a, mut b) = (rgb.clone(), rgb.clone());
        assert_eq!(got.apply(&mut a, &frame), got.apply(&mut b, &plane));
        assert_eq!(bits(&a), bits(&b));
    }
}

/// Codes of X-Trans's 3×3 cells run, full size and binned by 3; the
/// decision not to offer them is the engine's.
#[test]
fn x_trans_codes_run() {
    let weights = random_weights();
    let gains = [2.0, 1.0, 1.5];
    let (w, h) = (150, 102);
    let pattern = CfaPattern::xtrans();
    let samples: Vec<f32> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let c = pattern.color_at(y, x).rgb_index().unwrap();
            let d = ((x as f32 - 60.0).powi(2) + (y as f32 - 50.0).powi(2)).sqrt();
            (if d < 30.0 {
                1.5
            } else {
                0.2 + 0.003 * x as f32
            }) * gains[c]
        })
        .collect();
    let codes = clip_codes(&samples, w, h, &pattern, gains, 1.0);
    let camera = cameras()[1];
    for bin in [1, 3] {
        let frame = FrameCodes::new(&codes, bin).unwrap();
        let (bw, bh) = frame.size();
        let rgb: Vec<f32> = (0..bw * bh * 3).map(|i| [0.5, 0.6, 0.4][i % 3]).collect();
        let rgb = if bin == 3 {
            bin_mosaic(&samples, w, h, &pattern, 3).0
        } else {
            rgb
        };
        let f = frame.look_scale();
        let fill_ = camera_fields(
            &rgb,
            bw,
            bh,
            &camera,
            Some(&frame),
            frame.clips(),
            &weights,
            &LOOK,
            f,
        );
        let fill_ = fill_.unwrap_or_else(|| panic!("bin {bin}: a fill"));
        let mut out = rgb.clone();
        let changes = fill_.apply(&mut out, &frame);
        assert!(!changes.is_empty(), "bin {bin}");
        assert_eq!(differing(&rgb, &out), changed_set(&changes, bw));
    }
}

/// A noisy sky just at the clip, its codes flipping pixel by pixel:
/// the changes hold a bit a pixel, the picture's size, however the
/// noise scatters what changed.
#[test]
fn the_changes_are_the_picture_s_size_whatever_the_noise() {
    let weights = random_weights();
    let (w, h) = (300, 200);
    let mut state = 0x9E37_79B9u32;
    let mut noise = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut x = vec![0.0f32; w * h * 3];
    let mut codes = vec![0u8; w * h];
    for p in 0..w * h {
        let sky = p / w < h / 2;
        let n = noise();
        codes[p] = if sky && n % 3 != 0 { (n % 4) as u8 } else { 0 };
        let v = if sky {
            0.97 + 0.03 * (n % 7) as f32 / 7.0
        } else {
            0.2
        };
        x[3 * p..3 * p + 3].copy_from_slice(&[v, v * 0.98, v * 0.95]);
    }
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let camera = cameras()[2];
    let rgb = camera_picture(&x, &camera);
    let fill_ = camera_fields(
        &rgb,
        w,
        h,
        &camera,
        Some(&plane),
        [1.0; 3],
        &weights,
        &LOOK,
        1.0,
    )
    .unwrap();
    let mut out = rgb.clone();
    let changes = fill_.apply(&mut out, &plane);
    assert_eq!(differing(&rgb, &out), changed_set(&changes, w));
    assert!(changes.count() > w * h / 5, "{}", changes.count());
    assert_eq!(changes.bytes(), h * w.div_ceil(64) * 8);
}

/// Asked to give way between its parts, the fill stops there and makes
/// nothing; never told to, it asks three times (after the analysis,
/// the network, and the synthesis) and makes the fields it makes
/// without asking, to the bit.
#[test]
fn the_fill_gives_way_between_its_parts() {
    let weights = random_weights();
    let (w, h) = (160, 112);
    let (x, codes) = scene(w, h);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let camera = cameras()[1];
    let rgb = camera_picture(&x, &camera);
    let fields = |stop_at: usize, asked: &std::cell::Cell<usize>| {
        camera_fields_until(
            &rgb,
            w,
            h,
            &camera,
            Some(&plane),
            [1.0; 3],
            &weights,
            &LOOK,
            1.0,
            &|| {
                asked.set(asked.get() + 1);
                asked.get() == stop_at
            },
        )
    };
    let asked = std::cell::Cell::new(0);
    let made = fields(0, &asked).unwrap().unwrap();
    assert_eq!(asked.get(), 3);
    let want = camera_fields(
        &rgb,
        w,
        h,
        &camera,
        Some(&plane),
        [1.0; 3],
        &weights,
        &LOOK,
        1.0,
    )
    .unwrap();
    assert_eq!(made, want);
    for stop_at in 1..=3 {
        let asked = std::cell::Cell::new(0);
        assert!(matches!(
            fields(stop_at, &asked),
            Err(super::super::GaveWay)
        ));
        assert_eq!(asked.get(), stop_at);
    }
}

/// Fields taken through another camera apply through it: the same
/// fields, the new matrix and scale. Through their own camera they are
/// the fill they were; through another, the pixels changed are those
/// whose bits changed, each the working-space apply of the same fields
/// on that camera's input to rounding. A camera with no inverse or no
/// scale takes none.
#[test]
fn fields_through_another_camera_apply_through_it() {
    let weights = random_weights();
    let (w, h) = (160, 112);
    let (x, codes) = scene(w, h);
    let plane = CodePlane {
        codes: &codes,
        width: w,
    };
    let cams = cameras();
    let made = camera_fields(
        &camera_picture(&x, &cams[1]),
        w,
        h,
        &cams[1],
        Some(&plane),
        [1.0; 3],
        &weights,
        &LOOK,
        1.0,
    )
    .unwrap();
    assert_eq!(made.through(&cams[1]).unwrap(), made);
    for other in [cams[0], cams[2]] {
        let through = made.through(&other).unwrap();
        assert_eq!(through.fields(), made.fields());
        let rgb = camera_picture(&x, &other);
        let mut out = rgb.clone();
        let changes = through.apply(&mut out, &plane);
        assert!(changes.count() > 0);
        assert_eq!(differing(&rgb, &out), changed_set(&changes, w));
        let mut want = materialized(&rgb, &other);
        made.fields().apply(&mut want, &plane);
        assert!(worst(&materialized(&out, &other), &want) < 1e-6);
    }
    let flat = Camera {
        matrix: [[0.0; 3]; 3],
        ..cams[0]
    };
    assert!(made.through(&flat).is_none());
    let unscaled = Camera {
        ceiling: 0.0,
        ..cams[0]
    };
    assert!(made.through(&unscaled).is_none());
    assert!(made.bytes() > made.fields().bytes());
}
