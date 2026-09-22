//! The color mixer: hue, saturation and luminance by hue band, eight
//! bands from red round to magenta, in Oklab on the scene-linear
//! picture after exposure.

use serde::{Deserialize, Serialize};

pub const BANDS: usize = 8;

/// The bands, in hue order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Red,
    Orange,
    Yellow,
    Green,
    Aqua,
    Blue,
    Purple,
    Magenta,
}

impl Band {
    pub const ALL: [Band; BANDS] = [
        Band::Red,
        Band::Orange,
        Band::Yellow,
        Band::Green,
        Band::Aqua,
        Band::Blue,
        Band::Purple,
        Band::Magenta,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Band::Red => "Red",
            Band::Orange => "Orange",
            Band::Yellow => "Yellow",
            Band::Green => "Green",
            Band::Aqua => "Aqua",
            Band::Blue => "Blue",
            Band::Purple => "Purple",
            Band::Magenta => "Magenta",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.name() == name)
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&b| b == self).expect("a band")
    }

    /// The band's center in Oklab hue, degrees. From the Oklab hues of
    /// the sRGB primaries and secondaries (red 29, yellow 110, green
    /// 142, aqua 195, blue 264, magenta 328), spread a little so each
    /// band has room, with orange set where skin sits.
    pub fn center(self) -> f32 {
        CENTERS[self.index()]
    }
}

/// Band centers in Oklab hue, degrees, rising.
pub const CENTERS: [f32; BANDS] = [25.0, 60.0, 100.0, 140.0, 195.0, 265.0, 300.0, 335.0];

/// The Oklab chroma at which the mixer trusts a color's hue fully.
/// Below it a band's shift, scale and gain fade out, to nothing at
/// grey: a near-neutral pixel's hue is mostly noise, and a band's
/// gain printed on it is speckle. A dark rock's noise reads at 0.017
/// a pixel and 0.004 or so over a 5x5 mean; pale water sits at 0.028,
/// skin at 0.08 and up.
pub const CHROMA_FULL: f32 = 0.03;

/// The radius, in source pixels, of the mean of a and b the mixer
/// reads its hue and its confidence from: a 5x5 box.
pub const MEAN_RADIUS: usize = 2;

/// How far the mixer trusts a color of this Oklab chroma, 0 at grey
/// to 1 from `CHROMA_FULL` up, a smoothstep between.
pub fn confidence(chroma: f32) -> f32 {
    let t = (chroma / CHROMA_FULL).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mixer {
    pub enabled: bool,
    /// Hue shift per band, degrees.
    pub hue: [f32; BANDS],
    /// Chroma change per band, -1 (grey) to 1 (double).
    pub saturation: [f32; BANDS],
    /// Brightness change per band, in stops: a gain on the band's
    /// light, hue and saturation held.
    pub luminance: [f32; BANDS],
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            enabled: true,
            hue: [0.0; BANDS],
            saturation: [0.0; BANDS],
            luminance: [0.0; BANDS],
        }
    }
}

impl Mixer {
    /// What acts: this mixer, or with the switch off, every band at
    /// nothing. Consumers apply this, as they do `Light::effective`,
    /// so a switched-off mixer with its sliders still set cannot
    /// reach the picture or a dropper's reading of it.
    pub fn effective(&self) -> Mixer {
        if self.enabled {
            *self
        } else {
            Mixer {
                enabled: false,
                hue: [0.0; BANDS],
                saturation: [0.0; BANDS],
                luminance: [0.0; BANDS],
            }
        }
    }

    pub fn is_identity(&self) -> bool {
        !self.enabled
            || (self.hue.iter().all(|&v| v == 0.0)
                && self.saturation.iter().all(|&v| v == 0.0)
                && self.luminance.iter().all(|&v| v == 0.0))
    }

    /// The two bands a hue falls between and their shares, which sum
    /// to one: the band below and the band above, circularly.
    pub fn weights(hue_degrees: f32) -> [(usize, f32); 2] {
        let h = hue_degrees.rem_euclid(360.0);
        let above = CENTERS.iter().position(|&c| c > h).unwrap_or(0);
        let below = (above + BANDS - 1) % BANDS;
        let (lo, hi) = (CENTERS[below], CENTERS[above]);
        // The span across the wrap is measured the long way round.
        let span = (hi - lo).rem_euclid(360.0);
        let t = if span == 0.0 {
            0.0
        } else {
            ((h - lo).rem_euclid(360.0) / span).clamp(0.0, 1.0)
        };
        [(below, 1.0 - t), (above, t)]
    }

    /// The hue shift, the chroma scale and the scale on Oklab's L, a and
    /// b together at a hue. The last is the band's gain in linear light
    /// through the cube root, so it moves brightness alone.
    pub fn at(&self, hue_degrees: f32) -> (f32, f32, f32) {
        self.at_with(hue_degrees, 1.0)
    }

    /// `at`, with the bands' values scaled by `confidence` (0 to 1):
    /// at 0 the identity, whatever the sliders say.
    pub fn at_with(&self, hue_degrees: f32, confidence: f32) -> (f32, f32, f32) {
        let mut shift = 0.0;
        let mut sat = 0.0;
        let mut lum = 0.0;
        for (band, w) in Self::weights(hue_degrees) {
            shift += w * self.hue[band];
            sat += w * self.saturation[band];
            lum += w * self.luminance[band];
        }
        let c = confidence.clamp(0.0, 1.0);
        (
            c * shift,
            (1.0 + c * sat).max(0.0),
            2f32.powf(c * lum / 3.0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_fades_the_bands_out_toward_grey() {
        assert_eq!(confidence(0.0), 0.0);
        assert_eq!(confidence(CHROMA_FULL), 1.0);
        assert_eq!(confidence(0.3), 1.0);
        let half = confidence(CHROMA_FULL / 2.0);
        assert!((half - 0.5).abs() < 1e-6, "{half}");
        // A 5x5 mean of a dark rock's noise, against pale water.
        assert!(confidence(0.004) < 0.06);
        assert!(confidence(0.028) > 0.98);
        let m = Mixer {
            hue: [30.0; BANDS],
            saturation: [0.5; BANDS],
            luminance: [1.0; BANDS],
            ..Default::default()
        };
        let (shift, chroma, light) = m.at_with(100.0, 0.0);
        assert_eq!((shift, chroma, light), (0.0, 1.0, 1.0));
        assert_eq!(m.at_with(100.0, 1.0), m.at(100.0));
        let (shift, chroma, light) = m.at_with(100.0, 0.5);
        assert!((shift - 15.0).abs() < 1e-5 && (chroma - 1.25).abs() < 1e-5);
        assert!((light - 2f32.powf(0.5 / 3.0)).abs() < 1e-5);
    }

    #[test]
    fn a_switched_off_mixer_acts_as_no_mixer_at_all() {
        let wild = Mixer {
            enabled: false,
            hue: [30.0; BANDS],
            saturation: [1.0; BANDS],
            luminance: [1.0; BANDS],
        };
        assert_eq!(
            wild.effective(),
            Mixer {
                enabled: false,
                ..Mixer::default()
            }
        );
        for h in (0..360).step_by(30) {
            assert_eq!(wild.effective().at(h as f32), (0.0, 1.0, 1.0));
        }
        let on = Mixer {
            enabled: true,
            ..wild
        };
        assert_eq!(on.effective(), on);
    }

    #[test]
    fn the_weights_partition_the_circle() {
        for h in (0..360).step_by(5) {
            let w = Mixer::weights(h as f32);
            assert!((w[0].1 + w[1].1 - 1.0).abs() < 1e-6, "{h}: {w:?}");
        }
        // On a center the band has it all.
        for (i, &c) in CENTERS.iter().enumerate() {
            let w = Mixer::weights(c);
            assert!(
                w.iter().any(|&(b, s)| b == i && (s - 1.0).abs() < 1e-6),
                "{c}: {w:?}"
            );
        }
        // Across the wrap, magenta at 335 to red at 25 through zero.
        let w = Mixer::weights(0.0);
        assert!(
            w.iter().any(|&(b, s)| b == 7 && (s - 0.5).abs() < 1e-6),
            "{w:?}"
        );
        assert!(
            w.iter().any(|&(b, s)| b == 0 && (s - 0.5).abs() < 1e-6),
            "{w:?}"
        );
    }

    #[test]
    fn a_band_reaches_its_neighbors_and_no_further() {
        let mut m = Mixer::default();
        assert!(m.is_identity());
        m.saturation[Band::Green.index()] = 1.0;
        assert!(!m.is_identity());
        let (_, at_green, _) = m.at(Band::Green.center());
        assert!((at_green - 2.0).abs() < 1e-6);
        let (_, halfway, _) = m.at((Band::Green.center() + Band::Aqua.center()) / 2.0);
        assert!((halfway - 1.5).abs() < 1e-6);
        let (_, at_blue, _) = m.at(Band::Blue.center());
        assert!((at_blue - 1.0).abs() < 1e-6);
        // Grey is the floor, whatever the sum of the neighbors.
        m.saturation = [-1.0; BANDS];
        assert_eq!(m.at(100.0).1, 0.0);
        for b in Band::ALL {
            assert_eq!(Band::from_name(b.name()), Some(b));
        }
    }
}
