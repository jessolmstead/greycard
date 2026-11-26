//! A mask's grain Amount through the export, the CPU reference the
//! viewport's shader is checked against: the picture's one field, its
//! Amount and the masks' added by their weights, at full size and made smaller, with
//! and without the output sharpen's bands.

use super::*;
use greycard_edit::mask::{Component, Mask, Shape};
use greycard_edit::{Adjustment, Edit, Grain, OutputSharpen};

/// A smooth frame in the midtones, where the grain is strongest and
/// never clips: a gentle colored ramp.
fn midtones(w: usize, h: usize) -> WorkingImage {
    let mut image = WorkingImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let i = (y * w + x) * 3;
            image.data[i..i + 3].copy_from_slice(&[
                0.16 + 0.08 * u,
                0.2 + 0.05 * v,
                0.24 - 0.06 * u,
            ]);
        }
    }
    image
}

/// A mask ramping out across the frame from a third of its width to
/// half: whole left of the third, nothing right of the half.
fn ramp() -> Mask {
    Mask {
        components: vec![Component {
            shape: Shape::Linear {
                from: [0.33, 0.0],
                to: [0.5, 0.0],
            },
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// A mask whole over the frame.
fn whole() -> Mask {
    Mask {
        components: vec![Component {
            shape: Shape::Linear {
                from: [-1.0, 0.0],
                to: [-2.0, 0.0],
            },
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// An edit whose grain is at `amount` (none at zero), coarse enough to
/// span a few pixels of these small frames.
fn grained(amount: f32) -> Edit {
    Edit {
        grain: Grain {
            amount,
            size: 2.0,
            ..Grain::default()
        },
        ..Edit::default()
    }
}

/// `edit` with an adjustment over `mask` whose only setting is the
/// Grain Amount `share`.
fn with_share(edit: &Edit, mask: Mask, share: f32) -> Edit {
    let mut a = Adjustment {
        id: 7,
        mask,
        ..Default::default()
    };
    a.look.grain.amount = share;
    let mut edit = edit.clone();
    edit.adjustments.push(a);
    edit
}

/// `edit` with the output sharpen on, which finishes in bands of rows.
fn sharpened(edit: &Edit) -> Edit {
    Edit {
        output_sharpen: OutputSharpen {
            amount: 0.8,
            radius: 1.3,
            threshold: 0.005,
            ..Default::default()
        },
        ..edit.clone()
    }
}

/// `edit` over `image`, sixteen bits, at `long_edge`; and its size.
fn export(image: &WorkingImage, edit: &Edit, long_edge: Option<u32>) -> (usize, usize, Vec<u16>) {
    let settings = Settings {
        format: Format::Tiff,
        sharpen: Sharpen::Off,
        long_edge,
        ..Settings::default()
    };
    let rendered = render(
        image,
        edit,
        &greycard_edit::NoStores,
        (image.width as u32, image.height as u32),
        &settings,
        &Default::default(),
        f32::INFINITY,
        None,
        finish::Source::Display,
        None,
        None,
    );
    let Pixels::Sixteen(p) = rendered.pixels else {
        panic!("a TIFF is sixteen bits")
    };
    (rendered.width as usize, rendered.height as usize, p)
}

/// The mask's weight at each pixel of an export `ew` by `eh`, as the
/// export reads it.
fn weights(mask: &Mask, image: &WorkingImage, ew: usize, eh: usize) -> Vec<f32> {
    let a = Adjustment {
        mask: mask.clone(),
        ..Default::default()
    };
    let (sw, sh) = (image.width as f32, image.height as f32);
    let rasters = finish::rasterize(&a.mask, sh / sw, |_| None);
    let local = finish::Local::of(&a, rasters, None);
    (0..ew * eh)
        .map(|i| {
            let (x, y) = (i % ew, i / ew);
            local.weight((x as f32 + 0.5) / ew as f32, (y as f32 + 0.5) / ew as f32)
        })
        .collect()
}

/// The most a pixel's channels differ between two exports, in levels of
/// 65535, over the pixels `keep` says.
fn worst(a: &[u16], b: &[u16], keep: impl Fn(usize) -> bool) -> u16 {
    a.chunks(3)
        .zip(b.chunks(3))
        .enumerate()
        .filter(|(i, _)| keep(*i))
        .flat_map(|(_, (a, b))| (0..3).map(move |k| a[k].abs_diff(b[k])))
        .max()
        .unwrap_or(0)
}

/// The sizes each check runs at: the frame's own, and made smaller.
const SIZES: [Option<u32>; 2] = [None, Some(96)];

/// A mask's Grain at zero is the picture as it was to the bit, the
/// picture's grain on or off, at both sizes, with and without the
/// output sharpen's bands: so is one switched off at any Amount, and
/// one whose Amount is too small to act anywhere, which takes the path
/// that reads every pixel's factor and finds it one.
#[test]
fn a_share_of_zero_is_the_picture_to_the_bit() {
    let image = midtones(240, 90);
    for base in [grained(0.6), grained(0.0)] {
        for edit in [base.clone(), sharpened(&base)] {
            for long_edge in SIZES {
                let zero = export(&image, &with_share(&edit, ramp(), 0.0), long_edge);
                for share in [1.0, -1.0] {
                    let mut off = with_share(&edit, ramp(), share);
                    off.adjustments[0].look.grain.enabled = false;
                    assert!(zero == export(&image, &off, long_edge), "off {long_edge:?}");
                }
                let tiny = with_share(&edit, ramp(), 1e-4);
                assert!(zero == export(&image, &tiny, long_edge), "{long_edge:?}");
            }
        }
    }
}

/// A mask's Amount over the whole frame adds to the picture's, at both
/// sizes, with and without the bands: 70% with -100 is the picture with
/// its grain off, to the bit, and so is 40% with -100 (the sum is held
/// at none); 40% with +40 is 80% to the bit (the field is linear in its
/// amount, `0.4 + 0.4` is `2 * 0.4` exactly, and the factor 2 is exact);
/// 0% with +100 is 100% to the bit (the field drawn at a full Amount,
/// by one); and 40% with +50 is 90%, to a level's rounding.
#[test]
fn a_whole_frame_amount_adds_to_the_pictures() {
    let image = midtones(240, 90);
    for sharpen in [false, true] {
        let s = |e: Edit| if sharpen { sharpened(&e) } else { e };
        for long_edge in SIZES {
            let at = |picture: f32, mask: f32| {
                export(
                    &image,
                    &s(with_share(&grained(picture), whole(), mask)),
                    long_edge,
                )
            };
            let how = format!("at {long_edge:?}, sharpen {sharpen}");
            let off = at(0.0, 0.0);
            assert!(at(0.7, -1.0) == off, "70% -100 {how}");
            assert!(at(0.4, -1.0) == off, "40% -100 {how}");
            let (sum, eighty) = (at(0.4, 0.4), at(0.8, 0.0));
            assert!(
                sum == eighty,
                "40% +40 {how}: {} levels",
                worst(&sum.2, &eighty.2, |_| true)
            );
            assert!(at(0.0, 1.0) == at(1.0, 0.0), "0% +100 {how}");
            assert!(
                worst(&at(0.4, 0.5).2, &at(0.9, 0.0).2, |_| true) <= 1,
                "40% +50 {how}"
            );
        }
    }
}

/// No jump anywhere: at a fixed mask Amount over the whole frame, the
/// picture's Amount swept up through zero in steps of a twentieth of a
/// percent gives, at every step, the grain of the picture's own at their
/// sum, within a level of 65535; and the same with the mask's Amount
/// swept through zero at a fixed picture's. The picture's own grain is
/// continuous in its Amount, so the sum's is.
#[test]
fn the_grain_has_no_jump_through_zero() {
    let image = midtones(120, 60);
    let at = |picture: f32, mask: f32| {
        export(&image, &with_share(&grained(picture), whole(), mask), None).2
    };
    let check = |a: f32, k: f32| {
        let off = worst(&at(a, k), &at((a + k).max(0.0), 0.0), |_| true);
        assert!(off <= 1, "a {a}, k {k}: {off} levels from their sum");
    };
    for k in [0.3f32, -0.2] {
        for a in [0.0f32, 0.0005, 0.001, 0.002, 0.005, 0.01, 0.02] {
            check(a, k);
        }
    }
    for k in [-0.02f32, -0.01, -0.003, 0.003, 0.01, 0.02] {
        check(0.3, k);
        check(0.0, k);
    }
}

/// With the picture's grain at 0%, +100 grains inside the mask alone: not
/// a level moves where the mask does not reach, and where it is whole
/// the grain is the picture's own at 100%, to the bit. Through the
/// feather between, the grain is that one field at the mask's weight:
/// what it adds is the weight times what the whole grain adds, to a
/// level of rounding, so there is no seam and no second pattern.
#[test]
fn a_mask_alone_grains_inside_it_by_its_weight() {
    let image = midtones(240, 90);
    for sharpen in [false, true] {
        let s = |e: Edit| if sharpen { sharpened(&e) } else { e };
        for long_edge in SIZES {
            let (w, h, none) = export(
                &image,
                &s(with_share(&grained(0.0), ramp(), 0.0)),
                long_edge,
            );
            let (_, _, full) = export(
                &image,
                &s(with_share(&grained(1.0), ramp(), 0.0)),
                long_edge,
            );
            let (_, _, alone) = export(
                &image,
                &s(with_share(&grained(0.0), ramp(), 1.0)),
                long_edge,
            );
            let wts = weights(&ramp(), &image, w, h);
            assert!(wts.contains(&0.0) && wts.contains(&1.0));
            assert_eq!(worst(&alone, &none, |i| wts[i] == 0.0), 0, "outside");
            assert_eq!(worst(&alone, &full, |i| wts[i] == 1.0), 0, "inside");
            assert!(worst(&full, &none, |_| true) > 1000, "the grain shows");
            // The feather: what the mask's grain adds against the whole
            // grain's at the weight, pixel by pixel, and the two as one
            // pattern.
            let (mut feathered, mut most, mut ab, mut aa, mut bb) = (0, 0.0f64, 0.0, 0.0, 0.0);
            for (i, &wt) in wts.iter().enumerate() {
                if wt <= 0.05 || wt >= 0.95 {
                    continue;
                }
                feathered += 1;
                for k in 0..3 {
                    let j = i * 3 + k;
                    let by_mask = alone[j] as f64 - none[j] as f64;
                    let by_whole = wt as f64 * (full[j] as f64 - none[j] as f64);
                    most = most.max((by_mask - by_whole).abs());
                    ab += by_mask * by_whole;
                    aa += by_mask * by_mask;
                    bb += by_whole * by_whole;
                }
            }
            let alike = ab / (aa * bb).sqrt();
            eprintln!(
                "at {long_edge:?}, sharpen {sharpen}: {feathered} feathered, {most:.1} levels off the weight, alike {alike:.4}"
            );
            assert!(feathered > 50, "{feathered}");
            assert!(most <= 2.0, "{most}");
            assert!(alike > 0.99, "{alike}");
        }
    }
}

/// Two masks over one pixel add their Amounts by their weights, held to
/// -100 to +100, as a mask's Sharpen share is: +60 and +60 whole over
/// the frame are +100, the same as +50 and +50 to the bit; and -60 with
/// +20 over 50% is 10%, to a level.
#[test]
fn masks_amounts_add_and_are_held_at_the_ends() {
    let image = midtones(120, 60);
    let two = |p: f32, a: f32, b: f32| {
        export(
            &image,
            &with_share(&with_share(&grained(p), whole(), a), whole(), b),
            None,
        )
    };
    assert!(two(0.3, 0.6, 0.6) == two(0.3, 0.5, 0.5), "held at +100");
    assert!(
        worst(&two(0.5, -0.6, 0.2).2, &two(0.1, 0.0, 0.0).2, |_| true) <= 1,
        "-40"
    );
}

/// One rule says whether a mask acts on the grain: an enabled mask
/// that reaches somewhere at some opacity, with an Amount that can
/// change a pixel's grain (either way over the picture's grain, only
/// adding over none). Where it says no, nothing is drawn for the masks
/// and the export is the one without them, to the bit.
#[test]
fn one_rule_says_whether_a_mask_acts_on_the_grain() {
    let local = |amount: f32, change: &dyn Fn(&mut Adjustment)| {
        let mut a = Adjustment {
            id: 1,
            mask: ramp(),
            ..Default::default()
        };
        a.look.grain.amount = amount;
        change(&mut a);
        let rasters = finish::rasterize(&a.mask, 0.5, |_| None);
        vec![finish::Local::of(&a, rasters, None)]
    };
    let acts = |picture: f32, l: &[finish::Local]| finish::masks_act_on_grain(picture, l);
    let none = |_: &mut Adjustment| {};
    assert!(acts(0.4, &local(-0.5, &none)), "a take over the picture's");
    assert!(acts(0.4, &local(0.5, &none)));
    assert!(acts(0.0, &local(0.5, &none)), "an addition over none");
    assert!(!acts(0.0, &local(-0.5, &none)), "a take from nothing");
    assert!(!acts(0.4, &local(0.0, &none)));
    assert!(!acts(0.4, &local(0.5, &|a| a.opacity = 0.0)));
    assert!(!acts(0.4, &local(0.5, &|a| a.enabled = false)));
    assert!(!acts(0.4, &local(0.5, &|a| a.look.grain.enabled = false)));
    assert!(!acts(0.4, &local(0.5, &|a| a.mask.components.clear())));
    // Drawn by the same rule: a take over none draws nothing.
    let g = Grain::default();
    assert_eq!(finish::grain_drawn(&g, &local(-0.5, &none)), None);
    assert!(finish::grain_drawn(&g, &local(0.5, &none)).is_some());
    let image = midtones(120, 60);
    for edit in [grained(0.0), sharpened(&grained(0.0))] {
        let plain = export(&image, &with_share(&edit, ramp(), 0.0), None);
        let take = export(&image, &with_share(&edit, ramp(), -1.0), None);
        assert!(plain == take, "a take from nothing is nothing");
    }
}
