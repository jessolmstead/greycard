//! A mask's share of the output sharpen through the export, the CPU
//! reference the viewport's shader is checked against: at full size and
//! made smaller, added under the mask's weight to the picture's amount
//! at the picture's radius and threshold.

use super::*;
use greycard_edit::mask::{Component, Mask, Shape};
use greycard_edit::{Adjustment, Edit, OutputSharpen};

/// A frame of edges: grey bars of three heights over a colored ramp,
/// so a sharpen moves every column near an edge and no flat stretch.
fn bars(w: usize, h: usize) -> WorkingImage {
    let mut image = WorkingImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let bar = [0.05f32, 0.35, 0.12][(x / 7 + y / 9) % 3];
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let i = (y * w + x) * 3;
            image.data[i..i + 3].copy_from_slice(&[
                bar * (0.6 + 0.8 * u),
                bar * (0.8 + 0.4 * v),
                bar * (1.2 - 0.5 * u),
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

/// `edit` with an adjustment over `mask` whose only setting is the
/// Output sharpen `share`.
fn with_share(edit: &Edit, mask: Mask, share: f32) -> Edit {
    let mut a = Adjustment {
        id: 7,
        mask,
        ..Default::default()
    };
    a.look.detail.output_sharpen = share;
    let mut edit = edit.clone();
    edit.adjustments.push(a);
    edit
}

fn sharpened(amount: f32) -> Edit {
    Edit {
        output_sharpen: OutputSharpen {
            amount,
            radius: 1.3,
            threshold: 0.005,
            ..Default::default()
        },
        ..Edit::default()
    }
}

/// `edit` over `image`, sixteen bits, at `long_edge`, a picture already
/// rendered so the numbers are the frame's own; and its size.
fn export(image: &WorkingImage, edit: &Edit, long_edge: Option<u32>) -> (usize, usize, Vec<u16>) {
    export_sheet(image, edit, long_edge, Sharpen::Off)
}

/// [`export`] under the sheet's `sharpen`.
fn export_sheet(
    image: &WorkingImage,
    edit: &Edit,
    long_edge: Option<u32>,
    sharpen: Sharpen,
) -> (usize, usize, Vec<u16>) {
    let settings = Settings {
        format: Format::Tiff,
        sharpen,
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

/// With the picture's off, a mask at +100 sharpens under it alone: not
/// a level of a pixel the mask does not reach moves, at full size or
/// made smaller, and under it the picture is sharpened.
#[test]
fn a_mask_alone_sharpens_under_it_and_nothing_else() {
    let image = bars(240, 90);
    let off = Edit::default();
    let masked = with_share(&off, ramp(), 1.0);
    for long_edge in [None, Some(96)] {
        let (w, h, plain) = export(&image, &off, long_edge);
        let (_, _, shared) = export(&image, &masked, long_edge);
        let wts = weights(&ramp(), &image, w, h);
        assert!(wts.contains(&0.0) && wts.contains(&1.0));
        let outside = worst(&plain, &shared, |i| wts[i] == 0.0);
        let inside = worst(&plain, &shared, |i| wts[i] == 1.0);
        eprintln!("at {long_edge:?}: outside {outside} levels, inside {inside}");
        assert_eq!(outside, 0, "at {long_edge:?}");
        assert!(inside > 1000, "at {long_edge:?}: {inside}");
    }
}

/// A mask whole over the frame at +100 with the picture's off is the
/// picture's own at 100%, to the bit, at full size and made smaller;
/// and at +50 over the picture's 50%, the picture's at 100%.
#[test]
fn a_masks_share_is_the_pictures_amount() {
    let image = bars(240, 90);
    for long_edge in [None, Some(96)] {
        let by_picture = export(
            &image,
            &with_share(&sharpened(1.0), whole(), 0.0),
            long_edge,
        );
        let by_mask = export(
            &image,
            &with_share(&sharpened(0.0), whole(), 1.0),
            long_edge,
        );
        let wts = weights(&whole(), &image, by_mask.0, by_mask.1);
        let lo = wts.iter().fold(1.0f32, |a, &b| a.min(b));
        let n = by_mask
            .2
            .chunks(3)
            .zip(by_picture.2.chunks(3))
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            by_mask.2 == by_picture.2,
            "+100 at {long_edge:?}: {} levels, {n} pixels, weight {lo}",
            worst(&by_mask.2, &by_picture.2, |_| true)
        );
        let half = export(
            &image,
            &with_share(&sharpened(0.5), whole(), 0.5),
            long_edge,
        );
        assert!(half.2 == by_picture.2, "+50 on 50% at {long_edge:?}");
        // Half the weight is half the share.
        let mut thin = with_share(&sharpened(0.0), whole(), 1.0);
        thin.adjustments[0].opacity = 0.5;
        let by_opacity = export(&image, &thin, long_edge);
        let at_half = export(&image, &sharpened(0.5), long_edge);
        assert!(
            worst(&by_opacity.2, &at_half.2, |_| true) <= 1,
            "opacity at {long_edge:?}"
        );
    }
}

/// A mask at -100 takes the picture's sharpen all off under it whatever
/// its amount, 200% here, to the bit of the unsharpened picture there,
/// and leaves every pixel it does not reach as the picture's sharpen
/// made it. With the picture's off, -100 is nothing at all: the export
/// is the one with no mask's share, to the bit, at either size.
#[test]
fn minus_a_hundred_takes_the_pictures_off_and_alone_does_nothing() {
    let image = bars(240, 90);
    for long_edge in [None, Some(96)] {
        let (w, h, on) = export(&image, &with_share(&sharpened(2.0), ramp(), 0.0), long_edge);
        let (_, _, unsharpened) =
            export(&image, &with_share(&sharpened(0.0), ramp(), 0.0), long_edge);
        let (_, _, taken) = export(
            &image,
            &with_share(&sharpened(2.0), ramp(), -1.0),
            long_edge,
        );
        let wts = weights(&ramp(), &image, w, h);
        assert_eq!(worst(&taken, &unsharpened, |i| wts[i] == 1.0), 0);
        assert_eq!(worst(&taken, &on, |i| wts[i] == 0.0), 0);
        let (_, _, alone) = export(
            &image,
            &with_share(&Edit::default(), ramp(), -1.0),
            long_edge,
        );
        let (_, _, none) = export(
            &image,
            &with_share(&Edit::default(), ramp(), 0.0),
            long_edge,
        );
        assert!(alone == none, "at {long_edge:?}");
        assert!(!finish_shares(&with_share(&Edit::default(), ramp(), -1.0)));
    }
}

/// Two masks whole over the frame: a take scales the picture's amount
/// and an addition adds to it, the takes multiplying and the additions
/// summing, the takes on the picture's alone. Each export is checked
/// against the picture's own at the amount that rule gives, beside the
/// same two masks at none.
#[test]
fn takes_multiply_and_additions_sum_across_masks() {
    let image = bars(240, 90);
    let two = |picture: f32, a: f32, b: f32| {
        let mut e = with_share(&sharpened(picture), whole(), a);
        let mut second = e.adjustments[0].clone();
        second.id = 8;
        second.look.detail.output_sharpen = b;
        e.adjustments.push(second);
        e
    };
    for long_edge in [None, Some(96)] {
        // (picture, first share, second share, the amount it is).
        for (picture, a, b, amount) in [
            (2.0, -0.5, 0.0, 1.0),
            (2.0, -0.5, -0.5, 0.5),
            (1.5, -1.0, 0.5, 0.5),
            (1.5, 0.5, -1.0, 0.5),
            (1.0, -0.5, 0.25, 0.75),
            (0.0, -1.0, 0.5, 0.5),
            (0.5, 0.5, 0.5, 1.5),
        ] {
            let (_, _, got) = export(&image, &two(picture, a, b), long_edge);
            let (_, _, want) = export(&image, &two(amount, 0.0, 0.0), long_edge);
            let off = worst(&got, &want, |_| true);
            assert!(
                off <= 1,
                "{picture} with {a} and {b} at {long_edge:?}: {off} levels"
            );
        }
    }
}

/// The sheet's Sharpen gives way to an Output sharpen that acts: a
/// small export with the picture's on, or a mask's addition alone, is
/// the export with the sheet's off, to the bit, at every level of it;
/// with none acting (none at all, or a mask's take alone over the
/// picture's off) the sheet's sharpens as before.
#[test]
fn the_sheets_sharpen_gives_way_to_an_output_sharpen() {
    let image = bars(240, 90);
    let small = Some(96);
    let acting = [
        with_share(&sharpened(0.8), whole(), 0.0),
        with_share(&sharpened(0.0), ramp(), 0.5),
        with_own(&sharpened(0.0), ramp(), 1.5, Some(3.0), Some(0.0)),
    ];
    for edit in &acting {
        assert!(edit.output_sharpens());
        let off = export_sheet(&image, edit, small, Sharpen::Off);
        for sheet in [Sharpen::Low, Sharpen::Standard, Sharpen::High] {
            assert!(
                export_sheet(&image, edit, small, sheet) == off,
                "{sheet:?} under an Output sharpen"
            );
        }
    }
    let idle = [
        with_share(&sharpened(0.0), ramp(), 0.0),
        with_share(&sharpened(0.0), ramp(), -1.0),
        with_own(&sharpened(0.0), ramp(), -1.0, Some(3.0), Some(0.0)),
        with_own(&sharpened(0.0), ramp(), 0.0, Some(3.0), None),
    ];
    for edit in &idle {
        assert!(!edit.output_sharpens());
        let off = export_sheet(&image, edit, small, Sharpen::Off);
        let standard = export_sheet(&image, edit, small, Sharpen::Standard);
        assert!(standard != off, "the sheet's sharpens with none acting");
        let settings = Settings {
            sharpen: Sharpen::Standard,
            ..Settings::default()
        };
        assert!(matches!(
            settings.for_edit(edit),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}

/// Whether the finish runs the sharpen's blur for `edit`.
fn finish_shares(edit: &Edit) -> bool {
    let locals: Vec<finish::Local> = edit
        .adjustments
        .iter()
        .map(|a| finish::Local::of(a, vec![None; a.mask.components.len()], None))
        .collect();
    crate::output_sharpen::Plan::finishing(&edit.output_sharpen, &locals).is_some()
}

/// A dark, noisy frame, its channels' noise apart, sharpened by a
/// mask's share as the picture's own would be: no colored dots. The
/// sharpened luminance is the pixel's own moved by at most the amount
/// times its own excess over the blur, so a pixel near black comes out
/// at most `1 + amount` times as light (encoded), never lit from its
/// neighbors, and its channels keep their proportions: nothing like the
/// ratio of two small numbers a resized delta once made (§728). At full
/// size and made smaller, the same as the picture's own at that amount.
#[test]
fn a_dark_noisy_frame_makes_no_colored_dots() {
    let (w, h) = (200usize, 120usize);
    let mut image = WorkingImage::new(w, h);
    let mut seed = 0x9e37_79b9u32;
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    for px in image.data.chunks_mut(3) {
        let base = 0.002 + 0.004 * noise();
        for c in px.iter_mut() {
            *c = base * (0.3 + 1.4 * noise());
        }
    }
    for long_edge in [None, Some(80)] {
        let amount = 2.0;
        let (_, _, plain) = export(
            &image,
            &with_share(&Edit::default(), whole(), 0.0),
            long_edge,
        );
        // A share of +1 is 100%: two masks at +1 are the picture's 200%,
        // beside the same two masks at none.
        let two = |picture: f32, share: f32| {
            let mut e = with_share(&sharpened(picture), whole(), share);
            e.adjustments.push(Adjustment {
                id: 8,
                ..e.adjustments[0].clone()
            });
            e
        };
        let (_, _, own) = export(&image, &two(amount, 0.0), long_edge);
        let (_, _, twice) = export(&image, &two(0.0, 1.0), long_edge);
        assert!(twice == own, "two masks at +100 at {long_edge:?}");
        let luma = |p: &[u16]| {
            let c = [0, 1, 2].map(|k| finish::decode(f32::from(p[k]) / 65535.0));
            finish::encode(0.2627 * c[0] + 0.6780 * c[1] + 0.0593 * c[2])
        };
        let (mut most, mut moved) = (0.0f32, 0usize);
        for (a, b) in plain.chunks(3).zip(twice.chunks(3)) {
            let (la, lb) = (luma(a), luma(b));
            if a != b {
                moved += 1;
            }
            // Rounding to 16 bits aside, a level or so of 65535.
            assert!(
                lb <= (1.0 + amount) * la + 3.0 / 65535.0,
                "{a:?} went to {b:?} at {long_edge:?}"
            );
            most = most.max(lb - la);
        }
        eprintln!(
            "dark noise at {long_edge:?}: {moved} pixels moved, the most a pixel lightened \
             {:.1} levels of 255",
            most * 255.0
        );
        assert!(moved > w * h / 20 / if long_edge.is_some() { 6 } else { 1 });
    }
}

/// The clip guard holds with a mask's share: beside a blown highlight
/// a near-white edge sharpened by a mask at +100, the picture's off,
/// brightens until its largest channel reaches white and no further,
/// its channels in proportion, as the picture's own sharpen does it;
/// at full size and made smaller.
#[test]
fn the_clip_guard_holds_under_a_masks_share() {
    let (w, h) = (120usize, 40usize);
    let mut image = WorkingImage::new(w, h);
    for (i, px) in image.data.chunks_mut(3).enumerate() {
        let x = i % w;
        let c = if x < w / 3 {
            [0.2, 0.18, 0.15]
        } else if x < 2 * w / 3 {
            [0.92, 0.7, 0.45]
        } else {
            [1.0, 1.0, 1.0]
        };
        px.copy_from_slice(&c);
    }
    for long_edge in [None, Some(60)] {
        let by_mask = export(
            &image,
            &with_share(&sharpened(0.0), whole(), 1.0),
            long_edge,
        );
        let by_picture = export(
            &image,
            &with_share(&sharpened(1.0), whole(), 0.0),
            long_edge,
        );
        assert!(by_mask.2 == by_picture.2, "at {long_edge:?}");
        // In Rec.2020 out, the working space's own, no matrix: the
        // near-white band's light halo at the grey's edge, its warm
        // color in proportion, its red at white and no further.
        let settings = Settings {
            format: Format::Tiff,
            sharpen: Sharpen::Off,
            long_edge,
            space: Space::Rec2020,
            ..Settings::default()
        };
        let edit = with_share(&sharpened(0.0), whole(), 1.0);
        let rendered = render(
            &image,
            &edit,
            &greycard_edit::NoStores,
            (w as u32, h as u32),
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
        let ew = rendered.width as usize;
        let mut topped = 0;
        for (i, c) in p.chunks(3).enumerate() {
            let x = i % ew;
            if !(ew / 3 + 1..ew / 3 + 4).contains(&x) {
                continue;
            }
            let lin = [0, 1, 2].map(|k| finish::decode(f32::from(c[k]) / 65535.0));
            if c[0] == 65535 {
                topped += 1;
            }
            assert!(
                (lin[1] / lin[0] - 0.7 / 0.92).abs() < 0.01
                    && (lin[2] / lin[0] - 0.45 / 0.92).abs() < 0.01,
                "{c:?} at {x} at {long_edge:?}"
            );
        }
        assert!(topped > 0, "the halo reaches white at {long_edge:?}");
    }
}

/// `with_share` with the mask's own Radius and Threshold.
fn with_own(
    edit: &Edit,
    mask: Mask,
    share: f32,
    radius: Option<f32>,
    threshold: Option<f32>,
) -> Edit {
    let mut e = with_share(edit, mask, share);
    let detail = &mut e.adjustments.last_mut().unwrap().look.detail;
    detail.output_sharpen_radius = radius;
    detail.output_sharpen_threshold = threshold;
    e
}

/// [`bars`] softened, as a frame a little out of focus is: each row and
/// column through a Gaussian of three pixels.
fn soft_bars(w: usize, h: usize) -> WorkingImage {
    let sharp = bars(w, h);
    let mut image = WorkingImage::new(w, h);
    for k in 0..3 {
        let plane: Vec<f32> = sharp.data.chunks(3).map(|c| c[k]).collect();
        let soft = crate::output_sharpen::blur(&plane, w, h, 3.0);
        for (px, v) in image.data.chunks_mut(3).zip(soft) {
            px[k] = v;
        }
    }
    image
}

/// The mean change between two exports, in levels of 255.
fn mean_levels(a: &[u16], b: &[u16]) -> f64 {
    let sum: f64 = a
        .iter()
        .zip(b)
        .map(|(a, b)| f64::from(a.abs_diff(*b)) / 257.0)
        .sum();
    sum / a.len() as f64
}

/// A mask's own Radius and Threshold at the picture's are the picture's:
/// the export is the one with none of its own, to the bit, at full size
/// and made smaller, the picture's on and off; and a take's own Radius
/// and Threshold do nothing, the take at the picture's.
#[test]
fn own_settings_at_the_pictures_are_the_pictures_to_the_bit() {
    let image = bars(240, 90);
    for long_edge in [None, Some(96)] {
        for picture in [0.0, 0.7] {
            let base = sharpened(picture);
            let none = export(&image, &with_share(&base, ramp(), 0.8), long_edge);
            let same = export(
                &image,
                &with_own(&base, ramp(), 0.8, Some(1.3), Some(0.005)),
                long_edge,
            );
            assert!(none == same, "{picture} at {long_edge:?}");
            let radius_alone = export(
                &image,
                &with_own(&base, ramp(), 0.8, Some(1.3), None),
                long_edge,
            );
            assert!(none == radius_alone, "{picture} at {long_edge:?}");
        }
        let base = sharpened(1.4);
        let take = export(&image, &with_share(&base, ramp(), -0.6), long_edge);
        let own_take = export(
            &image,
            &with_own(&base, ramp(), -0.6, Some(3.0), Some(0.0)),
            long_edge,
        );
        assert!(take == own_take, "a take at {long_edge:?}");
    }
}

/// A mask whole over the frame at its own Radius and Threshold, the
/// picture's off, is the picture's own sharpen at those, to the bit: at
/// +100 its 100%, at +200 its 200%; at full size and made smaller.
#[test]
fn a_mask_at_its_own_settings_is_the_pictures_sharpen_at_them() {
    let image = bars(240, 90);
    for long_edge in [None, Some(96)] {
        for (share, amount) in [(1.0, 1.0), (2.0, 2.0)] {
            let by_mask = export(
                &image,
                &with_own(&sharpened(0.0), whole(), share, Some(3.0), Some(0.0)),
                long_edge,
            );
            let mut picture = sharpened(amount);
            picture.output_sharpen.radius = 3.0;
            picture.output_sharpen.threshold = 0.0;
            let by_picture = export(&image, &with_share(&picture, whole(), 0.0), long_edge);
            assert!(
                by_mask.2 == by_picture.2,
                "+{share} at {long_edge:?}: {} levels",
                worst(&by_mask.2, &by_picture.2, |_| true)
            );
        }
        // +200 at the picture's radius over its off is its 200% too.
        let by_mask = export(
            &image,
            &with_share(&sharpened(0.0), whole(), 2.0),
            long_edge,
        );
        let by_picture = export(
            &image,
            &with_share(&sharpened(2.0), whole(), 0.0),
            long_edge,
        );
        assert!(by_mask.2 == by_picture.2, "+200 at {long_edge:?}");
    }
}

/// The case a mask's own Radius is for: a soft frame. A mask at +100 at
/// the picture's Radius 1 and Threshold 1% moves it a little; at Radius
/// 3 and Threshold 0 several times as much, the picture's off; and at
/// Radius 3 it sharpens where Radius 1 at 200% still does less. Outside
/// the mask nothing moves.
#[test]
fn a_mask_at_radius_three_sharpens_what_radius_one_does_not() {
    let image = soft_bars(240, 90);
    let mut off = Edit::default();
    off.output_sharpen.radius = 1.0;
    off.output_sharpen.threshold = 0.01;
    let (w, h, plain) = export(&image, &off, None);
    let wts = weights(&ramp(), &image, w, h);
    let inside = |e: &Edit| {
        let (_, _, p) = export(&image, e, None);
        assert_eq!(worst(&plain, &p, |i| wts[i] == 0.0), 0, "outside the mask");
        let keep: Vec<usize> = (0..w * h).filter(|&i| wts[i] == 1.0).collect();
        let pick = |p: &[u16]| -> Vec<u16> {
            keep.iter()
                .flat_map(|&i| p[i * 3..i * 3 + 3].to_vec())
                .collect()
        };
        mean_levels(&pick(&plain), &pick(&p))
    };
    let at_one = inside(&with_share(&off, ramp(), 1.0));
    let at_one_twice = inside(&with_share(&off, ramp(), 2.0));
    let at_three = inside(&with_own(&off, ramp(), 1.0, Some(3.0), Some(0.0)));
    eprintln!(
        "a soft frame under a mask: +100 at radius 1, threshold 1% {at_one:.2} levels; \
         +200 there {at_one_twice:.2}; +100 at radius 3, threshold 0 {at_three:.2}"
    );
    assert!(at_three > 3.0 * at_one, "{at_three} against {at_one}");
    assert!(at_three > at_one_twice, "{at_three} against {at_one_twice}");
}

/// Two masks at radii of their own compose by their terms, order
/// apart: either order is the same export, to the bit; under both, the
/// two are neither mask alone (with the picture's on their sum is held,
/// so they need not move it further than one); and with the picture's
/// own on too, every pixel outside both masks is the picture's own, to
/// the bit.
#[test]
fn two_masks_at_their_own_radii_compose() {
    let image = soft_bars(240, 90);
    let left = Mask {
        components: vec![Component {
            shape: Shape::Linear {
                from: [0.5, 0.0],
                to: [0.6, 0.0],
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let right = Mask {
        components: vec![Component {
            shape: Shape::Linear {
                from: [0.4, 0.0],
                to: [0.3, 0.0],
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    for long_edge in [None, Some(96)] {
        for picture in [0.0, 0.6] {
            let base = sharpened(picture);
            let a = |e: &Edit| with_own(e, left.clone(), 1.2, Some(3.0), Some(0.0));
            let b = |e: &Edit| with_own(e, right.clone(), 0.9, Some(0.6), None);
            let ab = export(&image, &b(&a(&base)), long_edge);
            let ba = export(&image, &a(&b(&base)), long_edge);
            assert!(ab == ba, "order at {long_edge:?}");
            let (w, h, alone) = export(&image, &base, long_edge);
            let wl = weights(&left, &image, w, h);
            let wr = weights(&right, &image, w, h);
            assert_eq!(
                worst(&ab.2, &alone, |i| wl[i] == 0.0 && wr[i] == 0.0),
                0,
                "outside both at {long_edge:?}"
            );
            let only_a = export(&image, &a(&base), long_edge);
            let only_b = export(&image, &b(&base), long_edge);
            let both = |i: usize| wl[i] == 1.0 && wr[i] == 1.0;
            let pick = |p: &[u16]| -> Vec<u16> {
                (0..w * h)
                    .filter(|&i| both(i))
                    .flat_map(|i| p[i * 3..i * 3 + 3].to_vec())
                    .collect()
            };
            let (m_ab, m_a, m_b) = (
                mean_levels(&pick(&alone), &pick(&ab.2)),
                mean_levels(&pick(&alone), &pick(&only_a.2)),
                mean_levels(&pick(&alone), &pick(&only_b.2)),
            );
            assert!(
                m_ab > 0.5 && pick(&ab.2) != pick(&only_a.2) && pick(&ab.2) != pick(&only_b.2),
                "{m_ab} {m_a} {m_b}"
            );
            if picture == 0.0 {
                // 1.2 and 0.9, held to 2 together: still more than the
                // narrower alone.
                assert!(m_ab > m_b, "{m_ab} {m_b}");
            }
        }
    }
}
