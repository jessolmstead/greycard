//! Lens corrections: which of a profile's corrections to apply, a
//! distortion and a chromatic aberration set by hand beside or instead
//! of the profile's, how much of the corrected picture to show, and the
//! defringe that follows them. The profile itself is not
//! the edit's: it is looked up from the file each time, so an edit
//! moves between machines and database versions.

use greycard_core::develop::defringe::{self, DefringeOptions, HueWindow};
use greycard_core::develop::lens::{ChromaticAberration, Distortion, LensCorrection, Scale};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lens {
    /// The section's switch: off, nothing is corrected.
    pub enabled: bool,
    /// The profile's corrections, where a profile is found.
    pub profile: bool,
    pub distortion: bool,
    pub chromatic_aberration: bool,
    pub vignetting: bool,
    /// A distortion set by hand, on top of the profile's: `poly3`'s
    /// `k1`, negative for a picture that bows out, zero for none.
    pub manual: f32,
    /// A lateral chromatic aberration set by hand, on top of the
    /// profile's: how much the red's and the blue's radius is scaled
    /// beyond the green's, as a fraction; zero for none. Positive
    /// pulls a channel that landed too far out back in.
    pub ca_red: f32,
    pub ca_blue: f32,
    /// Show the smallest magnification that leaves no edge empty.
    pub auto_scale: bool,
    /// The magnification otherwise, one for the source's own.
    pub scale: f32,
    /// Neutralize axial chromatic aberration and purple fringing after
    /// the geometry: a different lens fault from the lateral one above,
    /// and off unless asked for.
    pub defringe: bool,
    /// How far the local mean chroma is taken over, pixels.
    pub defringe_radius: f32,
    /// How far past the frame's mean chroma deviation a pixel must sit
    /// to count as fringing, RawTherapee's scale.
    pub defringe_threshold: f32,
    /// The purple hue window: where it sits on the Oklab hue circle in
    /// degrees, how much of the circle it spans, and how much of the
    /// pass a deviation inside it gets. A deviation outside both
    /// windows is not a fringe.
    pub defringe_purple_center: f32,
    pub defringe_purple_width: f32,
    pub defringe_purple_amount: f32,
    /// And the green one.
    pub defringe_green_center: f32,
    pub defringe_green_width: f32,
    pub defringe_green_amount: f32,
}

impl Default for Lens {
    fn default() -> Self {
        Self {
            enabled: true,
            profile: true,
            distortion: true,
            chromatic_aberration: true,
            vignetting: true,
            manual: 0.0,
            ca_red: 0.0,
            ca_blue: 0.0,
            auto_scale: true,
            scale: 1.0,
            defringe: false,
            defringe_radius: defringe::DEFAULT_RADIUS,
            defringe_threshold: defringe::DEFAULT_THRESHOLD,
            defringe_purple_center: defringe::DEFAULT_PURPLE_CENTER,
            defringe_purple_width: defringe::DEFAULT_WINDOW_WIDTH,
            defringe_purple_amount: 1.0,
            defringe_green_center: defringe::DEFAULT_GREEN_CENTER,
            defringe_green_width: defringe::DEFAULT_WINDOW_WIDTH,
            defringe_green_amount: 1.0,
        }
    }
}

/// The largest manual distortion either way.
pub const MAX_MANUAL: f32 = 0.5;
/// The largest manual chromatic aberration either way: a percent of
/// the radius, thirty pixels at the long edge of a 24 MP frame,
/// several times what any lens the database knows shows.
pub const MAX_MANUAL_CA: f32 = 0.01;

impl Lens {
    /// Whether the edit wants anything of a profile.
    pub fn wants_profile(&self) -> bool {
        self.enabled
            && self.profile
            && (self.distortion || self.chromatic_aberration || self.vignetting)
    }

    /// Whether the edit moves or shades a pixel at all, given whether
    /// a profile is there to draw on. The defringe is not part of this
    /// answer: it is a second pass over the corrected picture, and a
    /// consumer asks [`Lens::defringe`] for it.
    pub fn is_identity(&self, has_profile: bool) -> bool {
        !self.enabled
            || (self.manual == 0.0
                && self.manual_ca().is_none()
                && !(has_profile && self.wants_profile()))
    }

    /// The defringe the edit asks for, or none: the section's switch
    /// carries it as it does everything else here.
    ///
    /// A sidecar written before the hue windows existed reads with
    /// today's windows, not with the all-hues pass it was saved
    /// against. §74 called the hue curve the thing the port left out
    /// and the all-hues pass its cost, so the windows are the fix
    /// rather than a new option; a file reopened is a shade less
    /// defringed than it was, and nowhere more.
    pub fn defringe(&self) -> Option<DefringeOptions> {
        (self.enabled && self.defringe).then_some(DefringeOptions {
            radius: self.defringe_radius,
            threshold: self.defringe_threshold,
            purple: HueWindow {
                center: self.defringe_purple_center,
                width: self.defringe_purple_width,
                amount: self.defringe_purple_amount,
            },
            green: HueWindow {
                center: self.defringe_green_center,
                width: self.defringe_green_width,
                amount: self.defringe_green_amount,
            },
        })
    }

    /// The aberration set by hand, as a model, or none when both are
    /// zero. The scale of a channel's radius is one plus the fraction
    /// in the edit.
    pub fn manual_ca(&self) -> Option<ChromaticAberration> {
        (self.ca_red != 0.0 || self.ca_blue != 0.0)
            .then(|| ChromaticAberration::IDENTITY.scaled(1.0 + self.ca_red, 1.0 + self.ca_blue))
    }

    /// The engine's correction: the profile's, when one is given, with
    /// the manual distortion put in where the profile has none, and
    /// the manual aberration folded into the profile's (a scale after
    /// a polynomial is the polynomial scaled). A manual distortion
    /// over a profile's is applied after it, as a second correction;
    /// the consumer runs both.
    pub fn corrections(&self, profile: Option<LensCorrection>) -> Vec<LensCorrection> {
        if !self.enabled {
            return Vec::new();
        }
        let scale = if self.auto_scale {
            Scale::Auto
        } else {
            Scale::Fixed(self.scale)
        };
        let manual_ca = self.manual_ca();
        let mut out = Vec::new();
        if let Some(mut p) = profile.filter(|_| self.wants_profile()) {
            if !self.distortion {
                p.distortion = None;
            }
            if !self.chromatic_aberration {
                p.chromatic_aberration = None;
            }
            if !self.vignetting {
                p.vignetting = None;
            }
            p.scale = scale;
            if self.manual != 0.0 && p.distortion.is_none() {
                p.distortion = Some(Distortion::Poly3 { k1: self.manual });
            }
            if let Some(m) = manual_ca {
                p.chromatic_aberration = Some(match p.chromatic_aberration {
                    Some(ca) => ca.scaled(1.0 + self.ca_red, 1.0 + self.ca_blue),
                    None => m,
                });
            }
            if !p.is_identity() {
                out.push(p);
            }
        }
        if self.manual != 0.0
            && !out
                .iter()
                .any(|c| c.distortion == Some(Distortion::Poly3 { k1: self.manual }))
        {
            out.push(LensCorrection {
                distortion: Some(Distortion::Poly3 { k1: self.manual }),
                // The aberration rides with the profile's correction
                // when there is one, else here.
                chromatic_aberration: manual_ca.filter(|_| out.is_empty()),
                scale,
                ..Default::default()
            });
        } else if out.is_empty()
            && let Some(m) = manual_ca
        {
            out.push(LensCorrection {
                chromatic_aberration: Some(m),
                scale,
                ..Default::default()
            });
        }
        // Only the last correction chooses the scale; the ones before
        // it show everything, since the last one's scale is the edit's.
        let n = out.len();
        for c in out.iter_mut().take(n.saturating_sub(1)) {
            c.scale = Scale::Fixed(1.0);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::develop::lens::Vignetting;

    #[test]
    fn the_edit_chooses_among_the_profiles_corrections() {
        let lens = Lens::default();
        assert!(lens.wants_profile() && lens.is_identity(false) && !lens.is_identity(true));
        assert!(lens.corrections(None).is_empty());
        let profile = LensCorrection {
            distortion: Some(Distortion::Poly3 { k1: 0.02 }),
            vignetting: Some(Vignetting {
                k: [-0.5, 0.0, 0.0],
            }),
            ..Default::default()
        };
        let c = lens.corrections(Some(profile));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].distortion, profile.distortion);
        assert_eq!(c[0].scale, Scale::Auto);
        // Vignetting off: the profile without it.
        let no_vig = Lens {
            vignetting: false,
            ..lens
        };
        assert!(no_vig.corrections(Some(profile))[0].vignetting.is_none());
        // Everything off but a manual distortion: one correction, the
        // manual one, at the fixed scale asked.
        let manual = Lens {
            profile: false,
            manual: -0.1,
            auto_scale: false,
            scale: 1.1,
            ..lens
        };
        let c = manual.corrections(Some(profile));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].distortion, Some(Distortion::Poly3 { k1: -0.1 }));
        assert_eq!(c[0].scale, Scale::Fixed(1.1));
        assert!(c[0].vignetting.is_none());
        // Manual over a profile with distortion: two corrections,
        // the profile's shown whole, the manual one scaled.
        let both = Lens {
            manual: -0.1,
            ..lens
        };
        let c = both.corrections(Some(profile));
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].scale, Scale::Fixed(1.0));
        assert_eq!(c[1].scale, Scale::Auto);
        // Manual with a profile that has no distortion: folded in.
        let vig_only = LensCorrection {
            distortion: None,
            ..profile
        };
        let c = both.corrections(Some(vig_only));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].distortion, Some(Distortion::Poly3 { k1: -0.1 }));
        assert!(c[0].vignetting.is_some());
    }

    #[test]
    fn the_defringe_is_off_until_asked_and_follows_the_sections_switch() {
        let lens = Lens::default();
        assert!(lens.defringe().is_none());
        let on = Lens {
            defringe: true,
            defringe_radius: 3.0,
            defringe_threshold: 20.0,
            ..lens
        };
        let o = on.defringe().unwrap();
        assert_eq!(o.radius, 3.0);
        assert_eq!(o.threshold, 20.0);
        // The windows come across whole, and by default they are the
        // two the engine measured.
        assert_eq!(o.purple, DefringeOptions::default().purple);
        assert_eq!(o.green, DefringeOptions::default().green);
        let narrowed = Lens {
            defringe_green_amount: 0.0,
            defringe_purple_center: 290.0,
            defringe_purple_width: 40.0,
            defringe_purple_amount: 0.5,
            ..on
        };
        let o = narrowed.defringe().unwrap();
        assert_eq!(
            o.purple,
            HueWindow {
                center: 290.0,
                width: 40.0,
                amount: 0.5
            }
        );
        assert_eq!(o.green.amount, 0.0);
        assert!(o.acts());
        // Both amounts at zero and the pass has nothing to do; the
        // edit still asks for it, and the engine answers with an
        // untouched frame.
        let shut = Lens {
            defringe_purple_amount: 0.0,
            ..narrowed
        };
        assert!(!shut.defringe().unwrap().acts());
        // The section off takes it with everything else.
        assert!(
            Lens {
                enabled: false,
                ..on
            }
            .defringe()
            .is_none()
        );
        // And it says nothing about whether the geometry moves.
        assert!(on.is_identity(false));
    }

    /// A sidecar written before the defringe existed reads with it off
    /// and the defaults in place: defaulted fields, no version bump
    /// (§48 bumped because a key changed meaning).
    #[test]
    fn a_sidecar_without_the_defringe_reads_with_it_off() {
        let old = r#"{"enabled":true,"profile":true,"distortion":true,
            "chromatic_aberration":true,"vignetting":true,"manual":0.0,
            "ca_red":0.0,"ca_blue":0.0,"auto_scale":true,"scale":1.0}"#;
        let lens: Lens = serde_json::from_str(old).unwrap();
        assert_eq!(lens, Lens::default());
        assert!(lens.defringe().is_none());
    }

    /// A sidecar written with the defringe on but before the hue
    /// windows reads with today's windows, not with the all-hues pass
    /// it was saved against: §74 called the missing hue curve the
    /// fault, so an old file gets the fix. Its radius and threshold
    /// are its own, untouched.
    #[test]
    fn a_sidecar_from_before_the_windows_gets_the_windows() {
        let old = r#"{"enabled":true,"profile":true,"distortion":true,
            "chromatic_aberration":true,"vignetting":true,"manual":0.0,
            "ca_red":0.0,"ca_blue":0.0,"auto_scale":true,"scale":1.0,
            "defringe":true,"defringe_radius":3.5,"defringe_threshold":8.0}"#;
        let lens: Lens = serde_json::from_str(old).unwrap();
        let o = lens.defringe().expect("the defringe it asked for");
        assert_eq!(o.radius, 3.5);
        assert_eq!(o.threshold, 8.0);
        assert_eq!(o.purple, DefringeOptions::default().purple);
        assert_eq!(o.green, DefringeOptions::default().green);
        // And a round trip carries them, so the next write says so.
        let text = serde_json::to_string(&lens).unwrap();
        assert_eq!(serde_json::from_str::<Lens>(&text).unwrap(), lens);
        assert!(text.contains("defringe_purple_center"));
    }

    #[test]
    fn a_manual_aberration_folds_into_the_profiles_or_stands_alone() {
        let by_hand = Lens {
            ca_red: 0.002,
            ca_blue: -0.001,
            ..Lens::default()
        };
        assert!(!by_hand.is_identity(false));
        let m = by_hand.manual_ca().unwrap();
        assert_eq!(m.red, [1.002, 0.0, 0.0]);
        assert_eq!(m.blue, [0.999, 0.0, 0.0]);
        // No profile: one correction, the aberration alone.
        let c = by_hand.corrections(None);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].chromatic_aberration, Some(m));
        assert!(c[0].distortion.is_none());
        // A profile with an aberration: folded in, one resample.
        let profile = LensCorrection {
            chromatic_aberration: Some(ChromaticAberration {
                red: [1.0005, 0.0, 0.0001],
                blue: [0.9995, 0.0, 0.0],
            }),
            ..Default::default()
        };
        let c = by_hand.corrections(Some(profile));
        assert_eq!(c.len(), 1);
        let ca = c[0].chromatic_aberration.unwrap();
        assert!((ca.red[0] - 1.0005 * 1.002).abs() < 1e-6);
        assert!((ca.red[2] - 0.0001 * 1.002).abs() < 1e-6);
        assert!((ca.blue[0] - 0.9995 * 0.999).abs() < 1e-6);
        // The profile's aberration switched off: the manual one alone.
        let off = Lens {
            chromatic_aberration: false,
            ..by_hand
        };
        assert_eq!(
            off.corrections(Some(profile))[0].chromatic_aberration,
            Some(m)
        );
        // With a manual distortion over a profile's distortion, the
        // aberration goes with the profile's correction, not the
        // second one.
        let with_distortion = LensCorrection {
            distortion: Some(Distortion::Poly3 { k1: 0.02 }),
            ..profile
        };
        let c = Lens {
            manual: -0.1,
            ..by_hand
        }
        .corrections(Some(with_distortion));
        assert_eq!(c.len(), 2);
        assert!(c[0].chromatic_aberration.is_some());
        assert!(c[1].chromatic_aberration.is_none());
    }
}
