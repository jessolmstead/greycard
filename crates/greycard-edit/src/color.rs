//! Global vibrance and saturation: a scale on Oklab chroma, on the
//! scene-linear picture in the same pass as the color mixer.

use serde::{Deserialize, Serialize};

use crate::mask::smoothstep;

/// Oklab chroma of the sRGB primaries: red .257, green .295, blue
/// .313. 0.3 stands in for "about as saturated as the gamut gets",
/// the reference vibrance measures room against.
const ROOM_CHROMA: f32 = 0.3;

/// The hue vibrance treats as skin, in Oklab degrees, where the
/// color mixer's orange band also sits.
const SKIN_HUE: f32 = 55.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Color {
    /// The section's switch.
    pub enabled: bool,
    /// Scale on chroma, -1 (grey) to 1 (double).
    pub saturation: f32,
    /// Scale on chroma weighted toward the pale colors, -1 to 1; half
    /// on skin.
    pub vibrance: f32,
}

impl Default for Color {
    fn default() -> Self {
        Self {
            enabled: true,
            saturation: 0.0,
            vibrance: 0.0,
        }
    }
}

impl Color {
    /// What acts: this color, or with the switch off, both sliders
    /// at nothing, as `Light::effective` and `Mixer::effective` do it.
    pub fn effective(&self) -> Color {
        if self.enabled {
            *self
        } else {
            Color {
                enabled: false,
                saturation: 0.0,
                vibrance: 0.0,
            }
        }
    }

    pub fn is_identity(&self) -> bool {
        !self.enabled || (self.saturation == 0.0 && self.vibrance == 0.0)
    }

    /// The scale on chroma at a chroma and a hue (after the mixer's
    /// own shift and scale). Saturation is flat; vibrance leans on the
    /// room left below [`ROOM_CHROMA`] and backs off toward
    /// [`SKIN_HUE`], where it acts at half.
    pub fn scale(&self, chroma: f32, hue_degrees: f32) -> f32 {
        let sat_scale = (1.0 + self.saturation).max(0.0);
        let room = (1.0 - chroma / ROOM_CHROMA).clamp(0.0, 1.0);
        let d = {
            let wrapped = (hue_degrees - SKIN_HUE + 180.0).rem_euclid(360.0);
            (wrapped - 180.0).abs()
        };
        let protect = 1.0 - 0.5 * (1.0 - smoothstep(15.0, 45.0, d));
        let vib_scale = (1.0 + self.vibrance * room * protect).max(0.0);
        sat_scale * vib_scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_identity() {
        assert!(Color::default().is_identity());
        assert_eq!(Color::default().scale(0.1, 30.0), 1.0);
    }

    #[test]
    fn saturation_scales_chroma_flat_wherever_it_sits() {
        let up = Color {
            saturation: 1.0,
            ..Color::default()
        };
        let down = Color {
            saturation: -1.0,
            ..Color::default()
        };
        for (chroma, hue) in [(0.0, 200.0), (0.15, 55.0), (0.3, 90.0)] {
            assert!((up.scale(chroma, hue) - 2.0).abs() < 1e-6);
            assert_eq!(down.scale(chroma, hue), 0.0);
        }
    }

    #[test]
    fn vibrance_leans_on_room_and_backs_off_on_skin() {
        let vib = Color {
            vibrance: 1.0,
            ..Color::default()
        };
        // Full room, far from skin: full effect.
        assert!((vib.scale(0.0, 200.0) - 2.0).abs() < 1e-6);
        // No room left: no effect at all.
        assert!((vib.scale(ROOM_CHROMA, 200.0) - 1.0).abs() < 1e-6);
        // Full room, right on skin: half effect.
        assert!((vib.scale(0.0, SKIN_HUE) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn a_disabled_section_is_identity_whatever_the_sliders() {
        let c = Color {
            enabled: false,
            saturation: 1.0,
            vibrance: 1.0,
        };
        assert!(c.is_identity());
        assert_eq!(c.effective().scale(0.05, 200.0), 1.0);
        assert_eq!(Color { enabled: true, ..c }.effective().saturation, 1.0);
    }
}
