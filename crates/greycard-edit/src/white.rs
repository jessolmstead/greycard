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
/// Tagged by `mode`, so a way of saying it that a later build adds (a
/// shift relative to the global, say) reads here as off rather than
/// failing the sidecar, and a sidecar from before any of it, with no
/// field at all, reads as off too.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
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
    Absolute {
        #[serde(default = "default_temperature")]
        temperature: f64,
        #[serde(default)]
        tint: f64,
    },
    /// No white balance of its own: the develop's. Last, as serde
    /// wants the variant an unknown mode reads as.
    #[default]
    #[serde(other)]
    Off,
}

/// The temperatures a mask's white balance takes, kelvin: the panel's
/// Temperature slider's ends.
pub const TEMPERATURE_RANGE: (f64, f64) = (2000.0, 12000.0);
/// How far its tint goes either way, Duv: the panel's Tint slider's.
pub const TINT_RANGE: f64 = 0.05;
/// A temperature a sidecar left out: the panel's slider's default.
pub const DEFAULT_TEMPERATURE: f64 = 5500.0;

fn default_temperature() -> f64 {
    DEFAULT_TEMPERATURE
}

impl LocalWhite {
    pub fn is_off(&self) -> bool {
        *self == Self::Off
    }

    /// Brought into the panel's range: a temperature from 2000 to
    /// 12000 K and a tint of 0.05 either way. Past them the matrix
    /// after the develop extrapolates the camera's calibrations, and a
    /// hand-edited 1e9 K or a tint of 0.5 comes out as a color no
    /// light makes. A value that is not a number is off. The global
    /// white balance is not held this way; a mask's is read only
    /// through here.
    pub fn clamped(self) -> Self {
        match self {
            Self::Absolute { temperature, tint } if temperature.is_finite() && tint.is_finite() => {
                Self::Absolute {
                    temperature: temperature.clamp(TEMPERATURE_RANGE.0, TEMPERATURE_RANGE.1),
                    tint: tint.clamp(-TINT_RANGE, TINT_RANGE),
                }
            }
            _ => Self::Off,
        }
    }

    /// The temperature and tint it asks for, when on, held to the
    /// panel's range ([`LocalWhite::clamped`]).
    pub fn temp_tint(&self) -> Option<TempTint> {
        match self.clamped() {
            Self::Off => None,
            Self::Absolute { temperature, tint } => Some(TempTint {
                cct: temperature,
                duv: tint,
            }),
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
}

impl WhiteShift {
    /// From the develop's resolved white balance.
    pub fn new(profile: CameraProfile, base: &WhiteBalance) -> Self {
        Self {
            profile,
            gains: base.coefficients_f32(),
            rows: matrix_rows(base),
        }
    }

    /// From the develop's gains and matrix as already taken apart.
    pub fn from_parts(profile: CameraProfile, gains: [f32; 3], rows: Matrix3) -> Self {
        Self {
            profile,
            gains,
            rows,
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

    /// A look's white balance as a matrix on the developed picture:
    /// `None` when it is off, or cannot be resolved, which is the same
    /// thing to the finish.
    pub fn local(&self, white: &LocalWhite) -> Option<Matrix3> {
        self.to_temp_tint(white.temp_tint()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> CameraProfile {
        use greycard_core::CalibrationIlluminant;
        use greycard_core::color::WORKING_SPACE;
        let mat = WORKING_SPACE.from_xyz_matrix().unwrap();
        CameraProfile::dual(
            rawcolor::Calibration::new(CalibrationIlluminant::StandardA, mat),
            rawcolor::Calibration::new(CalibrationIlluminant::D65, mat),
        )
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
            serde_json::from_str(r#"{"white":{"mode":"relative","temperature":300}}"#).unwrap();
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
                    .temp_tint()
                    .is_none()
            );
        }
        let read = |temperature: f64, tint: f64| {
            let tt = LocalWhite::Absolute { temperature, tint }
                .temp_tint()
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
