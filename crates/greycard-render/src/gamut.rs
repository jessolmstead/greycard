//! The gamut op (design of 2026-11-21, §6.4; the bound at the tighter
//! setting the owner picked): what every edit finishes through in place
//! of the clamp per channel at the output, and the floor before the
//! display curve that lets a far-out light whiten.
//!
//! Three parts, composed, each the identity under its own threshold:
//!
//! 1. The rail: the top, per channel, in sRGB's primaries for every
//!    output, so every export shows the same lights. Each channel's rise
//!    over the color's luminance, as a share of the headroom to white,
//!    is rolled; a channel under zero takes the least scale any channel
//!    got, so lightness does not turn back. sRGB's matrix into each of
//!    our outputs has no negative entry, so under sRGB's top is under
//!    every output's. A picture rendered for a display (a JPEG, a TIFF)
//!    takes thresholds of one ([`Params::EDGE`]) and its output's own
//!    top, so nothing inside its output moves.
//! 2. The bound: a color further out than `bound[0]` of the output's
//!    edge has its reach rolled toward `bound[1]`, its channels under
//!    zero moved toward a soft clamp by one ratio: a move toward the
//!    output's nearest color, which is what the eye calls a far-out
//!    color's hue.
//! 3. The roll: chroma at held Oklab hue and lightness, the identity to
//!    `roll[0]` of the edge, to the edge at `roll[1]`.
//!
//! Then the output's channels and one final clamp, which takes rounding
//! under zero and at most a hundredth over one (a color past white's
//! ray bowing over it on its way in).
//!
//! The reach is along the Oklab ray from the grey axis at the color's
//! lightness through the color, as Björn Ottosson's gamut clipping
//! (2021) takes it, and the roll is ACES' curve (ACES 1.3, Reference
//! Gamut Compression). Both are written here from the published math;
//! no code is taken from either. Along the ray each output channel is
//! a cubic in the share of the color's chroma, and the edge is found by
//! a fixed number of halvings ([`Op::reach_of`]), so the shader
//! (`viewport.wgsl`) does the same arithmetic with no loop that depends
//! on the data.

use greycard_core::color::{LAB_TO_LMS, LMS_TO_LAB, Matrix3, Oklab, apply3, invert3, mul3};

/// The roll's and the bound's power, ACES' own.
pub const POWER: f32 = 1.2;

/// The edge's spring, in depth past the faces (a share of the
/// brightest channel) to a share of the way out ([`Op::reach_of`]):
/// how hard the edge is held to where the straight line from grey
/// through the color leaves the faces. Stiffer, the edge moves less
/// with a dip in the ray, and more of the color lands past a face where
/// the ray and the line part, to be taken in along the line to grey at
/// a turn of hue. At 0.1 the op is at most 1.6 times as steep as the
/// clamp at the test's sampling (`the_op_is_no_steeper_than_the_clamp`;
/// 2.4 at 0.05, 1.4 at 0.25), and about 2.9 times, converged, under
/// thresholds of one at sRGB's blue corner; a color is brought to the
/// edge within 2.1 degrees of its hue (3.4 at 0.25, 1.5 at 0.05). The
/// edge at the chord itself is the spring's stiff limit: flatter still,
/// and it costs more hue.
pub const SPRING: f32 = 0.1;

/// The halvings that find the edge: to a millionth of the way out.
pub const BISECT: u32 = 20;

/// The floor's weight (§6.2): `3 * 2^-7.5`, held to its four places.
pub const FLOOR: f32 = 0.0166;

/// The working space's luminance, Rec.2020's.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// The share of its brightest channel a color's smallest must keep for
/// the op to leave it as it is, unrailed: under the roll's knee in
/// every output we have (a test holds it), so most pixels stop there.
pub const EARLY: f32 = 0.15;

/// The op's thresholds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// The rail's knee and limit on `e`; a knee at one takes only a
    /// channel over white; `None` no rail.
    pub rail: Option<[f32; 2]>,
    /// The bound's onset and asymptote on the reach; `None` no bound.
    pub bound: Option<[f32; 2]>,
    /// The roll's knee and limit on the reach; `[1, 1]` straight to the
    /// edge.
    pub roll: [f32; 2],
    /// The bound's soft clamp: its lower face rounded over this share
    /// of the brightest channel, so a channel crossing zero under it
    /// changes slope smoothly.
    pub soft: f32,
    /// The rail in the output's own primaries rather than sRGB's: a
    /// picture rendered for a display, whose colors inside its output
    /// must not move whatever sRGB holds.
    pub own_top: bool,
}

impl Params {
    /// A raw's scene, into an output narrower than the working space.
    pub const SCENE: Params = Params {
        rail: Some([0.9, 2.4]),
        bound: Some([1.1, 1.2]),
        roll: [0.85, 1.6],
        soft: 0.01,
        own_top: false,
    };

    /// Thresholds of one, for a picture already rendered for a display
    /// (a JPEG, a TIFF): nothing inside its output moves, and what an
    /// edit pushes outside comes to the edge at its hue. The rail is in
    /// the output's primaries and takes a channel over white alone.
    pub const EDGE: Params = Params {
        rail: Some([1.0, 2.4]),
        bound: None,
        roll: [1.0, 1.0],
        soft: 0.0,
        own_top: true,
    };

    /// A cube a table reads, or the working space's after a profile's
    /// curve: the lower faces alone, straight to them.
    pub const CUBE: Params = Params {
        rail: None,
        bound: None,
        roll: [1.0, 1.0],
        soft: 0.0,
        own_top: false,
    };

    /// The reach under which nothing of the bound or the roll acts.
    pub fn onset(&self) -> f32 {
        let b = self.bound.map_or(f32::INFINITY, |b| b[0]);
        b.min(self.roll[0]).min(1.0)
    }
}

/// The op for one output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Op {
    /// The working space to linear sRGB and back: the rail's reference.
    to_srgb: Matrix3,
    from_srgb: Matrix3,
    /// The working space to the output and back.
    to_out: Matrix3,
    from_out: Matrix3,
    /// The working space to Oklab's cone space, and cone space to the
    /// output.
    to_lms: Matrix3,
    lms_to_out: Matrix3,
    pub params: Params,
    /// The rail's and the roll's scales ([`roll_scale`]), worked out
    /// once.
    rail_scale: f32,
    roll_scale: f32,
}

/// ACES' roll on a share `r`: the identity to `knee`, then the power
/// curve with a slope of one there through one at `limit`; one past it.
/// A knee at one is `min(r, 1)`.
#[inline]
pub fn roll_at(r: f32, knee: f32, limit: f32) -> f32 {
    if r <= knee || r >= limit {
        return roll_by(r, knee, limit, 0.0);
    }
    roll_by(r, knee, limit, roll_scale(knee, limit))
}

/// [`roll_at`]'s scale on the power curve, for its knee and limit: the
/// same for every color, so an op works it out once. Zero where the
/// roll has no power curve.
#[inline]
fn roll_scale(knee: f32, limit: f32) -> f32 {
    if knee >= 1.0 || limit <= 1.0 + 1e-6 {
        return 0.0;
    }
    let xl = (limit - knee) / (1.0 - knee);
    xl / (xl.powf(POWER) - 1.0).powf(1.0 / POWER)
}

/// [`roll_at`] with its scale [`roll_scale`] handed in.
#[inline]
fn roll_by(r: f32, knee: f32, limit: f32, s: f32) -> f32 {
    if r <= knee {
        return r;
    }
    if knee >= 1.0 || limit <= 1.0 + 1e-6 {
        return r.min(1.0);
    }
    if r >= limit {
        return 1.0;
    }
    let x = (r - knee) / (1.0 - knee);
    (knee + (1.0 - knee) * x / (1.0 + (x / s).powf(POWER)).powf(1.0 / POWER)).min(1.0)
}

/// The bound's curve: the identity to `a`, then toward `b` and never
/// past it.
#[inline]
pub fn bound_at(r: f32, a: f32, b: f32) -> f32 {
    if r <= a {
        return r;
    }
    let x = (r - a) / (b - a);
    a + (b - a) * x / (1.0 + x.powf(POWER)).powf(1.0 / POWER)
}

/// `max(v, 0)` rounded over `w` either side of zero, its slope
/// continuous.
#[inline]
fn soft_max0(v: f32, w: f32) -> f32 {
    if v <= -w {
        0.0
    } else if v >= w {
        v
    } else {
        (v + w) * (v + w) / (4.0 * w)
    }
}

impl Op {
    /// For the output reached from the working space by `to_out`. An
    /// output as wide as the working space takes thresholds of one
    /// ([`Params::EDGE`]) whatever `params` asks for past the rail.
    pub fn for_output(to_out: &Matrix3, params: Params) -> Option<Self> {
        let same = (0..3).all(|r| (0..3).all(|c| (to_out[r][c] - f32::from(r == c)).abs() < 1e-5));
        let params = if same {
            Params {
                bound: None,
                roll: [1.0, 1.0],
                ..params
            }
        } else {
            params
        };
        let ok = Oklab::for_working_space();
        // The rail's reference: sRGB's top for a scene, for every
        // output; the output's own for a display's picture.
        let to_srgb = if params.own_top {
            *to_out
        } else {
            crate::export::Space::Srgb.matrix()
        };
        Some(Self {
            to_srgb,
            from_srgb: invert3(to_srgb)?,
            to_out: *to_out,
            from_out: invert3(*to_out)?,
            to_lms: ok.to_lms,
            lms_to_out: mul3(*to_out, ok.from_lms),
            params,
            rail_scale: params.rail.map_or(0.0, |[k, l]| roll_scale(k, l)),
            roll_scale: roll_scale(params.roll[0], params.roll[1]),
        })
    }

    /// A working-space color into the output, linear, inside its cube.
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        self.unclamped(c).map(|v| v.clamp(0.0, 1.0))
    }

    /// [`Op::apply`] before the final clamp: what the clamp moves is the
    /// solve's residue.
    pub fn unclamped(&self, c: [f32; 3]) -> [f32; 3] {
        self.solve(c, true)
    }

    /// [`Op::unclamped`], with the early out or without: the same to the
    /// bit either way (`a_color_under_the_knees_comes_back_to_the_bit`).
    fn solve(&self, c: [f32; 3], early: bool) -> [f32; 3] {
        // A color past every number is left to the final clamp, as the
        // clamp per channel takes it.
        if !c.iter().all(|v| v.is_finite()) {
            return apply3(&self.to_out, c);
        }
        let (mut c, railed) = self.rail(c);
        let o = apply3(&self.to_out, c);
        let (lo, hi) = (o[0].min(o[1]).min(o[2]), o[0].max(o[1]).max(o[2]));
        // Most pixels end here: well inside, and under the rail's knee.
        if early && !railed && lo >= self.early() * hi && hi <= 1.0 {
            return o;
        }
        let mut lab = self.lab(c);
        let mut ray = self.ray(lab);
        let mut rho = self.reach_along(&ray, lab);
        if let Some([a, b]) = self.params.bound
            && rho > a
        {
            let q = (bound_at(rho, a, b) - 1.0) / (rho - 1.0);
            let w = self.params.soft * o[0].max(o[1]).max(o[2]).max(1e-6);
            let o = if self.params.soft > 0.0 {
                o.map(|v| q * v + (1.0 - q) * soft_max0(v, w))
            } else {
                o.map(|v| if v < 0.0 { v * q } else { v })
            };
            c = apply3(&self.from_out, o);
            lab = self.lab(c);
            ray = self.ray(lab);
            rho = self.reach_along(&ray, lab);
        }
        let [knee, limit] = self.params.roll;
        if rho > knee {
            let p = ray.at(roll_by(rho, knee, limit, self.roll_scale) / rho);
            // What the spring leaves past a face (where the ray grazes
            // one): in to the face along the straight line to grey, a
            // move as smooth as the color's, not the final clamp's turn
            // of hue.
            let grey = ray.0[0][0];
            let mut back = 1.0f32;
            for v in p {
                if v < 0.0 {
                    back = back.min(grey / (grey - v));
                }
            }
            if back < 1.0 {
                p.map(|v| grey + back * (v - grey))
            } else {
                p
            }
        } else {
            apply3(&self.to_out, c)
        }
    }

    /// The rail, on a working-space color: the color, and whether a
    /// channel moved.
    pub fn rail(&self, c: [f32; 3]) -> ([f32; 3], bool) {
        let Some([knee, limit]) = self.params.rail else {
            return (c, false);
        };
        // The luminance held a hair under white, not white at one: a
        // color past the working cube can be that bright with a channel
        // far under it, and the roll then takes every channel over the
        // luminance to white as it nears one, the rest as they are, the
        // same on either side.
        let y = (LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2]).clamp(0.0, 1.0 - 1e-4);
        let head = 1.0 - y;
        let r = apply3(&self.to_srgb, c);
        let mut moved = false;
        let mut q = 1.0f32;
        let mut out = r;
        for k in 0..3 {
            let e = (r[k] - y) / head;
            if e > knee {
                out[k] = y + head * roll_by(e, knee, limit, self.rail_scale);
                if r[k] > 0.0 {
                    q = q.min(out[k] / r[k]);
                }
                moved = true;
            }
        }
        if !moved {
            return (c, false);
        }
        for v in &mut out {
            if *v < 0.0 {
                *v *= q;
            }
        }
        (apply3(&self.from_srgb, out), true)
    }

    /// Oklab of a working-space color, a cone response under zero held
    /// at zero: such a color is past what any eye sees and past every
    /// output, and the cube root's turn through zero would swing its hue
    /// by tens of degrees for a change in the fifth place, which no two
    /// implementations of the finish round alike.
    pub fn lab(&self, c: [f32; 3]) -> [f32; 3] {
        let lms = apply3(&self.to_lms, c).map(|v| v.max(0.0).cbrt());
        apply3(&LMS_TO_LAB, lms)
    }

    /// The ray from the grey axis at `lab`'s lightness out through it:
    /// each cone response is `u + t v` in the share `t` of its chroma,
    /// so each output channel is a cubic in `t`, its coefficients
    /// worked out once.
    #[inline]
    fn ray(&self, [l, a, b]: [f32; 3]) -> Ray {
        let u = [0, 1, 2].map(|j| LAB_TO_LMS[j][0] * l);
        let v = [0, 1, 2].map(|j| LAB_TO_LMS[j][1] * a + LAB_TO_LMS[j][2] * b);
        let terms = [
            u.map(|x| x * x * x),
            [0, 1, 2].map(|j| 3.0 * u[j] * u[j] * v[j]),
            [0, 1, 2].map(|j| 3.0 * u[j] * v[j] * v[j]),
            v.map(|x| x * x * x),
        ];
        Ray(terms.map(|w| apply3(&self.lms_to_out, w)))
    }

    /// The reach of a color: its chroma over the edge's along its ray,
    /// the lower faces alone. Read out to where the op starts acting
    /// ([`Params::onset`]); zero for a color nearer the axis than that,
    /// a neutral, or a black.
    ///
    /// The edge's slope in the color is bounded, so that no two
    /// neighbors find it far apart. Along the ray a channel can leave the
    /// cube and come back (by the blue corner, by a hair or by a lot), or
    /// run along a face, and wherever the ray grazes a face any crossing
    /// of it (the first, the last, or one weighed over a band) slides by
    /// the graze's length for a change in the fifth place. So the edge is
    /// where the ray's depth past the faces, held rising, meets a spring
    /// to the chord: where the straight line from grey through the color
    /// leaves the lower faces, which has one exit and moves with the
    /// color as smoothly as the color does. The depth past the chord is
    /// the deepest any channel has been under zero since; short of it,
    /// the least any one channel has been under zero all the way. Where
    /// the ray crosses a face cleanly the edge is near there; where it
    /// grazes one, the spring holds the edge near the chord and the line to
    /// grey takes in what is left ([`SPRING`]). An exit behind an inside
    /// color does not count, so nothing inside moves under thresholds of
    /// one. Held to the definition worked in f64
    /// (`the_edge_is_its_definition`).
    pub fn reach_of(&self, lab: [f32; 3]) -> f32 {
        self.reach_along(&self.ray(lab), lab)
    }

    /// [`Op::reach_of`] along the color's ray, worked out already.
    fn reach_along(&self, ray: &Ray, [l, a, b]: [f32; 3]) -> f32 {
        if !(l > 0.0 && a.hypot(b) > 1e-7) {
            return 0.0;
        }
        let span = 1.0 / self.params.onset();
        let grey = ray.0[0][0];
        if grey.is_nan() || grey <= 0.0 {
            return 0.0;
        }
        // Where the straight line from grey through the color leaves the
        // lower faces, as a share of the way to it.
        let mut chord = f32::INFINITY;
        for v in ray.at(1.0) {
            if v < grey {
                chord = chord.min(grey / (grey - v));
            }
        }
        let chord = chord.min(span);
        let t0 = chord.min(1.0);
        let at = ray.at(t0);
        let n = at[0].max(at[1]).max(at[2]).max(grey);
        let ch: [Cubic; 3] =
            [0, 1, 2].map(|k| [ray.0[0][k], ray.0[1][k], ray.0[2][k], ray.0[3][k]]);
        let turn = ch.map(|h| turns(&h, span));
        let rise = |e: f32| depth(&ch, &turn, t0, e) / n + SPRING * (e - chord);
        if rise(span) <= 0.0 {
            return 0.0;
        }
        let (mut lo, mut hi) = (0.0f32, span);
        for _ in 0..BISECT {
            let mid = 0.5 * (lo + hi);
            if rise(mid) > 0.0 {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        let edge = 0.5 * (lo + hi);
        if edge >= span { 0.0 } else { 1.0 / edge }
    }

    /// The reach of a working-space color.
    pub fn reach(&self, c: [f32; 3]) -> f32 {
        self.reach_of(self.lab(c))
    }

    /// The early out's share: [`EARLY`], or none where the op acts only
    /// outside the output.
    fn early(&self) -> f32 {
        if self.params.onset() >= 1.0 {
            0.0
        } else {
            EARLY
        }
    }

    /// What the shader is handed (`viewport.wgsl`'s `Gamut`): the
    /// rail's on, knee, limit, and the bound's soft share; the bound's
    /// on, onset, and asymptote; the roll's knee and limit, the onset,
    /// and the early out's share. Then, rows, the working space to
    /// linear sRGB and back, the output back to the working space, and
    /// cone space to the output.
    pub fn shader(&self) -> ([[f32; 4]; 3], [Matrix3; 4]) {
        let p = &self.params;
        let on = |b: bool| if b { 1.0 } else { 0.0 };
        let rail = p.rail.unwrap_or([0.0; 2]);
        let bound = p.bound.unwrap_or([0.0; 2]);
        (
            [
                [on(p.rail.is_some()), rail[0], rail[1], p.soft],
                [on(p.bound.is_some()), bound[0], bound[1], 0.0],
                [p.roll[0], p.roll[1], p.onset(), self.early()],
            ],
            [self.to_srgb, self.from_srgb, self.from_out, self.lms_to_out],
        )
    }

    /// Cone space to the output, rows: a look table's cube's, for the
    /// shader.
    pub fn cone_to_output(&self) -> Matrix3 {
        self.lms_to_out
    }
}

/// Each output channel along a color's ray, a cubic in the share of
/// its chroma: the constant, linear, square, and cube terms.
#[derive(Debug, Clone, Copy)]
struct Ray([[f32; 3]; 4]);

impl Ray {
    /// The output's channels a share `t` of the way out.
    #[inline]
    fn at(&self, t: f32) -> [f32; 3] {
        let [p0, p1, p2, p3] = self.0;
        [0, 1, 2].map(|k| ((p3[k] * t + p2[k]) * t + p1[k]) * t + p0[k])
    }
}

/// A cubic, lowest power first.
type Cubic = [f32; 4];

#[inline]
fn eval(h: &Cubic, u: f32) -> f32 {
    ((h[3] * u + h[2]) * u + h[1]) * u + h[0]
}

/// `0`, where a cubic turns in `(0, len)` (3 h3 u^2 + 2 h2 u + h1 = 0),
/// and `len`, ascending, and how many of them there are.
fn turns(h: &Cubic, len: f32) -> ([f32; 4], usize) {
    let mut cuts = [0.0, len, len, len];
    let mut n = 1;
    let (qa, qb, qc) = (3.0 * h[3], 2.0 * h[2], h[1]);
    let mut turn = |u: f32| {
        if u > 0.0 && u < len {
            cuts[n] = u;
            n += 1;
        }
    };
    if qa != 0.0 {
        let disc = qb * qb - 4.0 * qa * qc;
        if disc > 0.0 {
            let q = -0.5 * (qb + qb.signum() * disc.sqrt());
            let (r0, r1) = (q / qa, if q != 0.0 { qc / q } else { 0.0 });
            turn(r0.min(r1));
            turn(r0.max(r1));
        }
    } else if qb != 0.0 {
        turn(-qc / qb);
    }
    cuts[n] = len;
    (cuts, n + 1)
}

/// How far the ray is out of the cube at `e`, counted from `t0`, in
/// the output's units: past `t0`, the deepest any channel has been
/// under zero between them; short of it, the least any one channel has
/// been under zero all the way between. Rising with `e`, and the
/// deepest channel's depth at `t0` there. Each channel's least and most
/// over a stretch are at its ends or where it turns (`turn`).
fn depth(ch: &[Cubic; 3], turn: &[([f32; 4], usize); 3], t0: f32, e: f32) -> f32 {
    let (a, b) = if e >= t0 { (t0, e) } else { (e, t0) };
    let mut d = f32::NEG_INFINITY;
    for (h, (cuts, m)) in ch.iter().zip(turn) {
        let (mut low, mut high) = (eval(h, a), eval(h, b));
        if low > high {
            (low, high) = (high, low);
        }
        for u in &cuts[1..m - 1] {
            if *u > a && *u < b {
                let v = eval(h, *u);
                low = low.min(v);
                high = high.max(v);
            }
        }
        d = d.max(if e >= t0 { -low } else { -high });
    }
    d
}

/// One op kept on a thread, by the matrix and thresholds it was made
/// for: each caller has its own, so two that alternate on a pixel do not
/// make each other's again.
type Kept = std::cell::Cell<Option<(Matrix3, Params, Option<Op>)>>;

#[inline]
fn kept(
    cell: &'static std::thread::LocalKey<Kept>,
    to_out: &Matrix3,
    params: Params,
) -> Option<Op> {
    cell.with(|cell| match cell.get() {
        Some((m, p, op)) if m == *to_out && p == params => op,
        _ => {
            let op = Op::for_output(to_out, params);
            cell.set(Some((*to_out, params, op)));
            op
        }
    })
}

/// The op for `to_out` under `params`, kept from the last pixel on each
/// thread: the finish hands every pixel the same matrix. The output's
/// own; the look table's and the working cube's keep theirs apart.
#[inline]
pub fn op_for(to_out: &Matrix3, params: Params) -> Option<Op> {
    thread_local! {
        static OUTPUT: Kept = const { std::cell::Cell::new(None) };
    }
    kept(&OUTPUT, to_out, params)
}

/// The look table's form under the gamut op (§5): the table reads the
/// color brought to its cube with no knee (the top per channel, as its
/// fit saw it; the rest to the lower faces at held Oklab hue), and its
/// change is added to the color as it came, `c + s (T(g) - g)`. Inside
/// the table's gamut this is [`greycard_core::lut::Look::at`]'s
/// arithmetic, to rounding; outside, the color keeps what the table
/// cannot see, and the output brings it in once.
pub fn look_kept(look: &greycard_core::lut::Look, c: [f32; 3]) -> [f32; 3] {
    if look.is_off() {
        return c;
    }
    let to = look.to_lut();
    let from = look.from_lut();
    let r = apply3(to, c);
    let g = if r.iter().all(|v| (0.0..=1.0).contains(v)) {
        r
    } else {
        let top = apply3(from, r.map(|v| v.min(1.0)));
        thread_local! {
            static TABLE: Kept = const { std::cell::Cell::new(None) };
        }
        match kept(&TABLE, to, Params::CUBE) {
            Some(op) => op.apply(top),
            None => r.map(|v| v.clamp(0.0, 1.0)),
        }
    };
    let enc = look.lut.encoding;
    let o = look
        .lut
        .sample(g.map(|v| enc.encode(v)))
        .map(|v| enc.decode(v));
    let (back, base) = (apply3(from, o), apply3(from, g));
    [0, 1, 2].map(|k| c[k] + look.strength * (back[k] - base[k]))
}

/// The working space's lower faces at held hue, for a camera profile's
/// curve, whose result can leave the working cube (§5): in place of
/// the clamp under zero before the point curves. The top is left for
/// that clamp, which is the curves' index.
pub fn into_working(c: [f32; 3]) -> [f32; 3] {
    const IDENTITY: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    if c.iter().all(|v| *v >= 0.0) {
        return c;
    }
    static CUBE: std::sync::OnceLock<Option<Op>> = std::sync::OnceLock::new();
    match CUBE.get_or_init(|| Op::for_output(&IDENTITY, Params::CUBE)) {
        Some(op) => {
            let o = op.unclamped(c);
            o.map(|v| v.max(0.0))
        }
        None => c.map(|v| v.max(0.0)),
    }
}

/// The floor (§6.2): on the shaped scene, before the display curve per
/// channel, each channel taken toward the others by [`FLOOR`] eased in
/// from a stop under `white` to two over, on the differences, so a
/// neutral is the curve's to the bit and nothing under a stop below
/// white moves. Blue's share goes 0.9 to red and 1.1 to green, which
/// keeps a pure blue's hue on its way to white.
pub fn floor(s: [f32; 3], white: f32) -> [f32; 3] {
    let sq = s.map(|v| v.max(0.0));
    let den = sq[0] * sq[0] + sq[1] * sq[1] + sq[2] * sq[2];
    if den <= 0.0 {
        return s;
    }
    let num = sq[0] * sq[0] * sq[0] + sq[1] * sq[1] * sq[1] + sq[2] * sq[2] * sq[2];
    let n = num / den;
    let kappa = FLOOR * smoothstep(-1.0, 2.0, (n / white).log2());
    if kappa <= 0.0 {
        return s;
    }
    const THIRD: f32 = 1.0 / 3.0;
    const W: [[f32; 3]; 3] = [
        [1.1 * THIRD, THIRD, 0.9 * THIRD],
        [THIRD, 0.9 * THIRD, 1.1 * THIRD],
        [THIRD, THIRD, THIRD],
    ];
    [0, 1, 2].map(|i| s[i] + kappa * (0..3).map(|j| W[i][j] * (s[j] - s[i])).sum::<f32>())
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests;
