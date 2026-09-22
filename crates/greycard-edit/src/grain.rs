//! Film grain: a noise over the frame as shown, in the frame's own
//! units so a size looks the same at any size of export, laid on the
//! finished picture as luminance, weighted towards the midtones and
//! upper midtones, as a scanned negative shows it. Value noise on a
//! lattice from an integer hash, so the shader and the export draw
//! the same grain to the bit of the hash.

use serde::{Deserialize, Serialize};

/// The crystal's shape, as film has it: cubic grains are the classic
/// ones, coarse and clumpy; tabular ones are flat, finer and more
/// even for their size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Cubic,
    Tabular,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Cubic, Kind::Tabular];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Cubic => "Cubic",
            Kind::Tabular => "Tabular",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// What the crystals are like. The shader's `grain_at` has these
    /// too.
    pub fn character(self) -> Character {
        match self {
            // Large, of every size and strength, lumpy, dyed.
            Kind::Cubic => Character {
                radius: 0.2,
                spread: 0.3,
                floor: 0.0,
                tint: 0.3,
                norm: 1.0,
            },
            // Smaller, alike, even, lightly dyed.
            Kind::Tabular => Character {
                radius: 0.14,
                spread: 0.06,
                floor: 0.6,
                tint: 0.15,
                norm: 2.2,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grain {
    /// The section's switch.
    pub enabled: bool,
    /// How much, 0 to 1.
    pub amount: f32,
    /// The grain's cell, thousandths of the frame's width.
    pub size: f32,
    pub kind: Kind,
}

impl Default for Grain {
    fn default() -> Self {
        Self {
            enabled: true,
            amount: 0.0,
            size: 0.5,
            kind: Kind::Cubic,
        }
    }
}

/// What a full amount adds at the weighting's peak (`GRAIN_PEAK`) to
/// an encoded value; the shadows and highlights keep a little over
/// half of that. Unchanged since the shadow-weighted curve, so an
/// edit's `Amount` still means about what it did in the midtones.
pub const GRAIN_SCALE: f32 = 0.12;

/// Where the tonal weighting peaks: a touch above mid grey, where a
/// face or a bright sky sits.
const GRAIN_PEAK: f32 = 0.6;

/// The peak's height, a fraction of `GRAIN_SCALE`: close to what the
/// old shadow-weighted curve gave at mid grey, so an edit's `Amount`
/// reads about the same there even though the curve's shape has
/// moved.
const GRAIN_PEAK_WEIGHT: f32 = 0.8;

/// The floor at the shadow and highlight ends, a fraction of the
/// peak: never under half, so a sky or a bright face still shows
/// grain, and deep shadow keeps some too.
const GRAIN_FLOOR: f32 = 0.55;

/// A hash of a lattice point and a seed to 32 bits. The shader's
/// `grain_hash` is this, so the two must change together.
pub fn hash(x: u32, y: u32, seed: u32) -> u32 {
    let mut h =
        x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    h
}

/// The finest lattice's pitch, thousandths of the frame's width: the
/// smallest size, where every crystal has its home.
pub const BASE: f32 = 0.1;

/// A crystal: its home in the finest lattice (base cell and slot) and
/// what its hash says of it: its place in that cell (0 to 1 each way),
/// its radius (0 to 1), its strength (-1 to 1) and its dye tints (-1
/// to 1), all from one hash's bits.
#[inline]
fn crystal(base: (u32, u32), slot: u32) -> (f32, f32, f32, f32, f32, f32) {
    let h = hash(base.0, base.1, slot + 1);
    let ox = (h & 0x3f) as f32 / 63.0;
    let oy = ((h >> 6) & 0x3f) as f32 / 63.0;
    let r = ((h >> 12) & 0x1f) as f32 / 31.0;
    let amp = ((h >> 17) & 0x7f) as f32 / 127.0 * 2.0 - 1.0;
    let t1 = ((h >> 24) & 0xf) as f32 / 15.0 * 2.0 - 1.0;
    let t2 = ((h >> 28) & 0xf) as f32 / 15.0 * 2.0 - 1.0;
    (ox, oy, r, amp, t1, t2)
}

/// Which of a cell's four candidates stands for it one level up.
#[inline]
fn pick(level: u32, cell: (u32, u32)) -> u32 {
    hash(cell.0, cell.1, 0x100 + level) & 3
}

/// Candidate `d` of the cell at `level`: at the base its slot `d`;
/// above, the pick of child `d`, followed down to its home. The
/// crystal's home cell and slot.
#[inline]
fn candidate(level: u32, cell: (u32, u32), d: u32) -> ((u32, u32), u32) {
    if level == 0 {
        return (cell, d);
    }
    let mut cell = (2 * cell.0 + (d & 1), 2 * cell.1 + (d >> 1));
    let mut level = level - 1;
    while level > 0 {
        let c = pick(level, cell);
        cell = (2 * cell.0 + (c & 1), 2 * cell.1 + (c >> 1));
        level -= 1;
    }
    (cell, pick(0, cell))
}

impl Grain {
    pub fn is_off(&self) -> bool {
        !self.enabled || self.amount <= 0.0
    }

    /// The lattice for the size: its level above the base, and how far
    /// through that level's octave the size is (1 to 2).
    pub fn level(&self) -> (u32, f32) {
        let ratio = self.size.max(BASE) / BASE;
        let level = ratio.log2().floor().max(0.0);
        (level as u32, ratio / 2f32.powf(level))
    }

    /// The grain at (u, v), fractions of a frame `aspect` (width over
    /// height) wide, per channel, about -amount to amount: the
    /// crystals of the cell and its eight neighbors at the size's
    /// level, each a soft bump of its own radius and strength, summed,
    /// tinted a little by their dyes. A crystal keeps its home whatever
    /// the size; the size grows its radius through the octave while
    /// the three of four a coarser cell will not keep fade, so the
    /// coverage holds and nothing moves. The shader's `grain_at` is
    /// this.
    pub fn at(&self, u: f32, v: f32, aspect: f32) -> [f32; 3] {
        let (level, f) = self.level();
        let pitch = 2f32.powi(level as i32);
        // In base cells, kept off zero by more than the coarsest
        // pitch's neighborhood, so the lattice indices stay whole.
        let base_cell = BASE / 1000.0;
        let (x, y) = (u / base_cell + 64.0, v / aspect / base_cell + 64.0);
        let (ix, iy) = (
            (x / pitch).floor().max(1.0) as u32,
            (y / pitch).floor().max(1.0) as u32,
        );
        let c = self.kind.character();
        // The unpicked candidates' weight: coverage held across the octave.
        let fade = (4.0 / (f * f) - 1.0) / 3.0;
        let mut sum = [0.0f32; 3];
        for j in 0..3u32 {
            for i in 0..3u32 {
                let cell = (ix + i - 1, iy + j - 1);
                let kept = pick(level, cell);
                for d in 0..4u32 {
                    let (home, slot) = candidate(level, cell, d);
                    let (ox, oy, r, amp, t1, t2) = crystal(home, slot);
                    let radius = (c.radius + c.spread * r) * f * pitch;
                    let (dx, dy) = (home.0 as f32 + ox - x, home.1 as f32 + oy - y);
                    let q = (dx * dx + dy * dy) / (radius * radius);
                    if q >= 1.0 {
                        continue;
                    }
                    let bump = (1.0 - q) * (1.0 - q);
                    let strength = amp.signum() * (c.floor + (1.0 - c.floor) * amp.abs());
                    let weight = if d == kept { 1.0 } else { fade };
                    let g = bump * strength * weight;
                    sum[0] += g * (1.0 + c.tint * t1);
                    sum[1] += g * (1.0 + c.tint * t2);
                    sum[2] += g * (1.0 - c.tint * t1);
                }
            }
        }
        let a = self.amount.clamp(0.0, 1.0) * c.norm;
        sum.map(|s| a * s)
    }

    /// The grain's weight at an encoded luma: peaks at `GRAIN_PEAK`,
    /// in the midtones and upper midtones, and eases off towards both
    /// ends without going under half its peak. The shader's
    /// `grain_apply` has this same curve.
    ///
    /// A scanned negative is why: perceived grain is noise in density,
    /// and density is close to linear in log exposure only through the
    /// film's straight line, so encoded value (which bends that
    /// straight line towards black and white through the toe and
    /// shoulder) carries the most of that noise where the curve's
    /// slope is highest — the midtones and a stop or so over them,
    /// where a face or a clear sky sits — and less at either end: the
    /// toe, close to the film's base with little density left to vary,
    /// and the shoulder, compressing towards white. Grain never
    /// vanishes at either end, the way a print does not.
    fn weight(luma: f32) -> f32 {
        let l = luma.clamp(0.0, 1.0);
        // luma/peak to the 1.5, as `r * r.sqrt()` rather than `powf`:
        // the shader's `sqrt` is precise the way its general `pow` is
        // not, and the two must land on the same bit, pixel for pixel.
        let r = l / GRAIN_PEAK;
        let bump = r * r.sqrt() * ((1.0 - l) / (1.0 - GRAIN_PEAK));
        GRAIN_SCALE * GRAIN_PEAK_WEIGHT * (GRAIN_FLOOR + (1.0 - GRAIN_FLOOR) * bump)
    }

    /// An encoded output value with its grain: `noise` from `at`,
    /// scaled by `weight`'s tonal curve.
    pub fn apply(noise: [f32; 3], e: [f32; 3]) -> [f32; 3] {
        let luma = 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
        let w = Self::weight(luma);
        [
            (e[0] + noise[0] * w).clamp(0.0, 1.0),
            (e[1] + noise[1] * w).clamp(0.0, 1.0),
            (e[2] + noise[2] * w).clamp(0.0, 1.0),
        ]
    }
}

/// What a kind's crystals are like: the smallest radius and the
/// spread above it (in cells of the size's lattice, at most half a
/// cell together, so the neighbors suffice), the least strength (0
/// every strength, 1 all the same), the dye tint, and the scale that
/// brings the sum to about the amount.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Character {
    pub radius: f32,
    pub spread: f32,
    pub floor: f32,
    pub tint: f32,
    pub norm: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mean, root mean square and the largest magnitude of the green
    /// channel over a grid.
    fn stats(g: &Grain) -> (f32, f32, f32) {
        let steps = 300;
        let (mut sum, mut sq, mut max) = (0.0f64, 0.0f64, 0.0f32);
        for i in 0..steps {
            for j in 0..steps {
                let n = g.at(
                    i as f32 / steps as f32 * 0.02,
                    j as f32 / steps as f32 * 0.02,
                    1.5,
                )[1];
                sum += n as f64;
                sq += (n * n) as f64;
                max = max.max(n.abs());
            }
        }
        let n = (steps * steps) as f64;
        ((sum / n) as f32, (sq / n).sqrt() as f32, max)
    }

    #[test]
    fn the_grain_is_zero_mean_bounded_smooth_and_never_the_same_twice() {
        let g = Grain {
            enabled: true,
            amount: 1.0,
            size: 0.5,
            kind: Kind::Cubic,
        };
        let (mean, rms, max) = stats(&g);
        assert!(mean.abs() < 0.05, "{mean}");
        assert!((0.25..0.6).contains(&rms), "{rms}");
        assert!(max < 2.5, "{max}");
        // Nearby points are alike: a step of a hundredth of a cell.
        let a = g.at(0.3, 0.2, 1.5);
        let b = g.at(0.3 + 0.000005, 0.2, 1.5);
        assert!((a[1] - b[1]).abs() < 0.08, "{a:?} {b:?}");
        // The same place is the same grain, and half the amount is half.
        assert_eq!(g.at(0.3, 0.2, 1.5), a);
        let half = Grain { amount: 0.5, ..g };
        assert!((half.at(0.3, 0.2, 1.5)[1] - a[1] / 2.0).abs() < 1e-6);
        // A cell along is not the same grain again, nor a hundred.
        let cell = 0.5 / 1000.0;
        // Sized up, the crystals stay where they are: the fields at
        // one size and a third more are alike, across an octave too.
        let alike = |a: &Grain, b: &Grain| {
            let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
            for i in 0..120 {
                for j in 0..120 {
                    let (u, v) = (0.2 + i as f32 * 0.00004, 0.3 + j as f32 * 0.00004);
                    let (x, y) = (a.at(u, v, 1.5)[1] as f64, b.at(u, v, 1.5)[1] as f64);
                    ab += x * y;
                    aa += x * x;
                    bb += y * y;
                }
            }
            (ab / (aa * bb).sqrt()) as f32
        };
        let bigger = Grain { size: 0.65, ..g };
        assert!(alike(&g, &bigger) > 0.5, "{}", alike(&g, &bigger));
        let across = Grain { size: 0.85, ..g };
        assert!(alike(&g, &across) > 0.3, "{}", alike(&g, &across));
        assert_eq!(g.level(), (2, 1.25));
        assert_ne!(g.at(0.3 + cell, 0.2, 1.5), a);
        assert_ne!(g.at(0.3 + 100.0 * cell, 0.2, 1.5), a);
        // The dyes: the channels differ.
        let mut apart = 0;
        for i in 0..50 {
            let n = g.at(0.1 + i as f32 * 0.0003, 0.4, 1.5);
            if (n[0] - n[2]).abs() > 0.02 {
                apart += 1;
            }
        }
        assert!(apart > 10, "{apart}");
        assert!(Grain::default().is_off());
        // Tabular grain is the finer: over a quarter of a cell it
        // changes more, for its spread, than cubic does; and it too is
        // about the amount.
        let tabular = Grain {
            kind: Kind::Tabular,
            ..g
        };
        let (tmean, trms, _) = stats(&tabular);
        assert!(tmean.abs() < 0.05, "{tmean}");
        assert!((0.25..0.6).contains(&trms), "{trms}");
        let fineness = |g: &Grain, rms: f32| {
            let mut d = 0.0;
            for i in 0..400 {
                let u = 0.05 + i as f32 * 0.00013;
                d += (g.at(u, 0.3, 1.5)[1] - g.at(u + cell / 4.0, 0.3, 1.5)[1]).abs();
            }
            d / 400.0 / rms
        };
        assert!(fineness(&tabular, trms) > fineness(&g, rms) * 1.2);
        assert_eq!(Kind::from_name("Tabular"), Some(Kind::Tabular));
        // The weighting: the midtones take more than either end,
        // neither end under half the peak, and the ends (the curve's
        // minimum) are the weakest points there are.
        let peak = Grain::weight(GRAIN_PEAK);
        let dark = Grain::weight(0.05);
        let mid = Grain::weight(0.55);
        let light = Grain::weight(0.95);
        assert!(mid > dark, "{mid} {dark}");
        assert!(mid > light, "{mid} {light}");
        assert!(dark >= peak * 0.5 - 1e-4, "{dark} {peak}");
        assert!(light >= peak * 0.5 - 1e-4, "{light} {peak}");
        let (shadow_end, highlight_end) = (Grain::weight(0.0), Grain::weight(1.0));
        assert!(shadow_end >= peak * 0.5 - 1e-4, "{shadow_end} {peak}");
        assert!(highlight_end >= peak * 0.5 - 1e-4, "{highlight_end} {peak}");
        for l in 0..=20 {
            let w = Grain::weight(l as f32 / 20.0);
            assert!(w >= shadow_end.min(highlight_end) - 1e-4, "{l} {w}");
        }
        // Applied: nothing leaves 0..1.
        let one = [1.0; 3];
        assert_eq!(Grain::apply(one, [1.0; 3]), [1.0; 3]);
        assert_eq!(Grain::apply([-1.0; 3], [0.0; 3]), [0.0; 3]);
    }
}
