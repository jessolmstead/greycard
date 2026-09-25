//! Masks: where a local adjustment acts, as shapes evaluated at a
//! point. Positions are in units of the developed picture's width,
//! both ways, so a shape keeps its proportions and survives a crop
//! or the edit's own quarter turns, which only change what is looked
//! at.
//!
//! A turn of the *frame* ([`crate::Sidecar::turn`]) is a different
//! thing and does not survive on its own: it re-orients the
//! developed picture itself, so the picture under a shape has moved.
//! [`Turned`] is the map that carries a shape with it.

use serde::{Deserialize, Serialize};

use crate::brush::Stroke;

/// A point or a pair of radii, in units of the picture's width.
pub type Pos = [f32; 2];

/// How a point in the developed picture's own units moves when the
/// picture under it is turned a quarter at a time.
///
/// Both coordinates are in units of the picture's *width*, so a
/// quarter turn is not a plain rotation of the numbers: the width
/// becomes the height, and every distance in these units grows or
/// shrinks by the old picture's aspect. Left alone through a quarter
/// turn, a radial mask on the face of a portrait frame stays at 48%
/// across, slides to 63% down and grows by half. So the map is the
/// pixel rotation with the change of unit after it: for a quarter
/// clockwise of a picture `w` by `h`, a pixel `(x, y)` goes to
/// `(h - y, x)` and the new width is `h`, which in width units is
/// `(u, v) -> (1 - v·a, u·a)` with `a = w / h`, every length times
/// `a`. A half turn keeps the width, so nothing scales.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Turned {
    /// Quarter turns clockwise, 0 to 3.
    quarters: u8,
    /// The picture's width over its height, before the turn.
    aspect: f32,
}

impl Turned {
    /// `quarters` clockwise of a picture whose width over height is
    /// `aspect`. An aspect that is not a positive number is taken as
    /// a square, which is the nearest thing to doing no harm.
    pub fn new(quarters: i32, aspect: f32) -> Self {
        Self {
            quarters: quarters.rem_euclid(4) as u8,
            aspect: if aspect.is_finite() && aspect > 0.0 {
                aspect
            } else {
                1.0
            },
        }
    }

    pub fn is_identity(self) -> bool {
        self.quarters == 0
    }

    /// What a length in these units is multiplied by: the old
    /// picture's width over the new one's.
    pub fn scale(self) -> f32 {
        if self.quarters % 2 == 1 {
            self.aspect
        } else {
            1.0
        }
    }

    /// A point of the picture.
    pub fn pos(self, p: Pos) -> Pos {
        let a = self.aspect;
        match self.quarters {
            1 => [1.0 - p[1] * a, p[0] * a],
            2 => [1.0 - p[0], 1.0 / a - p[1]],
            3 => [p[1] * a, a * (1.0 - p[0])],
            _ => p,
        }
    }

    /// A vector rather than a point — a patch's offset to its source:
    /// the turn and the scale, and no origin to move.
    pub fn offset(self, d: Pos) -> Pos {
        let s = self.scale();
        match self.quarters {
            1 => [-d[1] * s, d[0] * s],
            2 => [-d[0], -d[1]],
            3 => [d[1] * s, -d[0] * s],
            _ => d,
        }
    }

    /// A radial's angle in degrees, which grows clockwise on the
    /// screen as [`Shape::Radial`] reads it, so it turns with the
    /// picture. Kept within ±180, where the viewport's drag puts it.
    pub fn angle(self, degrees: f32) -> f32 {
        if self.is_identity() {
            return degrees;
        }
        let a = (degrees + 90.0 * f32::from(self.quarters)).rem_euclid(360.0);
        if a > 180.0 { a - 360.0 } else { a }
    }
}

/// The smallest shape a drag makes, in units of the picture's width:
/// below it there is nothing to see and nothing to grab hold of, so a
/// drag that short is a click and a radius that short is that much.
pub const MIN_RADIUS: f32 = 0.005;

/// A shape's value at a point, 0 to 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Shape {
    /// One on the `from` side of the line through `from` at right
    /// angles to `from`-`to`, nothing beyond the like line through
    /// `to`, straight between.
    Linear { from: Pos, to: Pos },
    /// One inside an ellipse turned by `angle` degrees, fading over
    /// the outer `feather` (0 to 1) of its radii to nothing at the
    /// edge.
    Radial {
        #[serde(alias = "centre")]
        center: Pos,
        radius: Pos,
        angle: f32,
        feather: f32,
    },
    /// Painted: its strokes in order, rasterized by `brush::Raster`.
    /// It has no value of its own; `Mask::at_with` is given one.
    Brush { strokes: Vec<Stroke> },
    /// The picture's subject, found by a model (greycard-ai). Like a
    /// brush it has no value of its own.
    Subject {},
    /// The picture's sky, found by a model (greycard-ai's `sky`), and
    /// nothing on a picture with none. `picks` are clicks that correct
    /// it: on sky it missed, or (not positive) on what is not sky.
    /// Like a brush it has no value of its own.
    Sky {
        #[serde(default)]
        picks: Vec<Pick>,
    },
    /// Things picked out by a model from clicks and boxes. Like a
    /// brush it has no value of its own.
    Object {
        picks: Vec<Pick>,
        boxes: Vec<[Pos; 2]>,
    },
    /// A window on the picture's lightness: one where the [`Sample`]'s
    /// lightness lies from `low` to `high`, falling to nothing over
    /// `low_feather` below `low` and `high_feather` above `high`. All
    /// four in Oklab L, 0 to 1. It has no place of its own; it is
    /// given the picture's sample at a point (`Mask::at_sampled`).
    Luminance {
        low: f32,
        high: f32,
        low_feather: f32,
        high_feather: f32,
    },
    /// A window on the picture's color in Oklch: one where the
    /// sample's hue lies within `width / 2` degrees of `hue` and its
    /// chroma is `chroma` or more; nothing past `hue_feather` degrees
    /// further round, or `chroma_feather` under the floor. Given the
    /// sample as a luminance window is.
    Color {
        hue: f32,
        width: f32,
        hue_feather: f32,
        chroma: f32,
        chroma_feather: f32,
    },
    /// A shape this build does not know, from a sidecar written by a
    /// later one: loaded so the rest of the edit is not lost with it,
    /// counted as nothing (not even joined, as a shape switched off
    /// is not), and dropped when the mask is written again.
    #[serde(other)]
    Unknown,
}

/// The picture at a point, as the range shapes read it: the developed
/// picture there before any look — before the mask's own adjustments,
/// before the global tone and color — brought to the picture's global
/// exposure (not the baseline a raw is shown brighter by), in Oklab.
/// `lightness` is the pixel's own L, not clipped: 1 is the sensor's
/// white at an exposure of nothing, and a highlight past it reads
/// more. `a` and `b` are the mean of a small box about it, the one the
/// mixer reads a hue from, so a color window does not pick noise.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Sample {
    pub lightness: f32,
    pub a: f32,
    pub b: f32,
}

impl Sample {
    /// The Oklch hue in degrees, 0 to 360.
    pub fn hue(self) -> f32 {
        self.b.atan2(self.a).to_degrees().rem_euclid(360.0)
    }

    pub fn chroma(self) -> f32 {
        self.a.hypot(self.b)
    }
}

/// The hue the vibrance protection centers on (notes §60), where skin
/// sits in Oklab: the Skin preset's center.
pub const SKIN_HUE: f32 = 55.0;

impl Shape {
    /// A luminance window over the bright part of the picture: what a
    /// new one starts at, which a sky is usually in.
    pub const LUMINANCE: Shape = Shape::Luminance {
        low: 0.7,
        high: 1.0,
        low_feather: 0.1,
        high_feather: 0.0,
    };

    /// A color window at `hue`: 30 degrees wide at full, a further 30
    /// each side to nothing, and a floor on the chroma that keeps the
    /// near-greys out, whose hue is their noise: full from 0.03, half
    /// at 0.02, nothing under 0.01, where a flat grey's local mean sits
    /// well below and a pale skin well above.
    pub fn color_at(hue: f32) -> Shape {
        Shape::Color {
            hue: hue.rem_euclid(360.0),
            width: 30.0,
            hue_feather: 30.0,
            chroma: 0.03,
            chroma_feather: 0.02,
        }
    }

    /// The Skin preset, and where a new color window starts: centered
    /// at the vibrance protection's skin hue (notes §60), but wider
    /// and lower than a window for a thing. Measured on a pale
    /// portrait (`066A3439.CR3`), the face's local mean runs from 18
    /// degrees at the lips, the nose and the pink of the forehead to
    /// 53 on the cheeks, at chromas down to 0.022 in its highlights; a
    /// window a skin's width round 55 left the pink half of that face
    /// at a quarter. So: full from 25 to 85 degrees, nothing by 0 or
    /// 110 (the greens and the blues stay out), full from a chroma of
    /// 0.02, nothing under 0.01, where the cream cardigan beside it
    /// sits.
    pub fn skin() -> Shape {
        Shape::Color {
            hue: SKIN_HUE,
            width: 60.0,
            hue_feather: 25.0,
            chroma: 0.02,
            chroma_feather: 0.01,
        }
    }

    /// Whether the shape reads the picture rather than a place in it.
    pub fn is_range(&self) -> bool {
        matches!(self, Shape::Luminance { .. } | Shape::Color { .. })
    }

    /// Whether the shape reads the picture's color, which is the mean
    /// about a pixel and costs a pass of its own.
    pub fn is_color(&self) -> bool {
        matches!(self, Shape::Color { .. })
    }

    /// A range shape's value for the picture's `sample` at a point;
    /// nothing for any other shape.
    pub fn of_sample(&self, s: Sample) -> f32 {
        match *self {
            Shape::Luminance {
                low,
                high,
                low_feather,
                high_feather,
            } => {
                // High under Low is High at Low: the sliders push one
                // another, and a sidecar written by hand is read so.
                let high = high.max(low);
                // An edge at an end of the scale is open: a window up
                // to 1 takes the highlights past the sensor's white,
                // one down from 0 everything under it.
                let rise = if low <= 0.0 {
                    1.0
                } else {
                    smoothstep(low - low_feather.max(0.0), low, s.lightness)
                };
                let fall = if high >= 1.0 {
                    1.0
                } else {
                    smoothstep(-high - high_feather.max(0.0), -high, -s.lightness)
                };
                rise * fall
            }
            Shape::Color {
                hue,
                width,
                hue_feather,
                chroma,
                chroma_feather,
            } => {
                // No chroma, no hue: a true grey is in no color's window.
                let c = s.chroma();
                if c <= 0.0 {
                    return 0.0;
                }
                let d = (s.hue() - hue + 180.0).rem_euclid(360.0) - 180.0;
                let half = (width * 0.5).max(0.0);
                let h = window(d.abs(), 0.0, half, 0.0, hue_feather.max(0.0));
                // The fade runs down from the floor to no chroma at
                // most: under that there is nothing to fade to.
                let floor = chroma.max(0.0);
                let fade = chroma_feather.clamp(0.0, floor);
                h * smoothstep(floor - fade, floor, c)
            }
            _ => 0.0,
        }
    }
}

/// One from `low` to `high`, both ends in; rising from nothing at
/// `low - low_feather`, falling to nothing at `high + high_feather`,
/// each by Hermite's step; a feather of nothing a hard edge.
pub fn window(x: f32, low: f32, high: f32, low_feather: f32, high_feather: f32) -> f32 {
    smoothstep(low - low_feather.max(0.0), low, x)
        * smoothstep(-high - high_feather.max(0.0), -high, -x)
}

impl Shape {
    /// This shape carried round with a picture that has been turned,
    /// so it covers the same pixels. A shape with no positions of
    /// its own (a subject) is unchanged; its raster is not, and the
    /// caller has to make that again.
    pub fn turn(&mut self, t: Turned) {
        if t.is_identity() {
            return;
        }
        match self {
            Shape::Linear { from, to } => {
                *from = t.pos(*from);
                *to = t.pos(*to);
            }
            Shape::Radial {
                center,
                radius,
                angle,
                ..
            } => {
                *center = t.pos(*center);
                let s = t.scale();
                *radius = [radius[0] * s, radius[1] * s];
                *angle = t.angle(*angle);
            }
            Shape::Brush { strokes } => {
                for stroke in strokes {
                    stroke.turn(t);
                }
            }
            Shape::Subject {} | Shape::Luminance { .. } | Shape::Color { .. } | Shape::Unknown => {}
            Shape::Sky { picks } => {
                for pick in picks {
                    pick.pos = t.pos(pick.pos);
                }
            }
            Shape::Object { picks, boxes } => {
                for pick in picks {
                    pick.pos = t.pos(pick.pos);
                }
                for b in boxes {
                    *b = [t.pos(b[0]), t.pos(b[1])];
                }
            }
        }
    }
}

/// A click for an object: on it, or (not positive) on what is not it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pick {
    pub pos: Pos,
    pub positive: bool,
}

/// How a shape joins what is before it in the mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Where either is.
    #[default]
    Add,
    /// Taken away from what is there.
    Subtract,
    /// Only where both are.
    Intersect,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Add, Mode::Subtract, Mode::Intersect];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Add => "Add",
            Mode::Subtract => "Subtract",
            Mode::Intersect => "Intersect",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }

    /// The sign the panel shows before a shape.
    pub fn sign(self) -> &'static str {
        match self {
            Mode::Add => "+",
            Mode::Subtract => "\u{2212}",
            Mode::Intersect => "\u{2229}",
        }
    }
}

/// A shape in a mask, and how it joins what is before it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Component {
    pub shape: Shape,
    pub mode: Mode,
    pub invert: bool,
    /// Off, the shape is not there at all: not "subtract nothing"
    /// but absent from the join, so the mask reads as it did before
    /// the shape was put in. Defaults to on, so a sidecar written
    /// before the field loads unchanged (§16: a new field with a
    /// default is not a change of meaning, so `VERSION` stands).
    pub enabled: bool,
}

impl Default for Component {
    fn default() -> Self {
        Self {
            shape: Shape::Linear {
                from: [0.0, 0.0],
                to: [0.0, 1.0],
            },
            mode: Mode::Add,
            invert: false,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Mask {
    /// Its shapes in order; one this build does not know
    /// ([`Shape::Unknown`]) is not written back.
    #[serde(serialize_with = "known_only")]
    pub components: Vec<Component>,
    pub invert: bool,
}

impl Mask {
    /// Every shape carried round with a turned picture.
    pub fn turn(&mut self, t: Turned) {
        if t.is_identity() {
            return;
        }
        for c in &mut self.components {
            c.shape.turn(t);
        }
    }
}

impl Shape {
    /// The shape a drag will draw, before it has a place.
    pub fn of_kind(kind: &str) -> Self {
        match kind {
            "Brush" => Self::Brush {
                strokes: Vec::new(),
            },
            "Subject" => Shape::Subject {},
            "Luminance" => Shape::LUMINANCE,
            "Color" => Shape::skin(),
            "Sky" => Shape::Sky { picks: Vec::new() },
            "Object" => Shape::Object {
                picks: Vec::new(),
                boxes: Vec::new(),
            },
            "Radial" => Shape::Radial {
                center: [0.0, 0.0],
                radius: [0.0, 0.0],
                angle: 0.0,
                feather: 0.5,
            },
            _ => Shape::Linear {
                from: [0.0, 0.0],
                to: [0.0, 0.0],
            },
        }
    }

    /// A shape's handles, in the masks' units: the whole first, then the
    /// points that reshape it. For a linear gradient the middle, `from`
    /// and `to`; for a radial one the center, then the ends of its axes
    /// (+x, -x, +y, -y in its own frame). A brush has none.
    pub fn handles(&self) -> Vec<(f32, f32)> {
        match *self {
            Shape::Brush { .. }
            | Shape::Subject {}
            | Shape::Sky { .. }
            | Shape::Object { .. }
            | Shape::Luminance { .. }
            | Shape::Color { .. }
            | Shape::Unknown => Vec::new(),
            Shape::Linear { from, to } => vec![
                ((from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0),
                (from[0], from[1]),
                (to[0], to[1]),
            ],
            Shape::Radial {
                center,
                radius,
                angle,
                ..
            } => {
                let (s, c) = angle.to_radians().sin_cos();
                vec![
                    (center[0], center[1]),
                    (center[0] + radius[0] * c, center[1] + radius[0] * s),
                    (center[0] - radius[0] * c, center[1] - radius[0] * s),
                    (center[0] - radius[1] * s, center[1] + radius[1] * c),
                    (center[0] + radius[1] * s, center[1] - radius[1] * c),
                ]
            }
        }
    }

    /// The shape with handle `handle` moved to `p`, in the masks' units.
    pub fn dragged(&self, handle: usize, p: (f32, f32)) -> Self {
        match *self {
            Shape::Brush { .. }
            | Shape::Subject {}
            | Shape::Sky { .. }
            | Shape::Object { .. }
            | Shape::Luminance { .. }
            | Shape::Color { .. }
            | Shape::Unknown => self.clone(),
            Shape::Linear { from, to } => match handle {
                1 => Shape::Linear {
                    from: [p.0, p.1],
                    to,
                },
                2 => Shape::Linear {
                    from,
                    to: [p.0, p.1],
                },
                _ => {
                    let mid = ((from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0);
                    let (dx, dy) = (p.0 - mid.0, p.1 - mid.1);
                    Shape::Linear {
                        from: [from[0] + dx, from[1] + dy],
                        to: [to[0] + dx, to[1] + dy],
                    }
                }
            },
            Shape::Radial {
                center,
                radius,
                angle,
                feather,
            } => {
                if handle == 0 {
                    return Shape::Radial {
                        center: [p.0, p.1],
                        radius,
                        angle,
                        feather,
                    };
                }
                // An axis end: its distance is that radius, its direction
                // turns the ellipse.
                let (dx, dy) = (p.0 - center[0], p.1 - center[1]);
                let reach = (dx * dx + dy * dy).sqrt().max(MIN_RADIUS);
                let towards = dy.atan2(dx).to_degrees();
                let (angle, radius) = match handle {
                    1 => (towards, [reach, radius[1]]),
                    2 => (towards + 180.0, [reach, radius[1]]),
                    3 => (towards - 90.0, [radius[0], reach]),
                    _ => (towards + 90.0, [radius[0], reach]),
                };
                Shape::Radial {
                    center,
                    radius,
                    angle: angle.rem_euclid(360.0),
                    feather,
                }
            }
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Shape::Linear { .. } => "Linear",
            Shape::Radial { .. } => "Radial",
            Shape::Brush { .. } => "Brush",
            Shape::Subject {} => "Subject",
            Shape::Sky { .. } => "Sky",
            Shape::Object { .. } => "Object",
            Shape::Luminance { .. } => "Luminance",
            Shape::Color { .. } => "Color",
            Shape::Unknown => "Unknown shape",
        }
    }

    pub fn is_brush(&self) -> bool {
        matches!(self, Shape::Brush { .. })
    }

    /// Whether the shape is a raster made elsewhere: a brush, or a
    /// model's mask.
    pub fn is_raster(&self) -> bool {
        matches!(
            self,
            Shape::Brush { .. } | Shape::Subject {} | Shape::Sky { .. } | Shape::Object { .. }
        )
    }

    /// Whether a model makes the shape's raster.
    pub fn is_learned(&self) -> bool {
        matches!(
            self,
            Shape::Subject {} | Shape::Sky { .. } | Shape::Object { .. }
        )
    }

    /// The value at (u, v); nothing for a raster shape, whose raster
    /// the caller has, or a range shape, whose sample it has.
    pub fn at(&self, u: f32, v: f32) -> f32 {
        match *self {
            Shape::Brush { .. }
            | Shape::Subject {}
            | Shape::Sky { .. }
            | Shape::Object { .. }
            | Shape::Luminance { .. }
            | Shape::Color { .. }
            | Shape::Unknown => 0.0,
            Shape::Linear { from, to } => {
                let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
                let len2 = dx * dx + dy * dy;
                if len2 < 1e-12 {
                    return 0.0;
                }
                let d = ((u - from[0]) * dx + (v - from[1]) * dy) / len2;
                1.0 - d.clamp(0.0, 1.0)
            }
            Shape::Radial {
                center,
                radius,
                angle,
                feather,
            } => {
                let (s, c) = angle.to_radians().sin_cos();
                let (dx, dy) = (u - center[0], v - center[1]);
                // Into the ellipse's own axes.
                let (x, y) = (c * dx + s * dy, -s * dx + c * dy);
                let (rx, ry) = (radius[0].max(1e-6), radius[1].max(1e-6));
                let e = ((x / rx).powi(2) + (y / ry).powi(2)).sqrt();
                1.0 - smoothstep(1.0 - feather.clamp(0.0, 1.0), 1.0, e)
            }
        }
    }
}

impl Mask {
    /// The mask's value at (u, v) with its brushes counted as
    /// nothing; `at_with` gives them their rasters.
    pub fn at(&self, u: f32, v: f32) -> f32 {
        self.at_with(u, v, |_, _, _| 0.0)
    }

    /// The mask's value at (u, v): each shape in order joined to,
    /// taken from, or intersected with what is before it, the whole
    /// inverted if asked. A shape switched off is skipped, whatever
    /// its mode. `raster` gives a raster component's value (a
    /// brush's, a model's) by its index in the mask.
    ///
    /// A mask with nothing live is nothing everywhere, `invert` or
    /// not: an empty mask is no place rather than every place, so
    /// switching the last shape off does not turn the adjustment on
    /// over the whole frame.
    pub fn at_with(&self, u: f32, v: f32, raster: impl Fn(usize, f32, f32) -> f32) -> f32 {
        self.at_sampled(u, v, None, raster)
    }

    /// `at_with`, the range shapes given the picture's `sample` at
    /// (u, v). Without one they are nothing, as a raster shape is
    /// without its raster.
    pub fn at_sampled(
        &self,
        u: f32,
        v: f32,
        sample: Option<Sample>,
        raster: impl Fn(usize, f32, f32) -> f32,
    ) -> f32 {
        if self.is_empty() {
            return 0.0;
        }
        let mut acc = 0.0f32;
        for (i, c) in self.live() {
            let s = if c.shape.is_raster() {
                raster(i, u, v)
            } else if c.shape.is_range() {
                sample.map_or(0.0, |s| c.shape.of_sample(s))
            } else {
                c.shape.at(u, v)
            };
            let s = if c.invert { 1.0 - s } else { s };
            acc = match c.mode {
                Mode::Add => acc + s - acc * s,
                Mode::Subtract => acc * (1.0 - s),
                Mode::Intersect => acc * s,
            };
        }
        if self.invert { 1.0 - acc } else { acc }
    }

    /// The components that count, with their index in the mask (the
    /// index a raster is held by): the switched-on ones, in order.
    pub fn live(&self) -> impl Iterator<Item = (usize, &Component)> {
        self.components
            .iter()
            .enumerate()
            .filter(|(_, c)| c.enabled && c.shape != Shape::Unknown)
    }

    /// Whether a live shape reads the picture's color, so the caller
    /// has to make the mean about each pixel.
    pub fn reads_color(&self) -> bool {
        self.live().any(|(_, c)| c.shape.is_color())
    }

    /// How many of its shapes this build does not know.
    pub fn unknown(&self) -> usize {
        self.components
            .iter()
            .filter(|c| c.shape == Shape::Unknown)
            .count()
    }

    /// Whether a live shape reads the picture, so the caller has to
    /// sample it.
    pub fn reads_picture(&self) -> bool {
        self.live().any(|(_, c)| c.shape.is_range())
    }

    /// Whether the mask says nothing: no shapes, or none switched on.
    /// Either way the adjustment does nothing, and the callers turn
    /// their local off rather than blend a mask of zeroes.
    pub fn is_empty(&self) -> bool {
        self.live().next().is_none()
    }
}

/// A mask's components as written: the ones this build knows.
fn known_only<S: serde::Serializer>(components: &[Component], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(components.iter().filter(|c| c.shape != Shape::Unknown))
}

/// Hermite's step from `e0` to `e1`, a hard step when they meet, `e1`
/// itself on the high side.
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 - e0 < 1e-6 {
        return if x < e1 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_brush_has_no_handles_and_a_drag_leaves_it() {
        let brush = Shape::Brush {
            strokes: vec![Stroke::default()],
        };
        assert!(brush.handles().is_empty());
        assert_eq!(brush.dragged(0, (0.5, 0.5)), brush);
    }

    #[test]
    fn a_shapes_handles_move_and_reshape_it() {
        let line = Shape::Linear {
            from: [0.2, 0.2],
            to: [0.4, 0.2],
        };
        let handles = line.handles();
        assert_eq!(handles[0], (0.3, 0.2));
        // The middle moves the whole; an end moves itself.
        let Shape::Linear { from, to } = line.dragged(0, (0.5, 0.5)) else {
            panic!()
        };
        for (got, want) in [from[0], from[1], to[0], to[1]]
            .iter()
            .zip([0.4, 0.5, 0.6, 0.5])
        {
            assert!((got - want).abs() < 1e-6, "{from:?} {to:?}");
        }
        let Shape::Linear { from, to } = line.dragged(2, (0.9, 0.2)) else {
            panic!()
        };
        assert_eq!((from, to), ([0.2, 0.2], [0.9, 0.2]));

        let disc = Shape::Radial {
            center: [0.5, 0.5],
            radius: [0.2, 0.1],
            angle: 0.0,
            feather: 0.5,
        };
        let h = disc.handles();
        assert_eq!(h[1], (0.7, 0.5));
        assert!((h[3].0 - 0.5).abs() < 1e-6 && (h[3].1 - 0.6).abs() < 1e-6);
        // The +x end dragged straight down: a quarter turn, the long
        // axis now a tenth longer; the short axis kept.
        let Shape::Radial {
            radius,
            angle,
            center,
            ..
        } = disc.dragged(1, (0.5, 0.8))
        else {
            panic!()
        };
        assert_eq!(center, [0.5, 0.5]);
        assert!((angle - 90.0).abs() < 1e-3, "{angle}");
        assert!((radius[0] - 0.3).abs() < 1e-6 && radius[1] == 0.1);
        // And the handles follow: +x now points down.
        let h2 = disc.dragged(1, (0.5, 0.8)).handles();
        assert!((h2[1].0 - 0.5).abs() < 1e-6 && (h2[1].1 - 0.8).abs() < 1e-6);
        // A +y end dragged keeps the +x direction it implies.
        let Shape::Radial { angle, radius, .. } = disc.dragged(3, (0.5, 0.75)) else {
            panic!()
        };
        assert!(
            angle.abs() < 1e-3 || (angle - 360.0).abs() < 1e-3,
            "{angle}"
        );
        assert!((radius[1] - 0.25).abs() < 1e-6);
        // The center moves the whole.
        let Shape::Radial { center, .. } = disc.dragged(0, (0.1, 0.1)) else {
            panic!()
        };
        assert_eq!(center, [0.1, 0.1]);
    }

    #[test]
    fn a_linear_gradient_runs_from_one_to_nothing() {
        let g = Shape::Linear {
            from: [0.2, 0.5],
            to: [0.6, 0.5],
        };
        assert_eq!(g.at(0.0, 0.1), 1.0);
        assert_eq!(g.at(0.2, 0.9), 1.0);
        assert!((g.at(0.4, 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(g.at(0.6, 0.5), 0.0);
        assert_eq!(g.at(0.9, 0.5), 0.0);
        // Along the line's own direction only: sideways changes nothing.
        assert_eq!(g.at(0.3, 0.0), g.at(0.3, 1.0));
        // A collapsed gradient is nothing.
        assert_eq!(
            Shape::Linear {
                from: [0.5, 0.5],
                to: [0.5, 0.5]
            }
            .at(0.1, 0.1),
            0.0
        );
    }

    #[test]
    fn a_radial_gradient_is_one_inside_and_feathers_to_the_edge() {
        let r = Shape::Radial {
            center: [0.5, 0.3],
            radius: [0.2, 0.1],
            angle: 0.0,
            feather: 0.5,
        };
        assert_eq!(r.at(0.5, 0.3), 1.0);
        assert_eq!(r.at(0.55, 0.3), 1.0);
        assert!(r.at(0.65, 0.3) > 0.0 && r.at(0.65, 0.3) < 1.0);
        assert_eq!(r.at(0.71, 0.3), 0.0);
        // The short axis is the short one.
        assert_eq!(r.at(0.5, 0.41), 0.0);
        // Turned a quarter, the axes swap.
        let turned = Shape::Radial {
            center: [0.5, 0.3],
            radius: [0.2, 0.1],
            angle: 90.0,
            feather: 0.0,
        };
        assert_eq!(turned.at(0.5, 0.49), 1.0);
        assert_eq!(turned.at(0.69, 0.3), 0.0);
        // No feather is a hard edge.
        assert_eq!(turned.at(0.5, 0.499), 1.0);
        assert_eq!(turned.at(0.5, 0.501), 0.0);
    }

    #[test]
    fn components_join_subtract_intersect_and_invert() {
        let disc = |center: Pos| Component {
            shape: Shape::Radial {
                center,
                radius: [0.2, 0.2],
                angle: 0.0,
                feather: 0.0,
            },
            ..Default::default()
        };
        let mut mask = Mask {
            components: vec![disc([0.3, 0.5]), disc([0.6, 0.5])],
            invert: false,
        };
        assert_eq!(mask.at(0.3, 0.5), 1.0);
        assert_eq!(mask.at(0.45, 0.5), 1.0);
        assert_eq!(mask.at(0.9, 0.5), 0.0);
        mask.components[1].mode = Mode::Subtract;
        assert_eq!(mask.at(0.45, 0.5), 0.0);
        assert_eq!(mask.at(0.15, 0.5), 1.0);
        mask.components[1].mode = Mode::Intersect;
        assert_eq!(mask.at(0.45, 0.5), 1.0);
        assert_eq!(mask.at(0.15, 0.5), 0.0);
        assert_eq!(mask.at(0.75, 0.5), 0.0);
        assert_eq!(Mode::from_name("Intersect"), Some(Mode::Intersect));
        mask.components[1].mode = Mode::Subtract;
        mask.invert = true;
        assert_eq!(mask.at(0.15, 0.5), 0.0);
        assert_eq!(mask.at(0.9, 0.5), 1.0);
        // An inverted component is the outside of it.
        mask.invert = false;
        mask.components.truncate(1);
        mask.components[0].invert = true;
        assert_eq!(mask.at(0.3, 0.5), 0.0);
        assert_eq!(mask.at(0.9, 0.5), 1.0);
        assert!(Mask::default().is_empty());
        assert_eq!(Mask::default().at(0.5, 0.5), 0.0);
    }

    #[test]
    fn learned_shapes_take_their_value_from_the_caller_and_round_trip() {
        let mask = Mask {
            components: vec![
                Component {
                    shape: Shape::Subject {},
                    mode: Mode::Add,
                    invert: false,
                    enabled: true,
                },
                Component {
                    shape: Shape::Object {
                        picks: vec![Pick {
                            pos: [0.25, 0.5],
                            positive: true,
                        }],
                        boxes: vec![[[0.1, 0.1], [0.4, 0.3]]],
                    },
                    mode: Mode::Subtract,
                    invert: false,
                    enabled: true,
                },
            ],
            invert: false,
        };
        assert_eq!(mask.at(0.5, 0.5), 0.0);
        // The subject at 0.8, the object taking away 0.5 of it.
        let v = mask.at_with(0.5, 0.5, |i, _, _| if i == 0 { 0.8 } else { 0.5 });
        assert!((v - 0.4).abs() < 1e-6);
        assert!(
            mask.components
                .iter()
                .all(|c| c.shape.is_raster() && c.shape.is_learned())
        );
        let json = serde_json::to_string(&mask).unwrap();
        assert!(json.contains("\"kind\":\"subject\"") && json.contains("\"kind\":\"object\""));
        let back: Mask = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mask);
    }

    #[test]
    fn a_sky_is_learned_and_round_trips_with_its_picks() {
        let sky = Shape::Sky {
            picks: vec![Pick {
                pos: [0.7, 0.2],
                positive: false,
            }],
        };
        assert!(sky.is_raster() && sky.is_learned());
        assert!(sky.handles().is_empty());
        assert_eq!(sky.at(0.5, 0.5), 0.0);
        assert_eq!(sky.name(), "Sky");
        assert_eq!(Shape::of_kind("Sky"), Shape::Sky { picks: Vec::new() });
        let json = serde_json::to_string(&sky).unwrap();
        assert!(json.contains("\"kind\":\"sky\""), "{json}");
        assert_eq!(serde_json::from_str::<Shape>(&json).unwrap(), sky);
        // A sky with no picks written by hand, the field left out.
        assert_eq!(
            serde_json::from_str::<Shape>(r#"{"kind":"sky"}"#).unwrap(),
            Shape::Sky { picks: Vec::new() }
        );
        // Its picks turn with the picture, as an object's do.
        let mut turned = sky.clone();
        let t = Turned::new(1, 1.5);
        turned.turn(t);
        let Shape::Sky { picks } = &turned else {
            panic!("still a sky");
        };
        assert_eq!(picks[0].pos, t.pos([0.7, 0.2]));
    }

    #[test]
    fn a_brush_component_takes_its_value_from_the_caller() {
        let mask = Mask {
            components: vec![
                Component {
                    shape: Shape::Radial {
                        center: [0.3, 0.5],
                        radius: [0.2, 0.2],
                        angle: 0.0,
                        feather: 0.0,
                    },
                    ..Default::default()
                },
                Component {
                    shape: Shape::Brush {
                        strokes: Vec::new(),
                    },
                    mode: Mode::Subtract,
                    invert: false,
                    enabled: true,
                },
            ],
            invert: false,
        };
        assert_eq!(mask.components[1].shape.name(), "Brush");
        // Without a raster the brush is nothing.
        assert_eq!(mask.at(0.3, 0.5), 1.0);
        // With one, it is what the raster says, by the component's index.
        let painted = |i: usize, u: f32, _v: f32| {
            assert_eq!(i, 1);
            if u > 0.25 { 0.5 } else { 0.0 }
        };
        assert_eq!(mask.at_with(0.2, 0.5, painted), 1.0);
        assert_eq!(mask.at_with(0.3, 0.5, painted), 0.5);
    }

    #[test]
    fn a_shape_switched_off_is_absent_from_the_join() {
        let disc = |center: Pos, mode: Mode| Component {
            shape: Shape::Radial {
                center,
                radius: [0.2, 0.2],
                angle: 0.0,
                feather: 0.0,
            },
            mode,
            ..Default::default()
        };
        let mut mask = Mask {
            components: vec![
                disc([0.3, 0.5], Mode::Add),
                disc([0.4, 0.5], Mode::Subtract),
            ],
            invert: false,
        };
        let add_alone = Mask {
            components: vec![disc([0.3, 0.5], Mode::Add)],
            invert: false,
        };
        // The two together: the second takes its disc away.
        assert_eq!(mask.at(0.45, 0.5), 0.0);
        // The subtraction switched off is not "subtract nothing" but
        // gone: the mask reads as the add alone, everywhere.
        mask.components[1].enabled = false;
        for k in 0..64 {
            let (u, v) = (k as f32 / 63.0, 0.5);
            assert_eq!(mask.at(u, v), add_alone.at(u, v));
        }
        assert!(!mask.is_empty());
        // The add switched off too and there is nothing left, whatever
        // the subtraction would have said.
        mask.components[0].enabled = false;
        mask.components[1].enabled = true;
        assert_eq!(mask.at(0.4, 0.5), 0.0);
        mask.components[1].enabled = false;
        assert!(mask.is_empty());
        for k in 0..64 {
            assert_eq!(mask.at(k as f32 / 63.0, 0.5), 0.0);
        }
        // Inverted it is still nothing: an empty mask is no place,
        // not every place, so the last shape switched off does not
        // put the adjustment over the whole frame.
        mask.invert = true;
        assert_eq!(mask.at(0.3, 0.5), 0.0);
        assert_eq!(Mask::default().at(0.5, 0.5), 0.0);
        mask.invert = false;
        // An intersection off is gone the same way: it does not leave
        // the mask multiplied by nothing.
        mask.components[0].enabled = true;
        mask.components[1] = disc([0.9, 0.5], Mode::Intersect);
        assert_eq!(mask.at(0.3, 0.5), 0.0);
        mask.components[1].enabled = false;
        assert_eq!(mask.at(0.3, 0.5), 1.0);
        // A raster component off never asks for its raster.
        let mut brushed = Mask {
            components: vec![Component {
                shape: Shape::Brush {
                    strokes: Vec::new(),
                },
                ..Default::default()
            }],
            invert: false,
        };
        assert_eq!(brushed.at_with(0.5, 0.5, |_, _, _| 1.0), 1.0);
        brushed.components[0].enabled = false;
        assert_eq!(
            brushed.at_with(0.5, 0.5, |_, _, _| panic!("asked for the raster")),
            0.0
        );
    }

    #[test]
    fn a_sidecar_written_before_the_switch_loads_switched_on() {
        // No `enabled` in the JSON: §16's rule, a new field with a
        // default, so no schema bump and an older sidecar reads whole.
        let json = r#"{"components":[{"shape":{"kind":"radial","center":[0.5,0.5],
            "radius":[0.2,0.2],"angle":0.0,"feather":0.0},"mode":"add","invert":false}],
            "invert":false}"#;
        let mask: Mask = serde_json::from_str(json).unwrap();
        assert!(mask.components[0].enabled);
        assert!(!mask.is_empty());
        assert_eq!(mask.at(0.5, 0.5), 1.0);
        // And it is written back, so the next read says the same.
        let back: Mask = serde_json::from_str(&serde_json::to_string(&mask).unwrap()).unwrap();
        assert_eq!(back, mask);
    }

    #[test]
    fn a_sidecar_written_before_the_american_spelling_still_loads() {
        // "centre" was the field name before the 2026-09-19 rename; an
        // alias keeps sidecars written before it readable.
        let json = r#"{"components":[{"shape":{"kind":"radial","centre":[0.5,0.3],
            "radius":[0.2,0.1],"angle":0.0,"feather":0.0},"mode":"add","invert":false}],
            "invert":false}"#;
        let mask: Mask = serde_json::from_str(json).unwrap();
        let Shape::Radial { center, .. } = mask.components[0].shape else {
            panic!("expected a radial shape");
        };
        assert_eq!(center, [0.5, 0.3]);
    }

    fn lab(lightness: f32, hue: f32, chroma: f32) -> Sample {
        let (s, c) = hue.to_radians().sin_cos();
        Sample {
            lightness,
            a: chroma * c,
            b: chroma * s,
        }
    }

    #[test]
    fn a_luminance_window_is_one_inside_and_feathers_outward() {
        let w = Shape::Luminance {
            low: 0.4,
            high: 0.6,
            low_feather: 0.2,
            high_feather: 0.1,
        };
        let at = |l: f32| w.of_sample(lab(l, 0.0, 0.0));
        // In full from edge to edge, both edges in.
        for l in [0.4, 0.5, 0.6] {
            assert_eq!(at(l), 1.0, "{l}");
        }
        // Half way down each feather is half, by Hermite's step.
        assert!((at(0.3) - 0.5).abs() < 1e-5, "{}", at(0.3));
        assert!((at(0.65) - 0.5).abs() < 1e-5, "{}", at(0.65));
        // And nothing past them.
        assert!(at(0.2) < 1e-6);
        assert!(at(0.7) < 1e-6);
        assert_eq!(at(0.95), 0.0);
        // No feather is a hard edge.
        let hard = Shape::Luminance {
            low: 0.4,
            high: 0.6,
            low_feather: 0.0,
            high_feather: 0.0,
        };
        assert_eq!(hard.of_sample(lab(0.399, 0.0, 0.0)), 0.0);
        assert_eq!(hard.of_sample(lab(0.601, 0.0, 0.0)), 0.0);
        // The top of the scale includes what is past display white: a
        // blown sky is in a window reaching one, not dropped out of it.
        assert_eq!(Shape::LUMINANCE.of_sample(lab(1.4, 0.0, 0.0)), 1.0);
        assert_eq!(Shape::LUMINANCE.of_sample(lab(1.0, 0.0, 0.0)), 1.0);
        assert_eq!(Shape::LUMINANCE.of_sample(lab(0.5, 0.0, 0.0)), 0.0);
        // A place means nothing to it.
        assert_eq!(w.at(0.5, 0.5), 0.0);
        assert!(w.is_range() && !w.is_raster());
    }

    #[test]
    fn a_color_window_goes_round_the_hue_circle_and_keeps_greys_out() {
        let w = Shape::Color {
            hue: 350.0,
            width: 20.0,
            hue_feather: 20.0,
            chroma: 0.05,
            chroma_feather: 0.04,
        };
        let at = |hue: f32, chroma: f32| w.of_sample(lab(0.5, hue, chroma));
        // Full within ten degrees either side, across the wrap.
        for hue in [340.0, 350.0, 359.0, 0.0] {
            assert_eq!(at(hue, 0.1), 1.0, "{hue}");
        }
        // Half way through the feather on both sides.
        assert!((at(10.0, 0.1) - 0.5).abs() < 1e-4, "{}", at(10.0, 0.1));
        assert!((at(330.0, 0.1) - 0.5).abs() < 1e-4, "{}", at(330.0, 0.1));
        assert_eq!(at(20.0, 0.1), 0.0);
        assert_eq!(at(170.0, 0.1), 0.0);
        // The chroma floor: full at it, half half way down its
        // feather, nothing under it, whatever the hue says.
        assert_eq!(at(350.0, 0.05), 1.0);
        assert!((at(350.0, 0.03) - 0.5).abs() < 1e-4);
        assert_eq!(at(350.0, 0.005), 0.0);
        assert_eq!(at(350.0, 0.0), 0.0);
        // The lightness is none of its business.
        assert_eq!(
            w.of_sample(lab(0.05, 350.0, 0.1)),
            w.of_sample(lab(0.95, 350.0, 0.1))
        );
    }

    #[test]
    fn the_skin_preset_takes_a_whole_face_and_not_what_is_beside_it() {
        let skin = Shape::skin();
        let Shape::Color { hue, .. } = skin else {
            panic!()
        };
        assert_eq!(hue, SKIN_HUE);
        // The face of the portrait the preset was measured on, as its
        // samples read (hue, chroma): the pink of the forehead and
        // the nose, under the eyes, the cheeks; all in, nearly whole.
        for (h, c) in [
            (19.2, 0.023),
            (22.0, 0.0275),
            (20.4, 0.0315),
            (18.3, 0.0226),
            (39.7, 0.031),
            (43.2, 0.0397),
            (53.3, 0.0382),
        ] {
            let w = skin.of_sample(lab(0.55, h, c));
            assert!(w > 0.8, "{h} {c}: {w}");
        }
        // And beside it: the cream cardigan (a near grey), the window
        // frame's green, the jeans' blue.
        for (h, c) in [(51.5, 0.01), (111.1, 0.0216), (265.5, 0.0569)] {
            assert_eq!(skin.of_sample(lab(0.55, h, c)), 0.0, "{h} {c}");
        }
        // A new color shape starts there, and a click moves it.
        assert_eq!(Shape::of_kind("Color"), Shape::skin());
        let Shape::Color { hue, .. } = Shape::color_at(-30.0) else {
            panic!()
        };
        assert_eq!(hue, 330.0);
    }

    #[test]
    fn a_range_shape_intersects_with_a_drawn_one() {
        // A sky: the top of the frame, and of that only what is bright.
        let mask = Mask {
            components: vec![
                Component {
                    shape: Shape::Linear {
                        from: [0.0, 0.0],
                        to: [0.0, 0.5],
                    },
                    ..Default::default()
                },
                Component {
                    shape: Shape::LUMINANCE,
                    mode: Mode::Intersect,
                    ..Default::default()
                },
            ],
            invert: false,
        };
        assert!(mask.reads_picture());
        let none = |_: usize, _: f32, _: f32| 0.0;
        let bright = Some(lab(0.9, 0.0, 0.0));
        let dark = Some(lab(0.3, 0.0, 0.0));
        assert_eq!(mask.at_sampled(0.5, 0.0, bright, none), 1.0);
        assert_eq!(mask.at_sampled(0.5, 0.0, dark, none), 0.0);
        assert_eq!(mask.at_sampled(0.5, 0.9, bright, none), 0.0);
        // Without a sample the range shape is nothing, as a raster is
        // without its raster: the intersection is empty.
        assert_eq!(mask.at(0.5, 0.0), 0.0);
        // Switched off it is not read, and the mask does not ask.
        let mut off = mask.clone();
        off.components[1].enabled = false;
        assert!(!off.reads_picture());
        assert_eq!(off.at(0.5, 0.0), 1.0);
        // Inverted, it is the dark part of the top.
        let mut shade = mask.clone();
        shade.components[1].invert = true;
        assert_eq!(shade.at_sampled(0.5, 0.0, dark, none), 1.0);
        // A turn leaves it as it is: it has no place to carry.
        let mut turned = mask.clone();
        turned.turn(Turned::new(1, 1.5));
        assert_eq!(turned.components[1], mask.components[1]);
        assert!(Shape::LUMINANCE.handles().is_empty());
    }

    #[test]
    fn range_shapes_round_trip_and_older_sidecars_still_load() {
        let mask = Mask {
            components: vec![
                Component {
                    shape: Shape::LUMINANCE,
                    ..Default::default()
                },
                Component {
                    shape: Shape::skin(),
                    mode: Mode::Subtract,
                    invert: true,
                    enabled: false,
                },
            ],
            invert: true,
        };
        let json = serde_json::to_string(&mask).unwrap();
        assert!(json.contains("\"kind\":\"luminance\""), "{json}");
        assert!(json.contains("\"kind\":\"color\""), "{json}");
        assert!(json.contains("\"low_feather\"") && json.contains("\"chroma_feather\""));
        let back: Mask = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mask);
        // One written by hand, as a later sidecar would have it.
        let written = r#"{"components":[{"shape":{"kind":"color","hue":200.0,"width":40.0,
            "hue_feather":10.0,"chroma":0.05,"chroma_feather":0.02},"mode":"intersect",
            "invert":false,"enabled":true}],"invert":false}"#;
        let m: Mask = serde_json::from_str(written).unwrap();
        assert_eq!(m.components[0].mode, Mode::Intersect);
        assert_eq!(m.components[0].shape.name(), "Color");
    }

    #[test]
    fn a_luminance_window_is_open_at_the_ends_and_high_never_under_low() {
        let at = |w: &Shape, l: f32| w.of_sample(lab(l, 0.0, 0.0));
        // Pushed to the top, it takes what is past the sensor's white;
        // to the bottom, what is under nothing (a negative channel's
        // cube root), whatever the fades say.
        let top = Shape::Luminance {
            low: 0.9,
            high: 1.0,
            low_feather: 0.0,
            high_feather: 0.3,
        };
        assert_eq!(at(&top, 1.0), 1.0);
        assert_eq!(at(&top, 1.8), 1.0);
        let bottom = Shape::Luminance {
            low: 0.0,
            high: 0.2,
            low_feather: 0.3,
            high_feather: 0.0,
        };
        assert_eq!(at(&bottom, 0.0), 1.0);
        assert_eq!(at(&bottom, -0.1), 1.0);
        // Short of the top, the high edge falls as ever: a highlight
        // past it is out.
        let bright = Shape::Luminance {
            low: 0.7,
            high: 0.95,
            low_feather: 0.0,
            high_feather: 0.02,
        };
        assert_eq!(at(&bright, 0.95), 1.0);
        assert_eq!(at(&bright, 1.2), 0.0);
        // High under Low is High at Low: not a mask of nothing, nor a
        // bump between the two fades.
        let crossed = Shape::Luminance {
            low: 0.8,
            high: 0.2,
            low_feather: 0.0,
            high_feather: 0.0,
        };
        assert_eq!(at(&crossed, 0.8), 1.0);
        assert_eq!(at(&crossed, 0.5), 0.0);
        let soft = Shape::Luminance {
            low: 0.5,
            high: 0.4,
            low_feather: 0.1,
            high_feather: 0.1,
        };
        assert_eq!(at(&soft, 0.5), 1.0);
        assert!(at(&soft, 0.45) < 1.0 && at(&soft, 0.55) < 1.0);
    }

    #[test]
    fn a_grey_has_no_hue_and_the_chroma_fade_stops_at_none() {
        // A floor of nothing and no fade: every color at its hue, and
        // no true grey, which has no hue to be in the window by.
        let bare = Shape::Color {
            hue: 0.0,
            width: 20.0,
            hue_feather: 0.0,
            chroma: 0.0,
            chroma_feather: 0.0,
        };
        assert_eq!(bare.of_sample(lab(0.5, 0.0, 0.0)), 0.0);
        assert_eq!(bare.of_sample(lab(0.5, 5.0, 0.001)), 1.0);
        // A fade longer than the floor is the floor: it starts at no
        // chroma, so a grey is out and half the floor is half in.
        let long = Shape::Color {
            hue: 0.0,
            width: 20.0,
            hue_feather: 0.0,
            chroma: 0.02,
            chroma_feather: 0.05,
        };
        assert_eq!(long.of_sample(lab(0.5, 0.0, 0.0)), 0.0);
        assert!((long.of_sample(lab(0.5, 0.0, 0.01)) - 0.5).abs() < 1e-5);
        assert_eq!(long.of_sample(lab(0.5, 0.0, 0.02)), 1.0);
    }

    #[test]
    fn a_shape_from_a_later_build_is_nothing_and_the_rest_is_kept() {
        // A made-up kind between two shapes this build knows, with
        // fields of its own; the intersection it is in is not left
        // multiplied by nothing.
        let json = r#"{"components":[
            {"shape":{"kind":"radial","center":[0.5,0.5],"radius":[0.2,0.2],
                "angle":0.0,"feather":0.0},"mode":"add","invert":false},
            {"shape":{"kind":"depth","near":0.2,"far":[1,2]},"mode":"intersect",
                "invert":false,"enabled":true},
            {"shape":{"kind":"luminance","low":0.5,"high":1.0,"low_feather":0.1,
                "high_feather":0.0},"mode":"subtract","invert":false}],
            "invert":false}"#;
        let mask: Mask = serde_json::from_str(json).unwrap();
        assert_eq!(mask.components.len(), 3);
        assert_eq!(mask.components[1].shape, Shape::Unknown);
        assert_eq!(mask.unknown(), 1);
        assert_eq!(mask.at(0.5, 0.5), 1.0);
        assert_eq!(mask.live().count(), 2);
        assert!(mask.reads_picture());
        // Written again, it is gone and the others are as they were.
        let back: Mask = serde_json::from_str(&serde_json::to_string(&mask).unwrap()).unwrap();
        assert_eq!(back.components.len(), 2);
        assert_eq!(back.components[0], mask.components[0]);
        assert_eq!(back.components[1], mask.components[2]);
        // A mask of nothing but one is empty, not everywhere.
        let only: Mask = serde_json::from_str(
            r#"{"components":[{"shape":{"kind":"depth"},"mode":"add"}],"invert":true}"#,
        )
        .unwrap();
        assert!(only.is_empty());
        assert_eq!(only.at(0.5, 0.5), 0.0);
    }
}
