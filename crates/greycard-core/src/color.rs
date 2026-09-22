//! From the calibration a file carries to a usable camera profile and a
//! white balance.
//!
//! The color maths lives in `rawcolor`; this module only decides which
//! calibrations to use and how to ask.

use std::sync::Arc;

use rawcolor::{
    Calibration as ProfileCalibration, CalibrationIlluminant, CameraProfile, Cat, Mat3, RgbSpace,
    TempTint, Vec3, WhiteBalance,
};

use crate::dcp::{EmbedPolicy, HueSatMap, HueSatMaps};
use crate::error::{Error, Result};
use crate::raw::RawFrame;

/// The engine's working space: linear Rec.2020, D65.
///
/// Real primaries (per-channel operations behave predictably), wide enough
/// for every camera gamut that matters, and the same white as sRGB and
/// Display P3 so display transforms need no adaptation.
pub const WORKING_SPACE: RgbSpace = RgbSpace::REC2020;

/// Chromatic adaptation transform used everywhere. Bradford is what DNG,
/// ICC v4 and Adobe use, so results line up with existing profiles.
pub const CAT: Cat = Cat::Bradford;

/// How to choose the scene white.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhitePoint {
    /// The white balance the camera recorded.
    AsShot,
    /// A correlated color temperature and tint.
    TempTint(TempTint),
    /// Explicit camera-space gains, RGB, green normalized to 1.
    Coefficients([f64; 3]),
}

/// Build the profile for a frame from the calibrations it carries.
///
/// Uses the widest pair by temperature when there are several, and the one
/// there is otherwise. Calibrations with an unknown illuminant or a matrix
/// that is not an invertible 3x3 are ignored.
pub fn profile_from_frame(frame: &RawFrame) -> Result<CameraProfile> {
    let mut calibrations: Vec<(f64, ProfileCalibration)> = frame
        .calibrations
        .iter()
        .filter_map(|c| {
            let illuminant = CalibrationIlluminant::from_exif_code(c.illuminant)?;
            let color_matrix = to_mat3(&c.color_matrix)?;
            let mut calibration = ProfileCalibration::new(illuminant, color_matrix);
            if let Some(forward) = c.forward_matrix.as_deref().and_then(to_mat3) {
                calibration = calibration.with_forward_matrix(forward);
            }
            Some((illuminant.cct(), calibration))
        })
        .collect();

    if calibrations.is_empty() {
        return Err(Error::NoCalibration);
    }
    calibrations.sort_by(|a, b| a.0.total_cmp(&b.0));

    if calibrations.len() == 1 {
        return Ok(CameraProfile::single(calibrations.remove(0).1));
    }
    let cool = calibrations.pop().expect("at least two calibrations").1;
    let warm = calibrations.remove(0).1;
    Ok(CameraProfile::dual(warm, cool))
}

/// The camera's color as the engine develops with it: the matrix pair
/// rawcolor solves the white balance from, and the hue/saturation/value
/// map a DCP adds after it.
///
/// `rawcolor::CameraProfile` is a published crate of its own and knows
/// nothing of DNG's profile tables, so the map hangs here, beside it,
/// rather than on it; everything else about the camera's color is still
/// rawcolor's. The look table and the tone curve a DCP may carry are
/// not here at all: they are a look, and stay on [`crate::dcp::Dcp`].
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub camera: CameraProfile,
    /// What to call it in the panel; `None` for the file's own.
    pub name: Option<String>,
    /// The camera a DCP was made for, as it names it.
    pub unique_camera_model: Option<String>,
    pub copyright: Option<String>,
    pub embed_policy: EmbedPolicy,
    /// `BaselineExposureOffset` in stops: read and reported, applied by
    /// nothing. The engine is scene referred and leaves exposure to the
    /// consumer's tone mapping; a profile that wants the picture a third
    /// of a stop brighter is saying something about Adobe's rendering,
    /// not about the sensor.
    pub baseline_exposure_offset: Option<f32>,
    /// The hue/saturation/value map per illuminant, when a DCP brought
    /// one.
    pub maps: Option<Arc<HueSatMaps>>,
}

impl Profile {
    /// The profile the file itself carries: its calibrations, no map.
    pub fn from_frame(frame: &RawFrame) -> Result<Self> {
        Ok(Self::embedded(profile_from_frame(frame)?))
    }

    /// A profile with no map, from calibrations already resolved.
    pub fn embedded(camera: CameraProfile) -> Self {
        Profile {
            camera,
            name: None,
            unique_camera_model: None,
            copyright: None,
            embed_policy: EmbedPolicy::default(),
            baseline_exposure_offset: None,
            maps: None,
        }
    }

    /// The map to apply at a scene temperature: the two illuminants'
    /// blended by the weight their matrices are blended with, or the
    /// single one the profile has.
    pub fn map_at_cct(&self, cct: f64) -> Result<Option<HueSatMap>> {
        let Some(maps) = &self.maps else {
            return Ok(None);
        };
        match &maps.secondary {
            None => Ok(Some(maps.primary.clone())),
            Some(secondary) => {
                let weight = primary_weight(&self.camera, cct) as f32;
                maps.primary.blend(secondary, weight).map(Some)
            }
        }
    }
}

/// How much of the primary (warmer) calibration a scene temperature
/// asks for: DNG's linear blend in reciprocal temperature, 1 at or
/// below the warm illuminant and 0 at or above the cool one.
///
/// rawcolor keeps this to itself for the matrices; the map has to be
/// blended by the same number, so the same three lines are here. If
/// rawcolor ever exposes it, this goes.
pub fn primary_weight(profile: &CameraProfile, cct: f64) -> f64 {
    let Some(secondary) = &profile.secondary else {
        return 1.0;
    };
    let t1 = profile.primary.illuminant.cct();
    let t2 = secondary.illuminant.cct();
    if (t1 - t2).abs() < 1e-6 || cct <= t1 {
        return 1.0;
    }
    if cct >= t2 {
        return 0.0;
    }
    ((1.0 / cct) - (1.0 / t2)) / ((1.0 / t1) - (1.0 / t2))
}

/// Resolve a white point against a frame and its profile.
pub fn resolve_white_balance(
    frame: &RawFrame,
    profile: &CameraProfile,
    white_point: WhitePoint,
) -> Result<WhiteBalance> {
    let wb = match white_point {
        WhitePoint::AsShot => {
            let c = frame.as_shot_coefficients.ok_or_else(|| {
                Error::Unsupported("file records no as-shot white balance".into())
            })?;
            WhiteBalance::from_coefficients(
                profile,
                Vec3::new(c[0], c[1], c[2]),
                &WORKING_SPACE,
                CAT,
            )?
        }
        WhitePoint::Coefficients(c) => WhiteBalance::from_coefficients(
            profile,
            Vec3::new(c[0], c[1], c[2]),
            &WORKING_SPACE,
            CAT,
        )?,
        WhitePoint::TempTint(tt) => WhiteBalance::from_temp_tint(profile, tt, &WORKING_SPACE, CAT)?,
    };
    Ok(wb)
}

/// The illuminant the camera's as-shot white balance implies.
pub fn as_shot_temp_tint(frame: &RawFrame, profile: &CameraProfile) -> Result<TempTint> {
    resolve_white_balance(frame, profile, WhitePoint::AsShot).map(|wb| wb.temp_tint)
}

/// Reshape a flat row-major matrix into an invertible 3x3.
///
/// Four-color sensors carry 4x3 matrices; those need a transform rawcolor
/// does not model, so they come back `None`.
fn to_mat3(flat: &[f32]) -> Option<Mat3> {
    if flat.len() != 9 {
        return None;
    }
    let mut rows = [[0.0f64; 3]; 3];
    for (r, row) in rows.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = flat[r * 3 + c] as f64;
        }
    }
    let m = Mat3::new(rows);
    m.inverse().map(|_| m)
}

/// Björn Ottosson's linear sRGB to Oklab's LMS, rows.
pub const SRGB_TO_LMS: [[f32; 3]; 3] = [
    [0.412_221_47, 0.536_332_54, 0.051_445_99],
    [0.211_903_5, 0.680_699_5, 0.107_396_96],
    [0.088_302_46, 0.281_718_84, 0.629_978_7],
];
/// His cube-rooted LMS to Lab, rows.
pub const LMS_TO_LAB: [[f32; 3]; 3] = [
    [0.210_454_26, 0.793_617_8, -0.004_072_047],
    [1.977_998_5, -2.428_592_2, 0.450_593_7],
    [0.025_904_037, 0.782_771_77, -0.808_675_77],
];
/// Its inverse.
pub const LAB_TO_LMS: [[f32; 3]; 3] = [
    [1.0, 0.396_337_78, 0.215_803_76],
    [1.0, -0.105_561_346, -0.063_854_17],
    [1.0, -0.089_484_18, -1.291_485_5],
];

/// Oklab for the working space: [`WORKING_SPACE`], linear, to Oklab's
/// LMS and back. Ottosson's sRGB matrices composed with the working
/// space's matrix to sRGB, so a working-space pixel reaches Oklab in
/// one multiply and a cube root.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklab {
    pub to_lms: [[f32; 3]; 3],
    pub from_lms: [[f32; 3]; 3],
}

impl Default for Oklab {
    fn default() -> Self {
        Self::for_working_space()
    }
}

impl Oklab {
    pub fn for_working_space() -> Self {
        let to_srgb = WORKING_SPACE
            .to_space_matrix(&RgbSpace::SRGB, CAT)
            .expect("the working space reaches sRGB")
            .rows
            .map(|row| row.map(|v| v as f32));
        let to_lms = mul3(SRGB_TO_LMS, to_srgb);
        let from_lms = invert3(to_lms).expect("Oklab's matrix inverts");
        Self { to_lms, from_lms }
    }

    /// A working-space pixel as Oklab: lightness, a, b. A negative
    /// channel keeps its sign through the cube root, which is what the
    /// viewport's shader does and what keeps the round trip whole on
    /// scene values outside the space.
    #[inline]
    pub fn of(&self, c: [f32; 3]) -> [f32; 3] {
        let lms = apply3(&self.to_lms, c).map(|v| v.abs().cbrt().copysign(v));
        apply3(&LMS_TO_LAB, lms)
    }

    /// Back to the working space.
    #[inline]
    pub fn to_rgb(&self, lab: [f32; 3]) -> [f32; 3] {
        let lms = apply3(&LAB_TO_LMS, lab).map(|v| v * v * v);
        apply3(&self.from_lms, lms)
    }
}

/// A three by three matrix of the kind the color conversions are
/// made of: camera to working, working to sRGB, sRGB to LMS. Rows.
pub type Matrix3 = [[f32; 3]; 3];

/// A three by three matrix on a color, rows.
#[inline]
pub fn apply3(m: &Matrix3, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

/// One matrix after another: `mul3(a, b)` applies `b` then `a`.
pub fn mul3(a: Matrix3, b: Matrix3) -> Matrix3 {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

/// The matrix undone, by the adjugate over the determinant. `None`
/// for a matrix too near singular to undo, which a camera's can be.
pub fn invert3(m: Matrix3) -> Option<Matrix3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let d = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * d,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * d,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * d,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * d,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * d,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * d,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * d,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * d,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * d,
        ],
    ])
}

/// The camera-space gains that make a working-space pixel neutral,
/// green at one: the pixel back through `matrix` and `gains` to what
/// the sensor saw, and the reciprocal of that.
///
/// `gains` and `matrix` are the white balance the picture was
/// developed at — the camera gains, and camera to working, rows.
/// What a neutral dropper asks: the pixel the user says is grey, and
/// the gains that would have made it so. `None` for a matrix that
/// will not invert, or a pixel with a channel at or below nothing,
/// where there is no such white.
pub fn neutral_gains(px: [f32; 3], gains: [f32; 3], matrix: Matrix3) -> Option<[f64; 3]> {
    let inv = invert3(matrix)?;
    let cam = apply3(&inv, px);
    let cam: [f32; 3] = std::array::from_fn(|r| cam[r] / gains[r]);
    if cam.iter().any(|&v| v <= 0.0) {
        return None;
    }
    Some([(cam[1] / cam[0]) as f64, 1.0, (cam[1] / cam[2]) as f64])
}

/// sRGB's transfer curve, encoded to linear.
pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB's transfer curve, linear to encoded.
pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_neutral_picked_gives_back_the_gains_that_made_it() {
        const IDENTITY: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        // A grey the sensor saw as (0.5, 1, 0.6): balanced by the gains
        // it wants and through a matrix, it is neutral; picked at
        // another white, the pick undoes that white and asks for these.
        let gains = [1.4f32, 1.0, 1.2];
        let matrix: Matrix3 = [[1.6, -0.4, -0.2], [-0.1, 1.3, -0.2], [0.0, -0.3, 1.3]];
        let cam = [0.5f32, 1.0, 0.6];
        let px: [f32; 3] =
            std::array::from_fn(|r| (0..3).map(|c| matrix[r][c] * gains[c] * cam[c]).sum());
        let g = neutral_gains(px, gains, matrix).unwrap();
        assert!(
            (g[0] - 2.0).abs() < 1e-4 && g[1] == 1.0 && (g[2] - 1.0 / 0.6).abs() < 1e-4,
            "{g:?}"
        );
        // A channel at nothing has no gain to give.
        assert!(neutral_gains([0.0, 0.5, 0.5], [1.0; 3], IDENTITY).is_none());
    }

    use crate::raw::Calibration;

    #[test]
    fn the_matrix_helpers_agree_with_arithmetic() {
        let m: Matrix3 = [[2.0, 0.5, 0.0], [0.0, 1.0, 0.25], [0.1, 0.0, 1.5]];
        let inv = invert3(m).unwrap();
        let id = mul3(m, inv);
        for (r, row) in id.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                let want = if r == c { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-5, "{id:?}");
            }
        }
        // Applying the product is applying the two in turn.
        let n: Matrix3 = [[0.0, 1.0, 0.0], [0.5, 0.0, 0.5], [1.0, 0.0, -1.0]];
        let v = [0.2f32, 0.7, 0.4];
        let (a, b) = (apply3(&mul3(m, n), v), apply3(&m, apply3(&n, v)));
        assert!(a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6), "{a:?}");
        // A singular matrix, a row twice over, has nothing to give.
        assert!(invert3([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 0.0, 1.0]]).is_none());
    }

    /// A camera whose native space is exactly Rec.2020: XYZ to camera is the
    /// working space's own inverse primaries. Under D65 its neutral is
    /// (1, 1, 1) and camera to working is the identity.
    pub(crate) fn rec2020_camera_matrix() -> Vec<f32> {
        let m = WORKING_SPACE.from_xyz_matrix().unwrap();
        let mut flat = Vec::with_capacity(9);
        for r in 0..3 {
            for c in 0..3 {
                flat.push(m.rows[r][c] as f32);
            }
        }
        flat
    }

    fn frame_with(calibrations: Vec<Calibration>) -> RawFrame {
        RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            width: 2,
            height: 2,
            channels: 1,
            layout: crate::raw::SensorLayout::Cfa(crate::raw::CfaPattern::rggb()),
            samples: crate::raw::Samples::U16(vec![0; 4]),
            levels: crate::raw::Levels {
                black: crate::raw::LevelPattern::uniform(0.0, 1),
                white: crate::raw::LevelPattern::uniform(1.0, 1),
            },
            as_shot_coefficients: Some([1.0, 1.0, 1.0]),
            calibrations,
            crop: None,
            orientation: Default::default(),
            shot: Default::default(),
        }
    }

    const A: u16 = 17;
    const D65: u16 = 21;
    const D50: u16 = 23;

    #[test]
    fn dual_profile_takes_the_widest_pair_in_temperature_order() {
        let m = rec2020_camera_matrix();
        let cal = |illuminant| Calibration {
            illuminant,
            color_matrix: m.clone(),
            forward_matrix: None,
        };
        let frame = frame_with(vec![cal(D50), cal(D65), cal(A)]);
        let profile = profile_from_frame(&frame).unwrap();
        assert_eq!(profile.primary.illuminant, CalibrationIlluminant::StandardA);
        assert_eq!(
            profile.secondary.unwrap().illuminant,
            CalibrationIlluminant::D65
        );
    }

    #[test]
    fn single_and_missing_calibrations() {
        let m = rec2020_camera_matrix();
        let frame = frame_with(vec![Calibration {
            illuminant: D65,
            color_matrix: m,
            forward_matrix: None,
        }]);
        let profile = profile_from_frame(&frame).unwrap();
        assert!(profile.secondary.is_none());

        assert!(matches!(
            profile_from_frame(&frame_with(vec![])),
            Err(Error::NoCalibration)
        ));
        let unknown = Calibration {
            illuminant: 0,
            color_matrix: rec2020_camera_matrix(),
            forward_matrix: None,
        };
        assert!(matches!(
            profile_from_frame(&frame_with(vec![unknown])),
            Err(Error::NoCalibration)
        ));
        let four_color = Calibration {
            illuminant: D65,
            color_matrix: vec![0.5; 12],
            forward_matrix: None,
        };
        assert!(matches!(
            profile_from_frame(&frame_with(vec![four_color])),
            Err(Error::NoCalibration)
        ));
        let singular = Calibration {
            illuminant: D65,
            color_matrix: vec![1.0; 9],
            forward_matrix: None,
        };
        assert!(matches!(
            profile_from_frame(&frame_with(vec![singular])),
            Err(Error::NoCalibration)
        ));
    }

    #[test]
    fn rec2020_camera_under_d65_is_the_identity() {
        let frame = frame_with(vec![Calibration {
            illuminant: D65,
            color_matrix: rec2020_camera_matrix(),
            forward_matrix: None,
        }]);
        let profile = profile_from_frame(&frame).unwrap();
        let wb = resolve_white_balance(&frame, &profile, WhitePoint::AsShot).unwrap();
        for (i, g) in wb.coefficients_f32().iter().enumerate() {
            assert!((g - 1.0).abs() < 1e-4, "gain {i} = {g}");
        }
        for (c, col) in wb.matrix_f32().iter().enumerate() {
            for (r, v) in col.iter().enumerate() {
                let want = if r == c { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-3, "matrix col {c} row {r} = {v}");
            }
        }
        let tt = as_shot_temp_tint(&frame, &profile).unwrap();
        assert!((tt.cct - 6504.0).abs() < 20.0, "cct {}", tt.cct);
        // D65 itself lies about +0.003 off the Planckian locus.
        assert!(tt.duv.abs() < 5e-3, "duv {}", tt.duv);
    }

    #[test]
    fn oklab_is_grey_on_grey_and_goes_back() {
        let ok = Oklab::for_working_space();
        // A neutral working-space color has no chroma, and its
        // lightness is the cube root of the grey.
        for grey in [0.05f32, 0.18, 0.5, 1.0, 3.0] {
            let lab = ok.of([grey, grey, grey]);
            assert!(
                lab[1].abs() < 1e-3 && lab[2].abs() < 1e-3,
                "{grey}: {lab:?}"
            );
            assert!((lab[0] - grey.cbrt()).abs() < 2e-3, "{grey}: L {}", lab[0]);
        }
        // And every color comes back, negatives and highlights too.
        for c in [
            [0.18f32, 0.05, 0.4],
            [1.2, 0.9, 0.1],
            [-0.02, 0.3, 0.6],
            [0.0, 0.0, 0.0],
        ] {
            let back = ok.to_rgb(ok.of(c));
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 1e-5, "{c:?} came back {back:?}");
            }
        }
    }
}
