//! A vignette over the frame as shown: an exposure falloff, in stops,
//! from a shape about the center to the corners, so darkening keeps
//! its hues as the exposure slider does. Positions are fractions of
//! the frame, so it sits on the crop and any size of export.

use serde::{Deserialize, Serialize};

use crate::mask::smoothstep;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vignette {
    /// The section's switch.
    pub enabled: bool,
    /// Stops at the corners, negative to darken.
    pub amount: f32,
    /// Where the falloff begins, 0 at the center to 1 at the corner.
    pub midpoint: f32,
    /// How far past the midpoint it takes to reach the full amount,
    /// as a fraction of the way to the corner; 0 is a hard edge.
    pub feather: f32,
    /// The shape: 0 an ellipse in the frame's proportion, 1 a circle,
    /// -1 a rounded rectangle.
    pub roundness: f32,
}

impl Default for Vignette {
    fn default() -> Self {
        Self {
            enabled: true,
            amount: 0.0,
            midpoint: 0.5,
            feather: 0.5,
            roundness: 0.0,
        }
    }
}

impl Vignette {
    pub fn is_off(&self) -> bool {
        !self.enabled || self.amount == 0.0
    }

    /// How much of the amount acts at (u, v), fractions of a frame
    /// `aspect` (width over height) wide: 0 inside the midpoint, 1
    /// past the feather. The shader's `vignette_at` is this.
    pub fn at(&self, u: f32, v: f32, aspect: f32) -> f32 {
        let (p, q) = (2.0 * u - 1.0, 2.0 * v - 1.0);
        let r = self.roundness.clamp(-1.0, 1.0);
        // Towards a circle: the long side stretched by the aspect's
        // power, then the whole scaled so the long side's middle stays
        // at one.
        let stretch = if aspect >= 1.0 { aspect } else { 1.0 / aspect }.powf(r.max(0.0));
        let (x, y) = if aspect >= 1.0 {
            (p, q / stretch)
        } else {
            (p / stretch, q)
        };
        // Towards a rectangle: a superellipse.
        let n = if r < 0.0 { 2.0 / (1.0 + 0.85 * r) } else { 2.0 };
        let d = (x.abs().powf(n) + y.abs().powf(n)).powf(1.0 / n) / std::f32::consts::SQRT_2;
        let m = self.midpoint.clamp(0.0, 1.0);
        smoothstep(m, m + self.feather.clamp(0.0, 1.0), d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_falloff_runs_from_the_midpoint_to_the_corner() {
        let v = Vignette {
            enabled: true,
            amount: -1.0,
            midpoint: 0.5,
            feather: 0.5,
            roundness: 0.0,
        };
        assert_eq!(v.at(0.5, 0.5, 1.5), 0.0);
        // The frame's ellipse: the edge middles are 1/sqrt(2) of the
        // way to the corner, which the smoothstep puts at 0.372.
        let edge = 0.372;
        assert!((v.at(1.0, 0.5, 1.5) - edge).abs() < 0.01);
        assert!((v.at(0.5, 0.0, 1.5) - edge).abs() < 0.01);
        assert_eq!(v.at(0.0, 0.0, 1.5), 1.0);
        // A hard edge at the midpoint.
        let hard = Vignette { feather: 0.0, ..v };
        assert_eq!(hard.at(0.5 + 0.34, 0.5, 1.5), 0.0);
        assert_eq!(hard.at(0.5 + 0.36, 0.5, 1.5), 1.0);
        // A circle: the short side's middle is as far as the long
        // side's is on a 3:2 frame times the aspect, so it is past it.
        let round = Vignette {
            roundness: 1.0,
            ..v
        };
        assert!((round.at(1.0, 0.5, 1.5) - edge).abs() < 0.01);
        assert!(round.at(0.5, 0.0, 1.5) < edge - 0.05);
        assert!((round.at(0.5, 0.0, 1.0 / 1.5) - edge).abs() < 0.01);
        // A rectangle: the diagonal reaches less far than the ellipse's.
        let square = Vignette {
            roundness: -1.0,
            ..v
        };
        assert!(square.at(0.2, 0.2, 1.5) < v.at(0.2, 0.2, 1.5));
        assert!(Vignette::default().is_off());
    }
}
