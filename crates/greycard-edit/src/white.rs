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

use greycard_core::color::{Matrix3, matrix_rows, white_balance_at, white_shift};
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

/// The develop's white balance and the camera profile it was resolved
/// through: what a local white balance's matrix is made from.
#[derive(Debug, Clone)]
pub struct WhiteShift {
    pub profile: CameraProfile,
    /// The develop's gains, camera space, green at one.
    pub gains: [f32; 3],
    /// The develop's camera to working matrix, rows.
    pub rows: Matrix3,
    /// The develop's white as a temperature and tint, As shot resolved
    /// to what the camera's gains mean: what a mask's offset is from.
    pub global: TempTint,
}

impl WhiteShift {
    /// From the develop's resolved white balance.
    pub fn new(profile: CameraProfile, base: &WhiteBalance) -> Self {
        Self {
            profile,
            gains: base.coefficients_f32(),
            rows: matrix_rows(base),
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
            profile,
            gains,
            rows,
            global,
        }
    }

    /// The matrix to a resolved white balance.
    pub fn to(&self, target: &WhiteBalance) -> Option<Matrix3> {
        white_shift(
            self.gains,
            self.rows,
            target.coefficients_f32(),
            matrix_rows(target),
        )
    }

    /// The matrix to a temperature and tint.
    pub fn to_temp_tint(&self, tt: TempTint) -> Option<Matrix3> {
        match white_balance_at(&self.profile, tt) {
            Ok(wb) => self.to(&wb),
            Err(e) => {
                log::debug!("white balance {} K {:+.4}: {e}", tt.cct, tt.duv);
                None
            }
        }
    }

    /// A look's white balance as a matrix on the developed picture, an
    /// offset taken from the develop's white: `None` when it is off,
    /// or cannot be resolved, which is the same thing to the finish.
    pub fn local(&self, white: &LocalWhite) -> Option<Matrix3> {
        self.to_temp_tint(white.temp_tint(self.global)?)
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
