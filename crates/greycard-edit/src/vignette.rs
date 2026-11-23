//! A vignette over the frame as shown: an exposure falloff, in stops,
//! from a shape about the center to the corners, so darkening keeps
//! its hues as the exposure slider does. Positions are fractions of
//! the frame, so it sits on the crop and any size of export.
//!
//! Or, by its [`Place`], on the picture as shown: after the display
//! curve and the look table, where a bright edge darkens too.

use serde::{Deserialize, Serialize};

use crate::mask::smoothstep;

/// Where the vignette acts. In the scene, as an exposure falloff
/// before the display curve, under which a bright edge stays bright;
/// or on the picture as shown, after the display curve and the look
/// table, as most editors' post-crop vignette does
/// ([`Vignette::shown`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    #[default]
    Scene,
    Display,
    /// A place a later build writes: the scene here, so the rest of the
    /// edit loads with it. Like the scene it is not written, so the
    /// next write leaves it out and the vignette is in the scene from
    /// then on.
    #[serde(other)]
    Unknown,
}

impl Place {
    pub const ALL: [Place; 2] = [Place::Scene, Place::Display];

    /// In the scene: today's, or one this build does not know.
    pub fn is_scene(&self) -> bool {
        matches!(self, Place::Scene | Place::Unknown)
    }

    /// The panel's option, not its words; an unknown place shows as the
    /// scene it renders as.
    pub fn name(self) -> &'static str {
        match self {
            Place::Scene | Place::Unknown => "Scene",
            Place::Display => "Display",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

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
    /// Where it acts. Left out of a sidecar in the scene, so every
    /// sidecar and key written before it is unchanged; an older build
    /// reading one set to the display puts the vignette in the scene.
    #[serde(skip_serializing_if = "Place::is_scene")]
    pub place: Place,
}

impl Default for Vignette {
    fn default() -> Self {
        Self {
            enabled: true,
            amount: 0.0,
            midpoint: 0.5,
            feather: 0.5,
            roundness: 0.0,
            place: Place::Scene,
        }
    }
}

impl Vignette {
    pub fn is_off(&self) -> bool {
        !self.enabled || self.amount == 0.0
    }

    /// Whether it acts on the picture as shown rather than in the
    /// scene's exposure.
    pub fn on_display(&self) -> bool {
        self.place == Place::Display
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

    /// The vignette on a display-referred color `c`, linear, at `stops`
    /// of it (the amount times [`Vignette::at`]): darkening scales the
    /// light by `2^stops`, toward black, so hue and chroma hold as on a
    /// multiply. Lightening is the same stops in the shadows, rolled
    /// off toward display white, `v / (k + v (1 - k))` with `k` the
    /// darkening's scale: black stays black, white stays white, and
    /// nothing between clips, where a multiply up would clip the sky
    /// and a paint toward white would wash the shadows grey. Under black
    /// (a color outside the space, which the output's rail or clip
    /// takes) the shadows' slope goes on. Nothing at zero. The shader's
    /// `vignette_shown` is this.
    #[inline]
    pub fn shown(c: [f32; 3], stops: f32) -> [f32; 3] {
        let k = (-stops.abs()).exp2();
        if stops < 0.0 {
            c.map(|v| v * k)
        } else if stops > 0.0 {
            c.map(|v| {
                if v > 0.0 {
                    v / (k + v * (1.0 - k))
                } else {
                    v / k
                }
            })
        } else {
            c
        }
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
            place: Place::Scene,
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

    /// On the display a stop down halves the light; a stop up doubles
    /// the deep shadows and rolls off toward white, never past it; and
    /// nothing is nothing.
    #[test]
    fn shown_darkens_toward_black_and_lightens_toward_white() {
        let c = [0.8, 0.4, 0.1];
        assert_eq!(Vignette::shown(c, 0.0), c);
        let down = Vignette::shown(c, -1.0);
        for k in 0..3 {
            assert!((down[k] - c[k] * 0.5).abs() < 1e-6);
        }
        // The ratios hold: the hue and chroma of a multiply.
        assert!((down[0] / down[2] - c[0] / c[2]).abs() < 1e-4);
        let up = Vignette::shown(c, 1.0);
        for k in 0..3 {
            assert!(up[k] > c[k] && up[k] < 1.0, "{up:?}");
        }
        // A stop in the deep shadows, less toward white.
        let deep = Vignette::shown([0.001; 3], 1.0)[0];
        assert!((deep / 0.001 - 2.0).abs() < 0.01, "{deep}");
        assert!(up[0] / c[0] < 1.2, "{up:?}");
        // Under black the shadows' slope goes on.
        assert_eq!(Vignette::shown([-0.01; 3], 1.0), [-0.02; 3]);
        // White stays white going up, black black either way.
        assert_eq!(Vignette::shown([1.0; 3], 3.0), [1.0; 3]);
        assert_eq!(Vignette::shown([0.0; 3], 3.0), [0.0; 3]);
        assert_eq!(Vignette::shown([0.0; 3], -3.0), [0.0; 3]);
    }

    /// The place came in with no new version: in the scene it is not
    /// written, so a vignette serializes as it did before it and an
    /// edit from before it reads in the scene; on the display it is
    /// written and read back.
    #[test]
    fn the_scene_place_is_not_written_and_the_display_is() {
        let v = Vignette {
            amount: -1.0,
            ..Vignette::default()
        };
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"enabled":true,"amount":-1.0,"midpoint":0.5,"feather":0.5,"roundness":0.0}"#
        );
        let old: Vignette = serde_json::from_str(r#"{"amount":-1.0}"#).unwrap();
        assert_eq!(old.place, Place::Scene);
        let shown = Vignette {
            place: Place::Display,
            ..v
        };
        let json = serde_json::to_string(&shown).unwrap();
        assert!(json.contains(r#""place":"display""#), "{json}");
        assert_eq!(serde_json::from_str::<Vignette>(&json).unwrap(), shown);
        for p in Place::ALL {
            assert_eq!(Place::from_name(p.name()), Some(p));
        }
    }

    /// A place a later build writes loads, in an edit and in a sidecar,
    /// as the scene; written again it is left out.
    #[test]
    fn an_unknown_place_reads_as_the_scene() {
        let edit = r#"{"version": 9, "vignette": {"amount": -1.0, "place": "lens"}}"#;
        let e = crate::Edit::from_json(edit).unwrap();
        assert_eq!(e.vignette.place, Place::Unknown);
        assert_eq!(e.vignette.amount, -1.0);
        assert!(e.vignette.place.is_scene() && !e.vignette.on_display());
        assert_eq!(e.vignette.place.name(), "Scene");
        let json = e.to_json();
        assert!(!json.contains("place"), "{json}");
        let mut scene = e.clone();
        scene.vignette.place = Place::Scene;
        assert_eq!(
            e.develop_key_with(&[], None),
            scene.develop_key_with(&[], None)
        );
        let sidecar = format!(r#"{{"current": {edit}}}"#);
        let s = crate::Sidecar::from_json(&sidecar, &crate::Edit::default()).unwrap();
        assert_eq!(s.current.vignette.place, Place::Unknown);
        assert_eq!(s.current.vignette.amount, -1.0);
    }

    /// The key takes the place: a vignette on the display is another
    /// develop, one in the scene the key it always had.
    #[test]
    fn the_develop_key_takes_the_place() {
        let mut e = crate::Edit::default();
        e.vignette.amount = -1.0;
        let scene = e.develop_key_with(&[], None);
        let json = e.to_json();
        assert!(!json.contains("place"), "{json}");
        e.vignette.place = Place::Display;
        assert_ne!(e.develop_key_with(&[], None), scene);
        let back = crate::Edit::from_json(&e.to_json()).unwrap();
        assert_eq!(back.vignette.place, Place::Display);
        assert_eq!(
            back.develop_key_with(&[], None),
            e.develop_key_with(&[], None)
        );
    }
}
