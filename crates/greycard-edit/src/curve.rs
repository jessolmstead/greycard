//! Point curves: a master curve on all three channels and one per
//! channel, each a monotone cubic through its points, applied to the
//! encoded working-space picture after the tone curve; a parametric
//! curve before them, four region amounts and three split points,
//! Lightroom's model; and two color curves against lightness, red
//! against green and blue against yellow, a shift of Oklab's a and b
//! by L, applied after them.

use serde::{Deserialize, Serialize};

use crate::grading::Grading;

/// A point on a curve, x and y in 0 to 1: of the encoded value for
/// the point curves; for a color curve, x the Oklab lightness and y
/// the shift about a neutral middle.
pub type Point = [f32; 2];

pub const LUT_SIZE: usize = 256;

/// How far a color curve at its top or bottom moves Oklab's a or b:
/// half this either way of the middle. Full deflection is a strong
/// cast, about the chroma of a saturated display primary.
pub const COLOR_RANGE: f32 = 0.4;

/// The tables a curve set bakes to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveLut {
    /// For each of 256 encoded inputs, the red, green and blue outputs
    /// with the parametric and the master curve composed in, and the
    /// parametric then the master alone, no channel's own curve, in
    /// the fourth place.
    pub tone: [[f32; 4]; LUT_SIZE],
    /// For each of 256 lightnesses, the shift of a and of b.
    pub color: [[f32; 2]; LUT_SIZE],
    /// Any shift at all, so a finish can skip the round trip to Oklab.
    pub shaded: bool,
}

/// Which curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Rgb,
    Red,
    Green,
    Blue,
    /// Oklab's a against lightness: up is red, down is green.
    RedGreen,
    /// Oklab's b against lightness: up is yellow, down is blue.
    BlueYellow,
}

impl Channel {
    pub const ALL: [Channel; 6] = [
        Channel::Rgb,
        Channel::Red,
        Channel::Green,
        Channel::Blue,
        Channel::RedGreen,
        Channel::BlueYellow,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Channel::Rgb => "RGB",
            Channel::Red => "Red",
            Channel::Green => "Green",
            Channel::Blue => "Blue",
            Channel::RedGreen => "R/G",
            Channel::BlueYellow => "B/Y",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// A color curve, against lightness, rather than a point curve.
    pub fn is_color(self) -> bool {
        matches!(self, Channel::RedGreen | Channel::BlueYellow)
    }

    /// The curve that does nothing on this channel.
    pub fn identity(self) -> Vec<Point> {
        if self.is_color() { flat() } else { identity() }
    }
}

/// How close, in curve units, a press has to come to a point or a
/// split to take hold of it: an editor a couple of hundred pixels
/// square, so a twentieth of it is a comfortable target.
pub const HIT: f32 = 0.05;

/// How close two points' x may come. Nearer than this and the curve
/// between them is a cliff, and neither can be pressed apart from the
/// other.
pub const MIN_GAP: f32 = 0.01;

/// The point of `points` within reach of (x, y), the nearest first
/// where two are in reach.
pub fn nearest_point(points: &[Point], x: f32, y: f32) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(i, p)| (i, ((p[0] - x).powi(2) + (p[1] - y).powi(2)).sqrt()))
        .filter(|(_, d)| *d < HIT)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// How close a split point may come to its neighbor, or to an end of
/// the axis.
pub const SPLIT_GAP: f32 = 0.05;

/// How far a region's node moves at full amount, as a fraction of the
/// region's width. Under a third, so the curve's slope never falls
/// under a tenth: between two nodes the offset changes by at most
/// `REACH` times the two widths, over half their sum, and the
/// smoothstep's slope peaks at one and a half times the secant's.
pub const REACH: f32 = 0.3;

/// The parametric curve: a smooth curve on the master channel from
/// four region amounts and three split points, Lightroom's model.
/// Each region's node sits at its middle and is lifted or lowered by
/// its amount times `REACH` times the region's width; the curve is
/// the diagonal plus that offset, blended with a smoothstep between
/// neighboring nodes and falling to nothing at the ends. A node's
/// amount reaches its neighbors' middles and no further, and the
/// blend is a mix of the two, so a lift never dips on its way out.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Parametric {
    /// The four amounts, -1 to 1: above the highlight split, between
    /// the midtone and highlight splits, between the shadow and
    /// midtone splits, below the shadow split.
    pub highlights: f32,
    pub lights: f32,
    pub darks: f32,
    pub shadows: f32,
    /// The shadow, midtone and highlight splits, fractions of the
    /// encoded axis, in order with `SPLIT_GAP` between them.
    pub splits: [f32; 3],
}

impl Default for Parametric {
    fn default() -> Self {
        Self {
            highlights: 0.0,
            lights: 0.0,
            darks: 0.0,
            shadows: 0.0,
            splits: Self::DEFAULT_SPLITS,
        }
    }
}

impl Parametric {
    pub const DEFAULT_SPLITS: [f32; 3] = [0.25, 0.5, 0.75];

    /// The amounts in the nodes' order, from the shadows up.
    pub fn amounts(&self) -> [f32; 4] {
        [self.shadows, self.darks, self.lights, self.highlights]
    }

    pub fn is_identity(&self) -> bool {
        self.amounts().iter().all(|a| a.abs() < 1e-6)
    }

    /// The split within reach of `x`, the nearest first where two
    /// are in reach, indexed as [`Parametric::ordered_splits`] has
    /// them and so as [`Parametric::set_split`] wants them.
    pub fn nearest_split(&self, x: f32) -> Option<usize> {
        self.ordered_splits()
            .iter()
            .enumerate()
            .map(|(i, s)| (i, (s - x).abs()))
            .filter(|(_, d)| *d < HIT)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// The splits in order with the gap kept, whatever the sidecar
    /// says: each clamped between its neighbors and the ends.
    pub fn ordered_splits(&self) -> [f32; 3] {
        let mut s = self.splits.map(|v| if v.is_finite() { v } else { 0.5 });
        s[0] = between(s[0], SPLIT_GAP, 1.0 - 3.0 * SPLIT_GAP);
        s[1] = between(s[1], s[0] + SPLIT_GAP, 1.0 - 2.0 * SPLIT_GAP);
        s[2] = between(s[2], s[1] + SPLIT_GAP, 1.0 - SPLIT_GAP);
        s
    }

    /// Move one split, kept between its neighbors and the ends.
    pub fn set_split(&mut self, index: usize, x: f32) {
        let s = self.ordered_splits();
        let lo = if index == 0 { 0.0 } else { s[index - 1] } + SPLIT_GAP;
        let hi = if index == 2 { 1.0 } else { s[index + 1] } - SPLIT_GAP;
        self.splits = s;
        self.splits[index] = between(x, lo, hi);
    }

    /// The six nodes the curve goes through: the ends, and one at the
    /// middle of each region, moved by its amount.
    pub fn nodes(&self) -> [Point; 6] {
        let s = self.ordered_splits();
        let edges = [0.0, s[0], s[1], s[2], 1.0];
        let amounts = self.amounts();
        let mut nodes = [[0.0f32; 2]; 6];
        nodes[5] = [1.0, 1.0];
        for i in 0..4 {
            let (lo, hi) = (edges[i], edges[i + 1]);
            let mid = (lo + hi) / 2.0;
            let amount = if amounts[i].is_finite() {
                amounts[i].clamp(-1.0, 1.0)
            } else {
                0.0
            };
            nodes[i + 1] = [mid, mid + amount * REACH * (hi - lo)];
        }
        nodes
    }

    /// The curve's value at `x`.
    pub fn at(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        if self.is_identity() {
            return x;
        }
        let nodes = self.nodes();
        let k = (1..nodes.len())
            .find(|&k| x < nodes[k][0])
            .unwrap_or(nodes.len())
            - 1;
        if k + 1 >= nodes.len() {
            return 1.0;
        }
        let (a, b) = (nodes[k], nodes[k + 1]);
        let t = (x - a[0]) / (b[0] - a[0]).max(1e-6);
        let s = t * t * (3.0 - 2.0 * t);
        let offset = (a[1] - a[0]) * (1.0 - s) + (b[1] - b[0]) * s;
        (x + offset).clamp(0.0, 1.0)
    }
}

/// `v` held between `lo` and `hi`; `hi` when rounding puts `lo` past
/// it, where `clamp` would panic.
fn between(v: f32, lo: f32, hi: f32) -> f32 {
    v.max(lo).min(hi)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Curves {
    pub enabled: bool,
    /// The parametric curve, on the master channel before the point
    /// curves.
    pub parametric: Parametric,
    /// The master curve, on every channel after its own.
    pub rgb: Vec<Point>,
    pub red: Vec<Point>,
    pub green: Vec<Point>,
    pub blue: Vec<Point>,
    /// Red against green by lightness, the neutral at y = 0.5.
    pub red_green: Vec<Point>,
    /// Blue against yellow by lightness, the neutral at y = 0.5.
    pub blue_yellow: Vec<Point>,
}

impl Default for Curves {
    fn default() -> Self {
        Self {
            enabled: true,
            parametric: Parametric::default(),
            rgb: identity(),
            red: identity(),
            green: identity(),
            blue: identity(),
            red_green: flat(),
            blue_yellow: flat(),
        }
    }
}

/// The straight line: the point curve that does nothing.
pub fn identity() -> Vec<Point> {
    vec![[0.0, 0.0], [1.0, 1.0]]
}

/// The level line through the middle: the color curve that does
/// nothing.
pub fn flat() -> Vec<Point> {
    vec![[0.0, 0.5], [1.0, 0.5]]
}

impl Curves {
    pub fn is_identity(&self) -> bool {
        !self.enabled
            || (self.parametric.is_identity()
                && [&self.rgb, &self.red, &self.green, &self.blue]
                    .iter()
                    .all(|c| is_straight(c))
                && is_flat(&self.red_green)
                && is_flat(&self.blue_yellow))
    }

    pub fn channel(&self, channel: Channel) -> &Vec<Point> {
        match channel {
            Channel::Rgb => &self.rgb,
            Channel::Red => &self.red,
            Channel::Green => &self.green,
            Channel::Blue => &self.blue,
            Channel::RedGreen => &self.red_green,
            Channel::BlueYellow => &self.blue_yellow,
        }
    }

    pub fn channel_mut(&mut self, channel: Channel) -> &mut Vec<Point> {
        match channel {
            Channel::Rgb => &mut self.rgb,
            Channel::Red => &mut self.red,
            Channel::Green => &mut self.green,
            Channel::Blue => &mut self.blue,
            Channel::RedGreen => &mut self.red_green,
            Channel::BlueYellow => &mut self.blue_yellow,
        }
    }

    /// The tables with no grading.
    pub fn bake(&self) -> CurveLut {
        self.bake_with(&Grading::default())
    }

    /// The tables: each channel through the parametric curve, then
    /// its own curve, then the master; and the shift of a and b at
    /// each lightness, the color curves' and the grading's added.
    pub fn bake_with(&self, grading: &Grading) -> CurveLut {
        let mut lut = CurveLut {
            tone: [[0.0f32; 4]; LUT_SIZE],
            color: [[0.0f32; 2]; LUT_SIZE],
            shaded: false,
        };
        if self.enabled {
            let master = Spline::new(&self.rgb);
            let per = [
                Spline::new(&self.red),
                Spline::new(&self.green),
                Spline::new(&self.blue),
            ];
            for (i, entry) in lut.tone.iter_mut().enumerate() {
                let x = self.parametric.at(i as f32 / (LUT_SIZE - 1) as f32);
                for (c, spline) in per.iter().enumerate() {
                    entry[c] = master.at(spline.at(x));
                }
                entry[3] = master.at(x);
            }
        } else {
            for (i, entry) in lut.tone.iter_mut().enumerate() {
                let x = i as f32 / (LUT_SIZE - 1) as f32;
                *entry = [x; 4];
            }
        }
        // A color curve short of two points is the level line, not
        // the diagonal the spline would fall back to.
        let level = flat();
        let pick = |c: &Vec<Point>| {
            if c.len() < 2 {
                level.clone()
            } else {
                c.clone()
            }
        };
        let a = Spline::new(&pick(&self.red_green));
        let b = Spline::new(&pick(&self.blue_yellow));
        for (i, entry) in lut.color.iter_mut().enumerate() {
            let l = i as f32 / (LUT_SIZE - 1) as f32;
            let grade = grading.shift_at(l);
            *entry = if self.enabled {
                [
                    (a.at(l) - 0.5) * COLOR_RANGE + grade[0],
                    (b.at(l) - 0.5) * COLOR_RANGE + grade[1],
                ]
            } else {
                grade
            };
            lut.shaded |= entry[0].abs() > 1e-6 || entry[1].abs() > 1e-6;
        }
        lut
    }
}

fn is_straight(points: &[Point]) -> bool {
    points.len() < 2 || points.iter().all(|p| (p[0] - p[1]).abs() < 1e-6)
}

fn is_flat(points: &[Point]) -> bool {
    points.len() < 2 || points.iter().all(|p| (p[1] - 0.5).abs() < 1e-6)
}

/// The value of a curve through `points` at `x`, in 0 to 1.
pub fn evaluate(points: &[Point], x: f32) -> f32 {
    Spline::new(points).at(x)
}

/// A monotone cubic through points sorted by x: Fritsch and Carlson's
/// tangents, so the curve never overshoots between two points and a
/// rising set of points gives a rising curve. Flat beyond the ends.
struct Spline {
    xs: Vec<f32>,
    ys: Vec<f32>,
    tangents: Vec<f32>,
}

impl Spline {
    fn new(points: &[Point]) -> Self {
        let mut sorted: Vec<Point> = points.to_vec();
        sorted.sort_by(|a, b| a[0].total_cmp(&b[0]));
        sorted.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6);
        if sorted.len() < 2 {
            sorted = identity();
        }
        let n = sorted.len();
        let xs: Vec<f32> = sorted.iter().map(|p| p[0]).collect();
        let ys: Vec<f32> = sorted.iter().map(|p| p[1]).collect();
        let secants: Vec<f32> = (0..n - 1)
            .map(|k| (ys[k + 1] - ys[k]) / (xs[k + 1] - xs[k]).max(1e-6))
            .collect();
        let mut tangents = vec![0.0f32; n];
        tangents[0] = secants[0];
        tangents[n - 1] = secants[n - 2];
        for k in 1..n - 1 {
            tangents[k] = if secants[k - 1] * secants[k] <= 0.0 {
                0.0
            } else {
                (secants[k - 1] + secants[k]) / 2.0
            };
        }
        for k in 0..n - 1 {
            if secants[k] == 0.0 {
                tangents[k] = 0.0;
                tangents[k + 1] = 0.0;
                continue;
            }
            let a = tangents[k] / secants[k];
            let b = tangents[k + 1] / secants[k];
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                tangents[k] = t * a * secants[k];
                tangents[k + 1] = t * b * secants[k];
            }
        }
        Self { xs, ys, tangents }
    }

    fn at(&self, x: f32) -> f32 {
        let n = self.xs.len();
        if x <= self.xs[0] {
            return self.ys[0].clamp(0.0, 1.0);
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1].clamp(0.0, 1.0);
        }
        let k = self.xs.partition_point(|&v| v <= x) - 1;
        let h = self.xs[k + 1] - self.xs[k];
        let t = (x - self.xs[k]) / h;
        let (t2, t3) = (t * t, t * t * t);
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        let y = h00 * self.ys[k]
            + h10 * h * self.tangents[k]
            + h01 * self.ys[k + 1]
            + h11 * h * self.tangents[k + 1];
        y.clamp(0.0, 1.0)
    }
}

/// A tone table's value at `x`, linearly between entries, as the
/// shader and the CPU finish read it.
#[inline]
pub fn lookup(lut: &CurveLut, channel: usize, x: f32) -> f32 {
    let (i, f) = place(x);
    lut.tone[i][channel] + f * (lut.tone[i + 1][channel] - lut.tone[i][channel])
}

/// The shift of a and b at lightness `l`, linearly between entries.
#[inline]
pub fn color_shift(lut: &CurveLut, l: f32) -> [f32; 2] {
    let (i, f) = place(l);
    let (lo, hi) = (lut.color[i], lut.color[i + 1]);
    [lo[0] + f * (hi[0] - lo[0]), lo[1] + f * (hi[1] - lo[1])]
}

#[inline]
fn place(x: f32) -> (usize, f32) {
    let v = x.clamp(0.0, 1.0) * (LUT_SIZE - 1) as f32;
    let i = (v as usize).min(LUT_SIZE - 2);
    (i, v - i as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_takes_the_nearest_point_or_split_in_reach() {
        let points: Vec<Point> = vec![[0.0, 0.0], [0.4, 0.3], [0.44, 0.6], [1.0, 1.0]];
        // Nothing within reach, and the nearest of two that are.
        assert_eq!(nearest_point(&points, 0.7, 0.2), None);
        assert_eq!(nearest_point(&points, 0.42, 0.58), Some(2));
        assert_eq!(nearest_point(&points, 0.42, 0.32), Some(1));
        // Reach is in both directions at once, not in x alone: a
        // press under the curve at the same x takes neither.
        assert_eq!(nearest_point(&points, 0.42, 0.0), None);
        assert_eq!(nearest_point(&points, HIT * 0.9, 0.0), Some(0));

        // The splits are hit where the editor draws them, which is
        // `ordered_splits` and not what the sidecar wrote: this one
        // is out of order and too close together, and comes out at
        // 0.75, 0.80, 0.85 with the gap kept.
        let mut p = Parametric {
            splits: [0.75, 0.2, 0.5],
            ..Default::default()
        };
        assert_eq!(p.ordered_splits(), [0.75, 0.8, 0.85]);
        // Nothing is hit at the numbers as written...
        assert_eq!(p.nearest_split(0.2), None);
        assert_eq!(p.nearest_split(0.5), None);
        // ...and each is hit where it is drawn, nearest first.
        assert_eq!(p.nearest_split(0.755), Some(0));
        assert_eq!(p.nearest_split(0.81), Some(1));
        assert_eq!(p.nearest_split(0.86), Some(2));
        assert_eq!(p.nearest_split(0.6), None);
        // And the index a press gives back is the one that moves it:
        // the split pressed at 0.755 is the one that ends at 0.6,
        // which it could not be if the index were the sidecar's.
        let i = p.nearest_split(0.755).unwrap();
        p.set_split(i, 0.6);
        assert!((p.ordered_splits()[0] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn the_straight_line_is_the_identity() {
        let curves = Curves::default();
        assert!(curves.is_identity());
        let lut = curves.bake();
        for (i, entry) in lut.tone.iter().enumerate() {
            let x = i as f32 / 255.0;
            for v in entry {
                assert!((v - x).abs() < 1e-6);
            }
        }
        assert!((lookup(&lut, 0, 0.3) - 0.3).abs() < 1e-6);
        assert!(!lut.shaded);
        assert_eq!(color_shift(&lut, 0.4), [0.0, 0.0]);
        // Off is the identity whatever the points say.
        let off = Curves {
            enabled: false,
            rgb: vec![[0.0, 0.0], [0.5, 0.9], [1.0, 1.0]],
            ..Default::default()
        };
        assert!(off.is_identity());
        assert!((lookup(&off.bake(), 3, 0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_point_pulls_the_curve_and_it_stays_monotone() {
        let points = vec![[0.0, 0.0], [0.25, 0.15], [0.75, 0.9], [1.0, 1.0]];
        assert!((evaluate(&points, 0.25) - 0.15).abs() < 1e-6);
        assert!((evaluate(&points, 0.75) - 0.9).abs() < 1e-6);
        assert!(evaluate(&points, 0.5) > 0.4 && evaluate(&points, 0.5) < 0.65);
        let mut last = -1.0;
        for i in 0..=1000 {
            let y = evaluate(&points, i as f32 / 1000.0);
            assert!(y >= last, "turns back at {i}");
            last = y;
        }
        // Flat beyond the ends, and a lifted black stays lifted.
        let lifted = vec![[0.1, 0.2], [0.9, 0.8]];
        assert!((evaluate(&lifted, 0.0) - 0.2).abs() < 1e-6);
        assert!((evaluate(&lifted, 1.0) - 0.8).abs() < 1e-6);
        // Unsorted input is sorted; one point is the identity.
        assert!((evaluate(&[[1.0, 1.0], [0.0, 0.0]], 0.3) - 0.3).abs() < 1e-6);
        assert!((evaluate(&[[0.5, 0.5]], 0.3) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn a_channel_goes_through_its_own_curve_then_the_master() {
        let curves = Curves {
            red: vec![[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]],
            rgb: vec![[0.0, 0.0], [0.25, 0.5], [1.0, 1.0]],
            ..Default::default()
        };
        assert!(!curves.is_identity());
        let lut = curves.bake();
        // Red: 0.5 -> 0.25 by its own, then 0.25 -> 0.5 by the master.
        assert!(
            (lookup(&lut, 0, 0.5) - 0.5).abs() < 2e-3,
            "{}",
            lookup(&lut, 0, 0.5)
        );
        // Green has only the master: 0.25 -> 0.5.
        assert!((lookup(&lut, 1, 0.25) - 0.5).abs() < 2e-3);
        // The fourth place is the master alone.
        assert!((lookup(&lut, 3, 0.25) - 0.5).abs() < 2e-3);
    }

    #[test]
    fn a_color_curve_shifts_by_lightness_and_nowhere_else() {
        // Red at the top of the range in the middle of the lightnesses,
        // the neutral held at both ends.
        let curves = Curves {
            red_green: vec![[0.0, 0.5], [0.5, 1.0], [1.0, 0.5]],
            ..Default::default()
        };
        assert!(!curves.is_identity());
        let lut = curves.bake();
        assert!(lut.shaded);
        let mid = color_shift(&lut, 0.5);
        assert!((mid[0] - COLOR_RANGE / 2.0).abs() < 2e-3, "{mid:?}");
        assert_eq!(mid[1], 0.0);
        assert_eq!(color_shift(&lut, 0.0), [0.0, 0.0]);
        assert_eq!(color_shift(&lut, 1.0), [0.0, 0.0]);
        // A flat stretch is exactly flat: nothing below 0.8 here.
        let high = Curves {
            blue_yellow: vec![[0.0, 0.5], [0.8, 0.5], [1.0, 0.0]],
            ..Default::default()
        }
        .bake();
        assert_eq!(color_shift(&high, 0.6), [0.0, 0.0]);
        assert!(color_shift(&high, 0.95)[1] < -0.05);
        // One point is the level line.
        let one = Curves {
            red_green: vec![[0.5, 0.5]],
            ..Default::default()
        };
        assert!(!one.bake().shaded);
        // Off is the identity whatever the points say.
        let off = Curves {
            enabled: false,
            ..curves
        };
        assert!(off.is_identity() && !off.bake().shaded);
    }

    #[test]
    fn the_grading_adds_into_the_color_table() {
        use crate::grading::{RANGE, Wheel};
        let grading = Grading {
            shadows: Wheel {
                hue: 0.0,
                saturation: 1.0,
            },
            ..Default::default()
        };
        // Level curves: the table is the grading's alone.
        let lut = Curves::default().bake_with(&grading);
        assert!(lut.shaded);
        assert!((color_shift(&lut, 0.0)[0] - RANGE).abs() < 1e-6);
        assert_eq!(color_shift(&lut, 1.0), [0.0, 0.0]);
        // A curve and the grading add.
        let curves = Curves {
            red_green: vec![[0.0, 1.0], [1.0, 1.0]],
            ..Default::default()
        };
        let both = curves.bake_with(&grading);
        assert!((color_shift(&both, 0.0)[0] - RANGE - COLOR_RANGE / 2.0).abs() < 1e-6);
        // Curves off, the grading still counts.
        let off = Curves {
            enabled: false,
            ..curves
        }
        .bake_with(&grading);
        assert!((color_shift(&off, 0.0)[0] - RANGE).abs() < 1e-6);
        assert!((lookup(&off, 0, 0.3) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn a_sidecar_without_the_color_curves_reads_as_flat() {
        let json = r#"{"enabled":true,"rgb":[[0.0,0.0],[0.5,0.6],[1.0,1.0]]}"#;
        let curves: Curves = serde_json::from_str(json).unwrap();
        assert_eq!(curves.red_green, flat());
        assert_eq!(curves.blue_yellow, flat());
        assert_eq!(curves.parametric, Parametric::default());
        assert!(curves.parametric.is_identity());
        assert!(!curves.bake().shaded);
        assert_eq!(Channel::from_name("R/G"), Some(Channel::RedGreen));
        assert_eq!(Channel::BlueYellow.identity(), flat());
        assert_eq!(Channel::Red.identity(), identity());
    }

    /// The parametric curve sampled finely.
    fn samples(p: &Parametric) -> Vec<f32> {
        (0..=1000).map(|i| p.at(i as f32 / 1000.0)).collect()
    }

    #[test]
    fn the_parametric_curve_at_zero_is_the_identity() {
        let p = Parametric::default();
        assert!(p.is_identity());
        for i in 0..=100 {
            let x = i as f32 / 100.0;
            assert!((p.at(x) - x).abs() < 1e-6);
        }
        // With the splits moved and the amounts at zero, still.
        let moved = Parametric {
            splits: [0.1, 0.6, 0.9],
            ..Default::default()
        };
        assert!(moved.is_identity() && (moved.at(0.3) - 0.3).abs() < 1e-6);
        // In the curve set: the identity, and the bake is the diagonal.
        let curves = Curves {
            parametric: moved,
            ..Default::default()
        };
        assert!(curves.is_identity());
        assert!((lookup(&curves.bake(), 3, 0.7) - 0.7).abs() < 1e-6);
        // An amount is not.
        let lifted = Curves {
            parametric: Parametric {
                lights: 0.3,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!lifted.is_identity());
        // Off is the identity whatever the amounts say.
        let off = Curves {
            enabled: false,
            ..lifted
        };
        assert!(off.is_identity() && (lookup(&off.bake(), 3, 0.6) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn the_parametric_curve_is_monotone_for_every_amount() {
        let levels = [-1.0f32, -0.5, 0.0, 0.5, 1.0];
        let split_sets = [
            Parametric::DEFAULT_SPLITS,
            [0.05, 0.1, 0.15],
            [0.85, 0.9, 0.95],
            [0.05, 0.5, 0.95],
            [0.4, 0.45, 0.5],
        ];
        for splits in split_sets {
            for &shadows in &levels {
                for &darks in &levels {
                    for &lights in &levels {
                        for &highlights in &levels {
                            let p = Parametric {
                                highlights,
                                lights,
                                darks,
                                shadows,
                                splits,
                            };
                            let ys = samples(&p);
                            assert_eq!(ys[0], 0.0);
                            assert!((ys[1000] - 1.0).abs() < 1e-6);
                            for w in ys.windows(2) {
                                assert!(
                                    w[1] >= w[0],
                                    "turns back with {p:?}: {} then {}",
                                    w[0],
                                    w[1]
                                );
                            }
                            // The nodes stay in order, and inside.
                            let nodes = p.nodes();
                            for w in nodes.windows(2) {
                                assert!(w[1][1] > w[0][1], "{p:?}: {nodes:?}");
                            }
                        }
                    }
                }
            }
        }
        // Amounts past the range are clamped, and a bad number is
        // nothing.
        let wild = Parametric {
            highlights: 7.0,
            shadows: f32::NAN,
            ..Default::default()
        };
        let full = Parametric {
            highlights: 1.0,
            ..Default::default()
        };
        assert_eq!(samples(&wild), samples(&full));
    }

    #[test]
    fn each_amount_moves_its_own_region() {
        let flat = samples(&Parametric::default());
        let [s0, s1, s2] = Parametric::DEFAULT_SPLITS;
        let peak = |ys: &[f32]| {
            let (i, d) = ys
                .iter()
                .zip(&flat)
                .map(|(y, x)| y - x)
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap();
            (i as f32 / 1000.0, d)
        };
        // Highlights up: the peak of the lift lies above the highlight
        // split, nothing moves below the middle of the lights, and
        // nothing dips anywhere.
        let high = samples(&Parametric {
            highlights: 1.0,
            ..Default::default()
        });
        let (at, d) = peak(&high);
        assert!(at > s2 && at < 1.0, "peak at {at}");
        assert!(d > 0.07, "lift {d}");
        assert!(high.iter().zip(&flat).all(|(y, x)| *y >= x - 1e-6));
        for (i, (y, x)) in high.iter().zip(&flat).enumerate() {
            let xx = i as f32 / 1000.0;
            if xx <= (s1 + s2) / 2.0 {
                assert!((y - x).abs() < 1e-6, "moved at {xx}");
            }
        }
        // Shadows down: the dip lies below the shadow split, nothing
        // rises anywhere, nothing moves above the middle of the darks.
        let low = samples(&Parametric {
            shadows: -1.0,
            ..Default::default()
        });
        let (at, d) = peak(&low);
        assert!(at > 0.0 && at < s0, "dip at {at}");
        assert!(d < -0.07, "dip {d}");
        assert!(low.iter().zip(&flat).all(|(y, x)| *y <= x + 1e-6));
        for (i, (y, x)) in low.iter().zip(&flat).enumerate() {
            if i as f32 / 1000.0 >= (s0 + s1) / 2.0 {
                assert!((y - x).abs() < 1e-6);
            }
        }
        // Darks and lights: each peaks in its own quarter.
        let dark = samples(&Parametric {
            darks: 1.0,
            ..Default::default()
        });
        let (at, _) = peak(&dark);
        assert!(at > s0 && at < s1, "darks peak at {at}");
        let light = samples(&Parametric {
            lights: -1.0,
            ..Default::default()
        });
        let (at, _) = peak(&light);
        assert!(at > s1 && at < s2, "lights dip at {at}");
        // The nodes at the defaults: a node moves by REACH of its
        // quarter, so the middle of the lights sits at 0.625 - 0.075,
        // and the curve goes through it.
        let n = Parametric {
            lights: -1.0,
            ..Default::default()
        }
        .nodes();
        assert!((n[3][0] - 0.625).abs() < 1e-6 && (n[3][1] - 0.55).abs() < 1e-6);
        assert!((light[625] - 0.55).abs() < 1e-6);
        // Neighbors pulled apart at full: the slope bottoms out at a
        // tenth, never at nothing.
        let steep = Parametric {
            shadows: -1.0,
            darks: 1.0,
            lights: -1.0,
            highlights: 1.0,
            ..Default::default()
        };
        let ys = samples(&steep);
        let least = ys.windows(2).map(|w| w[1] - w[0]).fold(1.0f32, f32::min) * 1000.0;
        assert!(least > 0.09 && least < 0.12, "{least}");
    }

    #[test]
    fn the_splits_stay_in_order_and_move_the_regions() {
        // Out of order in a sidecar: put in order with the gap.
        let bad = Parametric {
            splits: [0.9, 0.2, 0.0],
            ..Default::default()
        };
        let s = bad.ordered_splits();
        assert!((s[0] - (1.0 - 3.0 * SPLIT_GAP)).abs() < 1e-6, "{s:?}");
        assert!((s[1] - (1.0 - 2.0 * SPLIT_GAP)).abs() < 1e-6, "{s:?}");
        assert!((s[2] - (1.0 - SPLIT_GAP)).abs() < 1e-6, "{s:?}");
        // A move stops at the neighbor's gap and at the end's.
        let mut p = Parametric::default();
        p.set_split(1, 0.9);
        assert!(
            (p.splits[1] - (0.75 - SPLIT_GAP)).abs() < 1e-6,
            "{:?}",
            p.splits
        );
        p.set_split(0, -1.0);
        assert!((p.splits[0] - SPLIT_GAP).abs() < 1e-6);
        p.set_split(2, 0.3);
        assert!((p.splits[2] - (p.splits[1] + SPLIT_GAP)).abs() < 1e-6);
        p.set_split(2, 0.99);
        assert!((p.splits[2] - (1.0 - SPLIT_GAP)).abs() < 1e-6);
        // The highlight split moved up narrows the highlights: the
        // lift's peak follows it and shrinks with the region.
        let wide = Parametric {
            highlights: 1.0,
            ..Default::default()
        };
        let narrow = Parametric {
            highlights: 1.0,
            splits: [0.25, 0.5, 0.9],
            ..Default::default()
        };
        let (mut at_wide, mut at_narrow) = (0.0, 0.0);
        let (mut peak_wide, mut peak_narrow) = (0.0f32, 0.0f32);
        for i in 0..=1000 {
            let x = i as f32 / 1000.0;
            let (dw, dn) = (wide.at(x) - x, narrow.at(x) - x);
            if dw > peak_wide {
                (peak_wide, at_wide) = (dw, x);
            }
            if dn > peak_narrow {
                (peak_narrow, at_narrow) = (dn, x);
            }
        }
        assert!(
            at_narrow > at_wide && at_narrow > 0.9,
            "{at_wide} {at_narrow}"
        );
        assert!(peak_narrow < peak_wide / 2.0, "{peak_wide} {peak_narrow}");
        // And the curve is untouched below the split's new place.
        assert!((narrow.at(0.6) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn the_parametric_curve_acts_before_the_point_curves() {
        let parametric = Parametric {
            darks: 1.0,
            ..Default::default()
        };
        let curves = Curves {
            parametric,
            red: vec![[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]],
            rgb: vec![[0.0, 0.0], [0.25, 0.5], [1.0, 1.0]],
            ..Default::default()
        };
        let lut = curves.bake();
        for i in [0usize, 37, 100, 128, 200, 255] {
            let x = i as f32 / 255.0;
            let px = parametric.at(x);
            // The fourth place: the parametric, then the master.
            let master = evaluate(&curves.rgb, px);
            assert!((lut.tone[i][3] - master).abs() < 1e-5, "{x}");
            // Red: the parametric, its own, then the master.
            let red = evaluate(&curves.rgb, evaluate(&curves.red, px));
            assert!((lut.tone[i][0] - red).abs() < 1e-5, "{x}");
            // Green has no curve of its own.
            assert!((lut.tone[i][1] - master).abs() < 1e-5, "{x}");
        }
        // The parametric alone rides in every place.
        let alone = Curves {
            parametric,
            ..Default::default()
        }
        .bake();
        assert!((lookup(&alone, 0, 0.375) - parametric.at(0.375)).abs() < 2e-3);
        assert!((lookup(&alone, 3, 0.375) - 0.45).abs() < 2e-3);
        // The sidecar carries it as plain numbers.
        let json = serde_json::to_string(&curves).unwrap();
        assert_eq!(serde_json::from_str::<Curves>(&json).unwrap(), curves);
    }
}
