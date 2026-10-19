//! A tint: one hue, and how far the picture is pulled toward it.
//!
//! Lightroom's local Color, and what a mask most often wants that no
//! band-by-band control gives: warm this patch, cool that one. A hue
//! in Oklab degrees and an amount, 0 to 1, off at nothing. The amount
//! blends the pixel's a and b toward a target on the tint's hue: as
//! much chroma as the pixel has, or a floor that follows the
//! lightness, whichever is more.
//!
//! A blend and not a turn. A pixel is moved in a straight line in a
//! and b toward the target, so a color opposite the tint passes
//! through grey on its way and never through the hues between the two:
//! a blue tint on an orange coat cools it toward beige and then toward
//! blue, never toward green. Turning the hue the short way round, as
//! this once did, sent every color of a warm scene under a cool tint
//! through yellow, green or magenta.
//!
//! The bell in the lightness gates the *lift* and not the pull, which
//! is the whole of what "a black stays black and a white stays white"
//! means: a neutral outside the bell has nothing to be pulled toward,
//! so it comes out the neutral it went in, while a pixel that already
//! has color is pulled toward the tint's hue at its own chroma, at the
//! top of the scale as anywhere else. That is deliberate. It is how a
//! bright sky is hand-colored: the pixels worth coloring up there have
//! color in them already, and a tint that gave up above scene white
//! would leave a rebuilt or pushed highlight behind while the rest of
//! the mask moved.
//!
//! The section has a switch, as every other has: off, the hue and the
//! amount stay where they were and the tint does nothing, on the CPU
//! (`applied` is a return, the vector is zero) and in the shader, which
//! is handed the vector. Left out of a sidecar while on, so a tint
//! nobody switched off writes what it always wrote, and the develop key
//! of every edit made before the switch is the key it had.
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
/// and in one place, so nothing downstream has to wonder. A hue wraps
/// round the circle and an amount is held to its ends; a number past
/// an `f32`'s range never gets here, since the edit holding it is
/// refused whole ([`crate::migrate`]). The two
/// paths through a tint (`finish.rs` blends every look's as a vector
/// and reads a tint back off the sum; `pick` takes one straight)
/// agree only on a hue that is a hue and an amount that is an amount.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tint {
    /// The section's switch: off, the tint does nothing and keeps its
    /// hue and amount for when it goes on again. Written only when off.
    #[serde(skip_serializing_if = "is_on")]
    pub enabled: bool,
    /// Oklab hue, degrees, 0 to 360.
    #[serde(deserialize_with = "wrapped_hue")]
    pub hue: f32,
    /// 0 (nothing) to 1 (the hue replaces the pixel's).
    #[serde(deserialize_with = "held_amount")]
    pub amount: f32,
}

impl Default for Tint {
    fn default() -> Self {
        Self::OFF
    }
}

fn is_on(enabled: &bool) -> bool {
    *enabled
}

/// A hue as read from a file: round the circle. One that is not finite
/// is no hue, though an edit's reader refuses such a number first.
fn wrapped_hue<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let v = f32::deserialize(d)?;
    Ok(if v.is_finite() {
        v.rem_euclid(360.0)
    } else {
        0.0
    })
}

/// An amount as read from a file: held to 0 to 1, and not a number is
/// none.
fn held_amount<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let v = f32::deserialize(d)?;
    Ok(if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) })
}

impl Tint {
    /// Nothing: the amount at zero, whatever the hue. The switch is
    /// on, as a new section's is; it is the amount that is nothing.
    pub const OFF: Self = Self::new(0.0, 0.0);

    /// A tint at a hue and an amount, switched on.
    pub const fn new(hue: f32, amount: f32) -> Self {
        Self {
            enabled: true,
            hue,
            amount,
        }
    }

    /// Nothing to do, and nothing done: [`Self::applied`] returns its
    /// input unchanged, so a look without a tint, or with its switch
    /// off, is bit for bit the look it was.
    pub fn is_identity(&self) -> bool {
        !self.enabled || self.amount <= 0.0
    }

    /// The chroma the tint pulls a *neutral* up to at this lightness:
    /// a bell peaking at [`MID_LIGHTNESS`] and nothing at black or at
    /// twice it, so neither end of the scale is given color it did
    /// not have. It is a floor and not a ceiling: a pixel already
    /// above it is pulled toward the tint's hue at its own chroma,
    /// here as anywhere. The lightness is the scene's, as the whole
    /// Oklab pass is.
    pub fn floor(lightness: f32) -> f32 {
        let t = (lightness - MID_LIGHTNESS) / MID_LIGHTNESS;
        CHROMA * (1.0 - t * t).max(0.0)
    }

    /// The tint as a vector, `amount` in the hue's direction: how a
    /// look's tint and each mask's add (`finish.rs`), and what the
    /// shader is handed. Zero while the switch is off.
    pub fn vector(&self) -> [f32; 2] {
        if !self.enabled {
            return [0.0, 0.0];
        }
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
        Self::new(
            v[1].atan2(v[0]).to_degrees().rem_euclid(360.0),
            amount.min(1.0),
        )
    }

    /// Oklab's a and b at a lightness, tinted.
    ///
    /// A blend of the pixel's a and b toward a target on the tint's
    /// hue, `ab + amount * (target - ab)`, the target's chroma
    /// `max(chroma, floor(lightness))`. So a neutral lands on the
    /// tint's hue exactly, at the floor's chroma at a full amount,
    /// rather than at a hue read off its own noise; a neutral where
    /// the floor is nothing (black, white, past the bell either way)
    /// has a target of nothing and stays neutral; a color of the
    /// tint's own hue keeps its hue and rises to the floor; and a
    /// color opposite the tint passes through grey, never through the
    /// hues between. A full amount is the target outright. Switched
    /// off, the input comes back as it went in.
    pub fn applied(&self, lightness: f32, a: f32, b: f32) -> [f32; 2] {
        let amount = self.amount.clamp(0.0, 1.0);
        if !self.enabled || amount <= 0.0 {
            return [a, b];
        }
        let chroma = (a * a + b * b).sqrt();
        let target = chroma.max(Self::floor(lightness));
        let (s, c) = self.hue.to_radians().sin_cos();
        [a + amount * (target * c - a), b + amount * (target * s - b)]
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
        let idle = Tint::new(210.0, 0.0);
        assert!(idle.is_identity());
        assert_eq!(idle.applied(0.5, 0.1, 0.02), [0.1, 0.02]);
        // The switch starts on: a new tint is off by its amount alone.
        assert!(off.enabled && idle.enabled);
    }

    #[test]
    fn switched_off_a_tint_does_nothing_and_keeps_its_values() {
        let on = Tint::new(210.0, 0.6);
        let off = Tint {
            enabled: false,
            ..on
        };
        assert!(!on.is_identity());
        assert!(off.is_identity());
        // The hue and the amount are kept for when it goes on again.
        assert_eq!((off.hue, off.amount), (210.0, 0.6));
        // Bit for bit the input, where the same tint on moves it.
        for (l, a, b) in [
            (MID_LIGHTNESS, 0.0, 0.0),
            (0.5, 0.123_45, -0.067_8),
            (1.5, 0.18, -0.06),
        ] {
            assert_eq!(off.applied(l, a, b), [a, b]);
            assert_ne!(on.applied(l, a, b), [a, b]);
        }
        // And the vector the finish adds and the shader is handed is
        // zero, so a mask's switched-off tint adds nothing to the sum.
        assert_eq!(off.vector(), [0.0, 0.0]);
        assert_ne!(on.vector(), [0.0, 0.0]);
        assert_eq!(Tint::from_vector(off.vector()), Tint::OFF);
    }

    #[test]
    fn the_switch_is_on_unless_a_sidecar_says_off_and_is_written_only_off() {
        let read = |s: &str| serde_json::from_str::<Tint>(s).expect("a tint");
        // A sidecar from before the switch reads as on.
        let old = read(r#"{"hue": 210, "amount": 0.4}"#);
        assert!(old.enabled);
        assert_eq!(old, Tint::new(210.0, 0.4));
        assert!(read(r#"{}"#).enabled);
        assert!(!read(r#"{"enabled": false, "hue": 210, "amount": 0.4}"#).enabled);
        // On, it writes what it always wrote, byte for byte, so the
        // develop key of an edit made before the switch is unchanged.
        let on = serde_json::to_string(&Tint::new(210.0, 0.4)).unwrap();
        assert_eq!(on, r#"{"hue":210.0,"amount":0.4}"#);
        assert_eq!(
            serde_json::to_string(&Tint::OFF).unwrap(),
            r#"{"hue":0.0,"amount":0.0}"#
        );
        // Off, the switch is written, and reads back off with its values.
        let off = Tint {
            enabled: false,
            ..Tint::new(210.0, 0.4)
        };
        let json = serde_json::to_string(&off).unwrap();
        assert_eq!(json, r#"{"enabled":false,"hue":210.0,"amount":0.4}"#);
        assert_eq!(read(&json), off);
    }

    #[test]
    fn a_mid_grey_at_full_lands_on_the_hue_with_the_floor_s_chroma() {
        for h in [0.0, 47.0, 120.0, 210.0, 315.0] {
            let tint = Tint::new(h, 1.0);
            let out = tint.applied(MID_LIGHTNESS, 0.0, 0.0);
            assert!((chroma(out) - CHROMA).abs() < 1e-6, "{h} {out:?}");
            assert!((hue(out) - h).abs() < 1e-3, "{h} {out:?}");
        }
        // Half the amount is half the chroma on a neutral, still on
        // the hue: a neutral has no hue of its own to argue with.
        let tint = Tint::new(210.0, 0.5);
        let out = tint.applied(MID_LIGHTNESS, 0.0, 0.0);
        assert!((chroma(out) - CHROMA / 2.0).abs() < 1e-6, "{out:?}");
        assert!((hue(out) - 210.0).abs() < 1e-3, "{out:?}");
    }

    #[test]
    fn a_neutral_at_either_end_is_left_alone() {
        // Black stays black and white stays white: what the bell
        // holds back is the chroma a *neutral* is given. A pixel with
        // color in it up there is another matter
        // (`past_the_bell_a_color_is_still_pulled_at_its_own_chroma`).
        let tint = Tint::new(210.0, 1.0);
        assert_eq!(Tint::floor(0.0), 0.0);
        assert_eq!(Tint::floor(2.0 * MID_LIGHTNESS), 0.0);
        for l in [0.0, 2.0 * MID_LIGHTNESS, 1.5, 3.0] {
            assert_eq!(tint.applied(l, 0.0, 0.0), [0.0, 0.0], "{l}");
        }
        // The bell is symmetric about mid grey and peaks there.
        assert!(Tint::floor(MID_LIGHTNESS) > Tint::floor(0.3));
        assert!((Tint::floor(0.3) - Tint::floor(2.0 * MID_LIGHTNESS - 0.3)).abs() < 1e-6);
    }

    /// How far a hue is from another, either way round, in degrees.
    fn apart(h: f32, from: f32) -> f32 {
        ((h - from + 180.0).rem_euclid(360.0) - 180.0).abs()
    }

    fn polar(chroma: f32, hue: f32) -> [f32; 2] {
        let (s, c) = hue.to_radians().sin_cos();
        [chroma * c, chroma * s]
    }

    #[test]
    fn a_color_is_blended_toward_the_target_in_a_straight_line() {
        // A saturated red, well above the floor, under a tint at 210:
        // the target is the red's own chroma on the tint's hue, and
        // each amount is that share of the way from one to the other
        // in a and b, not round the circle.
        let red = [0.2, 0.0];
        let target = polar(0.2, 210.0);
        for amount in [0.25, 0.5, 0.75, 1.0] {
            let tint = Tint::new(210.0, amount);
            let out = tint.applied(MID_LIGHTNESS, red[0], red[1]);
            for k in 0..2 {
                let want = red[k] + amount * (target[k] - red[k]);
                assert!((out[k] - want).abs() < 1e-6, "{amount} {out:?}");
            }
        }
        // At a full amount it is the target outright: the tint's hue
        // at the red's chroma.
        let full = Tint::new(210.0, 1.0);
        let out = full.applied(MID_LIGHTNESS, red[0], red[1]);
        assert!(apart(hue(out), 210.0) < 1e-3, "{out:?}");
        assert!((chroma(out) - 0.2).abs() < 1e-6, "{out:?}");
        // On the way the chord passes nearer grey than either end:
        // the chroma dips, which is what cooling a warm color is.
        let half = Tint::new(210.0, 0.5);
        let mid = half.applied(MID_LIGHTNESS, red[0], red[1]);
        assert!(chroma(mid) < 0.2 * 0.5, "{mid:?}");
    }

    #[test]
    fn a_color_of_the_tint_s_own_hue_keeps_its_hue_and_rises_to_the_floor() {
        let tint = Tint::new(50.0, 0.5);
        // A pale color of the tint's hue is lifted halfway to the
        // floor at half the amount, and its hue does not move.
        let pale = polar(0.02, 50.0);
        let out = tint.applied(MID_LIGHTNESS, pale[0], pale[1]);
        assert!(apart(hue(out), 50.0) < 1e-3, "{out:?}");
        assert!((chroma(out) - (0.02 + 0.5 * (CHROMA - 0.02))).abs() < 1e-6);
        // One already above the floor is its own target: nothing pulls
        // it anywhere, and it is never cut back.
        let strong = polar(0.2, 50.0);
        let out = tint.applied(MID_LIGHTNESS, strong[0], strong[1]);
        assert!((out[0] - strong[0]).abs() < 1e-6 && (out[1] - strong[1]).abs() < 1e-6);
    }

    #[test]
    fn an_opposite_color_passes_through_grey_and_never_the_hues_between() {
        // A warm scene under a cool tint: an orange at 60 degrees, a
        // blue tint at 236, 176 degrees apart. Turned the short way
        // round, as the tint once was, the orange went through yellow
        // and green. Blended, it stays orange while its chroma falls,
        // passes close by grey, and comes out blue: wherever the hue is
        // more than ten degrees from both ends, the color has a quarter
        // of the orange's chroma or less, and it reads green (100 to
        // 200) only with less than a tenth of it.
        for start in [0.05f32, 0.12, 0.2] {
            let orange = polar(start, 60.0);
            for step in 0..=1000 {
                let amount = step as f32 / 1000.0;
                let tint = Tint::new(236.0, amount);
                let out = tint.applied(MID_LIGHTNESS, orange[0], orange[1]);
                let (c, h) = (chroma(out), hue(out));
                let near = apart(h, 60.0).min(apart(h, 236.0));
                assert!(near <= 10.0 || c < start / 4.0, "{start} {amount} {c} {h}");
                assert!(
                    !(100.0..200.0).contains(&h) || c < start / 10.0,
                    "{start} {amount} {c} {h}"
                );
            }
        }
        // And the colors of a warm portrait under the same tint at a
        // small amount: a near-neutral beige, an orange, a red and a
        // skin tone each lose chroma and stay near their own hue. The
        // short-way turn took the beige to 125 (green) and the red to
        // 10 (magenta).
        let tint = Tint::new(236.0, 0.13);
        for (c, h) in [(0.03f32, 70.0f32), (0.12, 60.0), (0.1, 30.0), (0.06, 45.0)] {
            let ab = polar(c, h);
            let out = tint.applied(MID_LIGHTNESS, ab[0], ab[1]);
            assert!(chroma(out) < c, "{c} {h} {out:?}");
            assert!(apart(hue(out), h) < 15.0, "{c} {h} {}", hue(out));
        }
    }

    #[test]
    fn past_the_bell_a_color_is_still_pulled_at_its_own_chroma() {
        // Above scene white the floor is nothing, so a neutral is left
        // alone (`a_neutral_at_either_end_is_left_alone`), but a pixel
        // that has color is pulled toward the tint's hue at its own
        // chroma and lands on it at a full amount, which is what
        // hand-coloring a bright sky asks for.
        assert_eq!(Tint::floor(1.5), 0.0);
        let tint = Tint::new(210.0, 1.0);
        let out = tint.applied(1.5, 0.18, -0.06);
        let was = (0.18f32 * 0.18 + 0.06 * 0.06).sqrt();
        assert!((chroma(out) - was).abs() < 1e-6, "{out:?} {was}");
        assert!(apart(hue(out), 210.0) < 1e-3, "{out:?}");
        // And at half the amount it is the midpoint of the two, which
        // lies on the hue between them.
        let half = Tint::new(210.0, 0.5);
        let mid = half.applied(1.5, 0.18, -0.06);
        let target = polar(was, 210.0);
        assert!((mid[0] - (0.18 + target[0]) / 2.0).abs() < 1e-6, "{mid:?}");
        assert!((mid[1] - (-0.06 + target[1]) / 2.0).abs() < 1e-6, "{mid:?}");
        let start = hue([0.18, -0.06]);
        let whole = (210.0 - start + 180.0f32).rem_euclid(360.0) - 180.0;
        assert!(
            apart(hue(mid), start + whole / 2.0) < 1e-3,
            "{start} {}",
            hue(mid)
        );
    }

    #[test]
    fn a_sidecar_s_values_in_range_wrap_or_hold_and_past_it_are_refused() {
        let read = |s: &str| serde_json::from_str::<Tint>(s).expect("a tint");
        // Round the circle, either way, however many times.
        assert!((read(r#"{"hue": 400, "amount": 0.5}"#).hue - 40.0).abs() < 1e-3);
        assert!((read(r#"{"hue": -90, "amount": 0.5}"#).hue - 270.0).abs() < 1e-3);
        let big = read(r#"{"hue": 1e30, "amount": 0.5}"#);
        assert!((0.0..360.0).contains(&big.hue), "{}", big.hue);
        // The amount held to its ends.
        assert_eq!(read(r#"{"hue": 0, "amount": 5}"#).amount, 1.0);
        assert_eq!(read(r#"{"hue": 0, "amount": -1}"#).amount, 0.0);
        // A number past an f32's range is not a value to wrap or hold:
        // the edit holding it is refused.
        for tint in [r#"{"hue": 1e300}"#, r#"{"amount": 1e300}"#] {
            let edit = format!(r#"{{"version": 6, "tint": {tint}}}"#);
            assert!(crate::Edit::from_json(&edit).is_err(), "{tint}");
        }
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
            Tint::new(0.0, 1.0),
            Tint::new(47.0, 0.3),
            Tint::new(305.0, 0.75),
        ] {
            let back = Tint::from_vector(tint.vector());
            assert!((back.hue - tint.hue).abs() < 1e-3, "{back:?}");
            assert!((back.amount - tint.amount).abs() < 1e-6, "{back:?}");
        }
        assert_eq!(Tint::from_vector([0.0, 0.0]), Tint::OFF);
        // Two halves of the same hue make one whole; opposite hues at
        // the same amount cancel.
        let a = Tint::new(90.0, 0.5);
        let sum = Tint::from_vector([a.vector()[0] * 2.0, a.vector()[1] * 2.0]);
        assert!((sum.amount - 1.0).abs() < 1e-6 && (sum.hue - 90.0).abs() < 1e-3);
        let b = Tint::new(270.0, 0.5);
        let v = a.vector();
        let w = b.vector();
        assert!(Tint::from_vector([v[0] + w[0], v[1] + w[1]]).amount < 1e-6);
        // A vector longer than one is an amount of one: the cap.
        assert_eq!(Tint::from_vector([2.0, 0.0]).amount, 1.0);
    }
}
