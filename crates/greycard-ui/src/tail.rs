//! The display curve on a norm: a color's channels all scaled by the
//! gain the curve gives their mean, so its hue holds as it rolls off,
//! and the way to white a step in chroma at a constant Oklab hue.
//!
//! Ported from darktable's sigmoid module, `src/iop/sigmoid.c`, its
//! "RGB ratio" color processing (`process_loglogistic_rgb_ratio`),
//! GPL-3.0-or-later. Written by Jakob Dove for darktable 4.0;
//! copyright (C) 2020-2026 darktable developers. Taken from it: the
//! norm, the mean of the three channels; the one gain on all three;
//! and the hyperbolic compression of chroma against the display's
//! cube, which leaves a color far inside the cube about as it is and
//! takes one at the cube's edge, or past it, back to the edge, so the
//! nearer the mapped norm is to white the less chroma is left. Its
//! formula is the module's, written in reciprocals so that it needs
//! none of the module's epsilons (`chroma_kept` says how).
//!
//! Departures. The curve on the norm is greycard's own display curve
//! (`finish::tone`) and not the module's log-logistic sigmoid, so a
//! neutral is what it always was. And the compression is applied in
//! Oklab, the lightness and the chroma both moved toward the mapped
//! neutral's at the pixel's own hue, where the module mixes the RGB
//! toward the neutral: a mix in linear light is a straight line in
//! chromaticity, which Oklab, and the eye, read as a turn, a bright
//! blue as much as 26 degrees toward violet on its way to white.

use std::sync::LazyLock;

use greycard_core::color::Oklab;

use crate::finish::{DISPLAY_WHITE, tone, tone_slope};

static OKLAB: LazyLock<Oklab> = LazyLock::new(Oklab::for_working_space);

/// The share of a color's chroma about the mapped neutral `grey` that
/// is kept: one for a color well inside the display's cube, less as
/// it nears a face, and exactly what puts it on the face when it is
/// there or past it. `p` is the color after the norm's gain, so its
/// mean is `grey`, which is over zero and under one. The shader's
/// `chroma_kept` is this.
///
/// darktable's `process_loglogistic_rgb_ratio`, with a white target
/// of one and a black of zero, as the module writes it: `room` the
/// distance along the line from the neutral through the color to the
/// nearer face of the cube, in units of the color; `chroma` the
/// color's against the most it could have at this mean, where its
/// lowest channel is zero; then
/// `h = 2 chroma / (1 - chroma^2) / (chroma room)` and the share kept
/// `room h / (1 + sqrt(h^2 + 1))`. Here in the reciprocals, which
/// stay finite at both ends: with `a = (1 - chroma^2) / 2` and
/// `r = 1 / room`, the share is `1 / (a + sqrt(a^2 + r^2))`. The
/// module guards its divisions with an epsilon of a millionth, which
/// at a face leaves a share a millionth short of the face and lifts a
/// channel at zero by about that; this form needs no guard and keeps
/// a channel at zero at zero.
///
/// The module's formula is for a color inside the cube or past its
/// white face. One past the black face, a channel under zero, which
/// the slope's chroma below can make of a dark saturated color, is
/// put on the face: the share is at most `1 / r`, which for a color
/// inside is already so.
#[inline]
fn chroma_kept(p: [f32; 3], grey: f32) -> f32 {
    let lo = p[0].min(p[1]).min(p[2]);
    let hi = p[0].max(p[1]).max(p[2]);
    // The lowest channel as a share of the mean: the chroma is one
    // less it, and so is one over the distance to the black face.
    // Kept as the share, so a channel at zero makes the chroma one
    // and `a` zero exactly on any device, whatever its division
    // rounds `grey / grey` to.
    let t = (lo / grey).min(1.0);
    let r = ((hi - grey).max(0.0) / (1.0 - grey)).max(1.0 - t);
    let a = 0.5 * t * (2.0 - t);
    (1.0 / (a + (a * a + r * r).sqrt())).min(1.0 / r)
}

/// The scale on a color's Oklab chroma that gives it what the curve
/// per channel would have: the curve's slope in stops, both ways, over
/// one through the toe and under one over mid grey, in full for a
/// color near grey and less the more saturated it is, by a weight on
/// its lowest channel's share of the mean. The curve per channel
/// multiplies a color near grey's chroma by that slope, and a
/// saturated one's by less, since its channels sit at different points
/// of the curve and its lowest one near black; scaled in full, a dark
/// saturated orange went past the cube's black face, and the clip
/// there turned it five degrees. The weight is a square root with a
/// floor, `(sqrt(t + 0.01) - 0.1) / (sqrt(1.01) - 0.1)`: zero at a
/// share of zero and one at one, rising fast enough that a moderately
/// saturated color gets most of its slope (a linear weight left the
/// shadows' chroma a tenth under today's), with a slope of about 5.5
/// at zero. A plain square root, tried first, has no bound on its slope
/// there, which is where a saturated color's lowest channel sits, and
/// the GPU's rounding there made levels.
/// `p` is the color after the norm's gain, `grey` its mean and `norm`
/// the scene's.
#[inline]
fn chroma_boost(p: [f32; 3], grey: f32, norm: f32) -> f32 {
    let lo = p[0].min(p[1]).min(p[2]);
    let t = (lo / grey).clamp(0.0, 1.0);
    let near_grey = ((t + 0.01).sqrt() - 0.1) / (1.01f32.sqrt() - 0.1);
    1.0 + (tone_slope(norm) - 1.0) * near_grey
}

/// The display curve on a linear scene color, by the norm. A neutral
/// is `tone` exactly; a color keeps its Oklab hue through the whole
/// roll-off and loses its chroma by [`chroma_kept`] as it nears white,
/// which it reaches where a neutral does, at [`DISPLAY_WHITE`] on its
/// mean. A NaN anywhere is white, as `tone` takes one to white.
///
/// One gain on all three channels keeps the scene's chroma, where the
/// curve per channel multiplies a color's departure from grey by the
/// curve's slope in stops, which through the toe is up to 1.6 and over
/// mid grey falls toward a tenth: on the reference frames that is a
/// third more chroma in the mid-tones, and the picture without it
/// reads as faded. So the chroma is scaled in Oklab by that slope
/// ([`chroma_boost`]), which is what the curve per channel gives a
/// color near grey, and holds the hue; the cube's compression after it
/// takes care of what the scale pushes out.
pub fn tone_norm(c: [f32; 3]) -> [f32; 3] {
    if c.iter().any(|v| v.is_nan()) {
        return [1.0; 3];
    }
    let c = c.map(|v| v.max(0.0));
    let norm = (c[0] + c[1] + c[2]) / 3.0;
    if norm >= DISPLAY_WHITE {
        return [1.0; 3];
    }
    let grey = tone(norm);
    if grey >= 1.0 {
        return [1.0; 3];
    }
    if grey <= 0.0 {
        return [0.0; 3];
    }
    let gain = grey / norm;
    let p = c.map(|v| v * gain);
    let ok = &*OKLAB;
    let lab = ok.of(p);
    let back = ok.to_rgb(lab);
    let boost = chroma_boost(p, grey, norm);
    let boosted = ok.to_rgb([lab[0], boost * lab[1], boost * lab[2]]);
    let kept = chroma_kept([0, 1, 2].map(|k| p[k] + (boosted[k] - back[k])), grey);
    let light = ok.of([grey; 3])[0];
    let stepped = ok.to_rgb([
        light + kept * (lab[0] - light),
        kept * boost * lab[1],
        kept * boost * lab[2],
    ]);
    // Added to the color as the step's change, the round trip's own
    // rounding taken off: what the step moves is the difference of two
    // round trips that round alike, so a color the step leaves alone
    // comes back as it went in. What rounding is left on a channel at
    // zero beside a lit one, a billionth or so of either sign and a
    // different one on the GPU, is held to zero, as the Oklab pass
    // holds its own (`snap_residue`): the curves after this make
    // whole levels of it on one side only.
    crate::finish::snap_residue([0, 1, 2].map(|k| p[k] + (stepped[k] - back[k])))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MID_GREY: f32 = 0.18;

    fn hue(c: [f32; 3]) -> (f32, f32) {
        let lab = OKLAB.of(c);
        (lab[2].atan2(lab[1]).to_degrees(), lab[1].hypot(lab[2]))
    }

    fn turn(a: f32, b: f32) -> f32 {
        (a - b + 180.0).rem_euclid(360.0) - 180.0
    }

    /// The neutral axis is today's curve: what the feel track's
    /// baseline (§141), the medians against Lightroom (§143) and the
    /// shoulder reaching white at the sensor's clip (§145) were all
    /// measured on. Swept from eight stops under mid grey to past
    /// white, a neutral through the norm is `tone` to within the Oklab
    /// round trip's rounding, a few millionths, a hundredth of a level
    /// of 8-bit output; white is reached at the same scene value, with
    /// the slope `tone` meets it with, a soft corner and not a hard
    /// one; and the curve rises.
    #[test]
    fn a_neutral_through_the_norm_is_todays_curve() {
        let mut last = -1.0f32;
        for k in 0..=1200 {
            let stops = -8.0 + k as f32 * 0.01;
            let x = MID_GREY * 2f32.powf(stops);
            let want = tone(x);
            let got = tone_norm([x; 3]);
            for v in got {
                assert!(
                    (v - want).abs() <= 4e-6 + 4e-6 * want,
                    "{stops:+.2} stops: {got:?} against {want}"
                );
            }
            assert!(got[1] >= last - 1e-6, "{stops:+.2} stops falls");
            last = got[1];
        }
        // White where today's curve reaches it, and not before.
        assert_eq!(tone_norm([DISPLAY_WHITE; 3]), [1.0; 3]);
        let under = DISPLAY_WHITE * 2f32.powf(-0.05);
        assert!(tone_norm([under; 3])[0] < 0.999);
        // The soft corner: the last twentieth of a stop under white
        // climbs about a hundredth of a display stop per twentieth,
        // today's slope there, not a jump.
        let slope = |f: &dyn Fn(f32) -> f32| {
            (f(DISPLAY_WHITE * 2f32.powf(-0.01)).log2() - f(under).log2()) / 0.04
        };
        let ours = slope(&|x| tone_norm([x; 3])[0]);
        let today = slope(&tone);
        assert!((ours - today).abs() < 1e-3, "{ours} against {today}");
        assert!(ours > 0.05 && ours < 0.2, "{ours}");
        // A NaN or an infinity is white, as `tone` has it.
        assert_eq!(tone_norm([f32::NAN, 0.1, 0.1]), [1.0; 3]);
        assert_eq!(tone_norm([f32::INFINITY, 0.0, 0.0]), [1.0; 3]);
        assert_eq!(tone_norm([0.0; 3]), [0.0; 3]);
    }

    /// A saturated color scaled up from deep shadow to far past the
    /// sensor's clip: each working-space primary and secondary, an
    /// orange, a bright sky and a skin tone. Its Oklab hue holds to a
    /// tenth of a degree all the way, the step toward white included,
    /// for as long as it has chroma enough to have a hue (0.002, a
    /// tenth of Oklab's just noticeable difference of about 0.02).
    /// Once at the cube's white face the step is monotone, the chroma
    /// only falling and the distance to
    /// white in Oklab only shrinking, and the ramp ends at white.
    /// Clipped to the display's cube, as the finish clips it after the
    /// curve, the hue still holds to 0.003 across Oklab's a, b plane,
    /// under a sixth of that difference: the Oklab step leaves
    /// the cube by at most two hundredths, on a bright magenta and
    /// cyan; pulling that back at a constant hue is the gamut
    /// compression's, after the curves. Today's curve turns an orange
    /// by 45 degrees on the way, and a blue with a trace of red and
    /// green in it by 6.
    #[test]
    fn a_saturated_ramp_keeps_its_hue_and_steps_toward_white() {
        let colors: [(&str, [f32; 3]); 9] = [
            ("red", [1.0, 0.0, 0.0]),
            ("green", [0.0, 1.0, 0.0]),
            ("blue", [0.0, 0.0, 1.0]),
            ("cyan", [0.0, 1.0, 1.0]),
            ("magenta", [1.0, 0.0, 0.8]),
            ("yellow", [1.0, 1.0, 0.0]),
            ("orange", [1.0, 0.45, 0.05]),
            ("sky", [0.3, 0.5, 0.9]),
            ("skin", [0.6, 0.4, 0.3]),
        ];
        for (name, c) in colors {
            let (want, _) = hue(c);
            let mut stepping = false;
            let mut peak = 0.0f32;
            let mut last_chroma = f32::INFINITY;
            let mut last_distance = f32::INFINITY;
            let mut worst = 0.0f32;
            let mut clip_worst = 0.0f32;
            for k in 0..=140 {
                let stops = -6.0 + k as f32 * 0.1;
                let x = c.map(|v| v * MID_GREY * 2f32.powf(stops));
                let out = tone_norm(x);
                let (h, chroma) = hue(out);
                if chroma > 0.002 {
                    worst = worst.max(turn(h, want).abs());
                    // The clip's turn as a distance across Oklab's a, b
                    // plane at the clipped chroma.
                    let clipped = out.map(|v| v.clamp(0.0, 1.0));
                    let (hc, cc) = hue(clipped);
                    let across = 2.0 * cc * (turn(hc, want).abs().to_radians() / 2.0).sin();
                    clip_worst = clip_worst.max(across);
                    assert!(
                        across < 0.003,
                        "{name} at {stops:+.1}: clipped hue {hc} against {want}, {across} across"
                    );
                }
                let lab = OKLAB.of(out);
                let distance = (1.0 - lab[0]).hypot(chroma);
                // Once its brightest channel is at the white face and
                // its chroma past its peak, the step has it: from there
                // its chroma only falls and it only nears white. (Under
                // the face the chroma can dip and rise again, as what
                // the slope puts back fades over mid grey.)
                let top = out.iter().fold(0.0f32, |a, &b| a.max(b));
                if top >= 0.95 && chroma < peak - 1e-4 {
                    stepping = true;
                }
                peak = peak.max(chroma);
                if stepping {
                    assert!(
                        chroma <= last_chroma + 1e-4,
                        "{name} at {stops:+.1}: chroma rises in the step, {last_chroma} to {chroma}"
                    );
                    assert!(
                        distance <= last_distance + 1e-4,
                        "{name} at {stops:+.1}: moves away from white, \
                         {last_distance} to {distance}"
                    );
                }
                last_chroma = chroma;
                last_distance = distance;
            }
            assert!(worst < 0.1, "{name}: hue turns {worst} degrees");
            assert!(clip_worst < 0.003, "{name}: clipped, {clip_worst} across");
            assert!(stepping, "{name}: never steps toward white");
            let end = tone_norm(c.map(|v| v * MID_GREY * 2f32.powf(8.0)));
            assert_eq!(end, [1.0; 3], "{name} ends at {end:?}");
        }
        // And the per-channel curve on the same ramps, for the record
        // the doc gives: it turns them, by tens of degrees.
        let turned = |c: [f32; 3]| {
            (0..=140)
                .map(|k| c.map(|v| tone(v * MID_GREY * 2f32.powf(-6.0 + k as f32 * 0.1))))
                .filter(|out| hue(*out).1 > 0.002)
                .map(|out| turn(hue(out).0, hue(c).0).abs())
                .fold(0.0f32, f32::max)
        };
        let orange = turned([1.0, 0.45, 0.05]);
        let blue = turned([0.02, 0.03, 1.0]);
        assert!(orange > 30.0 && blue > 4.0, "{orange}, {blue}");
    }

    /// A color near grey keeps the chroma the curve per channel gives
    /// it, from five stops under mid grey to a stop and a half over,
    /// to a few percent: the slope the norm's chroma is scaled by is
    /// the curve per channel's to first order, more where the curve
    /// per channel lends a color near grey chroma and less where it
    /// takes it away. Without it the norm left a skin
    /// tone at two thirds of the chroma the curve per channel gives it
    /// two stops down.
    #[test]
    fn a_color_near_grey_keeps_the_chroma_of_the_curve_per_channel() {
        for c in [[1.05f32, 1.0, 0.95], [0.97, 1.0, 1.04], [1.0, 1.03, 0.98]] {
            for k in 0..=13 {
                let stops = -5.0 + k as f32 * 0.5;
                let x = c.map(|v| v * MID_GREY * 2f32.powf(stops));
                let ours = hue(tone_norm(x)).1;
                let theirs = hue(x.map(tone)).1;
                assert!(
                    (ours / theirs - 1.0).abs() < 0.04,
                    "{c:?} at {stops:+.1}: chroma {ours} against {theirs}"
                );
            }
        }
    }

    /// The slope the chroma is scaled by is the curve's own, in stops:
    /// against a central difference of `tone` across a fiftieth of a
    /// stop, from eight stops under mid grey to just under white.
    #[test]
    fn the_slope_is_the_curves() {
        let mut worst = 0.0f32;
        for k in 0..=110 {
            let stops = -8.0 + k as f32 * 0.1;
            let x = MID_GREY * 2f32.powf(stops);
            if x * 2f32.powf(0.01) >= DISPLAY_WHITE {
                break;
            }
            let h = 0.01;
            let numeric = (tone(x * 2f32.powf(h)).ln() - tone(x * 2f32.powf(-h)).ln())
                / (2.0 * h * std::f32::consts::LN_2);
            worst = worst.max((tone_slope(x) - numeric).abs());
        }
        assert!(worst < 2e-3, "{worst}");
        assert_eq!(tone_slope(DISPLAY_WHITE), 0.0);
    }

    /// The chroma kept is the module's: its formula transcribed as
    /// darktable writes it, epsilons and all, in double precision,
    /// agrees with the reciprocal form to a ten-thousandth, and to what
    /// its own epsilon over the grey moves it by, over colors
    /// from near neutral to past the cube's faces and from deep shadow
    /// to near white; and at the faces the reciprocal form is exact,
    /// where the module's is a millionth short.
    #[test]
    fn the_chroma_kept_is_darktables() {
        fn darktable(p: [f64; 3], grey: f64) -> f64 {
            let e = 1e-6;
            let lo = p[0].min(p[1]).min(p[2]);
            let hi = p[0].max(p[1]).max(p[2]);
            let white = (1.0 - grey) / (hi - grey + e);
            let black = (0.0 - grey) / (lo - grey - e);
            let room = white.min(black);
            let chroma = (grey - lo) / (grey + e);
            let adjust = 1.0 / (chroma * room + e);
            let h = 2.0 * chroma / (1.0 - chroma * chroma + e) * adjust;
            h / (1.0 + (h * h + 1.0).sqrt()) * room
        }
        let mut worst = 0.0f64;
        for grey in [0.01f32, 0.05, 0.18, 0.4, 0.7, 0.9, 0.98] {
            for spread in [0.02f32, 0.1, 0.3, 0.6, 0.9, 1.0, 1.5, 3.0] {
                for shape in [[1.0f32, -0.5, -0.5], [1.0, 0.2, -1.2], [-1.0, 0.4, 0.6]] {
                    let p = shape.map(|v| (grey * (1.0 + spread * v)).max(0.0));
                    // Back to the mean `grey`, as the norm's gain leaves it.
                    let mean = (p[0] + p[1] + p[2]) / 3.0;
                    let p = p.map(|v| v * grey / mean);
                    let ours = f64::from(chroma_kept(p, grey));
                    let theirs = darktable(p.map(f64::from), f64::from(grey));
                    // The module's epsilon over the grey is what it is off by.
                    let off = (ours - theirs).abs() / (1e-4 + 4e-6 / f64::from(grey));
                    worst = worst.max(off);
                }
            }
        }
        assert!(worst < 1.0, "{worst}");
        // A primary inside the cube: kept whole, exactly.
        assert_eq!(chroma_kept([0.0, 0.0, 0.6], 0.2), 1.0);
        // A primary past the white face: brought back onto it.
        let (p, grey) = ([0.0, 0.0, 1.5], 0.5);
        let k = chroma_kept(p, grey);
        assert!((grey + k * (p[2] - grey) - 1.0).abs() < 1e-6, "{k}");
        // A neutral keeps what it has, which is nothing.
        assert_eq!(chroma_kept([0.3; 3], 0.3), 1.0);
    }
}
