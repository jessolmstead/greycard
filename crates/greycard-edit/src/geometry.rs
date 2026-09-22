//! Orientation, straighten, perspective and crop: the picture turned
//! by quarters, mirrored, turned about its center by a fine angle,
//! its keystone taken out, and a rectangle of that leveled plane
//! shown. Crop positions are fractions of the plane's width and
//! height so an edit fits any size of the same frame.

use serde::{Deserialize, Serialize};

/// A rectangle of the leveled plane, fractions of the source's size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The shape a crop keeps.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Aspect {
    #[default]
    Free,
    /// The plane's own, the way up it already is; `portrait` turns it.
    Original,
    /// Width to height, as landscape; `portrait` turns it.
    Ratio { w: f32, h: f32 },
}

impl Aspect {
    /// Width over height for a plane of `w` by `h`, none when free.
    ///
    /// A named ratio is written as landscape and `portrait` turns it,
    /// so the flag is the whole of what says which way up it goes.
    /// The plane's own ratio already says that — a frame shot on end
    /// arrives here as a tall `w` by `h` — so `Original` keeps it and
    /// `portrait` turns that. Standing it up as landscape first and
    /// reading the way up off a flag that starts false is what sent
    /// a portrait frame's "Original" crop straight to landscape.
    pub fn ratio(self, w: f32, h: f32, portrait: bool) -> Option<f32> {
        let upright = match self {
            Aspect::Free => return None,
            Aspect::Original => (w / h).max(1e-3),
            Aspect::Ratio { w, h } => {
                let r = (w / h).max(1e-3);
                if r >= 1.0 { r } else { 1.0 / r }
            }
        };
        Some(if portrait { 1.0 / upright } else { upright })
    }

    /// The aspect a name from the panel's list stands for, with the text
    /// of the custom box for when the name is "Custom". An unknown name,
    /// or a custom box that will not read, is Free.
    pub fn from_name(name: &str, custom: &str) -> Self {
        match name {
            "Free" => Aspect::Free,
            "Original" => Aspect::Original,
            "Custom" => Self::parse_ratio(custom).unwrap_or(Aspect::Free),
            other => PRESETS
                .iter()
                .find(|(n, _)| *n == other)
                .map(|(_, a)| *a)
                .unwrap_or(Aspect::Free),
        }
    }

    /// The name for this aspect, and the custom box's text when it
    /// needs one: the inverse of [`Aspect::from_name`].
    pub fn name(self) -> (&'static str, Option<String>) {
        match self {
            Aspect::Free => ("Free", None),
            Aspect::Original => ("Original", None),
            Aspect::Ratio { w, h } => {
                let preset = PRESETS.iter().find(|(_, a)| {
                matches!(a, Aspect::Ratio { w: pw, h: ph } if (pw - w).abs() < 1e-4 && (ph - h).abs() < 1e-4)
            });
                match preset {
                    Some((name, _)) => (name, None),
                    None => ("Custom", Some(format!("{w}:{h}"))),
                }
            }
        }
    }

    /// "65:24", "65/24", "65x24" or "2.7" as a ratio; none for text
    /// that will not read, or a side at nothing.
    pub fn parse_ratio(text: &str) -> Option<Self> {
        let text = text.trim();
        let (w, h) = match text.split_once([':', '/', 'x', 'X']) {
            Some((a, b)) => (a.trim().parse::<f32>().ok()?, b.trim().parse::<f32>().ok()?),
            None => (text.parse::<f32>().ok()?, 1.0),
        };
        (w > 0.0 && h > 0.0).then_some(Aspect::Ratio { w, h })
    }
}

/// The presets the panel offers, by name; "Free", "Original" and a
/// custom ratio besides.
pub const PRESETS: [(&str, Aspect); 6] = [
    ("1:1", Aspect::Ratio { w: 1.0, h: 1.0 }),
    ("3:2", Aspect::Ratio { w: 3.0, h: 2.0 }),
    ("4:3", Aspect::Ratio { w: 4.0, h: 3.0 }),
    ("5:4", Aspect::Ratio { w: 5.0, h: 4.0 }),
    ("16:9", Aspect::Ratio { w: 16.0, h: 9.0 }),
    ("65:24", Aspect::Ratio { w: 65.0, h: 24.0 }),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Geometry {
    /// Quarter turns counter-clockwise on the screen, 0 to 3.
    pub turns: u8,
    /// The source mirrored left to right before it is turned.
    pub flip: bool,
    /// Degrees the picture turns counter-clockwise on the screen, ±45.
    pub angle: f32,
    /// The keystone taken out, as the tilt of the camera in degrees
    /// for a lens whose focal length is the plane's half height (a
    /// vertical field of 90 degrees): positive for a camera that
    /// looked up, whose verticals converge at the top. A wider lens
    /// wants less for the same tilt, a longer one more. ±[`MAX_TILT`].
    pub vertical: f32,
    /// The same about the vertical axis, for the plane's half width:
    /// positive for a camera that looked to the right, whose
    /// horizontals converge there.
    pub horizontal: f32,
    /// The crop, none for the whole frame (the largest that fits, when
    /// turned).
    pub crop: Option<Crop>,
    pub aspect: Aspect,
    pub portrait: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            turns: 0,
            flip: false,
            angle: 0.0,
            vertical: 0.0,
            horizontal: 0.0,
            crop: None,
            aspect: Aspect::Free,
            portrait: false,
        }
    }
}

/// The largest turn allowed, degrees.
pub const MAX_ANGLE: f32 = 45.0;
/// The largest keystone allowed, degrees of tilt: the far edge of the
/// plane shown at nearly six times the near one.
pub const MAX_TILT: f32 = 40.0;
/// Where a plane point is taken as beyond the horizon: the
/// perspective's divisor is held above this, so the point lands far
/// outside the source rather than nowhere or, worse, mirrored back
/// inside it.
const HORIZON: f32 = 1e-4;
/// The smallest crop, as a fraction of the source's shorter side.
pub const MIN_CROP: f32 = 0.02;

/// A rectangle of the leveled plane in source pixels: what the
/// viewport shows or the export writes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub origin: (f32, f32),
    pub size: (f32, f32),
}

impl Geometry {
    pub fn is_identity(&self) -> bool {
        self.turns == 0
            && !self.flip
            && self.angle == 0.0
            && !self.keystoned()
            && self.crop.is_none()
    }

    /// Whether a plane pixel lands between source pixels: the fine
    /// angle and the keystone do that; quarter turns and the mirror
    /// land on them.
    pub fn resamples(&self) -> bool {
        self.angle != 0.0 || self.keystoned()
    }

    /// Whether the perspective does anything.
    pub fn keystoned(&self) -> bool {
        self.vertical != 0.0 || self.horizontal != 0.0
    }

    /// The perspective's row: the divisor for a plane point about
    /// the center is one plus this dotted with the point, in plane
    /// pixels. Tangents of the tilts over the half sides, so the
    /// plane's edges sit at one plus or minus the tangent: the far
    /// edge (the top for a camera that looked up) divides by more
    /// and so reaches less of the source, spreading it out.
    pub fn perspective(&self, w: f32, h: f32) -> [f32; 2] {
        let (pw, ph) = self.plane_size(w, h);
        let kh = self
            .horizontal
            .clamp(-MAX_TILT, MAX_TILT)
            .to_radians()
            .tan();
        let kv = self.vertical.clamp(-MAX_TILT, MAX_TILT).to_radians().tan();
        [kh / (pw / 2.0), -kv / (ph / 2.0)]
    }

    /// The leveled plane's size for a source `w` by `h`: the source's,
    /// or turned on its side by an odd quarter turn.
    pub fn plane_size(&self, w: f32, h: f32) -> (f32, f32) {
        if self.turns % 2 == 1 { (h, w) } else { (w, h) }
    }

    /// The matrix taking a point of the plane, about its center, to
    /// the source about its center: the fine turn, then the quarter
    /// turns (exact), then the mirror. Rows.
    pub fn matrix(&self) -> [[f32; 2]; 2] {
        let (s, c) = self.angle.to_radians().sin_cos();
        // The fine turn as `to_source` had it.
        let r = [[c, -s], [s, c]];
        let q: [[f32; 2]; 2] = match self.turns % 4 {
            0 => [[1.0, 0.0], [0.0, 1.0]],
            1 => [[0.0, -1.0], [1.0, 0.0]],
            2 => [[-1.0, 0.0], [0.0, -1.0]],
            _ => [[0.0, 1.0], [-1.0, 0.0]],
        };
        let mut m = [[0.0f32; 2]; 2];
        for i in 0..2 {
            for j in 0..2 {
                m[i][j] = q[i][0] * r[0][j] + q[i][1] * r[1][j];
            }
        }
        if self.flip {
            m[0] = [-m[0][0], -m[0][1]];
        }
        m
    }

    /// The source point under a point of the leveled plane, both in
    /// pixels with the origin at the top left of each: the perspective
    /// first, about the plane's center, then the matrix. A point
    /// beyond the horizon lands far outside the source.
    pub fn to_source(&self, r: (f32, f32), w: f32, h: f32) -> (f32, f32) {
        let (pw, ph) = self.plane_size(w, h);
        let m = self.matrix();
        let k = self.perspective(w, h);
        let (dx, dy) = (r.0 - pw / 2.0, r.1 - ph / 2.0);
        let d = (1.0 + k[0] * dx + k[1] * dy).max(HORIZON);
        let (dx, dy) = (dx / d, dy / d);
        (
            w / 2.0 + m[0][0] * dx + m[0][1] * dy,
            h / 2.0 + m[1][0] * dx + m[1][1] * dy,
        )
    }

    /// Whether a point of the leveled plane is on the near side of
    /// the horizon, where it stands for a source point at all.
    pub fn in_front(&self, r: (f32, f32), w: f32, h: f32) -> bool {
        let (pw, ph) = self.plane_size(w, h);
        let k = self.perspective(w, h);
        1.0 + k[0] * (r.0 - pw / 2.0) + k[1] * (r.1 - ph / 2.0) > HORIZON
    }

    /// The point of the leveled plane over a source point: the
    /// inverse of `to_source`, the matrix's transpose since it is
    /// orthogonal, then the perspective undone (its inverse has the
    /// row negated). A source point beyond the plane's horizon lands
    /// far out in the plane.
    pub fn to_plane(&self, s: (f32, f32), w: f32, h: f32) -> (f32, f32) {
        let (pw, ph) = self.plane_size(w, h);
        let m = self.matrix();
        let k = self.perspective(w, h);
        let (dx, dy) = (s.0 - w / 2.0, s.1 - h / 2.0);
        let (ux, uy) = (m[0][0] * dx + m[1][0] * dy, m[0][1] * dx + m[1][1] * dy);
        let d = (1.0 - k[0] * ux - k[1] * uy).max(HORIZON);
        (pw / 2.0 + ux / d, ph / 2.0 + uy / d)
    }

    /// The geometry turned by `quarters` more quarter turns
    /// counter-clockwise on the screen, the crop turning with the
    /// picture and a held aspect swapping its way.
    pub fn turned(&self, quarters: i32) -> Self {
        let mut g = self.clone();
        let q = quarters.rem_euclid(4) as u8;
        g.turns = (self.turns + q) % 4;
        // The keystone turns with the picture: what converged at the
        // top converges on the left after a quarter counter-clockwise,
        // and what converged on the right, at the top.
        let (kh, kv) = (self.horizontal, self.vertical);
        // A negated zero is zero, but it is not what a file should
        // say: a turned edit with no keystone would otherwise differ
        // from a fresh one byte for byte.
        let zeroed = |v: f32| if v == 0.0 { 0.0 } else { v };
        (g.horizontal, g.vertical) = match q {
            0 => (kh, kv),
            1 => (zeroed(-kv), kh),
            2 => (zeroed(-kh), zeroed(-kv)),
            _ => (kv, zeroed(-kh)),
        };
        if let Some(c) = self.crop {
            g.crop = Some(match q {
                0 => c,
                // (x, y) to (y, 1 - x).
                1 => Crop {
                    x: c.y,
                    y: 1.0 - c.x - c.w,
                    w: c.h,
                    h: c.w,
                },
                2 => Crop {
                    x: 1.0 - c.x - c.w,
                    y: 1.0 - c.y - c.h,
                    w: c.w,
                    h: c.h,
                },
                _ => Crop {
                    x: 1.0 - c.y - c.h,
                    y: c.x,
                    w: c.h,
                    h: c.w,
                },
            });
        }
        // A named ratio held across a quarter turn stands the other
        // way up on the screen, and the flag is the only thing that
        // says so. `Original` needs no flip: the plane it is read
        // against turns too, and turning the flag as well would turn
        // it twice.
        if q % 2 == 1 && matches!(self.aspect, Aspect::Ratio { .. }) {
            g.portrait = !self.portrait;
        }
        g
    }

    /// The geometry for a source that has itself been turned
    /// `quarters` quarter turns clockwise under it: the frame's own
    /// orientation changed (`Sidecar::turn`), so everything the
    /// geometry says about *where* on the picture it acts has to
    /// turn with the picture, while what it says about which way up
    /// the picture is stays put.
    ///
    /// So the crop, the keystone and a held aspect turn exactly as
    /// [`Geometry::turned`] turns them the other way, and the
    /// quarter turns do not move: the source turned right already
    /// turns the picture right. A mirror is the one exception. It
    /// reverses which way a turn of the source reads on the screen,
    /// so a mirrored edit takes a half turn to keep the screen
    /// turning the way the key asked.
    ///
    /// Masks are not touched. They are in the developed picture's
    /// own units (see [`crate::mask`]), which is where an edit's own
    /// quarter turns leave them too: a turn moves what is under a
    /// mask, it does not carry the mask with it.
    pub fn under_turned_source(&self, quarters: i32) -> Self {
        let mut g = self.turned(-quarters);
        let half = if self.flip && quarters.rem_euclid(2) == 1 {
            2
        } else {
            0
        };
        g.turns = (self.turns + half) % 4;
        g
    }

    /// The quarter turns and the mirror a small picture of the
    /// *unturned* source has to be shown with, for a source this
    /// geometry sits on that carries `turn` quarter turns clockwise
    /// of its own: what the filmstrip, the grid and the culling
    /// loupe need, since all three draw the camera's own picture,
    /// which the turn has not been applied to.
    ///
    /// Folding the turn in rather than turning the picture first is
    /// what makes a turn free in those three: the same matrix the
    /// edit's quarter turns already ride on carries it.
    pub fn shown_turns(&self, turn: u8) -> (u8, bool) {
        let turn = i32::from(turn % 4);
        let q = if self.flip { turn } else { -turn };
        ((i32::from(self.turns) + q).rem_euclid(4) as u8, self.flip)
    }

    /// The geometry mirrored on the screen, left to right, or top to
    /// bottom when `vertical`: the mirror toggles and the turns
    /// reverse, so the picture stays level, and the crop mirrors.
    pub fn flipped(&self, vertical: bool) -> Self {
        let mut g = self.clone();
        g.flip = !self.flip;
        g.angle = -self.angle;
        g.turns = (4 - self.turns + if vertical { 2 } else { 0 }) % 4;
        // A mirror reverses the keystone along its own axis.
        if vertical {
            g.vertical = -self.vertical;
        } else {
            g.horizontal = -self.horizontal;
        }
        if let Some(c) = self.crop {
            g.crop = Some(if vertical {
                Crop {
                    y: 1.0 - c.y - c.h,
                    ..c
                }
            } else {
                Crop {
                    x: 1.0 - c.x - c.w,
                    ..c
                }
            });
        }
        g
    }

    /// Whether a rectangle of the leveled plane lies wholly on the
    /// source.
    pub fn fits(&self, crop: &Crop, w: f32, h: f32) -> bool {
        let inside =
            |p: (f32, f32)| p.0 >= -0.01 && p.1 >= -0.01 && p.0 <= w + 0.01 && p.1 <= h + 0.01;
        let (pw, ph) = self.plane_size(w, h);
        let (x0, y0) = (crop.x * pw, crop.y * ph);
        let (x1, y1) = ((crop.x + crop.w) * pw, (crop.y + crop.h) * ph);
        // A rectangle's corners bound it through the perspective too,
        // since straight lines stay straight; but only in front of
        // the horizon, past which a corner would come back mirrored.
        [(x0, y0), (x1, y0), (x0, y1), (x1, y1)]
            .into_iter()
            .all(|p| self.in_front(p, w, h) && inside(self.to_source(p, w, h)))
    }

    /// A drag of a handle by `dx, dy` (fractions of the plane) kept on
    /// a source `w` by `h`: the whole of it where that fits, else as
    /// much of it as does. The offset is first scaled back until the
    /// crop lands on the source, then whatever is left of each axis is
    /// tried on its own, so a crop pushed into one edge slides along
    /// it rather than stopping dead.
    #[allow(clippy::too_many_arguments)]
    pub fn drag_within(
        &self,
        start: &Crop,
        handle: Handle,
        dx: f32,
        dy: f32,
        ratio: Option<f32>,
        w: f32,
        h: f32,
    ) -> Crop {
        let (pw, ph) = self.plane_size(w, h);
        let at = |ox: f32, oy: f32| start.dragged(handle, ox, oy, ratio, pw, ph);
        let whole = at(dx, dy);
        if self.fits(&whole, w, h) {
            return whole;
        }
        // The largest share of the whole drag that still fits, then
        // the rest of each axis alone from there.
        let share = |from: (f32, f32), to: (f32, f32)| {
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            for _ in 0..16 {
                let mid = (lo + hi) / 2.0;
                let p = (
                    from.0 + (to.0 - from.0) * mid,
                    from.1 + (to.1 - from.1) * mid,
                );
                if self.fits(&at(p.0, p.1), w, h) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            (from.0 + (to.0 - from.0) * lo, from.1 + (to.1 - from.1) * lo)
        };
        let mut off = share((0.0, 0.0), (dx, dy));
        off = share(off, (dx, off.1));
        off = share(off, (off.0, dy));
        at(off.0, off.1)
    }

    /// The largest crop about `center` (fractions) of width over height
    /// `ratio` that lies on the turned source.
    pub fn largest_fit(&self, center: (f32, f32), ratio: f32, w: f32, h: f32) -> Crop {
        // Half sizes in plane pixels for a scale of one: the aspect in
        // pixels, so a ratio holds whatever the source's shape.
        let (pw, ph) = self.plane_size(w, h);
        let (a, b) = (ratio, 1.0);
        let at = |scale: f32| Crop {
            x: center.0 - a * scale / pw,
            y: center.1 - b * scale / ph,
            w: 2.0 * a * scale / pw,
            h: 2.0 * b * scale / ph,
        };
        let (mut lo, mut hi) = (0.0f32, w.max(h));
        for _ in 0..48 {
            let mid = (lo + hi) / 2.0;
            if self.fits(&at(mid), w, h) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let mut crop = at(lo);
        if self.angle == 0.0 {
            // Unturned, the fit is against the source's own edges:
            // land on them exactly rather than a hair inside or out.
            let x1 = (crop.x + crop.w).min(1.0);
            let y1 = (crop.y + crop.h).min(1.0);
            crop.x = crop.x.max(0.0);
            crop.y = crop.y.max(0.0);
            crop.w = x1 - crop.x;
            crop.h = y1 - crop.y;
        }
        crop
    }

    /// The crop in force: the one set, or the largest that fits.
    pub fn effective_crop(&self, w: f32, h: f32) -> Crop {
        match self.crop {
            Some(c) if self.fits(&c, w, h) => c,
            _ => {
                let (pw, ph) = self.plane_size(w, h);
                let ratio = self.aspect.ratio(pw, ph, self.portrait).unwrap_or(pw / ph);
                self.largest_fit((0.5, 0.5), ratio, w, h)
            }
        }
    }

    /// What the export writes and the viewport shows: the crop in
    /// source pixels of the leveled plane.
    pub fn frame(&self, w: f32, h: f32) -> Frame {
        let (pw, ph) = self.plane_size(w, h);
        let c = self.effective_crop(w, h);
        Frame {
            origin: (c.x * pw, c.y * ph),
            size: ((c.w * pw).round().max(1.0), (c.h * ph).round().max(1.0)),
        }
    }

    /// The turned source's whole extent in the leveled plane, for the
    /// crop's editing.
    pub fn bounds(&self, w: f32, h: f32) -> Frame {
        let (pw, ph) = self.plane_size(w, h);
        let (mut hx, mut hy) = (0.0f32, 0.0f32);
        for corner in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
            let p = self.to_plane(corner, w, h);
            hx = hx.max((p.0 - pw / 2.0).abs());
            hy = hy.max((p.1 - ph / 2.0).abs());
        }
        Frame {
            origin: (pw / 2.0 - hx, ph / 2.0 - hy),
            size: ((2.0 * hx).ceil(), (2.0 * hy).ceil()),
        }
    }

    /// The angle that levels a line drawn at `degrees` counter-clockwise
    /// on the screen: the nearer of horizontal and vertical is taken,
    /// and the result added to the turn in force, within its limit.
    pub fn leveled_by(&self, degrees: f32) -> f32 {
        let mut d = degrees.rem_euclid(360.0);
        while d > 45.0 {
            d -= 90.0;
        }
        (self.angle - d).clamp(-MAX_ANGLE, MAX_ANGLE)
    }

    /// The turn and the keystone that make two lines of the source run
    /// the way `axis` asks, from a stroke drawn along each: a stroke is
    /// its two ends in source pixels, and the two may be given either
    /// way round and in either order.
    ///
    /// The two lines meet at a vanishing point. Every plane point at
    /// infinity along (0, 1) reaches the one source point, which the
    /// matrix untwists to `(0, 1 / k.y)`: so the turn is the one that
    /// puts the vanishing point on the plane's vertical axis, and the
    /// tilt the one whose perspective row sends it to infinity. The two
    /// axes solve apart, since the vertical vanishing point does not
    /// depend on `k.x` nor the horizontal one on `k.y`; the keystone
    /// already in force is left alone, and the turn is the whole turn,
    /// not a correction to the one in force, so the tool run twice on
    /// its own result says the same thing.
    ///
    /// The turns and the mirror are read from `self`, which is where
    /// the matrix's fixed part comes from; nothing else is. `None` when
    /// a stroke is shorter than a pixel, when the two lie on one line,
    /// or when they cross within a pixel of the center, where the
    /// answer would be a tilt of a quarter turn.
    pub fn guided_by(
        &self,
        a: [(f32, f32); 2],
        b: [(f32, f32); 2],
        axis: Guide,
        w: f32,
        h: f32,
    ) -> Option<Guided> {
        let (pw, ph) = self.plane_size(w, h);
        // A stroke as the line through its ends, about the source's
        // center and with a unit normal, so the cross product below is
        // the sine of the angle between the two and the arithmetic
        // stays in scale whatever the pixels.
        let line = |s: [(f32, f32); 2]| -> Option<[f64; 3]> {
            let p = |q: (f32, f32)| {
                (
                    f64::from(q.0) - f64::from(w) / 2.0,
                    f64::from(q.1) - f64::from(h) / 2.0,
                )
            };
            let (p0, p1) = (p(s[0]), p(s[1]));
            let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
            let n = dx.hypot(dy);
            if !n.is_finite() || n < 1.0 {
                return None;
            }
            Some([-dy / n, dx / n, (p0.0 * dy - p0.1 * dx) / n])
        };
        let (l, m) = (line(a)?, line(b)?);
        // Where they meet, homogeneous: the third part is the sine of
        // the angle between them, zero when they are already parallel
        // and the point is at infinity.
        let v = [
            l[1] * m[2] - l[2] * m[1],
            l[2] * m[0] - l[0] * m[2],
            l[0] * m[1] - l[1] * m[0],
        ];
        let reach = v[0].hypot(v[1]);
        if !reach.is_finite() || reach < 1e-6 || reach <= v[2].abs() {
            return None;
        }
        // The matrix's fixed part undone: the mirror, then the quarter
        // turns backwards, leaving the fine turn to solve for.
        let vx = if self.flip { -v[0] } else { v[0] };
        let (wx, wy) = match self.turns % 4 {
            0 => (vx, v[1]),
            1 => (v[1], -vx),
            2 => (-vx, -v[1]),
            _ => (-v[1], vx),
        };
        // The turn that puts the point on the plane's own axis, taken
        // the way round that turns least: a line is the same line
        // either way up.
        let mut t = match axis {
            Guide::Vertical => (-wx).atan2(wy),
            Guide::Horizontal => wy.atan2(wx),
        };
        if t > std::f64::consts::FRAC_PI_2 {
            t -= std::f64::consts::PI;
        } else if t <= -std::f64::consts::FRAC_PI_2 {
            t += std::f64::consts::PI;
        }
        let (s, c) = t.sin_cos();
        // What is left of the point along the other axis: the whole of
        // it, since the turn zeroed the one axis and the turn is rigid.
        // The tilt is the one whose `perspective` row has this for its
        // reciprocal, half a side and a sign apart.
        let tan = match axis {
            Guide::Vertical => -f64::from(ph) / 2.0 * v[2] / (c * wy - s * wx),
            Guide::Horizontal => f64::from(pw) / 2.0 * v[2] / (c * wx + s * wy),
        };
        Some(Guided {
            angle: (t.to_degrees() as f32).clamp(-MAX_ANGLE, MAX_ANGLE),
            tilt: (tan.atan().to_degrees() as f32).clamp(-MAX_TILT, MAX_TILT),
        })
    }
}

/// Which way the lines a guided pair of strokes follows should run in
/// the leveled plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guide {
    /// Lines that should stand upright: the turn and [`Geometry::vertical`].
    Vertical,
    /// Lines that should lie flat: the turn and [`Geometry::horizontal`].
    Horizontal,
}

/// The perspective guide between its two strokes. The first stroke is
/// kept here, in source pixels, rather than in the overlay in view
/// pixels: between the two the user can zoom with the wheel or a key,
/// pan, or step to another file, and a line held in view pixels would
/// then be read against a view — or a picture — that is not the one it
/// was drawn on. It also remembers which axis it was drawn for, so a
/// half-finished pair can never be completed on the other one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Guiding {
    pub axis: Option<Guide>,
    pub first: Option<GuideStroke>,
}

/// A guide stroke: its two ends, in source pixels.
pub type GuideStroke = [(f32, f32); 2];

impl Guiding {
    /// A stroke released for `axis`, in source pixels: the pair when it
    /// completes one, else nothing and this stroke is kept. A stroke
    /// kept for another axis is dropped rather than paired.
    pub fn stroke(&mut self, axis: Guide, s: GuideStroke) -> Option<(GuideStroke, GuideStroke)> {
        let first = self.first.take().filter(|_| self.axis == Some(axis));
        self.axis = Some(axis);
        match first {
            Some(a) => Some((a, s)),
            None => {
                self.first = Some(s);
                None
            }
        }
    }

    /// Nothing in hand: the tool put down, or the picture changed.
    pub fn clear(&mut self) {
        self.axis = None;
        self.first = None;
    }
}

impl Guide {
    /// The axis a guide button asks for, none when the tool is down.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Vertical" => Some(Guide::Vertical),
            "Horizontal" => Some(Guide::Horizontal),
            _ => None,
        }
    }
}

/// What a guided pair of strokes asks for: the turn, and the keystone
/// on the strokes' own axis. Degrees, both within their limits, and
/// both the value the slider takes rather than a correction to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guided {
    pub angle: f32,
    pub tilt: f32,
}

/// The handles of a crop, clockwise from the top left, and the whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    Move,
}

impl Handle {
    pub fn from_index(i: i32) -> Option<Self> {
        use Handle::*;
        [
            TopLeft,
            Top,
            TopRight,
            Right,
            BottomRight,
            Bottom,
            BottomLeft,
            Left,
            Move,
        ]
        .get(usize::try_from(i).ok()?)
        .copied()
    }
}

impl Crop {
    /// The crop with a handle moved by `dx, dy` (fractions), its aspect
    /// held when `ratio` is given (width over height in pixels of a
    /// source `w` by `h`). Not checked against the source; the caller
    /// does that.
    pub fn dragged(
        &self,
        handle: Handle,
        dx: f32,
        dy: f32,
        ratio: Option<f32>,
        w: f32,
        h: f32,
    ) -> Crop {
        use Handle::*;
        if handle == Move {
            return Crop {
                x: self.x + dx,
                y: self.y + dy,
                ..*self
            };
        }
        let (mut x0, mut y0, mut x1, mut y1) = (self.x, self.y, self.x + self.w, self.y + self.h);
        let min_w = MIN_CROP * w.min(h) / w;
        let min_h = MIN_CROP * w.min(h) / h;
        match handle {
            TopLeft | Left | BottomLeft => x0 = (x0 + dx).min(x1 - min_w),
            TopRight | Right | BottomRight => x1 = (x1 + dx).max(x0 + min_w),
            _ => {}
        }
        match handle {
            TopLeft | Top | TopRight => y0 = (y0 + dy).min(y1 - min_h),
            BottomLeft | Bottom | BottomRight => y1 = (y1 + dy).max(y0 + min_h),
            _ => {}
        }
        if let Some(ratio) = ratio {
            // Fractions to pixels: the width the height asks for.
            let width_for = |height: f32| height * h * ratio / w;
            let height_for = |width: f32| width * w / ratio / h;
            match handle {
                Left | Right => {
                    let nh = height_for(x1 - x0);
                    let cy = (y0 + y1) / 2.0;
                    y0 = cy - nh / 2.0;
                    y1 = cy + nh / 2.0;
                }
                Top | Bottom => {
                    let nw = width_for(y1 - y0);
                    let cx = (x0 + x1) / 2.0;
                    x0 = cx - nw / 2.0;
                    x1 = cx + nw / 2.0;
                }
                _ => {
                    // A corner: the larger of the two moves wins, the
                    // other follows, anchored at the opposite corner.
                    let (cw, ch) = (x1 - x0, y1 - y0);
                    let by_width = height_for(cw);
                    let nh = if by_width * h >= ch * h { by_width } else { ch };
                    let nw = width_for(nh);
                    match handle {
                        TopLeft => {
                            x0 = x1 - nw;
                            y0 = y1 - nh;
                        }
                        TopRight => {
                            x1 = x0 + nw;
                            y0 = y1 - nh;
                        }
                        BottomLeft => {
                            x0 = x1 - nw;
                            y1 = y0 + nh;
                        }
                        _ => {
                            x1 = x0 + nw;
                            y1 = y0 + nh;
                        }
                    }
                }
            }
        }
        Crop {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// A small RGB8 picture turned `turns` quarters and mirrored if
/// `flip`: its size, and its pixels. Nearest neighbor, which is
/// exact for the quarter turns and the mirror.
///
/// It goes through [`Geometry::to_source`] rather than swapping
/// indices of its own, so a thumbnail on the filmstrip and the
/// picture in the viewport can never disagree about which way a
/// turn goes.
pub fn turn_pixels(w: u32, h: u32, rgb: &[u8], turns: u8, flip: bool) -> (u32, u32, Vec<u8>) {
    let geometry = Geometry {
        turns,
        flip,
        ..Geometry::default()
    };
    let (sw, sh) = (w as f32, h as f32);
    let (pw, ph) = geometry.plane_size(sw, sh);
    let (pw, ph) = (pw.round() as u32, ph.round() as u32);
    let mut out = vec![0u8; (pw * ph * 3) as usize];
    for y in 0..ph {
        for x in 0..pw {
            let (sx, sy) = geometry.to_source((x as f32 + 0.5, y as f32 + 0.5), sw, sh);
            let sx = (sx.floor().max(0.0) as u32).min(w - 1);
            let sy = (sy.floor().max(0.0) as u32).min(h - 1);
            let s = ((sy * w + sx) * 3) as usize;
            let d = ((y * pw + x) * 3) as usize;
            out[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
        }
    }
    (pw, ph, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_pairs_two_strokes_of_one_axis_and_no_others() {
        let a = [(100.0, 200.0), (120.0, 900.0)];
        let b = [(500.0, 200.0), (480.0, 900.0)];
        let mut g = Guiding::default();
        // The first is kept, the second completes the pair, in order.
        assert_eq!(g.stroke(Guide::Vertical, a), None);
        assert_eq!(g.first, Some(a));
        assert_eq!(g.stroke(Guide::Vertical, b), Some((a, b)));
        // And nothing is left over for a third stroke to pair with.
        assert_eq!(g.first, None);
        assert_eq!(g.stroke(Guide::Vertical, a), None);

        // A stroke drawn for one axis never completes a pair on the
        // other: it is dropped, and the new one becomes the first.
        let mut g = Guiding::default();
        assert_eq!(g.stroke(Guide::Vertical, a), None);
        assert_eq!(g.stroke(Guide::Horizontal, b), None);
        assert_eq!(g.first, Some(b));
        assert_eq!(g.axis, Some(Guide::Horizontal));
        assert_eq!(g.stroke(Guide::Horizontal, a), Some((b, a)));

        // The tool put down, or the picture changed: the kept stroke
        // goes with it.
        let mut g = Guiding::default();
        assert_eq!(g.stroke(Guide::Vertical, a), None);
        g.clear();
        assert_eq!(g, Guiding::default());
        assert_eq!(g.stroke(Guide::Vertical, b), None);

        assert_eq!(Guide::from_name("Vertical"), Some(Guide::Vertical));
        assert_eq!(Guide::from_name("Horizontal"), Some(Guide::Horizontal));
        assert_eq!(Guide::from_name(""), None);
    }

    #[test]
    fn a_small_picture_turns_as_the_viewport_does() {
        // 3 wide, 2 high, each pixel its own value.
        let rgb: Vec<u8> = (0..6).flat_map(|i| [i, i, i]).collect();
        let (w, h, once) = turn_pixels(3, 2, &rgb, 1, false);
        assert_eq!((w, h), (2, 3));
        // A quarter turn counterclockwise on screen: the top-right pixel
        // (2) comes to the top-left.
        assert_eq!(once[0], 2);
        assert_eq!(once[(2 * 2 + 1) * 3], 3);
        let mut again = (w, h, once);
        for _ in 0..3 {
            again = turn_pixels(again.0, again.1, &again.2, 1, false);
        }
        assert_eq!(again.2, rgb, "four quarters are the picture");
        let (_, _, mirrored) = turn_pixels(3, 2, &rgb, 0, true);
        assert_eq!(&mirrored[..9], &[2, 2, 2, 1, 1, 1, 0, 0, 0]);
        let (_, _, back) = turn_pixels(3, 2, &mirrored, 0, true);
        assert_eq!(back, rgb);
    }

    const W: f32 = 6000.0;
    const H: f32 = 4000.0;

    /// A stroke: its two ends, in source pixels.
    type Stroke = [(f32, f32); 2];

    #[test]
    fn the_whole_frame_is_the_identity() {
        let g = Geometry::default();
        assert!(g.is_identity());
        let f = g.frame(W, H);
        assert_eq!(f.origin, (0.0, 0.0));
        assert_eq!(f.size, (W, H));
        assert_eq!(g.to_source((123.0, 45.0), W, H), (123.0, 45.0));
        assert_eq!(g.bounds(W, H), f);
    }

    #[test]
    fn to_plane_undoes_to_source() {
        let g = Geometry {
            angle: 17.0,
            ..Default::default()
        };
        let p = (123.0, 456.0);
        let s = g.to_source(p, 6000.0, 4000.0);
        let back = g.to_plane(s, 6000.0, 4000.0);
        assert!(
            (back.0 - p.0).abs() < 1e-3 && (back.1 - p.1).abs() < 1e-3,
            "{back:?}"
        );
    }

    #[test]
    fn a_turned_source_carries_the_crop_and_leaves_the_picture_turning() {
        // The source turned right: (dx, dy) about its center becomes
        // (-dy, dx) about the turned source's.
        fn turn_point(p: (f32, f32), w: f32, h: f32, q: i32) -> ((f32, f32), (f32, f32)) {
            let (mut d, mut w, mut h) = ((p.0 - w / 2.0, p.1 - h / 2.0), w, h);
            for _ in 0..q.rem_euclid(4) {
                d = (-d.1, d.0);
                std::mem::swap(&mut w, &mut h);
            }
            ((w / 2.0 + d.0, h / 2.0 + d.1), (w, h))
        }
        let (w, h) = (600.0f32, 400.0f32);
        let cases = [
            Geometry::default(),
            Geometry {
                turns: 1,
                crop: Some(Crop {
                    x: 0.1,
                    y: 0.2,
                    w: 0.4,
                    h: 0.3,
                }),
                angle: 7.0,
                vertical: 12.0,
                horizontal: -5.0,
                ..Geometry::default()
            },
            Geometry {
                turns: 2,
                flip: true,
                angle: -3.0,
                horizontal: 9.0,
                crop: Some(Crop {
                    x: 0.0,
                    y: 0.0,
                    w: 0.5,
                    h: 0.5,
                }),
                aspect: Aspect::Ratio { w: 3.0, h: 2.0 },
                ..Geometry::default()
            },
        ];
        for g in &cases {
            for q in 0..4i32 {
                // What the picture should look like: the old picture
                // turned `q` quarters clockwise on the screen, which
                // is `turned(-q)` of the same source.
                let want = g.turned(-q);
                let under = g.under_turned_source(q);
                let (_, (tw, th)) = turn_point((0.0, 0.0), w, h, q);
                assert_eq!(
                    want.plane_size(w, h),
                    under.plane_size(tw, th),
                    "{g:?} + {q}"
                );
                let (pw, ph) = want.plane_size(w, h);
                for r in [
                    (0.0, 0.0),
                    (pw, 0.0),
                    (pw / 3.0, ph / 7.0),
                    (pw / 2.0, ph / 2.0),
                    (pw, ph),
                ] {
                    let (got, _) = turn_point(want.to_source(r, w, h), w, h, q);
                    let mine = under.to_source(r, tw, th);
                    assert!(
                        (got.0 - mine.0).abs() < 0.01 && (got.1 - mine.1).abs() < 0.01,
                        "{g:?} + {q} at {r:?}: {got:?} vs {mine:?}"
                    );
                }
            }
        }
        // And in words: the top left quarter of a frame turned right
        // is its top right quarter.
        let g = Geometry {
            crop: Some(Crop {
                x: 0.0,
                y: 0.0,
                w: 0.5,
                h: 0.5,
            }),
            ..Geometry::default()
        };
        assert_eq!(
            g.under_turned_source(1).crop,
            Some(Crop {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 0.5
            })
        );
        // Four turns are none, and the quarter turns never move for
        // a geometry that is not mirrored.
        for q in 0..4 {
            assert_eq!(g.under_turned_source(q).turns, 0);
        }
        let mut whole = g.clone();
        for _ in 0..4 {
            whole = whole.under_turned_source(1);
        }
        assert_eq!(whole.crop, g.crop);
    }

    #[test]
    fn a_small_picture_folds_the_frame_turn_into_its_own() {
        // The strip and the loupe draw the camera's own picture, so
        // the frame's turn is folded into the quarter turns rather
        // than the picture being turned first.
        let g = Geometry::default();
        assert_eq!(g.shown_turns(0), (0, false));
        assert_eq!(g.shown_turns(1), (3, false));
        assert_eq!(g.shown_turns(3), (1, false));
        let g = Geometry {
            turns: 1,
            ..Geometry::default()
        };
        assert_eq!(g.shown_turns(1), (0, false));
        // Mirrored, a turn of the source reads the other way round on
        // the screen, which is what the stored geometry has already
        // been moved by.
        let g = Geometry {
            turns: 2,
            flip: true,
            ..Geometry::default()
        };
        assert_eq!(g.shown_turns(1), (3, true));
        // A mirrored edit turned right: the stored turns take the
        // half turn, and the shown turns come out a quarter the other
        // way from where they started, which is the screen turning
        // right.
        let g = Geometry {
            flip: true,
            ..Geometry::default()
        };
        let turned = g.under_turned_source(1);
        assert_eq!(turned.turns, 2);
        assert_eq!(turned.shown_turns(1), (3, true));
        assert_eq!(g.shown_turns(0), (0, true));
    }

    #[test]
    fn quarter_turns_and_the_mirror_are_exact_and_carry_the_crop() {
        // One quarter turn counter-clockwise: the top right corner of
        // the source is the top left of the plane, which stands on end.
        let g = Geometry {
            turns: 1,
            ..Default::default()
        };
        assert_eq!(g.plane_size(W, H), (H, W));
        assert_eq!(g.to_source((0.0, 0.0), W, H), (W, 0.0));
        assert_eq!(g.to_source((H, W), W, H), (0.0, H));
        assert_eq!(g.to_plane((W, 0.0), W, H), (0.0, 0.0));
        assert_eq!(g.frame(W, H).size, (H, W));
        assert_eq!(g.bounds(W, H).size, (H, W));
        // A pixel center lands on a pixel center.
        assert_eq!(g.to_source((10.5, 20.5), W, H), (W - 20.5, 10.5));
        // The mirror alone: left is right.
        let m = Geometry {
            flip: true,
            ..Default::default()
        };
        assert_eq!(m.to_source((0.0, 0.0), W, H), (W, 0.0));
        assert_eq!(m.to_source((10.5, 20.5), W, H), (W - 10.5, 20.5));
        assert_eq!(m.to_plane((W - 10.5, 20.5), W, H), (10.5, 20.5));
        assert!(!m.is_identity() && !m.resamples());
        // Four turns are none; a turn and its undoing are none.
        let c = Crop {
            x: 0.6,
            y: 0.1,
            w: 0.3,
            h: 0.2,
        };
        let with = Geometry {
            crop: Some(c),
            angle: 3.0,
            aspect: Aspect::Ratio { w: 3.0, h: 2.0 },
            ..Default::default()
        };
        // Crops compared to rounding.
        let same = |a: &Geometry, b: &Geometry| {
            let (ca, cb) = (a.crop.unwrap(), b.crop.unwrap());
            (a.turns, a.flip, a.angle, a.portrait) == (b.turns, b.flip, b.angle, b.portrait)
                && (ca.x - cb.x).abs() < 1e-6
                && (ca.y - cb.y).abs() < 1e-6
                && (ca.w - cb.w).abs() < 1e-6
                && (ca.h - cb.h).abs() < 1e-6
        };
        assert!(same(&with.turned(4), &with));
        assert!(same(&with.turned(1).turned(-1), &with));
        assert!(same(&with.turned(1).turned(3), &with));
        // The crop at the top right of the plane is at the top left
        // after a counter-clockwise turn, standing on end, and the
        // aspect follows it.
        let t = with.turned(1);
        let tc = t.crop.unwrap();
        assert!((tc.x - 0.1).abs() < 1e-6 && (tc.y - 0.1).abs() < 1e-6);
        assert!((tc.w - 0.2).abs() < 1e-6 && (tc.h - 0.3).abs() < 1e-6);
        assert!(t.portrait);
        // `Original` is the one aspect the flag does not follow: the
        // plane it is read against has turned already.
        let o = Geometry {
            aspect: Aspect::Original,
            ..with.clone()
        };
        assert!(!o.turned(1).portrait);
        assert!(
            (o.turned(1).aspect.ratio(H, W, false).unwrap() - H / W).abs() < 1e-6,
            "a turned Original follows the plane"
        );
        // The same source pixels are in the crop: its center in the
        // source is where it was.
        let center = |g: &Geometry| {
            let (pw, ph) = g.plane_size(W, H);
            let c = g.crop.unwrap();
            g.to_source(((c.x + c.w / 2.0) * pw, (c.y + c.h / 2.0) * ph), W, H)
        };
        let (a, b) = (center(&with), center(&t));
        assert!(
            (a.0 - b.0).abs() < 1e-2 && (a.1 - b.1).abs() < 1e-2,
            "{a:?} {b:?}"
        );
        // A mirror keeps the picture level and the crop over the same
        // pixels; mirrored twice is as it was.
        let f = with.flipped(false);
        assert_eq!(f.angle, -3.0);
        let (a, b) = (center(&with), center(&f));
        assert!(
            (a.0 - b.0).abs() < 1e-2 && (a.1 - b.1).abs() < 1e-2,
            "{a:?} {b:?}"
        );
        assert!(same(&f.flipped(false), &with));
        let v = with.flipped(true);
        assert_eq!(v.turns, 2);
        let (a, b) = (center(&with), center(&v));
        assert!(
            (a.0 - b.0).abs() < 1e-2 && (a.1 - b.1).abs() < 1e-2,
            "{a:?} {b:?}"
        );
        assert!(same(&v.flipped(true), &with));
        // A turned, mirrored, angled plane still comes back through
        // `to_plane`.
        let g = Geometry {
            turns: 3,
            flip: true,
            angle: -11.0,
            ..Default::default()
        };
        let p = (321.0, 654.0);
        let back = g.to_plane(g.to_source(p, W, H), W, H);
        assert!((back.0 - p.0).abs() < 1e-2 && (back.1 - p.1).abs() < 1e-2);
    }

    #[test]
    fn a_turn_levels_a_line_and_the_crop_shrinks_to_fit() {
        // A horizon rising to the right by three degrees on screen.
        let phi = 3.0f32;
        let g = Geometry {
            angle: -phi,
            ..Default::default()
        };
        // Points of the leveled plane along a level line map to
        // source points along the tilted one.
        let (cx, cy) = (W / 2.0, H / 2.0);
        for t in [-1000.0f32, 500.0, 1500.0] {
            let s = g.to_source((cx + t, cy), W, H);
            let (sn, cs) = phi.to_radians().sin_cos();
            assert!(
                (s.0 - (cx + t * cs)).abs() < 1e-2 && (s.1 - (cy - t * sn)).abs() < 1e-2,
                "{t}: {s:?}"
            );
        }
        // And the level tool says the same: a line drawn at +3 degrees
        // asks for -3.
        assert!((Geometry::default().leveled_by(phi) + phi).abs() < 1e-6);
        // Near vertical folds to the vertical.
        assert!((Geometry::default().leveled_by(92.0) + 2.0).abs() < 1e-6);
        assert!((Geometry::default().leveled_by(-88.0) + 2.0).abs() < 1e-6);
        // The frame without a crop is the largest fit, inside the source
        // and smaller than it, keeping the source's shape.
        let f = g.frame(W, H);
        assert!(f.size.0 < W && f.size.1 < H);
        assert!((f.size.0 / f.size.1 - W / H).abs() < 1e-2);
        let c = g.effective_crop(W, H);
        assert!(g.fits(&c, W, H));
        let bigger = Crop { w: c.w * 1.02, ..c };
        assert!(!g.fits(&bigger, W, H));
        // The bounds hold the whole turned source.
        let b = g.bounds(W, H);
        assert!(b.size.0 > W && b.size.1 > H);
    }

    #[test]
    fn the_keystone_spreads_the_far_edge_and_comes_back_through_to_plane() {
        // A camera that looked up: the source's verticals converge at
        // the top. The correction spreads the top: the plane's top
        // corners reach less far into the source than its bottom ones.
        let g = Geometry {
            vertical: 20.0,
            ..Default::default()
        };
        assert!(g.keystoned() && g.resamples() && !g.is_identity());
        let top = g.to_source((0.0, 0.0), W, H);
        let bottom = g.to_source((0.0, H), W, H);
        let t = 20.0f32.to_radians().tan();
        assert!(
            (top.0 - (W / 2.0 - W / 2.0 / (1.0 + t))).abs() < 1e-2,
            "{top:?}"
        );
        assert!(
            (bottom.0 - (W / 2.0 - W / 2.0 / (1.0 - t))).abs() < 1e-2,
            "{bottom:?}"
        );
        assert!(top.0 > bottom.0, "the top reaches less far out");
        // The center line and the center stay put.
        let c = g.to_source((W / 2.0, H / 2.0), W, H);
        assert!((c.0 - W / 2.0).abs() < 1e-3 && (c.1 - H / 2.0).abs() < 1e-3);
        let mid = g.to_source((W / 2.0, 100.0), W, H);
        assert!((mid.0 - W / 2.0).abs() < 1e-3);
        // A plane vertical maps to a source line converging upwards:
        // straight, and nearer the center at the top.
        let line: Vec<(f32, f32)> = [0.0f32, H / 3.0, 2.0 * H / 3.0, H]
            .into_iter()
            .map(|y| g.to_source((W / 4.0, y), W, H))
            .collect();
        let slope = |a: (f32, f32), b: (f32, f32)| (b.0 - a.0) / (b.1 - a.1);
        assert!((slope(line[0], line[1]) - slope(line[2], line[3])).abs() < 1e-4);
        assert!(line[0].0 > line[3].0);
        // Looking to the right converges the horizontals there: the
        // right edge reaches less far out.
        let r = Geometry {
            horizontal: 15.0,
            ..Default::default()
        };
        let right = r.to_source((W, H / 2.0), W, H);
        let left = r.to_source((0.0, H / 2.0), W, H);
        assert!(right.0 - W / 2.0 < W / 2.0 - left.0, "{left:?} {right:?}");
        // The inverse, with a turn, a mirror and both tilts.
        let g = Geometry {
            turns: 1,
            flip: true,
            angle: -7.0,
            vertical: -25.0,
            horizontal: 12.0,
            ..Default::default()
        };
        for p in [(100.0f32, 200.0f32), (3000.0, 4000.0), (3999.0, 5999.0)] {
            let back = g.to_plane(g.to_source(p, W, H), W, H);
            assert!(
                (back.0 - p.0).abs() < 1e-1 && (back.1 - p.1).abs() < 1e-1,
                "{p:?} -> {back:?}"
            );
        }
        // The frame without a crop fits and is smaller than the plane.
        let f = g.frame(W, H);
        assert!(f.size.0 < H && f.size.1 < W);
        assert!(g.fits(&g.effective_crop(W, H), W, H));
        // Beyond the horizon nothing fits: the bisection is not fooled
        // by a corner mirrored back into the source.
        let far = Geometry {
            vertical: 40.0,
            ..Default::default()
        };
        assert!(far.in_front((W / 2.0, 0.0), W, H));
        assert!(!far.in_front((W / 2.0, 20.0 * H), W, H));
        assert!(!far.fits(
            &Crop {
                x: 0.4,
                y: -30.0,
                w: 0.2,
                h: 31.0
            },
            W,
            H
        ));
        let b = far.bounds(W, H);
        assert!(b.size.0.is_finite() && b.size.1.is_finite());
    }

    #[test]
    fn the_keystone_turns_and_mirrors_with_the_picture() {
        let g = Geometry {
            angle: 4.0,
            vertical: 18.0,
            horizontal: -9.0,
            ..Default::default()
        };
        let close =
            |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-1 && (a.1 - b.1).abs() < 1e-1;
        // Turned a quarter counter-clockwise, a point of the new
        // plane is the old plane's point a quarter clockwise of it.
        let t = g.turned(1);
        assert_eq!((t.horizontal, t.vertical), (-18.0, -9.0));
        for p in [(100.0f32, 300.0f32), (H - 50.0, W - 20.0), (2000.0, 2500.0)] {
            let old = (W - p.1, p.0);
            assert!(close(t.to_source(p, W, H), g.to_source(old, W, H)), "{p:?}");
        }
        let t2 = g.turned(2);
        for p in [(100.0f32, 300.0f32), (W - 20.0, H - 50.0)] {
            let old = (W - p.0, H - p.1);
            assert!(close(t2.to_source(p, W, H), g.to_source(old, W, H)));
        }
        assert_eq!(g.turned(1).turned(3), g);
        assert_eq!(g.turned(3).turned(1), g);
        // Mirrored, a point of the new plane is the old plane's point
        // across the axis.
        let m = g.flipped(false);
        assert_eq!((m.horizontal, m.vertical), (9.0, 18.0));
        for p in [(100.0f32, 300.0f32), (W - 20.0, H - 50.0)] {
            let old = (W - p.0, p.1);
            assert!(close(m.to_source(p, W, H), g.to_source(old, W, H)), "{p:?}");
        }
        let v = g.flipped(true);
        assert_eq!((v.horizontal, v.vertical), (-9.0, -18.0));
        for p in [(100.0f32, 300.0f32), (W - 20.0, H - 50.0)] {
            let old = (p.0, H - p.1);
            assert!(close(v.to_source(p, W, H), g.to_source(old, W, H)), "{p:?}");
        }
        assert_eq!(m.flipped(false), g);
        assert_eq!(v.flipped(true), g);
    }

    /// Two strokes along lines that should run `axis`'s way, drawn on
    /// the plane of `g` and handed back in source pixels: what the
    /// guide tool sees when the user traces two building edges on a
    /// picture that `g` has already straightened.
    fn strokes(g: &Geometry, axis: Guide, w: f32, h: f32) -> (Stroke, Stroke) {
        let (pw, ph) = g.plane_size(w, h);
        let at = |u: f32, t: f32| match axis {
            Guide::Vertical => g.to_source((u * pw, t * ph), w, h),
            Guide::Horizontal => g.to_source((t * pw, u * ph), w, h),
        };
        // Two lines a third of the plane apart, each spanning the
        // middle three fifths of it: a stroke a user could draw.
        ([at(0.3, 0.2), at(0.3, 0.8)], [at(0.7, 0.8), at(0.7, 0.2)])
    }

    #[test]
    fn two_guided_strokes_give_the_turn_and_the_tilt() {
        // The picture the strokes are drawn on is turned and keystoned
        // already; the solve reads only the quarter turns and the
        // mirror from it, and must find the rest back.
        for base in [
            Geometry::default(),
            Geometry {
                turns: 1,
                ..Default::default()
            },
            Geometry {
                flip: true,
                ..Default::default()
            },
            Geometry {
                turns: 3,
                flip: true,
                ..Default::default()
            },
        ] {
            for axis in [Guide::Vertical, Guide::Horizontal] {
                let mut worst: f32 = 0.0;
                for &turn in &[-10.0f32, -6.5, -1.0, 0.0, 1.0, 6.5, 10.0] {
                    for &tilt in &[-30.0f32, -18.0, -5.0, 0.0, 5.0, 18.0, 30.0] {
                        // The other axis's keystone is set too: it must
                        // not disturb this one's answer.
                        let g = Geometry {
                            angle: turn,
                            vertical: if axis == Guide::Vertical { tilt } else { 12.0 },
                            horizontal: if axis == Guide::Vertical { 12.0 } else { tilt },
                            ..base.clone()
                        };
                        let (a, b) = strokes(&g, axis, W, H);
                        let got = base.guided_by(a, b, axis, W, H).expect("a solution");
                        worst = worst
                            .max((got.angle - turn).abs())
                            .max((got.tilt - tilt).abs());
                        assert!(
                            (got.angle - turn).abs() < 0.05 && (got.tilt - tilt).abs() < 0.05,
                            "{axis:?} turn {turn} tilt {tilt} gave {got:?} (turns {}, flip {})",
                            base.turns,
                            base.flip
                        );
                        // And the lines are then straight up or across:
                        // the two ends of each stroke share a plane
                        // coordinate on the other axis.
                        let mut fixed = g.clone();
                        fixed.angle = got.angle;
                        match axis {
                            Guide::Vertical => fixed.vertical = got.tilt,
                            Guide::Horizontal => fixed.horizontal = got.tilt,
                        }
                        for s in [a, b] {
                            let p0 = fixed.to_plane(s[0], W, H);
                            let p1 = fixed.to_plane(s[1], W, H);
                            let off = match axis {
                                Guide::Vertical => (p0.0 - p1.0).abs(),
                                Guide::Horizontal => (p0.1 - p1.1).abs(),
                            };
                            assert!(off < 2.0, "{axis:?} {turn} {tilt}: {off} px out of true");
                        }
                    }
                }
                assert!(worst < 0.05, "worst {worst} for {axis:?}");
            }
        }
    }

    #[test]
    fn a_short_shaky_stroke_still_lands_within_a_third_of_a_degree() {
        // What a hand costs: strokes over a seventh of the plane with
        // both ends nudged by up to two pixels either way.
        let mut seed = 12345u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((seed >> 8) as f32 / 8388608.0 - 1.0) * 2.0
        };
        let mut worst: f32 = 0.0;
        for &turn in &[-8.0f32, 0.0, 5.0] {
            for &tilt in &[-25.0f32, -6.0, 0.0, 6.0, 25.0] {
                let g = Geometry {
                    angle: turn,
                    vertical: tilt,
                    ..Default::default()
                };
                let mut stroke = |u: f32| {
                    let ends = [
                        g.to_source((u * W, 0.425 * H), W, H),
                        g.to_source((u * W, 0.575 * H), W, H),
                    ];
                    ends.map(|(x, y)| (x + rnd(), y + rnd()))
                };
                let (a, b) = (stroke(0.3), stroke(0.7));
                let got = Geometry::default()
                    .guided_by(a, b, Guide::Vertical, W, H)
                    .expect("a solution");
                worst = worst
                    .max((got.angle - turn).abs())
                    .max((got.tilt - tilt).abs());
            }
        }
        assert!(worst < 0.3, "worst {worst} degrees");
    }

    #[test]
    fn parallel_strokes_set_the_turn_alone() {
        // No vanishing point: the tilt is nothing and the turn is the
        // lines' own, folded to the nearer quarter.
        let g = Geometry::default();
        let got = g
            .guided_by(
                [(1000.0, 500.0), (1100.0, 3500.0)],
                [(4000.0, 500.0), (4100.0, 3500.0)],
                Guide::Vertical,
                W,
                H,
            )
            .expect("a solution");
        assert_eq!(got.tilt, 0.0);
        // The lines lean 100 across in 3000 down, which is a turn of
        // that much clockwise to stand them up.
        assert!(
            (got.angle + (100.0f32 / 3000.0).atan().to_degrees()).abs() < 1e-3,
            "{got:?}"
        );
        // The same pair read as horizontals: upright is a quarter turn
        // from flat, and the turn is past the limit, so it clamps.
        let flat = g
            .guided_by(
                [(1000.0, 500.0), (1100.0, 3500.0)],
                [(4000.0, 500.0), (4100.0, 3500.0)],
                Guide::Horizontal,
                W,
                H,
            )
            .expect("a solution");
        assert_eq!(flat.tilt, 0.0);
        assert_eq!(flat.angle, MAX_ANGLE);
    }

    #[test]
    fn a_guide_refuses_what_it_cannot_read() {
        let g = Geometry::default();
        let good = [(1000.0, 500.0), (1100.0, 3500.0)];
        // A stroke of no length.
        assert!(
            g.guided_by(good, [(2000.0, 1000.0); 2], Guide::Vertical, W, H)
                .is_none()
        );
        // Both strokes on the one line.
        assert!(
            g.guided_by(
                good,
                [(1050.0, 2000.0), (1080.0, 2900.0)],
                Guide::Vertical,
                W,
                H
            )
            .is_none()
        );
        // Lines crossing at the center: a tilt of a quarter turn.
        assert!(
            g.guided_by(
                [(2000.0, 1000.0), (4000.0, 3000.0)],
                [(4000.0, 1000.0), (2000.0, 3000.0)],
                Guide::Vertical,
                W,
                H
            )
            .is_none()
        );
    }

    #[test]
    fn aspects_and_handles() {
        assert_eq!(Aspect::Free.ratio(W, H, false), None);
        assert!((Aspect::Original.ratio(W, H, false).unwrap() - 1.5).abs() < 1e-6);
        assert!((Aspect::Original.ratio(W, H, true).unwrap() - 1.0 / 1.5).abs() < 1e-6);
        // A frame shot on end keeps its way up: "Original" on it is
        // portrait with the flag still false, which is what a crop of
        // it going straight to landscape was.
        assert!((Aspect::Original.ratio(H, W, false).unwrap() - 1.0 / 1.5).abs() < 1e-6);
        assert!((Aspect::Original.ratio(H, W, true).unwrap() - 1.5).abs() < 1e-6);
        // The names the panel's list is made of, and their inverses.
        assert_eq!(Aspect::from_name("Free", ""), Aspect::Free);
        assert_eq!(Aspect::from_name("Original", ""), Aspect::Original);
        assert_eq!(Aspect::from_name("16:9", ""), PRESETS[4].1);
        assert_eq!(Aspect::from_name("what", ""), Aspect::Free);
        assert_eq!(Aspect::Free.name(), ("Free", None));
        assert_eq!(Aspect::Original.name(), ("Original", None));
        assert_eq!(PRESETS[5].1.name(), ("65:24", None));
        assert_eq!(
            Aspect::Ratio { w: 7.0, h: 3.0 }.name(),
            ("Custom", Some("7:3".into()))
        );
        // A custom box reads four ways, and a side at nothing or text
        // that will not read leaves the aspect free.
        for text in ["65:24", "65/24", "65x24", "65X24", " 65 : 24 "] {
            assert_eq!(
                Aspect::parse_ratio(text),
                Some(Aspect::Ratio { w: 65.0, h: 24.0 }),
                "{text}"
            );
        }
        assert_eq!(
            Aspect::parse_ratio("2.7"),
            Some(Aspect::Ratio { w: 2.7, h: 1.0 })
        );
        for text in ["", "0:1", "1:0", "-3:2", "wide", "3:"] {
            assert_eq!(Aspect::parse_ratio(text), None, "{text}");
            assert_eq!(Aspect::from_name("Custom", text), Aspect::Free, "{text}");
        }
        // A named ratio is written landscape whichever way the frame
        // is, and only the flag turns it.
        let wide = PRESETS.iter().find(|(n, _)| *n == "65:24").unwrap().1;
        assert!((wide.ratio(W, H, false).unwrap() - 65.0 / 24.0).abs() < 1e-6);
        assert!((wide.ratio(H, W, false).unwrap() - 65.0 / 24.0).abs() < 1e-6);
        assert!((wide.ratio(H, W, true).unwrap() - 24.0 / 65.0).abs() < 1e-6);
        // And the largest fit on a frame shot on end is the frame.
        let upright = Geometry {
            aspect: Aspect::Original,
            ..Default::default()
        };
        let whole = upright.effective_crop(H, W);
        assert!(
            (whole.w - 1.0).abs() < 1e-3 && (whole.h - 1.0).abs() < 1e-3,
            "{whole:?}"
        );
        // A square crop at the center of a 3:2 frame.
        let g = Geometry::default();
        let sq = g.largest_fit((0.5, 0.5), 1.0, W, H);
        assert!(
            (sq.h - 1.0).abs() < 1e-3 && (sq.w * W - H).abs() < 1.0,
            "{sq:?}"
        );
        // Dragging the right edge with the aspect held keeps it square
        // about the middle.
        let d = sq.dragged(Handle::Right, -0.1, 0.0, Some(1.0), W, H);
        assert!((d.w * W - d.h * H).abs() < 1.0, "{d:?}");
        assert!((d.center().1 - 0.5).abs() < 1e-6);
        // A corner drag free changes both; a move changes neither size.
        let free = sq.dragged(Handle::BottomRight, -0.1, -0.1, None, W, H);
        assert!(free.w < sq.w && free.h < sq.h);
        let moved = sq.dragged(Handle::Move, 0.05, 0.0, None, W, H);
        assert_eq!((moved.w, moved.h), (sq.w, sq.h));
        assert!(!g.fits(
            &Crop {
                x: 0.9,
                y: 0.0,
                w: 0.2,
                h: 0.2
            },
            W,
            H
        ));
        assert!(Handle::from_index(8) == Some(Handle::Move) && Handle::from_index(9).is_none());
    }

    #[test]
    fn a_drag_past_the_edge_slides_along_it() {
        let g = Geometry::default();
        // A move pushed well past the right edge ends flush with it,
        // and the height it was at is kept.
        let start = Crop {
            x: 0.6,
            y: 0.3,
            w: 0.3,
            h: 0.3,
        };
        let m = g.drag_within(&start, Handle::Move, 0.9, 0.0, None, W, H);
        assert!((m.x + m.w - 1.0).abs() < 1e-3, "{m:?}");
        assert!((m.y - start.y).abs() < 1e-6, "{m:?}");
        assert_eq!((m.w, m.h), (start.w, start.h));
        // Pushed into that edge at an angle, it still takes the whole
        // of the drag down.
        let d = g.drag_within(&start, Handle::Move, 0.9, 0.2, None, W, H);
        assert!((d.x + d.w - 1.0).abs() < 1e-3, "{d:?}");
        assert!((d.y - (start.y + 0.2)).abs() < 1e-3, "{d:?}");
    }

    #[test]
    fn a_handle_dragged_out_stops_on_the_edge() {
        let g = Geometry::default();
        // A square crop, in pixels, of a 3:2 source.
        let start = Crop {
            x: 0.5,
            y: 0.2,
            w: 0.2,
            h: 0.3,
        };
        let c = g.drag_within(&start, Handle::BottomRight, 0.5, 0.5, Some(1.0), W, H);
        assert!(g.fits(&c, W, H), "{c:?}");
        assert!((c.x + c.w - 1.0).abs() < 1e-3, "{c:?}");
        assert!(c.w > start.w && c.h > start.h, "{c:?}");
        // The aspect is still held: as many pixels across as down.
        assert!((c.w * W - c.h * H).abs() < 1.0, "{c:?}");
        // A drag that fits is passed through whole.
        let small = g.drag_within(&start, Handle::BottomRight, 0.05, 0.0, Some(1.0), W, H);
        assert_eq!(
            small,
            start.dragged(Handle::BottomRight, 0.05, 0.0, Some(1.0), W, H)
        );
    }
}
