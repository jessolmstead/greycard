//! Brush strokes and the raster they paint. A stroke is a run of
//! points in the masks' units with a size, a feather and a flow,
//! adding to the mask under it, taking from it, or erasing what the
//! brush did there. The strokes are what the sidecar keeps; the raster
//! is made from them, the same way on both paths, at a fixed width, so
//! a painted mask is one thing at any export size. While a stroke is
//! under way only its new points are stamped.
//!
//! A brush is a map on the mask before it, texel by texel: each stroke
//! sends the mask `m` under it to `m + k(1 - m)` (Add) or `m(1 - k)`
//! (Subtract), `k` its coverage times its flow, and an erase moves the
//! map back toward leaving `m` alone. Any run of those is
//! `m(1 - p - c) + p` for two planes, the paint `p` and the cut `c`,
//! which is what the raster keeps. The paint is what one plane held
//! before there was a cut, to the bit, so a brush joined by its mode
//! as any shape is reads its paint alone.

use serde::{Deserialize, Serialize};

use crate::mask::{Pos, smoothstep};

/// The raster's width, texels; its height follows the picture.
pub const RASTER_WIDTH: usize = 2048;

/// A Sky's raster on a frame whose width would spread a texel of
/// [`RASTER_WIDTH`] over more than [`MOST_ACROSS`] of its pixels: the
/// matte is made at the frame's own resolution and averaged to the
/// raster, so on a 45 MP frame each texel of a 2048 one is the mean of
/// four pixels across and four down, and the light rim along a leaf or
/// a twig that the averaging leaves is plain at 1:1. Twice as wide
/// halves it.
pub const WIDE_RASTER_WIDTH: usize = 4096;
/// The most of the frame's pixels across that a texel of a Sky's
/// raster may stand for before the raster is the wide one: the width,
/// not the megapixels, decides. A 24 MP frame 6000 wide (2.9 a texel)
/// keeps the raster it had, to the bit, as does any frame up to 6144
/// wide; but some 24 and 26 MP bodies are wider, 6240 for one (3.05 a
/// texel), and take the wide one, as do a 45 MP frame (8192, 4.0) and
/// a 100 MP one upright (8736, 4.3) or not (11648, 5.7).
pub const MOST_ACROSS: usize = 3;
/// The longest side a raster may have: 8192, the 2D texture size every
/// device wgpu runs on guarantees (its default
/// `max_texture_dimension_2d`), whatever more a given GPU takes. A
/// frame whose wide raster would be taller keeps the narrow one.
pub const MOST_SIDE: usize = 8192;

/// The width of the raster `shape` is made at on a frame of `frame`
/// (width, height), the full develop's size as the frame is turned:
/// [`WIDE_RASTER_WIDTH`] for a Sky on a frame wider than
/// [`MOST_ACROSS`] times [`RASTER_WIDTH`] and no taller than twice its
/// width, [`RASTER_WIDTH`] for everything else. The frame's size, not
/// the picture the models are run on: a reduced develop's half-size
/// picture of a 100 MP frame makes the same raster the full one does.
pub fn raster_width(shape: &crate::mask::Shape, frame: (usize, usize)) -> usize {
    let (w, h) = frame;
    let wide = matches!(shape, crate::mask::Shape::Sky { .. })
        && w > MOST_ACROSS * RASTER_WIDTH
        && raster_height(WIDE_RASTER_WIDTH, h as f32 / w as f32) <= MOST_SIDE;
    if wide {
        WIDE_RASTER_WIDTH
    } else {
        RASTER_WIDTH
    }
}

/// A raster's height at `width` for a picture `aspect` (height over
/// width) tall.
pub fn raster_height(width: usize, aspect: f32) -> usize {
    ((width as f32 * aspect).round() as usize).max(1)
}

// A row of a raster is a whole number of the GPU copy's row alignment.
const _: () = assert!(RASTER_WIDTH.is_multiple_of(256) && WIDE_RASTER_WIDTH.is_multiple_of(256));

/// What a stroke does to the mask under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Adds to the mask by its flow, feathered.
    #[default]
    Add,
    /// Takes from the mask by its flow, feathered, whatever the shapes
    /// before the brush gave there: on a brush that paints on the mask
    /// (`Component::paints_on_mask`, joined by Add and not inverted).
    /// On any other brush it takes paint away, and its cut is not read:
    /// under Intersect that narrows the mask, and on an inverted brush
    /// the inversion makes it more mask.
    Subtract,
    /// Clears the brush's own paint and cut over the whole radius,
    /// hard-edged, whatever the flow.
    Erase,
    /// Clears the brush's own paint and cut by the flow, feathered: an
    /// erase that is not hard. What a Subtract stroke did before the
    /// schema's version 8, when it lifted the brush's paint and took
    /// nothing from the shapes before it; a sidecar from then reads its
    /// Subtract strokes as this (`migrate`). Not offered: not in `ALL`.
    Lift,
}

impl Op {
    pub const ALL: [Op; 3] = [Op::Add, Op::Subtract, Op::Erase];

    pub fn name(self) -> &'static str {
        match self {
            Op::Add => "Add",
            Op::Subtract => "Subtract",
            Op::Erase => "Erase",
            Op::Lift => "Lift",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.name() == name)
    }
}

/// One stroke of the brush.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stroke {
    pub points: Vec<Pos>,
    /// The brush's radius, in units of the picture's width.
    pub radius: f32,
    /// How much of the radius fades, 0 to 1.
    pub feather: f32,
    /// How much the stroke lays down or takes away, 0 to 1.
    pub flow: f32,
    #[serde(deserialize_with = "crate::meta::newer_loose")]
    pub op: Op,
}

impl Stroke {
    /// The stroke carried round with a turned picture: its points
    /// and its radius, which is in the same units they are.
    pub fn turn(&mut self, t: crate::mask::Turned) {
        if t.is_identity() {
            return;
        }
        for p in &mut self.points {
            *p = t.pos(*p);
        }
        self.radius *= t.scale();
    }
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            radius: 0.05,
            feather: 0.5,
            flow: 1.0,
            op: Op::Add,
        }
    }
}

impl Stroke {
    pub fn new(op: Op, radius: f32, feather: f32, flow: f32) -> Self {
        Self {
            op,
            radius,
            feather,
            flow,
            ..Default::default()
        }
    }

    /// Whether `other` is this stroke with points added.
    fn continues(&self, other: &Stroke) -> bool {
        self.radius == other.radius
            && self.feather == other.feather
            && self.flow == other.flow
            && self.op == other.op
            && other.points.starts_with(&self.points)
    }
}

/// The strokes painted: a texel is the finished strokes composited,
/// with the stroke under way over them. Covers u in 0 to 1 and v in
/// 0 to `aspect`. Two planes, the paint and the cut (the module's
/// doc); the cut is made only once a Subtract stroke lands, so a
/// brush that never took anything away, and a model's mask, keep one.
#[derive(Clone)]
pub struct Raster {
    pub width: usize,
    pub height: usize,
    pub aspect: f32,
    /// What is shown, 0 to 255.
    data: Vec<u8>,
    /// The finished strokes alone.
    base: Vec<u8>,
    /// The coverage of the stroke under way, 0 where there is none.
    cover: Vec<u8>,
    /// What is cut, 0 to 255; empty while nothing is.
    cut: Vec<u8>,
    /// The finished strokes' cut alone; empty with `cut`.
    cut_base: Vec<u8>,
    /// The strokes as painted so far.
    done: Vec<Stroke>,
    /// The distance to the next stamp along the stroke under way.
    carry: f32,
    /// Counts every change, for a copy elsewhere to notice.
    pub version: u64,
}

impl std::fmt::Debug for Raster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Raster({}x{}, {} strokes, v{})",
            self.width,
            self.height,
            self.done.len(),
            self.version
        )
    }
}

impl Raster {
    /// An empty raster for a picture `aspect` (height over width) tall.
    pub fn new(aspect: f32) -> Self {
        let width = RASTER_WIDTH;
        let height = raster_height(width, aspect);
        Self {
            width,
            height,
            aspect,
            data: vec![0; width * height],
            base: vec![0; width * height],
            cover: vec![0; width * height],
            cut: Vec::new(),
            cut_base: Vec::new(),
            done: Vec::new(),
            carry: 0.0,
            version: 0,
        }
    }

    /// A raster made elsewhere (a model's mask): `data` is `width`
    /// texels wide and covers a picture `aspect` tall.
    pub fn from_data(aspect: f32, width: usize, data: Vec<u8>) -> Self {
        assert!(width > 0 && data.len().is_multiple_of(width), "whole rows");
        let height = data.len() / width;
        Self {
            width,
            height,
            aspect,
            base: data.clone(),
            cover: vec![0; width * height],
            cut: Vec::new(),
            cut_base: Vec::new(),
            data,
            done: Vec::new(),
            carry: 0.0,
            version: 1,
        }
    }

    /// The strokes painted from nothing.
    pub fn of(strokes: &[Stroke], aspect: f32) -> Self {
        let mut r = Self::new(aspect);
        r.update(strokes);
        r
    }

    /// The texels, row by row: the paint.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The cut's texels, row by row, when a stroke has cut anything.
    pub fn cut(&self) -> Option<&[u8]> {
        (!self.cut.is_empty()).then_some(&self.cut[..])
    }

    /// Whether the raster shows `strokes` already, so `update` would
    /// change nothing: the strokes painted, and after them only strokes
    /// with no points yet. A caller sharing the raster asks this first,
    /// so it neither copies it nor gives it a new identity for nothing.
    pub fn is_at(&self, strokes: &[Stroke]) -> bool {
        strokes.starts_with(&self.done)
            && strokes[self.done.len()..]
                .iter()
                .all(|s| s.points.is_empty())
    }

    /// Bring the raster to `strokes`: the points added since the last
    /// time are stamped; anything else is painted again from nothing.
    /// Whether anything changed.
    pub fn update(&mut self, strokes: &[Stroke]) -> bool {
        // Nothing to do touches nothing: a stroke with no points yet
        // would otherwise take the one under way into `base`, and a
        // repaint of that one would then paint over its old self.
        if self.is_at(strokes) {
            return false;
        }
        let n = self.done.len();
        // Only the stroke under way changed, other than by growing (a
        // stroke held straight, its end moved): the finished strokes
        // are kept in `base`, so it alone is painted again over them.
        let redone = n > 0
            && n == strokes.len()
            && self.done[..n - 1] == strokes[..n - 1]
            && !self.done[n - 1].continues(&strokes[n - 1]);
        if redone {
            self.data.copy_from_slice(&self.base);
            if self.cut.len() == self.cut_base.len() {
                self.cut.copy_from_slice(&self.cut_base);
            }
            // The cut made by the stroke's old self alone, and the new
            // one makes none: no cut, as painting from nothing gives.
            if strokes[n - 1].op != Op::Subtract && self.cut_base.iter().all(|&t| t == 0) {
                self.cut.clear();
                self.cut_base.clear();
            }
            self.cover.fill(0);
            self.carry = 0.0;
            if !strokes[n - 1].points.is_empty() {
                self.stamp_from(&strokes[n - 1], 0);
            }
            self.done = strokes.to_vec();
            self.version += 1;
            return true;
        }
        let continues = n <= strokes.len()
            && self.done[..n.saturating_sub(1)] == strokes[..n.saturating_sub(1)]
            && (n == 0 || self.done[n - 1].continues(&strokes[n - 1]));
        if !continues {
            self.data.fill(0);
            self.base.fill(0);
            self.cover.fill(0);
            self.cut.clear();
            self.cut_base.clear();
            self.done.clear();
            self.carry = 0.0;
            self.version += 1;
        }
        let n = self.done.len();
        let mut changed = !continues;
        if n > 0 {
            let from = self.done[n - 1].points.len();
            if from < strokes[n - 1].points.len() {
                self.stamp_from(&strokes[n - 1], from);
                changed = true;
            }
        }
        for stroke in &strokes[n..] {
            // The stroke before it is finished.
            self.base.copy_from_slice(&self.data);
            self.cut_base.copy_from_slice(&self.cut);
            self.cover.fill(0);
            self.carry = 0.0;
            if !stroke.points.is_empty() {
                self.stamp_from(stroke, 0);
                changed = true;
            }
        }
        if changed {
            self.done = strokes.to_vec();
            self.version += 1;
        }
        changed
    }

    /// Stamp the stroke's points from index `from`, along its line at
    /// a sixth of the radius, carrying the spacing across the points.
    fn stamp_from(&mut self, stroke: &Stroke, from: usize) {
        // The first cut makes the plane; only a Subtract adds to it.
        if stroke.op == Op::Subtract && self.cut.is_empty() {
            self.cut = vec![0; self.data.len()];
            self.cut_base = vec![0; self.data.len()];
        }
        let radius = (stroke.radius.max(1e-4) * self.width as f32).max(0.5);
        let spacing = (radius / 6.0).max(0.5);
        let (w, h, aspect) = (self.width as f32, self.height as f32, self.aspect);
        let px = move |p: Pos| (p[0] * w, p[1] / aspect * h);
        let p = &stroke.points;
        if from == 0 {
            self.stamp(px(p[0]), radius, stroke);
            self.carry = spacing;
        }
        for j in from.max(1)..p.len() {
            let (a, b) = (px(p[j - 1]), px(p[j]));
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            let mut t = self.carry;
            while t <= len {
                self.stamp((a.0 + dx * t / len, a.1 + dy * t / len), radius, stroke);
                t += spacing;
            }
            self.carry = t - len;
        }
    }

    /// One disc of the brush at `c` (texels): the stroke's coverage is
    /// the most any of its discs gave, and what is shown is the
    /// finished strokes with that put on by the flow. An Add lays
    /// paint and lifts the cut, a Subtract lays cut and lifts the
    /// paint, an erase or a lift lifts both; an erase clears the disc
    /// outright.
    fn stamp(&mut self, c: (f32, f32), radius: f32, stroke: &Stroke) {
        let op = stroke.op;
        let (feather, flow) = match op {
            Op::Erase => (0.0, 1.0),
            _ => (stroke.feather.clamp(0.0, 1.0), stroke.flow.clamp(0.0, 1.0)),
        };
        let x0 = ((c.0 - radius).floor().max(0.0)) as usize;
        let y0 = ((c.1 - radius).floor().max(0.0)) as usize;
        let x1 = ((c.0 + radius).ceil().max(0.0) as usize).min(self.width);
        let y1 = ((c.1 + radius).ceil().max(0.0) as usize).min(self.height);
        for y in y0..y1 {
            let dy = y as f32 + 0.5 - c.1;
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - c.0;
                let d = (dx * dx + dy * dy).sqrt() / radius;
                let s = 1.0 - smoothstep(1.0 - feather, 1.0, d);
                if s <= 0.0 {
                    continue;
                }
                let i = y * self.width + x;
                let s = (s * 255.0).round() as u8;
                if s <= self.cover[i] {
                    continue;
                }
                self.cover[i] = s;
                let (b, k) = (self.base[i] as f32 / 255.0, s as f32 / 255.0 * flow);
                let shown = match op {
                    Op::Add => b + k * (1.0 - b),
                    Op::Subtract | Op::Erase | Op::Lift => b * (1.0 - k),
                };
                self.data[i] = (shown * 255.0).round() as u8;
                if !self.cut.is_empty() {
                    let was = self.cut_base[i] as f32 / 255.0;
                    let cut = match op {
                        Op::Subtract => was + k * (1.0 - was),
                        Op::Add | Op::Erase | Op::Lift => was * (1.0 - k),
                    };
                    self.cut[i] = (cut * 255.0).round() as u8;
                }
            }
        }
    }

    /// The value at (u, v) in the masks' units, between texels as a
    /// linear sampler clamped to the edge has it.
    pub fn at(&self, u: f32, v: f32) -> f32 {
        self.sample(&self.data, u, v)
    }

    /// The cut at (u, v), read as [`Raster::at`] reads the paint;
    /// nothing where no stroke has cut.
    pub fn cut_at(&self, u: f32, v: f32) -> f32 {
        if self.cut.is_empty() {
            return 0.0;
        }
        self.sample(&self.cut, u, v)
    }

    fn sample(&self, plane: &[u8], u: f32, v: f32) -> f32 {
        let x = (u * self.width as f32 - 0.5).clamp(0.0, (self.width - 1) as f32);
        let y = (v / self.aspect * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let t = |x: usize, y: usize| plane[y * self.width + x] as f32 / 255.0;
        let top = t(x0, y0) * (1.0 - fx) + t(x1, y0) * fx;
        let bottom = t(x0, y1) * (1.0 - fx) + t(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dab(at: Pos, radius: f32, feather: f32, flow: f32, op: Op) -> Stroke {
        Stroke {
            points: vec![at],
            ..Stroke::new(op, radius, feather, flow)
        }
    }

    /// The Sky's raster is the wide one past three of the frame's
    /// pixels a texel across, and only the Sky's; a frame under that
    /// keeps the raster every shape had.
    #[test]
    fn a_sky_raster_is_wide_only_past_three_pixels_a_texel() {
        use crate::mask::{LandscapeGroup, Shape};
        let sky = Shape::Sky { picks: Vec::new() };
        // 24 MP both ways, 45 MP upright: today's raster.
        for frame in [(6000, 4000), (4000, 6000), (5464, 8192), (6144, 4096)] {
            assert_eq!(raster_width(&sky, frame), RASTER_WIDTH, "{frame:?}");
        }
        // A 24 MP body 6240 wide, 45 MP, 61 MP, 100 MP upright and
        // not: the wide one.
        for frame in [
            (6145, 4096),
            (6240, 4160),
            (8192, 5464),
            (9504, 6336),
            (8736, 11648),
            (11648, 8736),
        ] {
            assert_eq!(raster_width(&sky, frame), WIDE_RASTER_WIDTH, "{frame:?}");
        }
        // A wide raster taller than every device takes keeps the
        // narrow one.
        assert_eq!(raster_width(&sky, (7000, 14000)), WIDE_RASTER_WIDTH);
        assert_eq!(raster_height(WIDE_RASTER_WIDTH, 14001.0 / 7000.0), 8193);
        assert_eq!(raster_width(&sky, (7000, 14001)), RASTER_WIDTH);
        // No other shape, a Foliage the sky cuts among them.
        for shape in [
            Shape::Subject {},
            Shape::Landscape {
                group: LandscapeGroup::Foliage,
            },
            Shape::Brush {
                strokes: Vec::new(),
            },
        ] {
            assert_eq!(raster_width(&shape, (11648, 8736)), RASTER_WIDTH);
        }
        assert_eq!(raster_height(WIDE_RASTER_WIDTH, 8736.0 / 11648.0), 3072);
        assert_eq!(raster_height(RASTER_WIDTH, 2.0 / 3.0), 1365);
    }

    #[test]
    fn a_raster_from_data_samples_its_texels() {
        let r = Raster::from_data(0.5, 4, vec![0, 255, 0, 255, 0, 255, 0, 255]);
        assert_eq!((r.width, r.height), (4, 2));
        assert!((r.at(0.125, 0.125) - 0.0).abs() < 1e-6);
        assert!((r.at(0.375, 0.125) - 1.0).abs() < 1e-6);
        assert!((r.at(0.25, 0.25) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_dab_covers_its_disc_and_feathers_to_the_edge() {
        let r = Raster::of(&[dab([0.5, 0.3], 0.1, 0.5, 1.0, Op::Add)], 2.0 / 3.0);
        assert_eq!(r.height, 1365);
        assert!((r.at(0.5, 0.3) - 1.0).abs() < 1e-6);
        assert!((r.at(0.54, 0.3) - 1.0).abs() < 1e-6);
        let edge = r.at(0.58, 0.3);
        assert!(edge > 0.0 && edge < 1.0, "{edge}");
        assert_eq!(r.at(0.61, 0.3), 0.0);
        assert_eq!(r.at(0.5, 0.41), 0.0);
        assert_eq!(r.at(0.9, 0.6), 0.0);
        // The height is the picture's: the same fraction of v as of u.
        assert!((r.at(0.5, 0.34) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn strokes_add_subtract_erase_and_take_their_flow() {
        let aspect = 0.75;
        let paint = dab([0.5, 0.3], 0.1, 0.0, 1.0, Op::Add);
        let r = Raster::of(std::slice::from_ref(&paint), aspect);
        assert_eq!(r.at(0.5, 0.3), 1.0);
        // Half flow lays half; another half stroke over it fills half the rest.
        let half = dab([0.5, 0.3], 0.1, 0.0, 0.5, Op::Add);
        let r = Raster::of(std::slice::from_ref(&half), aspect);
        assert!((r.at(0.5, 0.3) - 0.5).abs() < 0.01);
        let r = Raster::of(&[half.clone(), half.clone()], aspect);
        assert!((r.at(0.5, 0.3) - 0.75).abs() < 0.01);
        // A subtract at half flow takes half of what is there.
        let lift = dab([0.5, 0.3], 0.05, 0.0, 0.5, Op::Subtract);
        let r = Raster::of(&[paint.clone(), lift], aspect);
        assert!((r.at(0.5, 0.3) - 0.5).abs() < 0.01);
        assert_eq!(r.at(0.58, 0.3), 1.0);
        // An erase clears its whole radius, hard, whatever its flow
        // and feather say.
        let erase = dab([0.5, 0.3], 0.05, 0.9, 0.1, Op::Erase);
        let r = Raster::of(&[paint.clone(), erase], aspect);
        assert_eq!(r.at(0.5, 0.3), 0.0);
        assert_eq!(r.at(0.545, 0.3), 0.0);
        assert_eq!(r.at(0.58, 0.3), 1.0);
        // A lift takes half of the paint as the subtract did, and cuts
        // nothing.
        let lift = dab([0.5, 0.3], 0.05, 0.0, 0.5, Op::Lift);
        let lifted = Raster::of(&[paint.clone(), lift], aspect);
        assert!((lifted.at(0.5, 0.3) - 0.5).abs() < 0.01);
        assert!(lifted.cut().is_none());
        // Within one stroke the discs do not pile up: a long stroke's
        // feathered edge is one disc's.
        let long = Stroke {
            points: vec![[0.2, 0.3], [0.5, 0.3], [0.8, 0.3]],
            ..Stroke::new(Op::Add, 0.1, 0.5, 1.0)
        };
        let one = Raster::of(&[dab([0.5, 0.3], 0.1, 0.5, 1.0, Op::Add)], aspect);
        let r = Raster::of(&[long], aspect);
        assert!((r.at(0.5, 0.38) - one.at(0.5, 0.38)).abs() < 0.01);
    }

    /// The paint is the one plane a raster held before the cut, to the
    /// bit, whatever the ops; the cut is laid by a Subtract only, lifted
    /// by an Add, cleared by an erase, and the two never pass one.
    #[test]
    fn a_subtract_lays_cut_an_add_lifts_it_and_an_erase_clears_both() {
        let aspect = 0.75;
        let at = [0.5, 0.3];
        let add = dab(at, 0.1, 0.0, 1.0, Op::Add);
        let cut = dab(at, 0.1, 0.0, 0.5, Op::Subtract);
        let r = Raster::of(std::slice::from_ref(&add), aspect);
        assert!(r.cut().is_none(), "no cut until a Subtract");
        assert_eq!(r.cut_at(0.5, 0.3), 0.0);
        // Over full paint: half the paint goes and half is cut.
        let r = Raster::of(&[add.clone(), cut.clone()], aspect);
        assert!((r.at(0.5, 0.3) - 0.5).abs() < 0.01);
        assert!((r.cut_at(0.5, 0.3) - 0.5).abs() < 0.01);
        // On nothing: all cut.
        let full_cut = dab(at, 0.1, 0.0, 1.0, Op::Subtract);
        let r = Raster::of(std::slice::from_ref(&full_cut), aspect);
        assert_eq!((r.at(0.5, 0.3), r.cut_at(0.5, 0.3)), (0.0, 1.0));
        // An Add over a cut lifts it by its flow.
        let half_add = dab(at, 0.1, 0.0, 0.5, Op::Add);
        let r = Raster::of(&[full_cut.clone(), half_add], aspect);
        assert!((r.at(0.5, 0.3) - 0.5).abs() < 0.01);
        assert!((r.cut_at(0.5, 0.3) - 0.5).abs() < 0.01);
        // An erase clears both, hard, at any flow.
        let erase = dab(at, 0.05, 0.9, 0.1, Op::Erase);
        let r = Raster::of(&[add.clone(), full_cut.clone(), erase], aspect);
        assert_eq!((r.at(0.5, 0.3), r.cut_at(0.5, 0.3)), (0.0, 0.0));
        assert_eq!(r.cut_at(0.58, 0.3), 1.0, "outside the erase");
        // The paint is the old raster's: subtract and lift alike take
        // the same from it.
        let ops = [Op::Add, Op::Subtract, Op::Add, Op::Erase, Op::Subtract];
        let strokes: Vec<Stroke> = ops
            .iter()
            .enumerate()
            .map(|(i, &op)| Stroke {
                points: vec![[0.3 + 0.08 * i as f32, 0.3], [0.5, 0.35]],
                ..Stroke::new(op, 0.07, 0.6, 0.6)
            })
            .collect();
        let lifted: Vec<Stroke> = strokes
            .iter()
            .map(|s| Stroke {
                op: if s.op == Op::Subtract { Op::Lift } else { s.op },
                ..s.clone()
            })
            .collect();
        let (cut, old) = (Raster::of(&strokes, aspect), Raster::of(&lifted, aspect));
        assert_eq!(cut.data(), old.data());
        assert!(old.cut().is_none());
        let planes = cut.data().iter().zip(cut.cut().unwrap());
        assert!(planes.clone().any(|(_, &c)| c > 0));
        assert!(planes.map(|(&p, &c)| p as u32 + c as u32).max() <= Some(256));
    }

    #[test]
    fn stamping_as_the_points_arrive_matches_painting_at_once() {
        let aspect = 0.75;
        let a = Stroke {
            points: vec![[0.1, 0.1], [0.3, 0.2], [0.35, 0.4], [0.6, 0.45]],
            ..Stroke::new(Op::Add, 0.03, 0.3, 0.7)
        };
        let b = Stroke {
            points: vec![[0.3, 0.2], [0.4, 0.3]],
            ..Stroke::new(Op::Subtract, 0.05, 0.0, 0.5)
        };
        let mut step = Raster::new(aspect);
        for n in 1..=a.points.len() {
            let partial = Stroke {
                points: a.points[..n].to_vec(),
                ..a.clone()
            };
            step.update(&[partial]);
        }
        for n in 1..=b.points.len() {
            let partial = Stroke {
                points: b.points[..n].to_vec(),
                ..b.clone()
            };
            step.update(&[a.clone(), partial]);
        }
        let whole = Raster::of(&[a.clone(), b.clone()], aspect);
        assert_eq!(step.data(), whole.data());
        assert!(whole.cut().is_some());
        assert_eq!(step.cut(), whole.cut());
        // Nothing to do leaves it alone; fewer strokes paints again.
        let v = step.version;
        assert!(!step.update(&[a.clone(), b.clone()]));
        assert_eq!(step.version, v);
        assert!(step.update(std::slice::from_ref(&a)));
        assert_eq!(step.data(), Raster::of(&[a], aspect).data());
    }

    /// The stroke under way replaced rather than grown, as one held
    /// straight is on every move: what is painted is what painting
    /// the strokes from nothing gives, the cut too.
    #[test]
    fn the_last_stroke_replaced_matches_painting_at_once() {
        let aspect = 0.75;
        let a = Stroke {
            points: vec![[0.1, 0.1], [0.3, 0.2], [0.35, 0.4]],
            ..Stroke::new(Op::Add, 0.03, 0.3, 0.7)
        };
        for op in [Op::Add, Op::Subtract] {
            let line = |end: Pos| Stroke {
                points: vec![[0.2, 0.3], end],
                ..Stroke::new(op, 0.05, 0.2, 0.6)
            };
            let mut step = Raster::of(&[a.clone(), line([0.2, 0.3])], aspect);
            let v = step.version;
            for end in [[0.6, 0.5], [0.4, 0.2], [0.7, 0.3]] {
                assert!(step.update(&[a.clone(), line(end)]));
            }
            assert!(step.version > v);
            let whole = Raster::of(&[a.clone(), line([0.7, 0.3])], aspect);
            assert_eq!(step.data(), whole.data(), "{op:?}");
            assert_eq!(step.cut(), whole.cut(), "{op:?}");
        }
        // A Subtract replaced by an Add over Adds alone: no cut left.
        let line = |op: Op| Stroke {
            points: vec![[0.2, 0.3], [0.6, 0.5]],
            ..Stroke::new(op, 0.05, 0.2, 0.6)
        };
        let mut step = Raster::of(&[a.clone(), line(Op::Subtract)], aspect);
        assert!(step.cut().is_some());
        assert!(step.update(&[a.clone(), line(Op::Add)]));
        let whole = Raster::of(&[a.clone(), line(Op::Add)], aspect);
        assert_eq!(step.cut(), None);
        assert_eq!(step.data(), whole.data());
    }

    /// `is_at` says what `update` would: nothing to do for the strokes
    /// as painted, with empty ones after them; something for a stroke
    /// grown, replaced, erased over, undone, or retuned.
    #[test]
    fn is_at_is_what_update_concludes() {
        let aspect = 0.75;
        let a = Stroke {
            points: vec![[0.1, 0.1], [0.3, 0.2]],
            ..Stroke::new(Op::Add, 0.03, 0.3, 0.7)
        };
        let b = Stroke {
            points: vec![[0.5, 0.2], [0.6, 0.3]],
            ..Stroke::new(Op::Subtract, 0.05, 0.0, 0.5)
        };
        let painted = Raster::of(&[a.clone(), b.clone()], aspect);
        let grown = Stroke {
            points: vec![[0.5, 0.2], [0.6, 0.3], [0.7, 0.3]],
            ..b.clone()
        };
        let moved = Stroke {
            points: vec![[0.5, 0.2], [0.4, 0.4]],
            ..b.clone()
        };
        let erase = Stroke {
            points: vec![[0.2, 0.15]],
            ..Stroke::new(Op::Erase, 0.04, 0.0, 1.0)
        };
        let wider = Stroke {
            radius: 0.04,
            ..a.clone()
        };
        let empty = Stroke::new(Op::Add, 0.05, 0.5, 1.0);
        let unchanged: [Vec<Stroke>; 3] = [
            vec![a.clone(), b.clone()],
            vec![a.clone(), b.clone(), empty.clone()],
            vec![a.clone(), b.clone(), empty.clone(), empty.clone()],
        ];
        let changed: [Vec<Stroke>; 7] = [
            vec![a.clone(), grown],
            vec![a.clone(), moved],
            vec![a.clone(), b.clone(), erase],
            vec![a.clone()],
            vec![],
            vec![wider, b.clone()],
            vec![b.clone(), a.clone()],
        ];
        for strokes in &unchanged {
            assert!(painted.is_at(strokes), "{}", strokes.len());
            let mut r = painted.clone();
            assert!(!r.update(strokes));
            assert_eq!(r.version, painted.version);
        }
        for strokes in &changed {
            assert!(!painted.is_at(strokes), "{}", strokes.len());
            let mut r = painted.clone();
            assert!(r.update(strokes));
            assert!(r.version > painted.version);
        }
        // An empty raster is at no strokes, or at empty ones alone.
        let fresh = Raster::new(aspect);
        assert!(fresh.is_at(&[]));
        assert!(fresh.is_at(std::slice::from_ref(&empty)));
        assert!(!fresh.is_at(std::slice::from_ref(&a)));
    }

    /// A stroke begun with no points yet, then dropped, and the one
    /// under way replaced: the finished strokes kept for that are
    /// still those before it, so it is painted once, not over itself.
    #[test]
    fn an_empty_stroke_leaves_the_one_under_way_unfinished() {
        let aspect = 0.75;
        let a = Stroke {
            points: vec![[0.1, 0.1], [0.3, 0.2]],
            ..Stroke::new(Op::Add, 0.03, 0.3, 0.7)
        };
        let line = |end: Pos| Stroke {
            points: vec![[0.2, 0.3], end],
            ..Stroke::new(Op::Add, 0.05, 0.2, 0.6)
        };
        let mut step = Raster::of(&[a.clone(), line([0.6, 0.5])], aspect);
        let empty = Stroke::new(Op::Add, 0.05, 0.5, 1.0);
        assert!(!step.update(&[a.clone(), line([0.6, 0.5]), empty]));
        assert!(step.update(&[a.clone(), line([0.4, 0.2])]));
        let whole = Raster::of(&[a.clone(), line([0.4, 0.2])], aspect);
        assert_eq!(step.data(), whole.data());
    }
}
