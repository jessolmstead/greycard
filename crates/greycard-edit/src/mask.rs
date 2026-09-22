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
    /// Things picked out by a model from clicks and boxes. Like a
    /// brush it has no value of its own.
    Object {
        picks: Vec<Pick>,
        boxes: Vec<[Pos; 2]>,
    },
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
            Shape::Subject {} => {}
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
            Shape::Brush { .. } | Shape::Subject {} | Shape::Object { .. } => Vec::new(),
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
            Shape::Brush { .. } | Shape::Subject {} | Shape::Object { .. } => self.clone(),
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
            Shape::Object { .. } => "Object",
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
            Shape::Brush { .. } | Shape::Subject {} | Shape::Object { .. }
        )
    }

    /// Whether a model makes the shape's raster.
    pub fn is_learned(&self) -> bool {
        matches!(self, Shape::Subject {} | Shape::Object { .. })
    }

    /// The value at (u, v); nothing for a raster shape, whose raster
    /// the caller has.
    pub fn at(&self, u: f32, v: f32) -> f32 {
        match *self {
            Shape::Brush { .. } | Shape::Subject {} | Shape::Object { .. } => 0.0,
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
        if self.is_empty() {
            return 0.0;
        }
        let mut acc = 0.0f32;
        for (i, c) in self.live() {
            let s = if c.shape.is_raster() {
                raster(i, u, v)
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
            .filter(|(_, c)| c.enabled)
    }

    /// Whether the mask says nothing: no shapes, or none switched on.
    /// Either way the adjustment does nothing, and the callers turn
    /// their local off rather than blend a mask of zeroes.
    pub fn is_empty(&self) -> bool {
        self.components.iter().all(|c| !c.enabled)
    }
}

/// Hermite's step from `e0` to `e1`, a hard step when they meet.
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
}
