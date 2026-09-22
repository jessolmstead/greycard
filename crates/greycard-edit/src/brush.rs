//! Brush strokes and the raster they paint. A stroke is a run of
//! points in the masks' units with a size, a feather and a flow,
//! laying paint down or lifting it. The strokes are what the sidecar
//! keeps; the raster is made from them, the same way on both paths,
//! at a fixed width, so a painted mask is one thing at any export
//! size. While a stroke is under way only its new points are stamped.

use serde::{Deserialize, Serialize};

use crate::mask::{Pos, smoothstep};

/// The raster's width, texels; its height follows the picture.
pub const RASTER_WIDTH: usize = 2048;

/// What a stroke does to the paint under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Lays paint down by its flow, feathered.
    #[default]
    Add,
    /// Takes paint away by its flow, feathered.
    Subtract,
    /// Clears the whole radius, hard-edged, whatever the flow.
    Erase,
}

impl Op {
    pub const ALL: [Op; 3] = [Op::Add, Op::Subtract, Op::Erase];

    pub fn name(self) -> &'static str {
        match self {
            Op::Add => "Add",
            Op::Subtract => "Subtract",
            Op::Erase => "Erase",
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
    pub op: Op,
    /// The first day's sidecars said this for a subtract; read, not written.
    #[serde(skip_serializing)]
    erase: bool,
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
            erase: false,
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

    /// What the stroke does, the old flag counted.
    pub fn op(&self) -> Op {
        if self.erase && self.op == Op::Add {
            Op::Subtract
        } else {
            self.op
        }
    }

    /// Whether `other` is this stroke with points added.
    fn continues(&self, other: &Stroke) -> bool {
        self.radius == other.radius
            && self.feather == other.feather
            && self.flow == other.flow
            && self.op() == other.op()
            && other.points.starts_with(&self.points)
    }
}

/// The strokes painted: a texel is the finished strokes composited,
/// with the stroke under way over them. Covers u in 0 to 1 and v in
/// 0 to `aspect`.
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
        let height = ((width as f32 * aspect).round() as usize).max(1);
        Self {
            width,
            height,
            aspect,
            data: vec![0; width * height],
            base: vec![0; width * height],
            cover: vec![0; width * height],
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

    /// The texels, row by row.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Bring the raster to `strokes`: the points added since the last
    /// time are stamped; anything else is painted again from nothing.
    /// Whether anything changed.
    pub fn update(&mut self, strokes: &[Stroke]) -> bool {
        let n = self.done.len();
        let continues = n <= strokes.len()
            && self.done[..n.saturating_sub(1)] == strokes[..n.saturating_sub(1)]
            && (n == 0 || self.done[n - 1].continues(&strokes[n - 1]));
        if !continues {
            self.data.fill(0);
            self.base.fill(0);
            self.cover.fill(0);
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
    /// finished strokes with that laid over or taken away by the
    /// flow; an erase clears the disc outright.
    fn stamp(&mut self, c: (f32, f32), radius: f32, stroke: &Stroke) {
        let op = stroke.op();
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
                    Op::Subtract | Op::Erase => b * (1.0 - k),
                };
                self.data[i] = (shown * 255.0).round() as u8;
            }
        }
    }

    /// The value at (u, v) in the masks' units, between texels as a
    /// linear sampler clamped to the edge has it.
    pub fn at(&self, u: f32, v: f32) -> f32 {
        let x = (u * self.width as f32 - 0.5).clamp(0.0, (self.width - 1) as f32);
        let y = (v / self.aspect * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let t = |x: usize, y: usize| self.data[y * self.width + x] as f32 / 255.0;
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
        // The old flag reads as a subtract.
        let old: Stroke = serde_json::from_str(
            r#"{"points":[[0.5,0.3]],"radius":0.05,"feather":0,"flow":0.5,"erase":true}"#,
        )
        .unwrap();
        assert_eq!(old.op(), Op::Subtract);
        assert!(!serde_json::to_string(&old).unwrap().contains("erase"));
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
        // Nothing to do leaves it alone; fewer strokes paints again.
        let v = step.version;
        assert!(!step.update(&[a.clone(), b.clone()]));
        assert_eq!(step.version, v);
        assert!(step.update(std::slice::from_ref(&a)));
        assert_eq!(step.data(), Raster::of(&[a], aspect).data());
    }
}
