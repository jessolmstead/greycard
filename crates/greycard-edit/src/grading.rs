//! Color grading: three wheels, for the shadows, the mid-tones and
//! the highlights, each a hue and a strength, blended by lightness
//! into a shift of Oklab's a and b, the same shift the color curves
//! make (`curve.rs`), so the two add into one table.

use serde::{Deserialize, Serialize};

/// One wheel: an Oklab hue in degrees, and how far out toward it.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Wheel {
    pub hue: f32,
    /// 0 to 1: nothing, to a shift of [`RANGE`] in a and b.
    pub saturation: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grading {
    pub enabled: bool,
    pub shadows: Wheel,
    pub midtones: Wheel,
    pub highlights: Wheel,
    /// Where the mid-tones sit, -1 to 1: positive hands more of the
    /// picture to the highlights wheel, negative to the shadows'.
    pub balance: f32,
}

impl Default for Grading {
    fn default() -> Self {
        Self {
            enabled: true,
            shadows: Wheel::default(),
            midtones: Wheel::default(),
            highlights: Wheel::default(),
            balance: 0.0,
        }
    }
}

/// The shift of a or b at a wheel's full strength: the same as a
/// color curve at its edge.
pub const RANGE: f32 = 0.2;

/// Which wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Shadows,
    Midtones,
    Highlights,
}

impl Range {
    pub const ALL: [Range; 3] = [Range::Shadows, Range::Midtones, Range::Highlights];

    pub fn name(self) -> &'static str {
        match self {
            Range::Shadows => "Shadows",
            Range::Midtones => "Mid-tones",
            Range::Highlights => "Highlights",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }
}

impl Grading {
    pub fn is_identity(&self) -> bool {
        !self.enabled
            || Range::ALL
                .iter()
                .all(|r| self.wheel(*r).saturation.abs() < 1e-6)
    }

    pub fn wheel(&self, range: Range) -> &Wheel {
        match range {
            Range::Shadows => &self.shadows,
            Range::Midtones => &self.midtones,
            Range::Highlights => &self.highlights,
        }
    }

    pub fn wheel_mut(&mut self, range: Range) -> &mut Wheel {
        match range {
            Range::Shadows => &mut self.shadows,
            Range::Midtones => &mut self.midtones,
            Range::Highlights => &mut self.highlights,
        }
    }

    /// How much of a pixel at lightness `l` each wheel gets: the
    /// three sum to one, the shadows have all of black, the highlights
    /// all of white, the mid-tones half of the middle. Balance moves
    /// the middle by a power of the lightness.
    pub fn weights(&self, l: f32) -> [f32; 3] {
        let t = l.clamp(0.0, 1.0).powf(2f32.powf(-self.balance));
        [(1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t), t * t]
    }

    /// The shift of a and b at lightness `l`.
    pub fn shift_at(&self, l: f32) -> [f32; 2] {
        if !self.enabled {
            return [0.0, 0.0];
        }
        let w = self.weights(l);
        let mut shift = [0.0, 0.0];
        for (k, range) in Range::ALL.iter().enumerate() {
            let wheel = self.wheel(*range);
            let (s, c) = wheel.hue.to_radians().sin_cos();
            let r = w[k] * wheel.saturation.clamp(0.0, 1.0) * RANGE;
            shift[0] += r * c;
            shift[1] += r * s;
        }
        shift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weights_partition_the_lightnesses_and_balance_moves_them() {
        let g = Grading::default();
        assert!(g.is_identity());
        for i in 0..=20 {
            let w = g.weights(i as f32 / 20.0);
            assert!((w[0] + w[1] + w[2] - 1.0).abs() < 1e-6);
        }
        assert_eq!(g.weights(0.0), [1.0, 0.0, 0.0]);
        assert_eq!(g.weights(1.0), [0.0, 0.0, 1.0]);
        assert!((g.weights(0.5)[1] - 0.5).abs() < 1e-6);
        // Balance toward the highlights: a mid lightness counts more
        // as highlight than before.
        let up = Grading { balance: 1.0, ..g };
        assert!(up.weights(0.5)[2] > g.weights(0.5)[2]);
        let down = Grading { balance: -1.0, ..g };
        assert!(down.weights(0.5)[0] > g.weights(0.5)[0]);
    }

    #[test]
    fn a_wheel_shifts_its_range_toward_its_hue() {
        // Shadows toward red (hue 0): a shift in a alone, full at
        // black, none at white.
        let g = Grading {
            shadows: Wheel {
                hue: 0.0,
                saturation: 1.0,
            },
            ..Default::default()
        };
        assert!(!g.is_identity());
        assert_eq!(g.shift_at(0.0), [RANGE, 0.0]);
        assert_eq!(g.shift_at(1.0), [0.0, 0.0]);
        let mid = g.shift_at(0.5);
        assert!((mid[0] - RANGE / 4.0).abs() < 1e-6 && mid[1].abs() < 1e-6);
        // Highlights toward yellow (hue 90): b alone, at white.
        let h = Grading {
            highlights: Wheel {
                hue: 90.0,
                saturation: 0.5,
            },
            ..Default::default()
        };
        let top = h.shift_at(1.0);
        assert!(top[0].abs() < 1e-6 && (top[1] - RANGE / 2.0).abs() < 1e-6);
        // Off is nothing.
        let off = Grading {
            enabled: false,
            ..g
        };
        assert!(off.is_identity() && off.shift_at(0.0) == [0.0, 0.0]);
    }
}
