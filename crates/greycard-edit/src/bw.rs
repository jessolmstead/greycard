//! Black and white: the conversion to a neutral picture, with the
//! grey of each hue band weighted, as a photographer's contrast
//! filter over the lens weights it.
//!
//! The bands and the chroma weighting are the color mixer's
//! ([`crate::mixer`]), so a band reaches exactly as far here as it
//! does there and a near-neutral pixel is left alone. A weight is a
//! gain on that band's grey, in stops once the section's `strength`
//! has scaled it; the chroma goes to nothing, so
//! nothing that *scales* chroma downstream of this can bring the
//! color back. The point curves and the grading wheels still can,
//! by adding, which is what toning a black and white is.

use serde::{Deserialize, Serialize};

use crate::mixer::{BANDS, Mixer};

/// Stops of gain on a band's grey at a weight of one and a strength
/// of one. The color mixer's luminance slider is the same stop per
/// unit, so at a strength of one the two read alike; past it this
/// section reaches further, which is what [`STRENGTH_MAX`] is for.
pub const RANGE: f32 = 1.0;

/// The most [`BlackWhite::strength`] reaches. One is the tables as
/// they are written, where the Red filter's deepest cut is a single
/// stop; three puts that cut where a Wratten 25 red puts a blue sky
/// against a panchromatic rendering.
pub const STRENGTH_MAX: f32 = 3.0;

/// A missing `strength` is a full one: what every sidecar and preset
/// written before the field meant.
fn full_strength() -> f32 {
    1.0
}

/// The classic contrast filters, as a set of band weights each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    None,
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
}

impl Filter {
    pub const ALL: [Filter; 6] = [
        Filter::None,
        Filter::Red,
        Filter::Orange,
        Filter::Yellow,
        Filter::Green,
        Filter::Blue,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Filter::None => "None",
            Filter::Red => "Red",
            Filter::Orange => "Orange",
            Filter::Yellow => "Yellow",
            Filter::Green => "Green",
            Filter::Blue => "Blue",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&f| f == self).expect("a filter")
    }

    /// The band weights, in the mixer's band order: red, orange,
    /// yellow, green, aqua, blue, purple, magenta. A filter passes
    /// its own color and holds back the rest, so a red filter
    /// lightens skin and darkens a blue sky, a green one lightens
    /// foliage, and so on.
    pub fn weights(self) -> [f32; BANDS] {
        match self {
            Filter::None => [0.0; BANDS],
            Filter::Red => [0.8, 0.6, 0.3, -0.4, -0.8, -1.0, -0.6, 0.2],
            Filter::Orange => [0.6, 0.7, 0.4, -0.2, -0.6, -0.8, -0.4, 0.1],
            Filter::Yellow => [0.3, 0.4, 0.5, 0.1, -0.3, -0.5, -0.3, 0.0],
            Filter::Green => [-0.5, -0.2, 0.3, 0.7, 0.3, -0.3, -0.4, -0.5],
            Filter::Blue => [-0.7, -0.5, -0.3, 0.1, 0.6, 0.8, 0.5, 0.0],
        }
    }
}

/// The black and white conversion: off by default, so a picture is
/// in color until it is asked not to be.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlackWhite {
    /// The section's switch.
    pub enabled: bool,
    /// A gain on each hue band's grey, -1 to 1, in the mixer's band
    /// order.
    pub weights: [f32; BANDS],
    /// How much of those weights acts, zero to [`STRENGTH_MAX`]: one
    /// scalar on the summed gain, so a filter keeps its shape and
    /// only its depth changes, and [`Self::filter`] still names it.
    #[serde(default = "full_strength")]
    pub strength: f32,
}

impl Default for BlackWhite {
    fn default() -> Self {
        Self::OFF
    }
}

impl BlackWhite {
    /// The section off, its weights flat: the picture in color.
    pub const OFF: Self = Self {
        enabled: false,
        weights: [0.0; BANDS],
        strength: 1.0,
    };

    pub fn is_off(&self) -> bool {
        !self.enabled
    }

    /// What acts: this conversion, or [`Self::OFF`] with the switch
    /// off, as `Light::effective` and `Mixer::effective` do it.
    pub fn effective(&self) -> BlackWhite {
        if self.enabled { *self } else { Self::OFF }
    }

    /// The filter whose weights these are, if one's exactly.
    pub fn filter(&self) -> Option<Filter> {
        Filter::ALL
            .into_iter()
            .find(|f| f.weights() == self.weights)
    }

    pub fn with_filter(filter: Filter, enabled: bool) -> Self {
        Self {
            enabled,
            weights: filter.weights(),
            strength: 1.0,
        }
    }

    /// The gain on this hue's grey, in stops: the two bands the hue
    /// lies between share it as the mixer's bands share a slider,
    /// faded by `confidence` so a near-neutral pixel keeps the grey
    /// it had (`mixer::confidence`), the sum scaled by `strength`.
    pub fn stops(&self, hue_degrees: f32, confidence: f32) -> f32 {
        let mut w = 0.0;
        for (band, share) in Mixer::weights(hue_degrees) {
            w += share * self.weights[band];
        }
        confidence.clamp(0.0, 1.0) * w * RANGE * self.strength
    }

    /// The scale on Oklab's lightness for that gain: the band's gain
    /// in linear light through the cube root, as the mixer's
    /// luminance slider does it.
    pub fn light(&self, hue_degrees: f32, confidence: f32) -> f32 {
        2f32.powf(self.stops(hue_degrees, confidence) / 3.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::Band;

    #[test]
    fn the_default_is_off_and_flat() {
        let bw = BlackWhite::default();
        assert!(bw.is_off());
        assert_eq!(
            BlackWhite::with_filter(Filter::Red, false).effective(),
            BlackWhite::OFF
        );
        assert_eq!(bw.weights, [0.0; BANDS]);
        assert_eq!(bw.filter(), Some(Filter::None));
        assert_eq!(bw.light(30.0, 1.0), 1.0);
    }

    #[test]
    fn a_red_filter_lifts_red_and_skin_and_drops_the_sky() {
        let bw = BlackWhite::with_filter(Filter::Red, true);
        let red = bw.stops(Band::Red.center(), 1.0);
        let skin = bw.stops(Band::Orange.center(), 1.0);
        let sky = bw.stops(Band::Blue.center(), 1.0);
        assert!(red > 0.5 && skin > 0.5, "{red} {skin}");
        assert!(sky < -0.5, "{sky}");
        // And the other way round under a blue one.
        let blue = BlackWhite::with_filter(Filter::Blue, true);
        assert!(blue.stops(Band::Red.center(), 1.0) < 0.0);
        assert!(blue.stops(Band::Blue.center(), 1.0) > 0.0);
    }

    #[test]
    fn a_weight_reaches_its_neighbors_and_no_further() {
        let mut bw = BlackWhite {
            enabled: true,
            ..BlackWhite::OFF
        };
        bw.weights[Band::Green.index()] = 1.0;
        assert!((bw.stops(Band::Green.center(), 1.0) - RANGE).abs() < 1e-6);
        let halfway = (Band::Green.center() + Band::Aqua.center()) / 2.0;
        assert!((bw.stops(halfway, 1.0) - RANGE / 2.0).abs() < 1e-6);
        assert_eq!(bw.stops(Band::Blue.center(), 1.0), 0.0);
        // Confidence fades the whole thing out toward grey.
        assert_eq!(bw.stops(Band::Green.center(), 0.0), 0.0);
        assert!((bw.light(Band::Green.center(), 0.5) - 2f32.powf(0.5 / 3.0)).abs() < 1e-6);
    }

    #[test]
    fn every_filter_round_trips_by_name_and_is_recognized() {
        for f in Filter::ALL {
            assert_eq!(Filter::from_name(f.name()), Some(f));
            assert_eq!(BlackWhite::with_filter(f, true).filter(), Some(f));
            assert_eq!(Filter::ALL[f.index()], f);
        }
        let custom = BlackWhite {
            enabled: true,
            weights: [0.11; BANDS],
            strength: 1.0,
        };
        assert_eq!(custom.filter(), None);
    }

    #[test]
    fn the_strength_scales_the_gain_and_leaves_the_filter_named() {
        let one = BlackWhite::with_filter(Filter::Red, true);
        let two = BlackWhite {
            strength: 2.0,
            ..one
        };
        let none = BlackWhite {
            strength: 0.0,
            ..one
        };
        for h in [0.0, 45.0, 120.0, 200.0, 264.0, 330.0] {
            let want = 2.0 * one.stops(h, 1.0);
            assert!((two.stops(h, 1.0) - want).abs() < 1e-6, "{h}");
            assert_eq!(none.stops(h, 1.0), 0.0);
        }
        // The Red filter's deepest cut, one stop as the tables are
        // written, is two at a strength of two and three at three.
        assert!((two.stops(Band::Blue.center(), 1.0) + 2.0).abs() < 1e-6);
        let deepest = BlackWhite {
            strength: STRENGTH_MAX,
            ..one
        };
        assert!((deepest.stops(Band::Blue.center(), 1.0) + 3.0).abs() < 1e-6);
        // The shape is the filter's still, whatever the strength, so
        // the panel names it and a hand-moved band still reads custom.
        assert_eq!(two.filter(), Some(Filter::Red));
        assert_eq!(none.filter(), Some(Filter::Red));
        assert_eq!(BlackWhite::OFF.strength, 1.0);
        assert_eq!(BlackWhite::default().strength, 1.0);
    }

    #[test]
    fn a_sidecar_from_before_the_strength_reads_at_one() {
        // What §16's rule asks of a new field: the old JSON still
        // means what it meant, so no `VERSION` moved for this.
        let old = r#"{"enabled":true,"weights":[0.8,0.6,0.3,-0.4,-0.8,-1.0,-0.6,0.2]}"#;
        let bw: BlackWhite = serde_json::from_str(old).expect("an old sidecar's section");
        assert_eq!(bw.strength, 1.0);
        assert_eq!(bw, BlackWhite::with_filter(Filter::Red, true));
        // And the round trip, strength and all.
        let mine = BlackWhite {
            strength: 2.35,
            ..BlackWhite::with_filter(Filter::Green, true)
        };
        let json = serde_json::to_string(&mine).expect("serializes");
        assert!(json.contains("strength"), "{json}");
        assert_eq!(
            serde_json::from_str::<BlackWhite>(&json).expect("round trips"),
            mine
        );
    }
}
