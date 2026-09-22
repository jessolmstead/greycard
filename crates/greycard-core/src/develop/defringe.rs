//! Defringe: axial chromatic aberration and purple fringing, the
//! color a fast lens smears along a high-contrast edge that no radial
//! shift can undo (§13's lateral correction is the other tool).
//!
//! A port of RawTherapee's `PF_correct_RT` in `rtengine/PF_correct_RT.cc`,
//! copyright 2008-2010 Emil Martinec <ejmartin@uchicago.edu>, optimized
//! September 2013 and February 2018 by Ingo Weyrich, GPL-3.0-or-later.
//!
//! The reference works in CIELAB: blur `a` and `b` with a Gaussian of
//! the radius, take each pixel's squared distance from that local mean
//! as its chroma deviation, and where the deviation is more than a
//! threshold times the frame's mean deviation, replace `a` and `b`
//! with the average of a window weighted by `1 / (deviation + mean)`,
//! so neighbors that are not fringing carry the average. Lightness is
//! never touched.
//!
//! Differences here. The chroma is Oklab's `a` and `b` of the linear
//! Rec.2020 working space ([`crate::color::Oklab`]), which is the space
//! the mixer and the grading already read a hue in, rather than a
//! CIELAB the engine does not have. The scale of `a` and `b` does not
//! matter to any of it: every step is a ratio of chroma deviations to
//! their own mean, and the weights are one over a deviation plus that
//! mean, so a uniform factor cancels out of the test and the weighted
//! average alike. Measured, the factor from CIELAB to Oklab is uniform
//! in lightness (about 1.4, the same from a linear 0.02 to 4.0) but not
//! in hue: for the same perturbation of a color, the ratio of the two
//! spaces' deviations varies by about 2.9 over the hue circle, blue and
//! violet ranking higher than yellow and green. So one threshold here
//! does not pick quite the set of pixels it would pick in RawTherapee:
//! it leans toward the blues and violets, which for purple fringing is
//! the direction to lean, and 13 is still a sensible default, but the
//! two are not the same selection.
//!
//! The reference's hue curve is here as two windows. RawTherapee lets
//! a user draw a curve over the hue circle saying how much of the pass
//! each hue gets, and ships one that is up over the purples and down
//! everywhere else; the port left the curve out and acted on every
//! hue, which makes a red berry on a grey wall a fringe. A curve is
//! more than the tool needs: axial aberration puts one color in front
//! of the focus and its complement behind, so two windows — purple and
//! green, each a center, a width and an amount — say the same thing
//! with three numbers a slider can hold and a dropper can set. The hue
//! read is not the pixel's own: it is the direction of its chroma
//! deviation, `atan2(b - mean_b, a - mean_a)`, which is the quantity
//! the threshold already measures the length of.

use rayon::prelude::*;

use crate::color::Oklab;
use crate::develop::dual::gaussian_blur;
use crate::image::WorkingImage;

/// RawTherapee's defaults: `procparams.cc`.
pub const DEFAULT_RADIUS: f32 = 2.0;
pub const DEFAULT_THRESHOLD: f32 = 13.0;
/// The range the sliders offer, RawTherapee's.
pub const MIN_RADIUS: f32 = 0.5;
pub const MAX_RADIUS: f32 = 5.0;
pub const MAX_THRESHOLD: f32 = 100.0;

/// Where the purple window sits by default, degrees of Oklab hue.
/// Measured on the chroma deviation of two fringed frames: the violet
/// side of the orchids' corner fringe runs 280 to 300, the magenta
/// rims on the lighthouse frame's water 310 to 340. At the default
/// width the plateau is 280 to 340, which is both of them whole.
pub const DEFAULT_PURPLE_CENTER: f32 = 310.0;
/// And the green one: the yellow-green side of the same two, the
/// orchids' corner fringe at 100 to 110 and the lighthouse's at 110 to
/// 120. The plateau at the default width is 100 to 160, which holds
/// all of that, and the oranges at 80 come out under three tenths.
/// It lands exactly opposite the purple one, which is what an axial
/// aberration ought to do: one color in front of the focus and its
/// complement behind.
pub const DEFAULT_GREEN_CENTER: f32 = 130.0;
/// The default width of both, degrees: at this width the plateau runs
/// 30 degrees either side of the center and the falloff the 30 beyond
/// that, which puts every fringe bin measured on the two frames inside
/// the plateau and the reds and oranges (5 to 70) and the cyans (190
/// to 245) outside the windows entirely.
pub const DEFAULT_WINDOW_WIDTH: f32 = 120.0;
/// A window this wide is the whole circle: its falloff has nowhere to
/// go, so every hue gets the amount.
pub const FULL_CIRCLE: f32 = 360.0;

/// One window over the hue of a pixel's chroma deviation: which
/// fringes the pass acts on, and how much of it they get.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HueWindow {
    /// The hue at the middle of the window, degrees, 0 at `+a` and
    /// rising through green at 130 and purple at 310. Read modulo the
    /// circle, so -20 and 340 are the same window.
    pub center: f32,
    /// How much of the circle the window spans, degrees: full strength
    /// over a plateau in the middle, then a smooth falloff to nothing
    /// at the edge. The falloff is a quarter of the width each side
    /// until the window is wide enough that the rest of the circle
    /// cannot hold it, and from there it is what is left, so the
    /// window grows into [`FULL_CIRCLE`], every hue, without a step.
    pub width: f32,
    /// How much of the pass a deviation inside gets, 0 to 1. At 1 the
    /// chroma is replaced outright, as the ungated pass did; below it
    /// the replacement is mixed with what was there. Zero is off.
    pub amount: f32,
}

impl HueWindow {
    /// Every hue, at full strength: the pass as it was before the
    /// windows, and what a caller asks for to get it back.
    pub const ALL_HUES: Self = Self {
        center: 0.0,
        width: FULL_CIRCLE,
        amount: 1.0,
    };

    /// Off.
    pub const NONE: Self = Self {
        center: 0.0,
        width: 0.0,
        amount: 0.0,
    };

    /// How much of the pass this window gives a deviation pointing at
    /// `hue` degrees: the amount over the plateau, falling smoothly to
    /// nothing at the edge, and nothing beyond it.
    ///
    /// The falloff is a quarter of the width each side, but never more
    /// than half of what is left of the circle outside the window, so
    /// that a window opened all the way meets every hue rather than
    /// stepping to it: at 240 degrees both readings are 60, past that
    /// the plateau grows and the falloff shrinks, and at 360 the
    /// falloff is nothing and the plateau is the circle.
    pub fn weight(&self, hue: f32) -> f32 {
        let amount = self.amount.clamp(0.0, 1.0);
        let width = self.width.clamp(0.0, FULL_CIRCLE);
        if amount <= 0.0 || width <= 0.0 {
            return 0.0;
        }
        let half = width * 0.5;
        let falloff = (width * 0.25).min((FULL_CIRCLE - width) * 0.5);
        let d = hue_distance(hue, self.center);
        // The plateau first: at the full circle it is the whole of it,
        // and the edge is the same place.
        if d <= half - falloff {
            return amount;
        }
        if d >= half {
            return 0.0;
        }
        amount * smooth((half - d) / falloff)
    }
}

/// The hue of a chroma deviation, degrees in 0..360, 360 itself folded
/// back to 0: `rem_euclid` rounds a hair below zero up to the whole
/// circle, and a hue of 360 would read as outside a window centered on
/// nothing.
pub fn hue_of(a: f32, b: f32) -> f32 {
    let h = b.atan2(a).to_degrees().rem_euclid(FULL_CIRCLE);
    if h >= FULL_CIRCLE { 0.0 } else { h }
}

/// How far apart two hues are around the circle, degrees in 0..180.
pub fn hue_distance(x: f32, y: f32) -> f32 {
    let d = (x - y).rem_euclid(FULL_CIRCLE);
    d.min(FULL_CIRCLE - d)
}

/// Which of the two hue windows a picked hue belongs to: the one
/// whose center is nearer around the circle, the purple one where
/// they are the same distance away. True for purple.
///
/// The dropper's whole decision. A hue is on a circle, so the nearer
/// center is not the nearer number: 20 degrees is nearer 310 than it
/// is 130.
pub fn nearer_window(hue: f32, purple: f32, green: f32) -> bool {
    hue_distance(hue, purple) <= hue_distance(hue, green)
}

/// The hue a dropper reads: the direction from a neighborhood's mean
/// color to the pixel sitting in it, both working-space, which is the
/// deviation the pass gates on. `None` where the two are the same
/// color to within `floor` and the direction means nothing.
pub fn deviation_hue(pixel: [f32; 3], local_mean: [f32; 3], floor: f32) -> Option<f32> {
    let ok = Oklab::for_working_space();
    let (p, m) = (ok.of(pixel), ok.of(local_mean));
    let (da, db) = (p[1] - m[1], p[2] - m[2]);
    (hypot(da, db) > floor).then(|| hue_of(da, db))
}

/// Smoothstep on 0..1, for the falloff to a window's edge.
#[inline]
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefringeOptions {
    /// The Gaussian's standard deviation in pixels: how far the local
    /// mean chroma is taken over, and so how wide a fringe can be and
    /// still be seen as one.
    pub radius: f32,
    /// How far past the frame's mean chroma deviation a pixel must sit
    /// to count as fringing, in RawTherapee's scale: the multiplier is
    /// `5 (threshold / 33)²`, so its default of 13 asks for 0.78 times
    /// the mean and 33 asks for five times it. Zero defringes
    /// everything above the mean.
    pub threshold: f32,
    /// The purple window: the violet-to-magenta side of an axial
    /// aberration.
    pub purple: HueWindow,
    /// And the green one, the other side of the same fault.
    pub green: HueWindow,
}

impl Default for DefringeOptions {
    fn default() -> Self {
        Self {
            radius: DEFAULT_RADIUS,
            threshold: DEFAULT_THRESHOLD,
            purple: HueWindow {
                center: DEFAULT_PURPLE_CENTER,
                width: DEFAULT_WINDOW_WIDTH,
                amount: 1.0,
            },
            green: HueWindow {
                center: DEFAULT_GREEN_CENTER,
                width: DEFAULT_WINDOW_WIDTH,
                amount: 1.0,
            },
        }
    }
}

impl DefringeOptions {
    /// How much of the pass a deviation pointing at `hue` gets: the
    /// stronger of the two windows, nothing outside both. The stronger
    /// rather than the sum, so two windows that overlap do not ask for
    /// more of the pass than there is.
    pub fn weight(&self, hue: f32) -> f32 {
        self.purple.weight(hue).max(self.green.weight(hue))
    }

    /// Whether any hue is acted on at all: with both windows shut the
    /// pass has nothing to do and does not run.
    pub fn acts(&self) -> bool {
        self.purple.amount > 0.0 && self.purple.width > 0.0
            || self.green.amount > 0.0 && self.green.width > 0.0
    }
}

/// What a defringe did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefringeStats {
    /// The radius the pass actually ran at: the options' radius held
    /// to [`MIN_RADIUS`]..[`MAX_RADIUS`]. What a caller reports.
    pub radius: f32,
    /// The fraction of the frame whose chroma was moved: past the
    /// threshold and inside a hue window. A pixel a window holds at
    /// less than its full amount counts here as one whose chroma
    /// moved, not as a fraction of one.
    pub fraction: f32,
    /// The fraction past the threshold whatever its hue: what the
    /// pass would have touched with the windows open on every hue,
    /// so a caller can say how much the windows spared.
    pub over_threshold: f32,
    /// The frame's mean squared chroma deviation, Oklab: what the
    /// threshold is a multiple of.
    pub mean_deviation: f32,
    /// The deviation a pixel had to beat.
    pub threshold: f32,
    /// The half-width of the averaging window, pixels.
    pub window: usize,
    /// Mean Oklab chroma of the pixels replaced, as they were: what
    /// the fringe measured before the pass.
    pub chroma_before: f32,
    /// And as they are now.
    pub chroma_after: f32,
}

/// The magic 33 of the reference's threshold scale.
const THRESHOLD_SCALE: f32 = 33.0;

/// Neutralize fringing on `image`, in place. Lightness is untouched;
/// only the chroma of pixels whose color departs from their
/// neighborhood's by more than the threshold, in a direction one of
/// the hue windows holds, is moved.
pub fn defringe(image: &mut WorkingImage, options: &DefringeOptions) -> DefringeStats {
    let (width, height) = (image.width, image.height);
    let n = width * height;
    let sigma = options.radius.clamp(MIN_RADIUS, MAX_RADIUS);
    let half = (2.0 * sigma).ceil() as usize + 1;
    let empty = DefringeStats {
        radius: sigma,
        fraction: 0.0,
        over_threshold: 0.0,
        mean_deviation: 0.0,
        threshold: 0.0,
        window: half,
        chroma_before: 0.0,
        chroma_after: 0.0,
    };
    if n == 0 || width < 2 * half + 1 || height < 2 * half + 1 || !options.acts() {
        return empty;
    }

    let ok = Oklab::for_working_space();
    // The chroma planes. Lightness is not kept: only a pixel that is
    // replaced needs it back, and there are few of those.
    let mut chroma_a = vec![0.0f32; n];
    let mut chroma_b = vec![0.0f32; n];
    chroma_a
        .par_chunks_mut(width)
        .zip(chroma_b.par_chunks_mut(width))
        .zip(image.data.par_chunks(width * 3))
        .for_each(|((da, db), src)| {
            for x in 0..da.len() {
                let lab = ok.of([src[x * 3], src[x * 3 + 1], src[x * 3 + 2]]);
                da[x] = lab[1];
                db[x] = lab[2];
            }
        });

    // Each pixel's chroma less its local mean's, kept signed: the
    // length of it is what the threshold tests, and the direction of
    // it is what the hue windows read.
    let mut da = chroma_a.clone();
    gaussian_blur(&mut da, width, height, sigma);
    da.par_iter_mut()
        .zip(chroma_a.par_iter())
        .for_each(|(m, v)| *m = v - *m);
    let mut db = chroma_b.clone();
    gaussian_blur(&mut db, width, height, sigma);
    db.par_iter_mut()
        .zip(chroma_b.par_iter())
        .for_each(|(m, v)| *m = v - *m);
    let mut deviation: Vec<f32> = da
        .par_iter()
        .zip(db.par_iter())
        .map(|(x, y)| sq(*x) + sq(*y))
        .collect();

    // Double for the sum, as the reference has it: a 45 MP frame is
    // more terms than an f32 accumulator keeps honest.
    let mean = (deviation.par_iter().map(|&v| v as f64).sum::<f64>() / n as f64) as f32;
    if mean <= 0.0 || !mean.is_finite() {
        return empty;
    }

    let threshold = 5.0 * sq(options.threshold.max(0.0) / THRESHOLD_SCALE) * mean;
    // How much of the pass each pixel gets: none below the threshold,
    // none outside both windows, the window's amount inside it. Over
    // the `da` plane, which has said what it had to say by then. The
    // count of what cleared the threshold comes out of the same pass,
    // a row at a time, rather than a second walk of the plane.
    let over = da
        .par_chunks_mut(width)
        .zip(db.par_chunks(width))
        .zip(deviation.par_chunks(width))
        .map(|((row, y), d)| {
            let mut over = 0usize;
            for x in 0..row.len() {
                row[x] = if d[x] > threshold {
                    over += 1;
                    options.weight(hue_of(row[x], y[x]))
                } else {
                    0.0
                };
            }
            over
        })
        .sum::<usize>();
    let over_threshold = over as f32 / n as f32;
    drop(db);
    let gate = da;

    // The weight of a neighbor: a pixel that is itself fringing
    // counts for little, a clean one for the most. The reference
    // inverts the plane in place to save a division per tap.
    deviation
        .par_iter_mut()
        .for_each(|d| *d = 1.0 / (*d + mean));
    let weights = deviation;

    let (replaced, before, after) = image
        .data
        .par_chunks_mut(width * 3)
        .enumerate()
        .map(|(y, row)| {
            let top = y.saturating_sub(half - 1);
            let bottom = (y + half).min(height);
            let (mut count, mut before, mut after) = (0usize, 0.0f64, 0.0f64);
            for x in 0..width {
                let g = gate[y * width + x];
                if g <= 0.0 {
                    continue;
                }
                let left = x.saturating_sub(half - 1);
                let right = (x + half).min(width);
                let (mut atot, mut btot, mut norm) = (0.0f32, 0.0f32, 0.0f32);
                for y1 in top..bottom {
                    let base = y1 * width;
                    for i in base + left..base + right {
                        let w = weights[i];
                        atot += w * chroma_a[i];
                        btot += w * chroma_b[i];
                        norm += w;
                    }
                }
                let lab = ok.of([row[x * 3], row[x * 3 + 1], row[x * 3 + 2]]);
                // At the full amount the neighborhood's chroma
                // replaces the pixel's outright, as the ungated pass
                // did; below it the two are mixed.
                let (na, nb) = if g >= 1.0 {
                    (atot / norm, btot / norm)
                } else {
                    (
                        lab[1] + g * (atot / norm - lab[1]),
                        lab[2] + g * (btot / norm - lab[2]),
                    )
                };
                let rgb = ok.to_rgb([lab[0], na, nb]);
                row[x * 3] = rgb[0];
                row[x * 3 + 1] = rgb[1];
                row[x * 3 + 2] = rgb[2];
                before += hypot(lab[1], lab[2]) as f64;
                after += hypot(na, nb) as f64;
                count += 1;
            }
            (count, before, after)
        })
        .reduce(|| (0, 0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2));

    let mean_of = |total: f64| {
        if replaced == 0 {
            0.0
        } else {
            (total / replaced as f64) as f32
        }
    };
    DefringeStats {
        radius: sigma,
        fraction: replaced as f32 / n as f32,
        over_threshold,
        mean_deviation: mean,
        threshold,
        window: half,
        chroma_before: mean_of(before),
        chroma_after: mean_of(after),
    }
}

#[inline]
fn hypot(a: f32, b: f32) -> f32 {
    (a * a + b * b).sqrt()
}

#[inline]
fn sq(v: f32) -> f32 {
    v * v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Oklab;

    /// The defringe's dropper hands a picked hue to the nearer of the
    /// two windows, around the circle rather than along the numbers.
    #[test]
    fn the_defringe_dropper_picks_the_nearer_window() {
        let (purple, green) = (DEFAULT_PURPLE_CENTER, DEFAULT_GREEN_CENTER);
        assert!(nearer_window(300.0, purple, green));
        assert!(nearer_window(20.0, purple, green), "the circle wraps");
        assert!(!nearer_window(150.0, purple, green));
        assert!(!nearer_window(100.0, purple, green));
        // A hue the same distance from both goes to purple. The two
        // centers sit opposite each other, so the midpoints are at
        // 40 and 220; either side of one of those settles it.
        assert!(nearer_window(220.0, purple, green));
        assert!(nearer_window(40.0, purple, green));
        assert!(!nearer_window(210.0, purple, green));
        assert!(nearer_window(230.0, purple, green));
        assert!(!nearer_window(50.0, purple, green));
        assert!(nearer_window(30.0, purple, green));
        // Windows the user has moved, not the defaults.
        assert!(nearer_window(10.0, 0.0, 180.0));
        assert!(!nearer_window(170.0, 0.0, 180.0));
    }

    /// A working-space color of a given Oklab chroma at a lightness.
    fn from_chroma(l: f32, a: f32, b: f32) -> [f32; 3] {
        Oklab::for_working_space().to_rgb([l, a, b])
    }

    fn chroma_of(image: &WorkingImage, x: usize, y: usize) -> [f32; 2] {
        let i = (y * image.width + x) * 3;
        let lab =
            Oklab::for_working_space().of([image.data[i], image.data[i + 1], image.data[i + 2]]);
        [lab[1], lab[2]]
    }

    fn magnitude(c: [f32; 2]) -> f32 {
        (c[0] * c[0] + c[1] * c[1]).sqrt()
    }

    /// A frame with a neutral vertical edge two thirds of the way
    /// across, a purple fringe on the bright side of it, and a flat
    /// genuinely purple patch in the corner far from any edge.
    fn frame() -> (WorkingImage, [f32; 2]) {
        let (w, h) = (96usize, 64usize);
        let mut image = WorkingImage::new(w, h);
        // Oklab's purple: positive a, negative b.
        let purple = [0.09f32, -0.11f32];
        let edge = 64;
        for y in 0..h {
            for x in 0..w {
                let bright = x >= edge;
                let l = if bright { 0.85 } else { 0.12 };
                let mut c = from_chroma(l, 0.0, 0.0);
                // The fringe: three columns on the bright side.
                if (edge..edge + 3).contains(&x) {
                    c = from_chroma(l, purple[0], purple[1]);
                }
                // The flat purple patch, 16 by 16 in the top left,
                // nowhere near the edge.
                if x < 16 && y < 16 {
                    c = from_chroma(0.5, purple[0], purple[1]);
                }
                let i = (y * w + x) * 3;
                image.data[i..i + 3].copy_from_slice(&c);
            }
        }
        (image, purple)
    }

    #[test]
    fn a_fringe_on_an_edge_goes_and_a_flat_patch_stays() {
        let (mut image, purple) = frame();
        let before = magnitude(chroma_of(&image, 65, 32));
        assert!((before - magnitude(purple)).abs() < 1e-3);
        let stats = defringe(&mut image, &DefringeOptions::default());
        assert!(stats.fraction > 0.0 && stats.fraction < 0.2, "{stats:?}");
        // What the pass reports of itself agrees with the picture. The
        // aggregate is milder than the fringe alone because the purple
        // patch's own border is a chroma edge too and counts here.
        assert!(stats.chroma_after < stats.chroma_before * 0.5, "{stats:?}");
        // The fringe is neutralized: a tenth of what it was.
        for x in 64..67 {
            let after = magnitude(chroma_of(&image, x, 32));
            assert!(after < before * 0.1, "column {x}: {after} of {before}");
        }
        // The flat patch keeps its color, to a thousandth.
        for (x, y) in [(4usize, 4usize), (8, 8), (11, 6)] {
            let c = chroma_of(&image, x, y);
            assert!(
                (c[0] - purple[0]).abs() < 1e-3 && (c[1] - purple[1]).abs() < 1e-3,
                "patch at {x},{y}: {c:?}"
            );
        }
        // And the neutral sides stay neutral.
        assert!(magnitude(chroma_of(&image, 40, 40)) < 1e-4);
        assert!(magnitude(chroma_of(&image, 90, 40)) < 1e-4);
    }

    #[test]
    fn lightness_is_never_touched() {
        let (mut image, _) = frame();
        let ok = Oklab::for_working_space();
        let before: Vec<f32> = image
            .data
            .chunks(3)
            .map(|c| ok.of([c[0], c[1], c[2]])[0])
            .collect();
        defringe(&mut image, &DefringeOptions::default());
        for (i, c) in image.data.chunks(3).enumerate() {
            let l = ok.of([c[0], c[1], c[2]])[0];
            let was = before[i];
            assert!((l - was).abs() < 2e-4, "pixel {i}: {l} was {was}");
        }
    }

    #[test]
    fn a_frame_of_one_color_is_left_alone() {
        let mut image = WorkingImage::new(48, 48);
        for c in image.data.chunks_mut(3) {
            c.copy_from_slice(&from_chroma(0.5, 0.06, -0.08));
        }
        let before = image.data.clone();
        let stats = defringe(&mut image, &DefringeOptions::default());
        assert_eq!(stats.fraction, 0.0);
        assert_eq!(image.data, before);
    }

    #[test]
    fn the_threshold_decides_how_much_is_touched() {
        let (mut low, _) = frame();
        let (mut high, _) = frame();
        let a = defringe(
            &mut low,
            &DefringeOptions {
                threshold: 5.0,
                ..Default::default()
            },
        );
        let b = defringe(
            &mut high,
            &DefringeOptions {
                threshold: 60.0,
                ..Default::default()
            },
        );
        assert!(a.fraction > b.fraction, "{a:?} against {b:?}");
        assert!(b.threshold > a.threshold);
    }

    #[test]
    fn the_radius_reported_is_the_one_that_ran() {
        let (mut image, _) = frame();
        let stats = defringe(
            &mut image,
            &DefringeOptions {
                radius: 100.0,
                ..Default::default()
            },
        );
        assert_eq!(stats.radius, MAX_RADIUS);
        let mut small = WorkingImage::new(4, 4);
        let stats = defringe(
            &mut small,
            &DefringeOptions {
                radius: 0.01,
                ..Default::default()
            },
        );
        assert_eq!(stats.radius, MIN_RADIUS);
    }

    #[test]
    fn a_frame_smaller_than_the_window_is_left_alone() {
        let mut image = WorkingImage::new(4, 4);
        let before = image.data.clone();
        let stats = defringe(&mut image, &DefringeOptions::default());
        assert_eq!(stats.fraction, 0.0);
        assert_eq!(image.data, before);
    }

    /// A frame like [`frame`] but with the fringe in a hue neither
    /// window holds: a red bar on a neutral field, which is the case
    /// the windows exist for.
    fn red_frame() -> (WorkingImage, [f32; 2]) {
        let (w, h) = (96usize, 64usize);
        let mut image = WorkingImage::new(w, h);
        // Oklab's red: both positive, hue about 29 degrees.
        let red = [0.13f32, 0.07f32];
        for y in 0..h {
            for x in 0..w {
                let l = if x >= 64 { 0.85 } else { 0.12 };
                let mut c = from_chroma(l, 0.0, 0.0);
                if (64..67).contains(&x) {
                    c = from_chroma(l, red[0], red[1]);
                }
                let i = (y * w + x) * 3;
                image.data[i..i + 3].copy_from_slice(&c);
            }
        }
        (image, red)
    }

    /// The windows open on every hue at full strength put the pass
    /// back the way it was before they existed. The numbers are the
    /// ungated pass's own, taken off it before the windows went in.
    #[test]
    fn all_hues_at_full_amount_is_the_ungated_pass() {
        let (mut image, _) = frame();
        let stats = defringe(
            &mut image,
            &DefringeOptions {
                purple: HueWindow::ALL_HUES,
                green: HueWindow::NONE,
                ..Default::default()
            },
        );
        assert!((stats.fraction - 0.10091146).abs() < 1e-6, "{stats:?}");
        assert_eq!(stats.fraction, stats.over_threshold);
        assert!(
            (stats.chroma_before - 0.063957065).abs() < 1e-6,
            "{stats:?}"
        );
        assert!((stats.chroma_after - 0.02459148).abs() < 1e-6, "{stats:?}");
        for (x, y, a, b) in [
            (64usize, 32usize, 0.006499738f32, -0.007943988f32),
            (65, 32, 0.007595122, -0.009283066),
            (66, 32, 0.006499588, -0.007943988),
            (63, 32, 0.004880473, -0.005965032),
            (67, 32, 0.004880369, -0.005964816),
            (4, 4, 0.090_000_09, -0.109999985),
        ] {
            let c = chroma_of(&image, x, y);
            assert!(
                (c[0] - a).abs() < 1e-6 && (c[1] - b).abs() < 1e-6,
                "at {x},{y}: {c:?} wanted {a} {b}"
            );
        }
        // Which window says it does not matter, nor where a window
        // that holds the whole circle is centered.
        let (mut other, _) = frame();
        defringe(
            &mut other,
            &DefringeOptions {
                purple: HueWindow::NONE,
                green: HueWindow {
                    center: 77.0,
                    ..HueWindow::ALL_HUES
                },
                ..Default::default()
            },
        );
        assert_eq!(other.data, image.data);
    }

    /// The gate: a purple fringe is a fringe to the purple window and
    /// nothing to the green one, and a red edge is nothing to either.
    #[test]
    fn a_window_gates_the_pass_on_the_hue_of_the_deviation() {
        let (mut image, purple) = frame();
        let before = magnitude(chroma_of(&image, 65, 32));
        let stats = defringe(
            &mut image,
            &DefringeOptions {
                green: HueWindow::NONE,
                ..Default::default()
            },
        );
        assert!(stats.fraction > 0.0);
        assert!(
            magnitude(chroma_of(&image, 65, 32)) < before * 0.1,
            "{stats:?}"
        );

        // The green window alone leaves the purple fringe itself
        // standing; what it does touch is the other side of the same
        // edge, where the deviation points the opposite way.
        let (mut image, _) = frame();
        let stats = defringe(
            &mut image,
            &DefringeOptions {
                purple: HueWindow::NONE,
                ..Default::default()
            },
        );
        let after = chroma_of(&image, 65, 32);
        assert!(
            (after[0] - purple[0]).abs() < 1e-3 && (after[1] - purple[1]).abs() < 1e-3,
            "the green window moved a purple fringe: {after:?}, {stats:?}"
        );

        // And a red bar on a grey field is not a fringe at all, to
        // either window: the ungated pass neutralizes it, the
        // default lets it be.
        let (mut red, color) = red_frame();
        let stats = defringe(&mut red, &DefringeOptions::default());
        assert_eq!(stats.fraction, 0.0, "{stats:?}");
        assert!(stats.over_threshold > 0.0, "nothing cleared the bar");
        for x in 64..67 {
            let c = chroma_of(&red, x, 32);
            assert!(
                (c[0] - color[0]).abs() < 1e-3 && (c[1] - color[1]).abs() < 1e-3,
                "column {x}: {c:?}"
            );
        }
        let (mut red, color) = red_frame();
        defringe(
            &mut red,
            &DefringeOptions {
                purple: HueWindow::ALL_HUES,
                ..Default::default()
            },
        );
        assert!(magnitude(chroma_of(&red, 65, 32)) < magnitude(color) * 0.2);
    }

    /// The falloff: full over the plateau, smoothly down to nothing at
    /// the edge, nothing beyond it, and the circle wraps.
    #[test]
    fn a_windows_weight_falls_smoothly_to_its_edge() {
        let w = HueWindow {
            center: 300.0,
            width: 120.0,
            amount: 1.0,
        };
        // The plateau is the inner half of the half-width: 30 degrees
        // either way of 300.
        assert_eq!(w.weight(300.0), 1.0);
        assert_eq!(w.weight(270.0), 1.0);
        assert_eq!(w.weight(330.0), 1.0);
        // Halfway from the plateau to the edge is half the amount.
        assert!((w.weight(345.0) - 0.5).abs() < 1e-6, "{}", w.weight(345.0));
        assert!((w.weight(255.0) - 0.5).abs() < 1e-6);
        // The edge and past it.
        assert_eq!(w.weight(360.0), 0.0);
        assert_eq!(w.weight(240.0), 0.0);
        assert_eq!(w.weight(30.0), 0.0);
        assert_eq!(w.weight(140.0), 0.0);
        // Monotone from the plateau out.
        let mut last = 1.0;
        for k in 30..=60 {
            let v = w.weight(300.0 + k as f32);
            assert!(v <= last + 1e-6, "{k}: {v} after {last}");
            last = v;
        }
        // The circle wraps: a window at 350 holds 10 as readily as
        // 330, and a center given below zero is the same window.
        let wrapped = HueWindow { center: 350.0, ..w };
        assert_eq!(wrapped.weight(10.0), 1.0);
        assert_eq!(wrapped.weight(330.0), 1.0);
        assert_eq!(
            wrapped.weight(10.0),
            HueWindow { center: -10.0, ..w }.weight(10.0)
        );
        // The amount scales the whole window; zero is off, and a
        // width of nothing is off whatever the amount.
        let half = HueWindow { amount: 0.5, ..w };
        assert_eq!(half.weight(300.0), 0.5);
        assert!((half.weight(345.0) - 0.25).abs() < 1e-6);
        assert_eq!(HueWindow { amount: 0.0, ..w }.weight(300.0), 0.0);
        assert_eq!(HueWindow { width: 0.0, ..w }.weight(300.0), 0.0);
        // Two windows: the stronger of the two, never the sum.
        let options = DefringeOptions {
            purple: w,
            green: HueWindow {
                center: 320.0,
                width: 120.0,
                amount: 0.5,
            },
            ..Default::default()
        };
        assert_eq!(options.weight(300.0), 1.0);
        assert_eq!(options.weight(350.0), 0.5);
        assert_eq!(options.weight(100.0), 0.0);
    }

    /// Both windows shut and the pass does not run.
    #[test]
    fn with_both_amounts_at_zero_nothing_happens() {
        let d = DefringeOptions::default();
        let (mut image, _) = frame();
        let was = image.data.clone();
        let stats = defringe(
            &mut image,
            &DefringeOptions {
                purple: HueWindow {
                    amount: 0.0,
                    ..d.purple
                },
                green: HueWindow {
                    amount: 0.0,
                    ..d.green
                },
                ..d
            },
        );
        assert_eq!(stats.fraction, 0.0);
        assert_eq!(stats.over_threshold, 0.0);
        assert_eq!(image.data, was);
    }

    /// An amount between the ends mixes: half of it moves the fringe
    /// half the way the whole of it does.
    #[test]
    fn an_amount_below_one_mixes_with_what_was_there() {
        let at = |amount: f32| {
            let (mut image, _) = frame();
            defringe(
                &mut image,
                &DefringeOptions {
                    purple: HueWindow {
                        amount,
                        ..DefringeOptions::default().purple
                    },
                    green: HueWindow::NONE,
                    ..Default::default()
                },
            );
            chroma_of(&image, 65, 32)
        };
        let off = chroma_of(&frame().0, 65, 32);
        let (half, full) = (at(0.5), at(1.0));
        let want = [0.5 * (off[0] + full[0]), 0.5 * (off[1] + full[1])];
        assert!(
            (half[0] - want[0]).abs() < 1e-5 && (half[1] - want[1]).abs() < 1e-5,
            "half {half:?} wanted {want:?}"
        );
        assert!(magnitude(full) < magnitude(half) && magnitude(half) < magnitude(off));
    }

    /// What a dropper reads at a fringe, and that the default window
    /// holds it: the pixel against the mean of the neighborhood the
    /// pass takes its local mean over.
    #[test]
    fn a_dropper_reads_the_fringes_hue() {
        let (image, _) = frame();
        // The viewport's sample is the mean of a box, both times: the
        // droppers' own two pixels each way for the point, and a wider
        // one for the neighborhood. Both are flat boxes where the pass
        // takes a Gaussian, and the hue survives that.
        let box_mean = |image: &WorkingImage, cx: usize, cy: usize, reach: usize| {
            let mut sum = [0.0f64; 3];
            let mut n = 0.0f64;
            for y in cy - reach..=cy + reach {
                for x in cx - reach..=cx + reach {
                    let i = (y * image.width + x) * 3;
                    for (k, s) in sum.iter_mut().enumerate() {
                        *s += image.data[i + k] as f64;
                    }
                    n += 1.0;
                }
            }
            sum.map(|v| (v / n) as f32)
        };
        let point = box_mean(&image, 65, 32, 2);
        let hue =
            deviation_hue(point, box_mean(&image, 65, 32, 5), 1e-4).expect("a deviation to read");
        // The synthetic fringe is Oklab (0.09, -0.11), hue 309.3, and
        // the deviation points the same way.
        assert!(hue_distance(hue, 309.3) < 5.0, "{hue}");
        let d = DefringeOptions::default();
        assert_eq!(d.weight(hue), 1.0, "the default purple window missed {hue}");
        assert!(hue_distance(hue, DEFAULT_PURPLE_CENTER) < hue_distance(hue, DEFAULT_GREEN_CENTER));
        // A flat patch has no deviation to read.
        assert!(deviation_hue(point, point, 1e-4).is_none());
        // And the red bar reads as red, which neither window holds.
        let (red, _) = red_frame();
        let hue = deviation_hue(box_mean(&red, 65, 32, 2), box_mean(&red, 65, 32, 5), 1e-4)
            .expect("a deviation to read");
        assert!(hue_distance(hue, 29.0) < 15.0, "{hue}");
        assert_eq!(d.weight(hue), 0.0, "{hue}");
    }

    /// The defaults are the two windows, on, at full strength: an
    /// edit that says "defringe" gets RawTherapee's hue curve, not
    /// the pass on every hue.
    #[test]
    fn the_defaults_are_the_two_measured_windows() {
        let d = DefringeOptions::default();
        assert!(d.acts());
        assert_eq!(d.purple.center, DEFAULT_PURPLE_CENTER);
        assert_eq!(d.green.center, DEFAULT_GREEN_CENTER);
        assert_eq!(d.purple.width, DEFAULT_WINDOW_WIDTH);
        assert_eq!(d.green.width, DEFAULT_WINDOW_WIDTH);
        assert_eq!((d.purple.amount, d.green.amount), (1.0, 1.0));
        // Every bin the two sample frames put a fringe in gets the
        // whole pass: the orchids' violet at 280 to 300 and its
        // yellow-green at 100 to 110, the lighthouse's magenta at 320
        // to 340 and its green at 110 to 120.
        for hue in [280.0, 290.0, 300.0, 310.0, 320.0, 330.0, 340.0] {
            assert_eq!(d.weight(hue), 1.0, "purple missed {hue}");
        }
        for hue in [100.0, 110.0, 120.0, 130.0, 150.0, 160.0] {
            assert_eq!(d.weight(hue), 1.0, "green missed {hue}");
        }
        // The two windows sit opposite each other, as the two sides
        // of one aberration should.
        assert_eq!(hue_distance(d.purple.center, d.green.center), 180.0);
        // The oranges at 80 get under three tenths, and the reds and
        // the cyans nothing at all.
        assert!(d.weight(80.0) < 0.3, "{}", d.weight(80.0));
        for hue in [10.0, 29.0, 50.0, 70.0, 190.0, 209.0, 220.0, 250.0] {
            assert_eq!(d.weight(hue), 0.0, "{hue}");
        }
        // The far side of the lighthouse's sparkles, at 180, is the
        // one measured bin the windows only part-hold.
        assert!((d.weight(180.0) - 0.26).abs() < 0.01, "{}", d.weight(180.0));
    }

    /// A window opened all the way meets every hue rather than
    /// stepping to it: the last notch of the width slider used to
    /// hand the far half of the circle nothing at 355 and everything
    /// at 360.
    #[test]
    fn a_window_grows_into_the_whole_circle_without_a_step() {
        let at = |width: f32, hue: f32| {
            HueWindow {
                center: 0.0,
                width,
                amount: 1.0,
            }
            .weight(hue)
        };
        // The hue the old cliff was about: two thirds of the way
        // round from the center, at a width of 355. The falloff used
        // to start at 89 degrees out and leave it a fiftieth of the
        // pass; now it is inside the plateau.
        assert_eq!(at(355.0, 170.0), 1.0);
        assert_eq!(at(360.0, 170.0), 1.0);
        // How much of the pass the whole circle gets, summed over the
        // hues, moves smoothly with the width: no width of the slider
        // is a step. (It is 0.75 of the width up to 240 and 1.5 of it
        // less 180 from there, meeting at 180 and reaching 360.)
        let covered = |width: f32| {
            (0..720)
                .map(|k| at(width, k as f32 * 0.5) * 0.5)
                .sum::<f32>()
        };
        let mut last = covered(1.0);
        for w in 2..=360 {
            let now = covered(w as f32);
            assert!(
                now >= last && now - last < 2.0,
                "width {w}: {last} to {now}"
            );
            last = now;
        }
        assert!((covered(360.0) - 360.0).abs() < 0.5, "{}", covered(360.0));
        assert!((covered(240.0) - 180.0).abs() < 0.5, "{}", covered(240.0));
        // Up to 240 the falloff is still a quarter of the width each
        // side, so nothing about the settings anyone uses changed.
        for w in [60.0f32, 120.0, 180.0, 240.0] {
            assert_eq!(at(w, w * 0.25), 1.0);
            assert_eq!(at(w, w * 0.5), 0.0);
            assert!((at(w, w * 0.375) - 0.5).abs() < 1e-6);
        }
    }

    /// Where a frame's fringe hues actually sit: what the default
    /// windows were measured with. `GREYCARD_FRINGE_FRAME` a raw file,
    /// `GREYCARD_FRINGE_BOX` an optional `x,y,w,h` to look at instead
    /// of the whole frame. Prints a histogram of the chroma
    /// deviation's hue, in tens of degrees, weighted by the length of
    /// the deviation.
    #[test]
    #[ignore]
    fn measure_the_fringe_hues() {
        let path = std::env::var("GREYCARD_FRINGE_FRAME")
            .expect("GREYCARD_FRINGE_FRAME: a raw file to measure the fringe hues on");
        let frame = crate::decode::decode_path(&path).expect("decoding the frame");
        let image = crate::develop::develop(&frame, &Default::default())
            .expect("developing the frame")
            .image;
        let (width, height) = (image.width, image.height);
        let n = width * height;
        let ok = Oklab::for_working_space();
        let mut da = vec![0.0f32; n];
        let mut db = vec![0.0f32; n];
        for i in 0..n {
            let lab = ok.of([
                image.data[i * 3],
                image.data[i * 3 + 1],
                image.data[i * 3 + 2],
            ]);
            da[i] = lab[1];
            db[i] = lab[2];
        }
        let (ca, cb) = (da.clone(), db.clone());
        for (plane, chroma) in [(&mut da, &ca), (&mut db, &cb)] {
            gaussian_blur(plane, width, height, DEFAULT_RADIUS);
            for i in 0..n {
                plane[i] = chroma[i] - plane[i];
            }
        }
        let dev: Vec<f32> = (0..n).map(|i| sq(da[i]) + sq(db[i])).collect();
        let mean = (dev.iter().map(|&v| v as f64).sum::<f64>() / n as f64) as f32;
        let threshold = 5.0 * sq(DEFAULT_THRESHOLD / THRESHOLD_SCALE) * mean;
        println!("{path}: {width}x{height}, mean deviation {mean:.3e}");

        let mut which: Vec<usize> = match std::env::var("GREYCARD_FRINGE_BOX") {
            Ok(v) => {
                let p: Vec<usize> = v.split(',').map(|t| t.trim().parse().unwrap()).collect();
                let mut out = Vec::new();
                for y in p[1]..(p[1] + p[3]).min(height) {
                    for x in p[0]..(p[0] + p[2]).min(width) {
                        out.push(y * width + x);
                    }
                }
                println!("-- box {v}");
                out
            }
            Err(_) => {
                println!("-- the {:.2}% of the frame over the threshold", {
                    dev.iter().filter(|&&d| d > threshold).count() as f64 / n as f64 * 100.0
                });
                (0..n).filter(|&i| dev[i] > threshold).collect()
            }
        };
        which.sort_by(|&a, &b| dev[b].partial_cmp(&dev[a]).unwrap());
        for (name, take) in [
            ("all of them", which.len()),
            ("the worst tenth", which.len() / 10),
            ("the worst hundredth", which.len() / 100),
        ] {
            let mut bins = [0.0f64; 36];
            for &i in which.iter().take(take) {
                bins[(hue_of(da[i], db[i]) / 10.0) as usize % 36] += dev[i].sqrt() as f64;
            }
            let total: f64 = bins.iter().sum();
            let mut order: Vec<usize> = (0..36).collect();
            order.sort_by(|&a, &b| bins[b].partial_cmp(&bins[a]).unwrap());
            println!(
                "   {name} ({take} px): peaks {:?}",
                order
                    .iter()
                    .take(4)
                    .map(|&k| (k * 10, (bins[k] / total * 1000.0).round() / 10.0))
                    .collect::<Vec<_>>()
            );
            for (k, bin) in bins.iter().enumerate() {
                print!("{:>3}:{:>5.1} ", k * 10, bin / total * 100.0);
                if k % 12 == 11 {
                    println!();
                }
            }
        }
    }
}
