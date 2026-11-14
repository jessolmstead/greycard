//! A white balance a mask carries: the light under one part of the
//! picture, where it is not the light under the rest.
//!
//! The global white balance runs in the develop, as gains in camera
//! space before the camera matrix; a mask's cannot, since the develop
//! makes one picture. So a local one is the same change made after the
//! develop, exactly, as a working-space matrix: back through the
//! develop's camera matrix to the sensor's balanced values, the ratio
//! of the gains, and forward through the matrix the local white asks
//! for ([`greycard_core::color::white_shift`]). Exact for the matrix
//! and the gains; the demosaic and the highlight reconstruction saw the
//! develop's gains.
//!
//! [`WhiteShift`] holds what the matrix is made from — the develop's
//! white and the profile it was resolved through — so the viewport and
//! a headless export make the same matrices from the same two things.
//!
//! A picture that is not a raw has no camera: its white balance is an
//! offset from its own white ([`crate::WhiteBalance::Offset`]), and its
//! matrices, the global's and a mask's, are the Bradford adaptation in
//! XYZ between two lights ([`adaptation`], [`WhiteShift::picture`]).

use greycard_core::color::{Matrix3, matrix_rows, white_balance_at, white_shift};
pub use greycard_core::color::{adaptation, picture_neutral};
use greycard_core::{CameraProfile, TempTint, WhiteBalance};
use serde::{Deserialize, Serialize};

/// A look's white balance. Off on every look but a mask's: the
/// picture's own is the edit's [`crate::WhiteBalance`], in the develop.
///
/// The switch and the light are each kept: on, a light named
/// ([`LocalWhite::Absolute`]) or an offset from the global's
/// ([`LocalWhite::Relative`]); switched off with that light kept for
/// when it goes on again ([`LocalWhite::Kept`],
/// [`LocalWhite::KeptRelative`]), as every other switched-off
/// adjustment keeps its values, so the switch compares the two; and
/// off with nothing kept ([`LocalWhite::Off`]), following the global,
/// so switching it on starts at the global's white.
///
/// Written as a `mode`, `absolute`, `relative`, `off` or
/// `kept_relative`, with the light beside it whenever there is one:
/// `{"mode":"off","temperature":3200.0,"tint":0.002}` is a kept light,
/// `{"mode":"relative","mired":25.0,"tint":0.002}` an offset. A build
/// from before the kept light reads that as off, as it read every
/// `off`. A mode this build does not know reads as off with nothing
/// kept, whatever comes with it, rather than failing the sidecar; and
/// a sidecar from before any of it, with no field at all, reads as off
/// too. So an offset is its own mode, not a flag beside `absolute`: a
/// build from before the offset reads one as off, never as a light,
/// and every sidecar from before it reads as it always did, with no
/// new version.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(from = "Wire", into = "Wire")]
pub enum LocalWhite {
    /// The light here is this, whatever the global says: a correlated
    /// color temperature in kelvin and a tint as Duv, positive toward
    /// green, resolved through the picture's camera profile as the
    /// global's is. It does not follow the global: a mask over a lamp
    /// says what the lamp is.
    ///
    /// Each field has a default, so a hand-edited sidecar missing one
    /// keeps the rest of its edit: a temperature left out is daylight,
    /// [`DEFAULT_TEMPERATURE`], and a tint left out is none.
    Absolute { temperature: f64, tint: f64 },
    /// The light here is the global's moved by this much: `mired`
    /// toward the higher kelvin, as the sync sheet's relative white
    /// moves (`kelvin_moved` there), and `tint` in Duv added to the
    /// global's. In mired, not kelvin, so the same offset is the same
    /// change of warmth wherever the global stands: 800 K is about 75
    /// mired at 3000 K and 15 at 7000 K. The sum is held to the
    /// panel's range ([`LocalWhite::temp_tint`]).
    Relative { mired: f64, tint: f64 },
    /// Switched off, the develop's white on the picture, and the light
    /// named while on kept for when it goes on again. No pixel reads it.
    Kept { temperature: f64, tint: f64 },
    /// Switched off with an offset kept, as [`LocalWhite::Kept`] keeps
    /// a light.
    KeptRelative { mired: f64, tint: f64 },
    /// No white balance of its own and none kept: the develop's.
    #[default]
    Off,
}

/// How a [`LocalWhite`] is written: the mode, and the light beside it,
/// a temperature in kelvin or an offset in mired.
#[derive(Serialize, Deserialize)]
struct Wire {
    #[serde(default)]
    mode: Mode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mired: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tint: Option<f64>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Absolute,
    Relative,
    #[default]
    Off,
    KeptRelative,
    /// A mode a later build writes: off here, and its values are not a
    /// light this build would know how to bring back.
    #[serde(other)]
    Unknown,
}

impl From<Wire> for LocalWhite {
    fn from(w: Wire) -> Self {
        let temperature = w.temperature.unwrap_or(DEFAULT_TEMPERATURE);
        let tint = w.tint.unwrap_or(0.0);
        // An offset left out is none: the global's own.
        let mired = w.mired.unwrap_or(0.0);
        match w.mode {
            Mode::Absolute => Self::Absolute { temperature, tint },
            Mode::Relative => Self::Relative { mired, tint },
            Mode::KeptRelative => Self::KeptRelative { mired, tint },
            Mode::Off if w.temperature.is_some() || w.tint.is_some() => {
                Self::Kept { temperature, tint }
            }
            Mode::Off | Mode::Unknown => Self::Off,
        }
    }
}

impl From<LocalWhite> for Wire {
    fn from(w: LocalWhite) -> Self {
        let wire = |mode, temperature, mired, tint| Wire {
            mode,
            temperature,
            mired,
            tint,
        };
        match w {
            LocalWhite::Absolute { temperature, tint } => {
                wire(Mode::Absolute, Some(temperature), None, Some(tint))
            }
            LocalWhite::Relative { mired, tint } => {
                wire(Mode::Relative, None, Some(mired), Some(tint))
            }
            LocalWhite::Kept { temperature, tint } => {
                wire(Mode::Off, Some(temperature), None, Some(tint))
            }
            LocalWhite::KeptRelative { mired, tint } => {
                wire(Mode::KeptRelative, None, Some(mired), Some(tint))
            }
            LocalWhite::Off => wire(Mode::Off, None, None, None),
        }
    }
}

/// The temperatures a mask's white balance takes, kelvin: the panel's
/// Temperature slider's ends.
pub const TEMPERATURE_RANGE: (f64, f64) = (2000.0, 12000.0);
/// How far its tint goes either way, Duv: the panel's Tint slider's.
pub const TINT_RANGE: f64 = 0.05;
/// A temperature a sidecar left out: the panel's slider's default.
pub const DEFAULT_TEMPERATURE: f64 = 5500.0;
/// How far an offset goes either way, mired: past the span of
/// [`TEMPERATURE_RANGE`], 500 less 83⅓, so any light in the range is an
/// offset from any global in it.
pub const OFFSET_MIRED_RANGE: f64 = 450.0;
/// How far an offset's tint goes either way, Duv: twice
/// [`TINT_RANGE`], for the same reason.
pub const OFFSET_TINT_RANGE: f64 = 2.0 * TINT_RANGE;

/// Mired, the reciprocal megakelvin.
pub fn mired(kelvin: f64) -> f64 {
    1e6 / kelvin
}

impl LocalWhite {
    /// Off with nothing kept: what a sidecar leaves out.
    pub fn is_off(&self) -> bool {
        *self == Self::Off
    }

    /// As the picture takes it: a kept light reaches no pixel, so it is
    /// off. What the develop key reads.
    pub fn acting(self) -> Self {
        match self {
            Self::Kept { .. } | Self::KeptRelative { .. } => Self::Off,
            other => other,
        }
    }

    /// Whether its numbers are an offset from the global's white, on
    /// or kept.
    pub fn is_relative(&self) -> bool {
        matches!(self, Self::Relative { .. } | Self::KeptRelative { .. })
    }

    /// The same light as an offset from `global`, on or kept as it
    /// was: what the panel's Follow global stores when it goes on, so
    /// nothing moves on screen. Already an offset, or off, it is
    /// itself.
    pub fn to_relative(self, global: TempTint) -> Self {
        let less = |temperature: f64, tint: f64| {
            (mired(global.cct) - mired(temperature), tint - global.duv)
        };
        match self {
            Self::Absolute { temperature, tint } => {
                let (mired, tint) = less(temperature, tint);
                Self::Relative { mired, tint }
            }
            Self::Kept { temperature, tint } => {
                let (mired, tint) = less(temperature, tint);
                Self::KeptRelative { mired, tint }
            }
            other => other,
        }
    }

    /// The light an offset names at `global`, on or kept as it was,
    /// held to the panel's range as every absolute light is: what the
    /// panel stores when Follow global goes off. Already absolute, or
    /// off, it is itself.
    pub fn to_absolute(self, global: TempTint) -> Self {
        let named = |mired: f64, tint: f64| {
            let tt = Self::Relative { mired, tint }.temp_tint(global);
            tt.map_or((f64::NAN, f64::NAN), |tt| (tt.cct, tt.duv))
        };
        match self {
            Self::Relative { mired, tint } => {
                let (temperature, tint) = named(mired, tint);
                Self::Absolute { temperature, tint }.clamped()
            }
            Self::KeptRelative { mired, tint } => {
                let (temperature, tint) = named(mired, tint);
                Self::Kept { temperature, tint }.clamped()
            }
            other => other,
        }
    }

    /// Brought into the panel's range: a temperature from 2000 to
    /// 12000 K and a tint of 0.05 either way, or an offset of 450
    /// mired and 0.1 either way. Past them the matrix after the
    /// develop extrapolates the camera's calibrations, and a
    /// hand-edited 1e9 K or a tint of 0.5 comes out as a color no
    /// light makes. A value that is not a number is off. The global
    /// white balance is not held this way; a mask's is read only
    /// through here.
    pub fn clamped(self) -> Self {
        let light = |t: f64, d: f64| {
            (
                t.clamp(TEMPERATURE_RANGE.0, TEMPERATURE_RANGE.1),
                d.clamp(-TINT_RANGE, TINT_RANGE),
            )
        };
        let offset = |m: f64, d: f64| {
            (
                m.clamp(-OFFSET_MIRED_RANGE, OFFSET_MIRED_RANGE),
                d.clamp(-OFFSET_TINT_RANGE, OFFSET_TINT_RANGE),
            )
        };
        let finite = |a: f64, b: f64| a.is_finite() && b.is_finite();
        match self {
            Self::Absolute { temperature, tint } if finite(temperature, tint) => {
                let (temperature, tint) = light(temperature, tint);
                Self::Absolute { temperature, tint }
            }
            Self::Kept { temperature, tint } if finite(temperature, tint) => {
                let (temperature, tint) = light(temperature, tint);
                Self::Kept { temperature, tint }
            }
            Self::Relative { mired, tint } if finite(mired, tint) => {
                let (mired, tint) = offset(mired, tint);
                Self::Relative { mired, tint }
            }
            Self::KeptRelative { mired, tint } if finite(mired, tint) => {
                let (mired, tint) = offset(mired, tint);
                Self::KeptRelative { mired, tint }
            }
            _ => Self::Off,
        }
    }

    /// The temperature and tint it asks for, when on, at the global's
    /// white `global`: its own light, held to the panel's range
    /// ([`LocalWhite::clamped`]), or the global's moved by its offset.
    /// `None` while off, a light kept or not.
    ///
    /// An offset's sum is held to the panel's range widened to take in
    /// the global itself: an As shot can resolve past it (a tint past
    /// 0.05 on some profiles), and a mask with no offset must be the
    /// global's own white there, not the nearest edge of the range.
    /// Past the range, then, an offset only moves toward it. No offset
    /// is the global to the last bit, not its reciprocal's reciprocal.
    pub fn temp_tint(&self, global: TempTint) -> Option<TempTint> {
        match self.clamped() {
            Self::Off | Self::Kept { .. } | Self::KeptRelative { .. } => None,
            Self::Absolute { temperature, tint } => Some(TempTint {
                cct: temperature,
                duv: tint,
            }),
            Self::Relative { mired: by, tint } => {
                if !(global.cct.is_finite() && global.cct > 0.0 && global.duv.is_finite()) {
                    return None;
                }
                let (lo, hi) = (
                    TEMPERATURE_RANGE.0.min(global.cct),
                    TEMPERATURE_RANGE.1.max(global.cct),
                );
                let cct = if by == 0.0 {
                    global.cct
                } else {
                    // Toward the higher kelvin is fewer mired; at or
                    // past none, the top of the range.
                    let at = mired(global.cct) - by;
                    if at <= mired(hi) {
                        hi
                    } else {
                        (1e6 / at).clamp(lo, hi)
                    }
                };
                let duv = (global.duv + tint)
                    .clamp((-TINT_RANGE).min(global.duv), TINT_RANGE.max(global.duv));
                Some(TempTint { cct, duv })
            }
        }
    }
}

/// What a local white balance's matrix is made from: a raw's develop,
/// its resolved white balance and the camera profile it was resolved
/// through; or a picture's white and the light its own offset names.
#[derive(Debug, Clone)]
pub struct WhiteShift {
    from: Base,
    /// The develop's white as a temperature and tint, As shot resolved
    /// to what the camera's gains mean, or the light a picture's offset
    /// names: what a mask's offset is from.
    pub global: TempTint,
}

#[derive(Debug, Clone)]
enum Base {
    /// A raw: back through the develop's matrix and gains, forward
    /// through the ones the target asks of the profile.
    Camera {
        profile: Box<CameraProfile>,
        /// The develop's gains, camera space, green at one.
        gains: [f32; 3],
        /// The develop's camera to working matrix, rows.
        rows: Matrix3,
    },
    /// A picture: no camera, so the adaptation in XYZ from the light
    /// it shows ([`WhiteShift::global`]) to the target.
    Picture,
    /// A monochrome sensor's raw: no color to balance, so no white
    /// moves it.
    Neutral,
}

const IDENTITY: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

impl WhiteShift {
    /// From the develop's resolved white balance.
    pub fn new(profile: CameraProfile, base: &WhiteBalance) -> Self {
        Self {
            from: Base::Camera {
                profile: Box::new(profile),
                gains: base.coefficients_f32(),
                rows: matrix_rows(base),
            },
            global: base.temp_tint,
        }
    }

    /// From the develop's gains, matrix, and temperature and tint, as
    /// already taken apart.
    pub fn from_parts(
        profile: CameraProfile,
        gains: [f32; 3],
        rows: Matrix3,
        global: TempTint,
    ) -> Self {
        Self {
            from: Base::Camera {
                profile: Box::new(profile),
                gains,
                rows,
            },
            global,
        }
    }

    /// From a white point alone: a picture shown at `light`, its own
    /// white moved by its offset (or its own white, at none).
    pub fn picture(light: TempTint) -> Self {
        Self {
            from: Base::Picture,
            global: light,
        }
    }

    /// A monochrome sensor's raw, developed at `global`: every white
    /// asked of it, the global's or a mask's, is the identity.
    pub fn neutral(global: TempTint) -> Self {
        Self {
            from: Base::Neutral,
            global,
        }
    }

    /// A raw's develop gains and camera to working matrix, rows, as
    /// it was made from; none for a picture's.
    pub fn camera(&self) -> Option<([f32; 3], Matrix3)> {
        match &self.from {
            Base::Camera { gains, rows, .. } => Some((*gains, *rows)),
            Base::Neutral => Some(([1.0; 3], IDENTITY)),
            Base::Picture => None,
        }
    }

    /// The matrix to a resolved white balance.
    pub fn to(&self, target: &WhiteBalance) -> Option<Matrix3> {
        match &self.from {
            Base::Camera { gains, rows, .. } => white_shift(
                *gains,
                *rows,
                target.coefficients_f32(),
                matrix_rows(target),
            ),
            Base::Picture => adaptation(self.global, target.temp_tint),
            Base::Neutral => Some(IDENTITY),
        }
    }

    /// The matrix to a temperature and tint.
    pub fn to_temp_tint(&self, tt: TempTint) -> Option<Matrix3> {
        match &self.from {
            Base::Camera { profile, .. } => match white_balance_at(profile, tt) {
                Ok(wb) => self.to(&wb),
                Err(e) => {
                    log::debug!("white balance {} K {:+.4}: {e}", tt.cct, tt.duv);
                    None
                }
            },
            Base::Picture => adaptation(self.global, tt),
            Base::Neutral => Some(IDENTITY),
        }
    }

    /// A look's white balance as a matrix on the developed picture, an
    /// offset taken from the develop's white: `None` when it is off,
    /// or cannot be resolved, which is the same thing to the finish.
    ///
    /// On a picture an absolute light is read as the offset it is from
    /// the picture's white, which comes to the same adaptation: a
    /// picture has no light of its own to name one against.
    pub fn local(&self, white: &LocalWhite) -> Option<Matrix3> {
        self.to_temp_tint(white.temp_tint(self.global)?)
    }
}

/// The light a picture's white balance names from its own white
/// `own`: itself at As shot, its offset added (held as a mask's offset
/// is, [`LocalWhite::temp_tint`]), and `None` for a kelvin light,
/// which is a raw's and no picture's.
pub fn picture_light(wb: &crate::WhiteBalance, own: TempTint) -> Option<TempTint> {
    match *wb {
        crate::WhiteBalance::AsShot => Some(own),
        crate::WhiteBalance::Offset { mired, tint } => {
            LocalWhite::Relative { mired, tint }.temp_tint(own)
        }
        crate::WhiteBalance::Custom { .. } => None,
    }
}

/// How far a picture's own offset goes either way from its white
/// `own`: the mired and Duv that reach the panel's range of lights
/// ([`TEMPERATURE_RANGE`], [`TINT_RANGE`]) from it, and no further,
/// since the light is held there ([`LocalWhite::temp_tint`]). From D65
/// that is about -346 to +70 mired; from D50 -300 to +117. Each takes
/// in none, for a white past the range. A mask's offset keeps
/// [`OFFSET_MIRED_RANGE`].
pub fn picture_offset_range(own: TempTint) -> ((f64, f64), (f64, f64)) {
    let to = |kelvin: f64| mired(own.cct) - mired(kelvin);
    let m = (
        to(TEMPERATURE_RANGE.0).min(0.0),
        to(TEMPERATURE_RANGE.1).max(0.0),
    );
    let d = (
        (-TINT_RANGE - own.duv).min(0.0),
        (TINT_RANGE - own.duv).max(0.0),
    );
    (m, d)
}

/// The offset from a picture's own white `own` that names `light`, held
/// to the picture's sliders' range ([`picture_offset_range`]): what the
/// panel stores for a picture from the dropper or Auto.
pub fn offset_to(light: TempTint, own: TempTint) -> (f64, f64) {
    let ((m0, m1), (d0, d1)) = picture_offset_range(own);
    let m = (mired(own.cct) - mired(light.cct)).clamp(m0, m1);
    let d = (light.duv - own.duv).clamp(d0, d1);
    (m, d)
}

/// A white balance as it lands on a frame, a picture or a raw: what a
/// preset or the copy sheet lays. An offset applies to either, on a
/// raw as the light it names at the raw's As shot (`as_shot`, read only
/// then), since a raw's edit holds a light and never an offset; a
/// kelvin light applies to a raw alone. `Err` says why the frame keeps
/// its own: a light on a picture, or a raw whose As shot will not say.
pub fn landing(
    wb: &crate::WhiteBalance,
    picture: bool,
    as_shot: impl FnOnce() -> Option<TempTint>,
) -> Result<crate::WhiteBalance, &'static str> {
    use crate::WhiteBalance::{AsShot, Custom, Offset};
    match (wb, picture) {
        (Custom { .. }, true) => Err("a kelvin light is a raw's, and a picture has none"),
        (Offset { mired, tint }, false) => {
            let shot = as_shot().ok_or("the raw's as-shot white could not be read")?;
            match (LocalWhite::Relative {
                mired: *mired,
                tint: *tint,
            })
            .to_absolute(shot)
            {
                LocalWhite::Absolute { temperature, tint } => Ok(Custom { temperature, tint }),
                _ => Err("the offset names no light at the raw's as-shot white"),
            }
        }
        (AsShot | Offset { .. }, true) | (AsShot | Custom { .. }, false) => Ok(wb.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A global for the tests that need one and do not care which.
    const DAYLIGHT: TempTint = TempTint {
        cct: 5500.0,
        duv: 0.0,
    };

    fn profile() -> CameraProfile {
        use greycard_core::CalibrationIlluminant;
        use greycard_core::color::WORKING_SPACE;
        let mat = WORKING_SPACE.from_xyz_matrix().unwrap();
        CameraProfile::dual(
            rawcolor::Calibration::new(CalibrationIlluminant::StandardA, mat),
            rawcolor::Calibration::new(CalibrationIlluminant::D65, mat),
        )
    }

    /// Switched off, a mask's light is kept and written beside `off`;
    /// it comes back through an edit's sidecar, reaches no pixel, and a
    /// sidecar from before, with `off` and nothing beside it or no
    /// field at all, comes back off with nothing kept.
    /// A monochrome raw's shift is the identity to every white: the
    /// global's, a light, an offset.
    #[test]
    fn a_monochrome_raw_s_white_moves_nothing() {
        let global = TempTint {
            cct: 6504.0,
            duv: 0.003,
        };
        let shift = WhiteShift::neutral(global);
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        for local in [
            LocalWhite::Absolute {
                temperature: 3000.0,
                tint: 0.01,
            },
            LocalWhite::Relative {
                mired: 100.0,
                tint: -0.01,
            },
        ] {
            assert_eq!(shift.local(&local), Some(identity));
        }
        assert_eq!(
            shift.to_temp_tint(TempTint {
                cct: 2500.0,
                duv: 0.0
            }),
            Some(identity)
        );
    }

    #[test]
    fn a_switched_off_light_is_kept_through_the_sidecar() {
        let kept = LocalWhite::Kept {
            temperature: 3200.0,
            tint: 0.002,
        };
        let json = serde_json::to_string(&kept).unwrap();
        assert_eq!(json, r#"{"mode":"off","temperature":3200.0,"tint":0.002}"#);
        assert_eq!(serde_json::from_str::<LocalWhite>(&json).unwrap(), kept);
        assert_eq!(
            serde_json::to_string(&LocalWhite::Off).unwrap(),
            r#"{"mode":"off"}"#
        );
        assert_eq!(
            serde_json::from_str::<LocalWhite>(r#"{"mode":"off"}"#).unwrap(),
            LocalWhite::Off
        );
        // Kept, it is no light on the picture.
        assert!(kept.temp_tint(DAYLIGHT).is_none());
        let p = profile();
        let base = white_balance_at(
            &p,
            TempTint {
                cct: 5500.0,
                duv: 0.0,
            },
        )
        .unwrap();
        assert!(WhiteShift::new(p, &base).local(&kept).is_none());
        assert_eq!(kept.acting(), LocalWhite::Off);
        // Through an edit and back.
        let mut edit = crate::Edit::default();
        edit.adjustments.push(crate::Adjustment {
            id: 1,
            ..Default::default()
        });
        edit.adjustments[0].look.white_balance = kept;
        let read = crate::Edit::from_json(&edit.to_json()).unwrap();
        assert_eq!(read.adjustments[0].look.white_balance, kept);
        assert_eq!(read, edit);
        // Off with nothing kept is left out, as it always was.
        edit.adjustments[0].look.white_balance = LocalWhite::Off;
        let look = serde_json::to_value(&edit.adjustments[0].look).unwrap();
        assert!(look.get("white_balance").is_none(), "{look}");
        // A sidecar from before the kept light: the field left out.
        let old = crate::Edit::from_json(
            r#"{"version":4,"adjustments":[{"id":1,"look":{"light":{"exposure":0.5}}}]}"#,
        )
        .unwrap();
        assert_eq!(old.adjustments[0].look.white_balance, LocalWhite::Off);
        assert_eq!(old.adjustments[0].look.light.exposure, 0.5);
        // And a build from before reads the kept light as off: its
        // schema, tagged by mode with off for anything else.
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(tag = "mode", rename_all = "snake_case")]
        #[allow(dead_code)]
        enum Before {
            Absolute {
                temperature: f64,
                tint: f64,
            },
            #[serde(other)]
            Off,
        }
        assert_eq!(serde_json::from_str::<Before>(&json).unwrap(), Before::Off);
    }

    #[test]
    fn off_reads_from_nothing_and_from_a_mode_this_build_does_not_know() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(default)]
            white: LocalWhite,
        }
        let none: Holder = serde_json::from_str("{}").unwrap();
        assert!(none.white.is_off());
        let later: Holder =
            serde_json::from_str(r#"{"white":{"mode":"scene","temperature":300}}"#).unwrap();
        assert!(later.white.is_off());
        let on: Holder = serde_json::from_str(
            r#"{"white":{"mode":"absolute","temperature":3200,"tint":0.002}}"#,
        )
        .unwrap();
        assert_eq!(
            on.white,
            LocalWhite::Absolute {
                temperature: 3200.0,
                tint: 0.002
            }
        );
        let back = serde_json::to_string(&on.white).unwrap();
        assert_eq!(
            back,
            r#"{"mode":"absolute","temperature":3200.0,"tint":0.002}"#
        );
    }

    #[test]
    fn nonsense_is_off_and_the_rest_is_held_to_the_panels_range() {
        for (temperature, tint) in [(f64::NAN, 0.0), (5000.0, f64::INFINITY)] {
            assert!(
                LocalWhite::Absolute { temperature, tint }
                    .temp_tint(DAYLIGHT)
                    .is_none()
            );
        }
        let read = |temperature: f64, tint: f64| {
            let tt = LocalWhite::Absolute { temperature, tint }
                .temp_tint(DAYLIGHT)
                .unwrap();
            (tt.cct, tt.duv)
        };
        assert_eq!(read(1e9, 0.5), (12000.0, 0.05));
        assert_eq!(read(-5.0, -0.5), (2000.0, -0.05));
        assert_eq!(read(0.0, 0.0), (2000.0, 0.0));
        assert_eq!(read(3200.0, -0.004), (3200.0, -0.004));
        // So a sidecar's 1e9 K makes the same matrix as 12000 K.
        let p = profile();
        let base = white_balance_at(
            &p,
            TempTint {
                cct: 5500.0,
                duv: 0.0,
            },
        )
        .unwrap();
        let shift = WhiteShift::new(p, &base);
        let far = shift.local(&LocalWhite::Absolute {
            temperature: 1e9,
            tint: 0.5,
        });
        let edge = shift.local(&LocalWhite::Absolute {
            temperature: 12000.0,
            tint: 0.05,
        });
        assert!(far.is_some());
        assert_eq!(far, edge);
    }

    #[test]
    fn a_field_left_out_keeps_the_rest() {
        let read = |json: &str| serde_json::from_str::<LocalWhite>(json).unwrap();
        assert_eq!(
            read(r#"{"mode":"absolute","temperature":3200}"#),
            LocalWhite::Absolute {
                temperature: 3200.0,
                tint: 0.0
            }
        );
        assert_eq!(
            read(r#"{"mode":"absolute","tint":0.01}"#),
            LocalWhite::Absolute {
                temperature: DEFAULT_TEMPERATURE,
                tint: 0.01
            }
        );
        // And the edit around it survives.
        let edit = crate::Edit::from_json(
            r#"{"version":4,"light":{"exposure":0.5},"adjustments":[{"id":1,"look":{"white_balance":{"mode":"absolute","temperature":3200}}}]}"#,
        )
        .unwrap();
        assert_eq!(edit.light.exposure, 0.5);
        assert_eq!(
            edit.adjustments[0].look.white_balance,
            LocalWhite::Absolute {
                temperature: 3200.0,
                tint: 0.0
            }
        );
    }

    #[test]
    fn the_global_white_as_a_local_is_nothing() {
        let p = profile();
        let tt = TempTint {
            cct: 5000.0,
            duv: 0.003,
        };
        let base = white_balance_at(&p, tt).unwrap();
        let shift = WhiteShift::new(p, &base);
        let m = shift
            .local(&LocalWhite::Absolute {
                temperature: 5000.0,
                tint: 0.003,
            })
            .unwrap();
        for (r, row) in m.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                assert!((v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-5, "{m:?}");
            }
        }
        assert!(shift.local(&LocalWhite::Off).is_none());
        // Warmer light asks for a bluer picture: a grey under a 3200 K
        // white, developed at 5000 K, comes out with more blue than red.
        let m = shift
            .local(&LocalWhite::Absolute {
                temperature: 3200.0,
                tint: 0.003,
            })
            .unwrap();
        let g = greycard_core::color::apply3(&m, [0.18; 3]);
        assert!(g[2] > g[0], "{g:?}");
    }
}

#[cfg(test)]
mod relative {
    use super::*;

    // Tints dyadic, so their sums and differences are exact in binary
    // and the round trips can be held to equality; the temperature
    // goes through a reciprocal and back, so to a billionth of a
    // kelvin.
    const GLOBAL: TempTint = TempTint {
        cct: 4800.0,
        duv: 0.00390625,
    };

    fn near(a: TempTint, b: TempTint) -> bool {
        (a.cct - b.cct).abs() < 1e-9 && a.duv == b.duv
    }

    /// Follow global on turns the light into its offset from the
    /// global, in mired and Duv, off turns it back, on or kept, at the
    /// global it was taken from; and the light each names there is
    /// the same.
    #[test]
    fn an_offset_is_the_light_less_the_global_both_ways() {
        let lamp = LocalWhite::Absolute {
            temperature: 3200.0,
            tint: -0.001953125,
        };
        let offset = lamp.to_relative(GLOBAL);
        let LocalWhite::Relative { mired: by, tint } = offset else {
            panic!("{offset:?}")
        };
        // 208⅓ mired at 4800 K, 312½ at 3200 K: the lamp is warmer
        // light, toward the lower kelvin, so the offset is negative.
        assert!((by - (1e6 / 4800.0 - 1e6 / 3200.0)).abs() < 1e-12, "{by}");
        assert!(by < 0.0);
        assert_eq!(tint, -0.005859375);
        let LocalWhite::Absolute { temperature, tint } = offset.to_absolute(GLOBAL) else {
            panic!()
        };
        assert!((temperature - 3200.0).abs() < 1e-9 && tint == -0.001953125);
        assert!(near(
            offset.temp_tint(GLOBAL).unwrap(),
            lamp.temp_tint(GLOBAL).unwrap()
        ));
        let kept = LocalWhite::Kept {
            temperature: 6100.0,
            tint: 0.009765625,
        };
        let kept_offset = kept.to_relative(GLOBAL);
        assert!(matches!(kept_offset, LocalWhite::KeptRelative { .. }));
        assert!(kept_offset.is_relative() && !kept.is_relative());
        let LocalWhite::Kept { temperature, .. } = kept_offset.to_absolute(GLOBAL) else {
            panic!()
        };
        assert!((temperature - 6100.0).abs() < 1e-9);
        assert!(kept_offset.temp_tint(GLOBAL).is_none());
        assert_eq!(kept_offset.acting(), LocalWhite::Off);
        // Each is its own already, and off is off either way.
        assert_eq!(lamp.to_absolute(GLOBAL), lamp);
        assert_eq!(offset.to_relative(GLOBAL), offset);
        assert_eq!(LocalWhite::Off.to_relative(GLOBAL), LocalWhite::Off);
        // Any light in the panel's range is an offset from any global
        // in it, so the far corners come back too.
        for (global, light) in [(12000.0, 2000.0), (2000.0, 12000.0)] {
            let global = TempTint {
                cct: global,
                duv: 0.05,
            };
            let far = LocalWhite::Absolute {
                temperature: light,
                tint: -0.05,
            };
            let off = far.to_relative(global);
            assert_eq!(off.clamped(), off, "inside the offset's range");
            let LocalWhite::Absolute { temperature, tint } = off.to_absolute(global) else {
                panic!()
            };
            assert!((temperature - light).abs() < 1e-6 && tint == -0.05);
        }
    }

    /// The global moves, an offset keeps its warmth in mired and an
    /// absolute light stays put; held to the panel's range either way.
    #[test]
    fn an_offset_follows_the_global_and_a_light_does_not() {
        let warmer = LocalWhite::Relative {
            mired: 25.0,
            tint: 0.001,
        };
        let lamp = LocalWhite::Absolute {
            temperature: 3200.0,
            tint: 0.0,
        };
        for cct in [3000.0, 5500.0, 7000.0] {
            let global = TempTint { cct, duv: 0.002 };
            let tt = warmer.temp_tint(global).unwrap();
            // 25 mired fewer, toward the higher kelvin, wherever it is.
            assert!((mired(cct) - mired(tt.cct) - 25.0).abs() < 1e-9, "{tt:?}");
            assert!(tt.cct > cct);
            assert_eq!(tt.duv, 0.002 + 0.001);
            let tt = lamp.temp_tint(global).unwrap();
            assert_eq!((tt.cct, tt.duv), (3200.0, 0.0));
        }
        let tt = warmer
            .temp_tint(TempTint {
                cct: 11800.0,
                duv: 0.05,
            })
            .unwrap();
        assert_eq!((tt.cct, tt.duv), (12000.0, 0.05));
        // Past every kelvin at all: the top of the range.
        let far = LocalWhite::Relative {
            mired: 450.0,
            tint: 0.0,
        };
        assert_eq!(far.temp_tint(GLOBAL).unwrap().cct, 12000.0);
        let warm = LocalWhite::Relative {
            mired: -450.0,
            tint: 0.0,
        };
        assert_eq!(warm.temp_tint(GLOBAL).unwrap().cct, 2000.0);
        // A global past the range, as an As shot can resolve: no offset
        // is that global exactly, and an offset away from the range
        // goes no further, one toward it moves.
        let past = TempTint {
            cct: 5200.0,
            duv: 0.062,
        };
        let none = LocalWhite::Relative {
            mired: 0.0,
            tint: 0.0,
        };
        assert_eq!(none.temp_tint(past), Some(past));
        assert_eq!(none.temp_tint(GLOBAL), Some(GLOBAL));
        let greener = LocalWhite::Relative {
            mired: 0.0,
            tint: 0.01,
        };
        assert_eq!(greener.temp_tint(past).unwrap().duv, 0.062);
        let magenta = LocalWhite::Relative {
            mired: 0.0,
            tint: -0.02,
        };
        assert_eq!(magenta.temp_tint(past).unwrap().duv, 0.062 - 0.02);
        let hot = TempTint {
            cct: 13000.0,
            duv: 0.0,
        };
        assert_eq!(none.temp_tint(hot).unwrap().cct, 13000.0);
        assert_eq!(far.temp_tint(hot).unwrap().cct, 13000.0);
    }

    /// A mask whose white follows the global develops to the gains an
    /// absolute mask at the light its offset names does, through the
    /// same resolve, and so to the same matrix.
    #[test]
    fn an_offset_develops_as_the_light_it_names() {
        use greycard_core::CalibrationIlluminant;
        use greycard_core::color::WORKING_SPACE;
        let mat = WORKING_SPACE.from_xyz_matrix().unwrap();
        let p = CameraProfile::dual(
            rawcolor::Calibration::new(CalibrationIlluminant::StandardA, mat),
            rawcolor::Calibration::new(CalibrationIlluminant::D65, mat),
        );
        let base = white_balance_at(&p, GLOBAL).unwrap();
        let shift = WhiteShift::new(p.clone(), &base);
        let offset = LocalWhite::Relative {
            mired: -60.0,
            tint: 0.002,
        };
        let at = TempTint {
            cct: 1e6 / (1e6 / shift.global.cct + 60.0),
            duv: shift.global.duv + 0.002,
        };
        let named = white_balance_at(&p, offset.temp_tint(shift.global).unwrap()).unwrap();
        let wanted = white_balance_at(&p, at).unwrap();
        assert_eq!(named.coefficients_f32(), wanted.coefficients_f32());
        let absolute = LocalWhite::Absolute {
            temperature: at.cct,
            tint: at.duv,
        };
        assert!(shift.local(&offset).is_some());
        assert_eq!(shift.local(&offset), shift.local(&absolute));
        // No offset at all is the develop's own white: nothing moves.
        let none = shift
            .local(&LocalWhite::Relative {
                mired: 0.0,
                tint: 0.0,
            })
            .unwrap();
        for (r, row) in none.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                assert!(
                    (v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-5,
                    "{none:?}"
                );
            }
        }
    }

    /// A copy of the masks, through a preset's file, carries the offset
    /// as an offset: laid on another picture, the mask follows that
    /// picture's global.
    #[test]
    fn a_copied_mask_keeps_its_offset() {
        let mut edit = crate::Edit::default();
        edit.adjustments.push(crate::Adjustment {
            id: 4,
            ..Default::default()
        });
        let warmer = LocalWhite::Relative {
            mired: 25.0,
            tint: 0.0,
        };
        edit.adjustments[0].look.white_balance = warmer;
        let preset = crate::Preset::from_edit("Warm", &edit, &[crate::Section::Adjustments]);
        let preset: crate::Preset =
            serde_json::from_str(&serde_json::to_string(&preset).unwrap()).unwrap();
        let mut onto = crate::Edit::default();
        preset.apply(&mut onto);
        assert_eq!(onto.adjustments[0].look.white_balance, warmer);
    }

    /// Written as its own mode with its offset named `mired`, on or
    /// kept; a sidecar from before, with `absolute`, still reads as a
    /// light named; and a build from before reads an offset as off,
    /// never as a light.
    #[test]
    fn an_offset_is_its_own_mode_in_the_sidecar() {
        let on = LocalWhite::Relative {
            mired: 25.0,
            tint: -0.002,
        };
        let json = serde_json::to_string(&on).unwrap();
        assert_eq!(json, r#"{"mode":"relative","mired":25.0,"tint":-0.002}"#);
        assert_eq!(serde_json::from_str::<LocalWhite>(&json).unwrap(), on);
        let kept = LocalWhite::KeptRelative {
            mired: -40.0,
            tint: 0.0,
        };
        let kept_json = serde_json::to_string(&kept).unwrap();
        assert_eq!(
            kept_json,
            r#"{"mode":"kept_relative","mired":-40.0,"tint":0.0}"#
        );
        assert_eq!(
            serde_json::from_str::<LocalWhite>(&kept_json).unwrap(),
            kept
        );
        // An offset left out is none; a temperature is not one.
        assert_eq!(
            serde_json::from_str::<LocalWhite>(r#"{"mode":"relative","temperature":800}"#).unwrap(),
            LocalWhite::Relative {
                mired: 0.0,
                tint: 0.0
            }
        );
        // A sidecar from before the offset: absolute, as it was.
        let old = crate::Edit::from_json(
            r#"{"version":6,"adjustments":[{"id":1,"look":{"white_balance":{"mode":"absolute","temperature":3200.0,"tint":0.002}}}]}"#,
        )
        .unwrap();
        assert_eq!(
            old.adjustments[0].look.white_balance,
            LocalWhite::Absolute {
                temperature: 3200.0,
                tint: 0.002
            }
        );
        // Through an edit and back, at the version it always was.
        let mut edit = crate::Edit::default();
        edit.adjustments.push(crate::Adjustment {
            id: 1,
            ..Default::default()
        });
        edit.adjustments[0].look.white_balance = on;
        let read = crate::Edit::from_json(&edit.to_json()).unwrap();
        assert_eq!(read, edit);
        assert_eq!(read.version, crate::VERSION);
        // A build from before reads either as off: its schema, the
        // mode with off for anything it does not know.
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Before {
            Absolute,
            #[serde(other)]
            Off,
        }
        #[derive(Deserialize)]
        struct BeforeWire {
            mode: Before,
        }
        for json in [&json, &kept_json] {
            let read: BeforeWire = serde_json::from_str(json).unwrap();
            assert_eq!(read.mode, Before::Off, "{json}");
        }
    }
}

#[cfg(test)]
mod picture {
    use super::*;
    use crate::WhiteBalance as Edited;
    use rawcolor::cct::temp_tint_to_chromaticity;

    type M = [[f64; 3]; 3];

    fn mul(a: M, b: M) -> M {
        std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
    }

    fn apply(m: M, v: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|r| (0..3).map(|k| m[r][k] * v[k]).sum())
    }

    fn inverse(m: M) -> M {
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        let c = |r: usize, k: usize| {
            let (r0, r1) = ((r + 1) % 3, (r + 2) % 3);
            let (k0, k1) = ((k + 1) % 3, (k + 2) % 3);
            m[r0][k0] * m[r1][k1] - m[r0][k1] * m[r1][k0]
        };
        std::array::from_fn(|r| std::array::from_fn(|k| c(k, r) / det))
    }

    /// Bradford's cone matrix and Rec.2020's to XYZ, written out here
    /// from the standards rather than taken from the code under test.
    const BRADFORD: M = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    const REC2020: M = [
        [0.636958, 0.144617, 0.168881],
        [0.262700, 0.677998, 0.059302],
        [0.000000, 0.028073, 1.060985],
    ];

    fn xyz(tt: TempTint) -> [f64; 3] {
        let c = temp_tint_to_chromaticity(tt).unwrap();
        [c.x / c.y, 1.0, (1.0 - c.x - c.y) / c.y]
    }

    /// The working-space matrix from a picture shown at `from` to one
    /// shown at `to`, by hand.
    fn by_hand(from: TempTint, to: TempTint) -> M {
        let (a, b) = (apply(BRADFORD, xyz(from)), apply(BRADFORD, xyz(to)));
        let mut d = [[0.0; 3]; 3];
        for i in 0..3 {
            d[i][i] = a[i] / b[i];
        }
        let cat = mul(inverse(BRADFORD), mul(d, BRADFORD));
        mul(inverse(REC2020), mul(cat, REC2020))
    }

    const D65: TempTint = TempTint {
        cct: 6504.0,
        duv: 0.0032,
    };

    /// An offset's matrix at a few lights, against Bradford by hand:
    /// to a part in ten thousand, the standards' rounding.
    #[test]
    fn an_offset_is_bradford_between_the_two_lights() {
        for (from, by, tint) in [
            (D65, 25.0, 0.0),
            (D65, -60.0, 0.004),
            (TempTint::new(5003.0, 0.0033), 40.0, -0.01),
            (TempTint::new(3200.0, 0.0), -100.0, 0.0),
        ] {
            let to = picture_light(&Edited::Offset { mired: by, tint }, from).unwrap();
            assert!((mired(from.cct) - mired(to.cct) - by).abs() < 1e-9);
            let got = adaptation(from, to).unwrap();
            let want = by_hand(from, to);
            for r in 0..3 {
                for c in 0..3 {
                    assert!(
                        (got[r][c] as f64 - want[r][c]).abs() < 1e-4,
                        "{from:?} {by} {tint}: {got:?} against {want:?}"
                    );
                }
            }
        }
    }

    /// A warmer offset warms a grey: red up, blue down, by the amount
    /// the adaptation says; no offset is no light but the picture's
    /// own, and As shot is that.
    #[test]
    fn a_warmer_offset_warms_a_grey() {
        let to = picture_light(
            &Edited::Offset {
                mired: 25.0,
                tint: 0.0,
            },
            D65,
        )
        .unwrap();
        assert!(to.cct > D65.cct);
        let m = adaptation(D65, to).unwrap();
        let grey = greycard_core::color::apply3(&m, [0.18; 3]);
        assert!(grey[0] > 0.18 && grey[2] < 0.18, "{grey:?}");
        let want = apply(by_hand(D65, to), [0.18; 3]);
        for i in 0..3 {
            assert!((grey[i] as f64 - want[i]).abs() < 1e-5, "{grey:?} {want:?}");
        }
        assert_eq!(picture_light(&Edited::AsShot, D65), Some(D65));
        assert_eq!(
            picture_light(
                &Edited::Offset {
                    mired: 0.0,
                    tint: 0.0
                },
                D65
            ),
            Some(D65)
        );
        assert_eq!(
            picture_light(
                &Edited::Custom {
                    temperature: 3200.0,
                    tint: 0.0
                },
                D65
            ),
            None
        );
        // The light to itself moves nothing.
        let same = adaptation(D65, D65).unwrap();
        for (r, row) in same.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                assert!(
                    (v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-6,
                    "{same:?}"
                );
            }
        }
    }

    /// The dropper's light makes the pixel grey, from wherever the
    /// picture is shown; a pixel with a channel at nothing names none.
    #[test]
    fn the_neutral_light_makes_the_pick_grey() {
        for shown in [D65, TempTint::new(7400.0, -0.002)] {
            for px in [[0.30f32, 0.25, 0.18], [0.12, 0.15, 0.2]] {
                let light = picture_neutral(px, shown).unwrap();
                let out = greycard_core::color::apply3(&adaptation(shown, light).unwrap(), px);
                let mean = (out[0] + out[1] + out[2]) / 3.0;
                for v in out {
                    assert!((v - mean).abs() < 1e-3 * mean, "{out:?}");
                }
            }
        }
        assert!(picture_neutral([0.0, 0.0, 0.0], D65).is_none());
        // Past the range the light reaches, the dropper's offset is held
        // to where it still does something.
        let ((m0, m1), (d0, d1)) = picture_offset_range(D65);
        assert!((m0 - (1e6 / 6504.0 - 500.0)).abs() < 1e-9 && m0 < -340.0);
        assert!((m1 - (1e6 / 6504.0 - 1e6 / 12000.0)).abs() < 1e-9 && m1 < 71.0);
        assert!((d0 + 0.0532).abs() < 1e-9 && (d1 - 0.0468).abs() < 1e-9);
        let blue = offset_to(TempTint::new(20000.0, 0.0), D65);
        assert_eq!(blue.0, m1);
        let d50 = picture_offset_range(TempTint::new(5003.0, 0.0033));
        assert!(d50.0.1 > m1, "a D50 picture reaches further toward blue");
        // At the range's ends the light is the range's own.
        let edge = picture_light(
            &Edited::Offset {
                mired: m1,
                tint: 0.0,
            },
            D65,
        )
        .unwrap();
        assert!((edge.cct - 12000.0).abs() < 1e-6);
        let (m, d) = offset_to(TempTint::new(1e6 / (1e6 / 6504.0 - 30.0), 0.0052), D65);
        assert!((m - 30.0).abs() < 1e-9 && (d - 0.002).abs() < 1e-12);
    }

    /// A mask's offset on a picture is from the picture's light, and
    /// two adaptations make the one between their ends: the global's
    /// then the mask's is the picture's own straight to the mask's.
    #[test]
    fn a_masks_offset_adds_to_the_pictures() {
        let global = picture_light(
            &Edited::Offset {
                mired: 20.0,
                tint: 0.002,
            },
            D65,
        )
        .unwrap();
        let shift = WhiteShift::picture(global);
        let mask = LocalWhite::Relative {
            mired: 15.0,
            tint: 0.0,
        };
        let local = shift.local(&mask).unwrap();
        let first = adaptation(D65, global).unwrap();
        let both = greycard_core::color::mul3(local, first);
        let target = mask.temp_tint(global).unwrap();
        let straight = adaptation(D65, target).unwrap();
        for r in 0..3 {
            for c in 0..3 {
                assert!(
                    (both[r][c] - straight[r][c]).abs() < 1e-5,
                    "{both:?} {straight:?}"
                );
            }
        }
        // An absolute light is the offset it is from the picture's.
        let lamp = LocalWhite::Absolute {
            temperature: target.cct,
            tint: target.duv,
        };
        assert_eq!(shift.local(&lamp), shift.local(&mask));
    }

    /// The camera profile path and the XYZ path agree on a raw: a
    /// develop at its As shot, moved by an offset, through the profile
    /// (gains and the matrices interpolated at the light) and through
    /// Bradford alone. A tint alone they make to the same grey, the
    /// profile's matrix being the same at the same kelvin; a move in
    /// kelvin reads the profile's matrix at another light, whose
    /// rendering of the old white is not quite Bradford's. Measured on
    /// an ordinary sensor's two calibrations, the grey each makes is
    /// within 0.0047 in CIE 1960 uv over offsets of ±60 mired (0.0017
    /// at +25 mired; a just noticeable difference is about 0.004).
    #[test]
    fn a_raws_offset_through_the_profile_and_through_xyz_agree() {
        use greycard_core::CalibrationIlluminant;
        let mat = |m: [f64; 9]| {
            rawcolor::Mat3::new([[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]])
        };
        let p = CameraProfile::dual(
            rawcolor::Calibration::new(
                CalibrationIlluminant::StandardA,
                mat([
                    1.1564, -0.4626, -0.1081, -0.4229, 1.2260, 0.2187, -0.0711, 0.1500, 0.6268,
                ]),
            ),
            rawcolor::Calibration::new(
                CalibrationIlluminant::D65,
                mat([
                    0.9766, -0.2953, -0.1254, -0.4276, 1.2116, 0.2433, -0.0437, 0.1336, 0.5131,
                ]),
            ),
        );
        let uv = |rgb: [f32; 3]| {
            let x = apply(REC2020, rgb.map(f64::from));
            let s = x[0] + 15.0 * x[1] + 3.0 * x[2];
            (4.0 * x[0] / s, 6.0 * x[1] / s)
        };
        let mut worst: f64 = 0.0;
        for shot in [TempTint::new(5200.0, 0.002), TempTint::new(3300.0, -0.001)] {
            let base = white_balance_at(&p, shot).unwrap();
            let raw = WhiteShift::new(p.clone(), &base);
            let xyz = WhiteShift::picture(base.temp_tint);
            for (mired, tint) in [(25.0, 0.0), (-60.0, 0.0), (60.0, 0.004), (0.0, -0.004)] {
                let off = LocalWhite::Relative { mired, tint };
                let a = greycard_core::color::apply3(&raw.local(&off).unwrap(), [0.18; 3]);
                let b = greycard_core::color::apply3(&xyz.local(&off).unwrap(), [0.18; 3]);
                let (ua, va) = uv(a);
                let (ub, vb) = uv(b);
                let d = ((ua - ub).powi(2) + (va - vb).powi(2)).sqrt();
                worst = worst.max(d);
            }
        }
        eprintln!("profile path against XYZ path, worst grey: {worst:.5} uv");
        assert!(worst < 0.006, "{worst}");
    }

    /// A white balance landing on a frame: an offset onto a picture as
    /// it is and onto a raw as the light it names at the raw's As shot;
    /// a light onto a raw as it is and onto a picture not at all.
    #[test]
    fn what_lands_on_a_picture_and_on_a_raw() {
        let offset = Edited::Offset {
            mired: 25.0,
            tint: 0.002,
        };
        let light = Edited::Custom {
            temperature: 3200.0,
            tint: 0.0,
        };
        let never = || -> Option<TempTint> { panic!("a picture's As shot is never read") };
        assert_eq!(landing(&offset, true, never), Ok(offset.clone()));
        assert!(landing(&light, true, never).is_err());
        assert_eq!(landing(&Edited::AsShot, true, never), Ok(Edited::AsShot));
        let shot = TempTint::new(5000.0, 0.001);
        let Ok(Edited::Custom { temperature, tint }) = landing(&offset, false, || Some(shot))
        else {
            panic!()
        };
        assert!((mired(5000.0) - mired(temperature) - 25.0).abs() < 1e-9);
        assert!((tint - 0.003).abs() < 1e-12);
        assert!(landing(&offset, false, || None).is_err());
        assert_eq!(landing(&light, false, || None), Ok(light));
    }
}
