//! The output sharpen: an unsharp mask on the finished picture's
//! luminance, after the tone curve, the curves, and the look table,
//! beside the capture sharpen, which deconvolves the scene-linear
//! develop. The picture's; a mask takes from its amount or adds to it
//! in its Detail (`Detail::output_sharpen`): a take at the picture's
//! radius and threshold, an addition at the mask's own where it has
//! them (`Detail::output_sharpen_radius`, `output_sharpen_threshold`)
//! and the picture's where not. The render crate's `output_sharpen` is
//! the op.

use serde::{Deserialize, Serialize};

/// The panel's ends, which the op holds a written value to.
pub const AMOUNT_MAX: f32 = 2.0;
pub const RADIUS_MIN: f32 = 0.5;
pub const RADIUS_MAX: f32 = 3.0;
pub const THRESHOLD_MAX: f32 = 0.1;
/// A mask's share's ends (`Detail::output_sharpen`): -100 takes all the
/// picture's away, +200 adds the picture's whole Amount's range.
pub const SHARE_MIN: f32 = -1.0;
pub const SHARE_MAX: f32 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSharpen {
    /// The section's switch.
    pub enabled: bool,
    /// How much of the difference from the blur is added back, 0 to 2
    /// (the panel shows 0 to 200%). Zero is no op at all.
    pub amount: f32,
    /// The blur's standard deviation, 0.5 to 3, in pixels of the
    /// picture made: a full-size export's, a smaller export's own, and
    /// the window's at the zoom under 1:1 (a full-size pixel's past
    /// it). The same radius crisps a web-size export as it does the
    /// full-size one; a 0.12 edit's full-size export is as it was.
    pub radius: f32,
    /// The difference, in encoded luminance (0 to 0.1, the panel shows
    /// 0 to 10%), at which half of it is sharpened: less is kept back
    /// on a soft knee, more goes nearly whole.
    pub threshold: f32,
}

impl Default for OutputSharpen {
    fn default() -> Self {
        Self {
            enabled: true,
            amount: 0.0,
            radius: 1.0,
            threshold: 0.01,
        }
    }
}

impl OutputSharpen {
    /// Whether this is the default, which a sidecar leaves out.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether it does nothing: switched off or at no amount.
    pub fn is_off(&self) -> bool {
        !self.enabled || self.amount <= 0.0
    }

    /// The settings as the op takes them, held to the panel's ends, or
    /// none when off.
    pub fn acting(&self) -> Option<OutputSharpen> {
        (!self.is_off()).then(|| self.held())
    }

    /// The settings held to the panel's ends whether the section is on
    /// or off: the radius and threshold a mask's share of it sharpens
    /// at while the picture's own is off.
    pub fn held(&self) -> OutputSharpen {
        OutputSharpen {
            enabled: true,
            amount: self.amount.clamp(0.0, AMOUNT_MAX),
            radius: self.radius.clamp(RADIUS_MIN, RADIUS_MAX),
            threshold: self.threshold.clamp(0.0, THRESHOLD_MAX),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_at_no_amount_or_switched_off_and_held_to_the_ends() {
        assert!(OutputSharpen::default().is_off());
        assert!(OutputSharpen::default().acting().is_none());
        let on = OutputSharpen {
            amount: 5.0,
            radius: 9.0,
            threshold: -1.0,
            ..Default::default()
        };
        let acting = on.acting().unwrap();
        assert_eq!(
            (acting.amount, acting.radius, acting.threshold),
            (AMOUNT_MAX, RADIUS_MAX, 0.0)
        );
        assert!(
            OutputSharpen {
                enabled: false,
                ..on
            }
            .acting()
            .is_none()
        );
    }
}
