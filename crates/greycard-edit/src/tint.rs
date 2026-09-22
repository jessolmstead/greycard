//! A tint: one hue, and how far the picture is pulled toward it.
//!
//! Lightroom's local Color, and what a mask most often wants that no
//! band-by-band control gives: warm this patch, cool that one. A hue
//! in Oklab degrees and an amount, 0 to 1, off at nothing. The amount
//! moves the pixel's hue toward the tint's and lifts its chroma to a
//! floor that follows the lightness.
//!
//! The bell in the lightness gates the *lift* and not the turn, which
//! is the whole of what "a black stays black and a white stays white"
//! means: a neutral outside the bell is given no chroma, so it comes
//! out the neutral it went in, while a pixel that already has color
//! keeps every bit of that chroma and still has its hue turned, at
//! the top of the scale as anywhere else. That is deliberate. It is
//! how a bright sky is hand-colored: the pixels worth coloring up
//! there have color in them already, and a tint that gave up above
//! scene white would leave a rebuilt or pushed highlight behind while
//! the rest of the mask moved.
//!
//! It lives on a [`crate::Look`], so a mask carries one and it blends
//! with the mask's weight like everything else in a look; the blend
//! is of the vector (`amount` in the hue's direction), which is how
//! the grading wheels' three hues already add.

use serde::{Deserialize, Deserializer, Serialize};

/// The Oklab chroma a mid grey takes at a full amount: clearly
/// colored, short of the gamut's edge (the sRGB primaries sit near
/// 0.3, `color.rs`).
pub const CHROMA: f32 = 0.1;

/// Oklab's lightness of mid grey, `0.18^(1/3)`: where the tint is
/// strongest. The floor falls to nothing at black and again at twice
/// this lightness, a stop and a half above mid grey, where the scene
/// is white.
pub const MID_LIGHTNESS: f32 = 0.5646;

/// A tint on a look: a hue to pull toward, and how far.
///
/// A sidecar's values are brought into range as they are read, once
/// and in one place, so nothing downstream has to wonder: the two
/// paths through a tint (`finish.rs` blends every look's as a vector
/// and reads a tint back off the sum; `pick` takes one straight)
/// agree only on a hue that is a hue and an amount that is an amount.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Tint {
    /// Oklab hue, degrees, 0 to 360.
    #[serde(deserialize_with = "wrapped_hue")]
    pub hue: f32,
    /// 0 (nothing) to 1 (the hue replaces the pixel's).
    #[serde(deserialize_with = "held_amount")]
    pub amount: f32,
}

/// A hue as read from a file: round the circle, and anything that is
/// not a number at all is no hue.
fn wrapped_hue<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let v = f32::deserialize(d)?;
    Ok(if v.is_finite() {
        v.rem_euclid(360.0)
    } else {
        0.0
    })
}

/// An amount as read from a file: 0 to 1, and not a number is none.
fn held_amount<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let v = f32::deserialize(d)?;
    Ok(if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) })
}

impl Tint {
    /// Nothing: the amount at zero, whatever the hue.
    pub const OFF: Self = Self {
        hue: 0.0,
        amount: 0.0,
    };

    /// Nothing to do, and nothing done: [`Self::applied`] returns its
    /// input unchanged, so a look without a tint is bit for bit the
    /// look it was.
    pub fn is_identity(&self) -> bool {
        self.amount <= 0.0
    }

    /// The chroma the tint pulls a *neutral* up to at this lightness:
    /// a bell peaking at [`MID_LIGHTNESS`] and nothing at black or at
    /// twice it, so neither end of the scale is given color it did
    /// not have. It is a floor and not a ceiling: a pixel already
    /// above it keeps its own chroma, here as anywhere, and still has
    /// its hue turned. The lightness is the scene's, as the whole
    /// Oklab pass is.
    pub fn floor(lightness: f32) -> f32 {
        let t = (lightness - MID_LIGHTNESS) / MID_LIGHTNESS;
        CHROMA * (1.0 - t * t).max(0.0)
    }

    /// The tint as a vector, `amount` in the hue's direction: how a
    /// look's tint and each mask's add (`finish.rs`).
    pub fn vector(&self) -> [f32; 2] {
        let (s, c) = self.hue.to_radians().sin_cos();
        let a = self.amount.clamp(0.0, 1.0);
        [a * c, a * s]
    }

    /// The tint a vector means: the amount is its length, capped at
    /// one, and the hue its direction. The zero vector is [`Self::OFF`].
    pub fn from_vector(v: [f32; 2]) -> Self {
        let amount = (v[0] * v[0] + v[1] * v[1]).sqrt();
        if amount <= 0.0 {
            return Self::OFF;
        }
        Self {
            hue: v[1].atan2(v[0]).to_degrees().rem_euclid(360.0),
            amount: amount.min(1.0),
        }
    }

    /// Oklab's a and b at a lightness, tinted.
    ///
    /// The chroma rises toward `max(chroma, floor(lightness))`, never
    /// falls: the tint colors, it does not wash out. Where the floor
    /// is nothing — black, white, and past the bell either way — a
    /// neutral stays neutral, and a color keeps every bit of the
    /// chroma it had and is turned like any other.
    ///
    /// The hue turns toward the tint's by the share of that chroma
    /// the tint is answerable for, `amount * target` against
    /// `(1 - amount) * chroma`, so a neutral takes the tint's hue
    /// exactly rather than a hue read off its own noise, a color as
    /// saturated as the floor meets it halfway at half the amount,
    /// and a full amount replaces the hue outright.
    pub fn applied(&self, lightness: f32, a: f32, b: f32) -> [f32; 2] {
        let amount = self.amount.clamp(0.0, 1.0);
        if amount <= 0.0 {
            return [a, b];
        }
        let chroma = (a * a + b * b).sqrt();
        let target = chroma.max(Self::floor(lightness));
        // What is left of the pixel's own color against what the
        // tint puts there.
        let own = (1.0 - amount) * chroma;
        let theirs = amount * target;
        let sum = own + theirs;
        if sum <= 0.0 {
            // Black, white, or beyond the bell: no chroma either way.
            return [a, b];
        }
        let share = theirs / sum;
        let chroma = chroma + amount * (target - chroma);
        let hue = b.atan2(a).to_degrees();
        // The short way round, so a tint never turns the long way.
        let delta = (self.hue - hue + 180.0).rem_euclid(360.0) - 180.0;
        let (s, c) = (hue + share * delta).to_radians().sin_cos();
        [chroma * c, chroma * s]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chroma(ab: [f32; 2]) -> f32 {
        (ab[0] * ab[0] + ab[1] * ab[1]).sqrt()
    }

    fn hue(ab: [f32; 2]) -> f32 {
        ab[1].atan2(ab[0]).to_degrees().rem_euclid(360.0)
    }

    #[test]
    fn the_default_is_off_and_does_nothing() {
        let off = Tint::default();
        assert!(off.is_identity());
        assert_eq!(off, Tint::OFF);
        // Bit for bit: the identity is a return, not a round trip.
        for (l, a, b) in [(0.5, 0.123_45, -0.067_8), (0.0, 0.0, 0.0), (1.9, 0.3, 0.3)] {
            assert_eq!(off.applied(l, a, b), [a, b]);
        }
        // And so is a tint whose amount is nothing, hue wherever.
        let idle = Tint {
            hue: 210.0,
            amount: 0.0,
        };
        assert!(idle.is_identity());
        assert_eq!(idle.applied(0.5, 0.1, 0.02), [0.1, 0.02]);
    }

    #[test]
    fn a_mid_grey_at_full_lands_on_the_hue_with_the_floor_s_chroma() {
        for h in [0.0, 47.0, 120.0, 210.0, 315.0] {
            let tint = Tint {
                hue: h,
                amount: 1.0,
            };
            let out = tint.applied(MID_LIGHTNESS, 0.0, 0.0);
            assert!((chroma(out) - CHROMA).abs() < 1e-6, "{h} {out:?}");
            assert!((hue(out) - h).abs() < 1e-3, "{h} {out:?}");
        }
        // Half the amount is half the chroma on a neutral, still on
        // the hue: a neutral has no hue of its own to argue with.
        let tint = Tint {
            hue: 210.0,
            amount: 0.5,
        };
        let out = tint.applied(MID_LIGHTNESS, 0.0, 0.0);
        assert!((chroma(out) - CHROMA / 2.0).abs() < 1e-6, "{out:?}");
        assert!((hue(out) - 210.0).abs() < 1e-3, "{out:?}");
    }

    #[test]
    fn a_neutral_at_either_end_is_left_alone() {
        // Black stays black and white stays white: what the bell
        // holds back is the chroma a *neutral* is given. A pixel with
        // color in it up there is another matter
        // (`past_the_bell_a_color_still_turns_and_keeps_its_chroma`).
        let tint = Tint {
            hue: 210.0,
            amount: 1.0,
        };
        assert_eq!(Tint::floor(0.0), 0.0);
        assert_eq!(Tint::floor(2.0 * MID_LIGHTNESS), 0.0);
        for l in [0.0, 2.0 * MID_LIGHTNESS, 1.5, 3.0] {
            assert_eq!(tint.applied(l, 0.0, 0.0), [0.0, 0.0], "{l}");
        }
        // The bell is symmetric about mid grey and peaks there.
        assert!(Tint::floor(MID_LIGHTNESS) > Tint::floor(0.3));
        assert!((Tint::floor(0.3) - Tint::floor(2.0 * MID_LIGHTNESS - 0.3)).abs() < 1e-6);
    }

    #[test]
    fn a_saturated_color_turns_halfway_at_half_and_keeps_its_chroma() {
        // A saturated red, well above the floor: at half the amount
        // the hue is halfway to the tint's, the short way round (210
        // is 150 degrees back from 0, not 210 on), and the chroma is
        // its own.
        let red = [0.2, 0.0];
        let tint = Tint {
            hue: 210.0,
            amount: 0.5,
        };
        let out = tint.applied(MID_LIGHTNESS, red[0], red[1]);
        assert!((chroma(out) - 0.2).abs() < 1e-6, "{out:?}");
        assert!((hue(out) - 285.0).abs() < 1e-3, "{out:?}");
        // At a full amount the hue is replaced outright, not added to.
        let full = Tint {
            hue: 210.0,
            amount: 1.0,
        };
        let out = full.applied(MID_LIGHTNESS, red[0], red[1]);
        assert!((hue(out) - 210.0).abs() < 1e-3, "{out:?}");
        assert!((chroma(out) - 0.2).abs() < 1e-6, "{out:?}");
        // The short way round: a hue 20 degrees below wraps through
        // zero rather than crossing the wheel.
        let back = Tint {
            hue: 340.0,
            amount: 0.5,
        };
        let out = back.applied(MID_LIGHTNESS, red[0], red[1]);
        assert!((hue(out) - 350.0).abs() < 1e-3, "{out:?}");
    }

    #[test]
    fn a_pale_color_gives_way_sooner_than_a_saturated_one() {
        let tint = Tint {
            hue: 210.0,
            amount: 0.5,
        };
        // A chroma a tenth of the floor at half the amount is all but
        // the tint's hue; the same hue at twice the floor is a long
        // way short of it.
        let from = |ab| ((hue(ab) - 210.0 + 180.0).rem_euclid(360.0) - 180.0).abs();
        let pale = tint.applied(MID_LIGHTNESS, 0.01, 0.0);
        let strong = tint.applied(MID_LIGHTNESS, 0.2, 0.0);
        assert!(from(pale) < 20.0, "{pale:?}");
        assert!(from(strong) > 60.0, "{strong:?}");
        // And a pale color is lifted to the floor, never cut back.
        assert!(chroma(pale) > 0.01 && chroma(pale) <= CHROMA);
        assert!((chroma(strong) - 0.2).abs() < 1e-6);
    }

    #[test]
    fn past_the_bell_a_color_still_turns_and_keeps_its_chroma() {
        // Above scene white the floor is nothing, so a neutral is
        // left alone (`a_neutral_at_either_end_is_left_alone`) — but a pixel
        // that has color keeps all of it and lands on the tint's hue
        // at a full amount, which is what hand-coloring a bright sky
        // asks for.
        assert_eq!(Tint::floor(1.5), 0.0);
        let tint = Tint {
            hue: 210.0,
            amount: 1.0,
        };
        let out = tint.applied(1.5, 0.18, -0.06);
        let was = (0.18f32 * 0.18 + 0.06 * 0.06).sqrt();
        assert!((chroma(out) - was).abs() < 1e-6, "{out:?} {was}");
        assert!((hue(out) - 210.0).abs() < 1e-3, "{out:?}");
        // And at half the amount it is halfway there, chroma kept.
        let half = Tint {
            hue: 210.0,
            amount: 0.5,
        };
        let mid = half.applied(1.5, 0.18, -0.06);
        assert!((chroma(mid) - was).abs() < 1e-6, "{mid:?}");
        let start = hue([0.18, -0.06]);
        let step = (hue(mid) - start + 180.0).rem_euclid(360.0) - 180.0;
        let whole = (210.0 - start + 180.0f32).rem_euclid(360.0) - 180.0;
        assert!((step - whole / 2.0).abs() < 1e-3, "{start} {}", hue(mid));
    }

    #[test]
    fn a_sidecar_s_values_are_brought_into_range_as_they_are_read() {
        let read = |s: &str| serde_json::from_str::<Tint>(s).expect("a tint");
        // Round the circle, either way, however many times.
        assert!((read(r#"{"hue": 400, "amount": 0.5}"#).hue - 40.0).abs() < 1e-3);
        assert!((read(r#"{"hue": -90, "amount": 0.5}"#).hue - 270.0).abs() < 1e-3);
        let big = read(r#"{"hue": 1e30, "amount": 0.5}"#);
        assert!((0.0..360.0).contains(&big.hue), "{}", big.hue);
        // The amount held to its ends.
        assert_eq!(read(r#"{"hue": 0, "amount": 5}"#).amount, 1.0);
        assert_eq!(read(r#"{"hue": 0, "amount": -1}"#).amount, 0.0);
        // Nothing that is not a number gets through either field.
        let mad = read(r#"{"hue": 1e300, "amount": 1e300}"#);
        assert_eq!(mad.hue, 0.0);
        assert_eq!(mad.amount, 1.0);
        // A missing field is still the default, and a whole one is
        // untouched.
        assert_eq!(read(r#"{}"#), Tint::OFF);
        let plain = read(r#"{"hue": 210, "amount": 0.4}"#);
        assert_eq!(plain.hue, 210.0);
        assert_eq!(plain.amount, 0.4);
        // Once in range, a tint taken straight and a tint through the
        // vector round trip the finish uses are the same tint.
        for t in [plain, big, read(r#"{"hue": -90, "amount": 0.5}"#)] {
            let there_and_back = Tint::from_vector(t.vector());
            let a = t.applied(MID_LIGHTNESS, 0.05, 0.02);
            let b = there_and_back.applied(MID_LIGHTNESS, 0.05, 0.02);
            assert!(
                (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5,
                "{a:?} {b:?}"
            );
        }
    }

    #[test]
    fn the_vector_round_trips_and_adds() {
        for tint in [
            Tint {
                hue: 0.0,
                amount: 1.0,
            },
            Tint {
                hue: 47.0,
                amount: 0.3,
            },
            Tint {
                hue: 305.0,
                amount: 0.75,
            },
        ] {
            let back = Tint::from_vector(tint.vector());
            assert!((back.hue - tint.hue).abs() < 1e-3, "{back:?}");
            assert!((back.amount - tint.amount).abs() < 1e-6, "{back:?}");
        }
        assert_eq!(Tint::from_vector([0.0, 0.0]), Tint::OFF);
        // Two halves of the same hue make one whole; opposite hues at
        // the same amount cancel.
        let a = Tint {
            hue: 90.0,
            amount: 0.5,
        };
        let sum = Tint::from_vector([a.vector()[0] * 2.0, a.vector()[1] * 2.0]);
        assert!((sum.amount - 1.0).abs() < 1e-6 && (sum.hue - 90.0).abs() < 1e-3);
        let b = Tint {
            hue: 270.0,
            amount: 0.5,
        };
        let v = a.vector();
        let w = b.vector();
        assert!(Tint::from_vector([v[0] + w[0], v[1] + w[1]]).amount < 1e-6);
        // A vector longer than one is an amount of one: the cap.
        assert_eq!(Tint::from_vector([2.0, 0.0]).amount, 1.0);
    }
}
