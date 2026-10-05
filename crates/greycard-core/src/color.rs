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

/// A temperature and tint resolved through a profile alone: what
/// [`resolve_white_balance`] does for [`WhitePoint::TempTint`], which
/// needs nothing of the frame.
pub fn white_balance_at(profile: &CameraProfile, temp_tint: TempTint) -> Result<WhiteBalance> {
    Ok(WhiteBalance::from_temp_tint(
        profile,
        temp_tint,
        &WORKING_SPACE,
        CAT,
    )?)
}

/// A white balance's camera to working matrix, rows, as [`apply3`]
/// takes it; rawcolor hands it over in columns, for a GPU.
pub fn matrix_rows(wb: &WhiteBalance) -> Matrix3 {
    let cols = wb.matrix_f32();
    std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r]))
}

/// The working-space matrix that takes a picture developed at one white
/// balance to another: back through the first's camera to working
/// matrix (`base_rows`, rows) to camera space with its gains on, the
/// gains' ratio, and through the second's matrix. Exact for the matrix
/// and the gains; the demosaic and the highlight reconstruction saw the
/// first's gains, which no matrix after them can change. `None` when
/// the first matrix has no inverse.
///
/// The viewport's white balance preview, until the develop at the new
/// white lands, and a local white balance, for good.
pub fn white_shift(
    base_gains: [f32; 3],
    base_rows: Matrix3,
    target_gains: [f32; 3],
    target_rows: Matrix3,
) -> Option<Matrix3> {
    let inv = invert3(base_rows)?;
    let mut scaled = target_rows;
    for row in scaled.iter_mut() {
        for (c, v) in row.iter_mut().enumerate() {
            *v *= target_gains[c] / base_gains[c];
        }
    }
    let m = mul3(scaled, inv);
    m.iter().flatten().all(|v| v.is_finite()).then_some(m)
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

/// A working-space pixel back to what the sensor saw: through `inv`,
/// the inverse of the develop's camera to working matrix, and out of
/// the develop's `gains`.
fn to_camera(px: [f32; 3], gains: [f32; 3], inv: &Matrix3) -> [f32; 3] {
    let cam = apply3(inv, px);
    std::array::from_fn(|r| cam[r] / gains[r])
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
    let cam = to_camera(px, gains, &invert3(matrix)?);
    if cam.iter().any(|&v| v <= 0.0) {
        return None;
    }
    Some([(cam[1] / cam[0]) as f64, 1.0, (cam[1] / cam[2]) as f64])
}

/// Above this fraction of a channel's clip a pixel is taken as
/// clipped by [`auto_neutral`]: its channels no longer say what the
/// light was.
pub const AUTO_CLIP: f32 = 0.95;
/// Below this fraction of the sensor's white a pixel is noise to
/// [`auto_neutral`].
pub const AUTO_FLOOR: f32 = 0.005;
/// Fewer usable pixels than this and [`auto_neutral`] will not guess.
pub const AUTO_MIN_PIXELS: usize = 300;
/// How far from the as-shot white a pixel may be, in the log ratios
/// of its channels, before [`auto_neutral`]'s first pass counts it at
/// half.
const AUTO_PRIOR_SPREAD: f32 = 0.25;
/// The width of the second pass's pull toward grey, in the same units.
const AUTO_GREY_SPREAD: f32 = 0.1;

/// A pixel's distance from `white`, squared, in log ratios: red and
/// blue each against green, after dividing by `white`. Zero for any
/// pixel of `white`'s color, whatever its brightness.
fn chroma2(px: [f32; 3], white: [f32; 3]) -> f32 {
    let g = px[1] / white[1];
    let rg = (px[0] / white[0] / g).ln();
    let bg = (px[2] / white[2] / g).ln();
    rg * rg + bg * bg
}

/// The weighted mean of `pixels`, or `None` when the weights add up
/// to nothing.
fn weighted_mean(pixels: &[[f32; 3]], weight: impl Fn([f32; 3]) -> f32) -> Option<[f32; 3]> {
    let (mut sum, mut total) = ([0.0f64; 3], 0.0f64);
    for &p in pixels {
        let w = f64::from(weight(p));
        total += w;
        for k in 0..3 {
            sum[k] += w * f64::from(p[k]);
        }
    }
    (total > 1e-12 && sum.iter().all(|&s| s > 0.0)).then(|| sum.map(|s| (s / total) as f32))
}

/// The color of the light over a whole picture, as a working-space
/// pixel to hand to [`neutral_gains`] the way a neutral dropper hands
/// it the pixel it picked.
///
/// `pixels` are the developed picture on a grid `width` points across,
/// row after row, linear, in the working space at the white it was
/// developed at: `gains` and `matrix` as for [`neutral_gains`], and
/// `clip` the working-space value its channels were clipped at.
/// `as_shot` are the camera's own gains for the frame.
///
/// The estimate is made in camera space, each pixel taken back through
/// the matrix and the gains to what the sensor saw, so it does not
/// depend on the white the picture happens to be developed at: from
/// any white the same frame gives the same light, and the answer run
/// on the picture developed at its own answer is that answer again.
///
/// Grey-edge (van de Weijer, Gevers and Gijsenij, "Edge-based color
/// constancy", 2007; darktable color calibration's detection from
/// image edges is the same idea): the mean difference between
/// neighboring surfaces is grey, where grey-world takes the mean
/// surface to be. A big smooth sky or wall is a lot of pixels but few
/// and faint edges, so it no longer decides the answer by its area.
///
/// 1. Only the pixels that can say something: every channel above
///    [`AUTO_FLOOR`] of the sensor's white (below it is noise) and
///    none above [`AUTO_CLIP`] of where that channel clipped, the
///    sensor's white or `clip` over its gain, whichever is lower (a
///    clipped channel has lost the ratio).
/// 2. At each point whose right and lower neighbors are usable too,
///    each channel's gradient: the length of its two differences.
///    These are the samples; with fewer than [`AUTO_MIN_PIXELS`] of
///    them (a picture with no structure, or no rows) the usable
///    pixels themselves are, and this is grey-world.
/// 3. Their mean, each sample weighted `1 / (1 + (d / 0.25)^2)` by
///    its distance `d` from the as-shot white, `d` the log ratios of
///    red and blue to green: a sample far off grey under the
///    camera's own white counts for less. Among samples that all
///    share a cast every weight is the same.
/// 4. One re-weighting: the mean again, each sample weighted
///    `exp(-d^2 / (2 * 0.1^2))` by its distance from the first
///    estimate, so the result settles on the samples that estimate
///    calls grey. When none is near enough to weigh anything the
///    first estimate stands.
///
/// The means are of the samples themselves, not of their
/// chromaticity, so a strong edge counts for more than a faint one,
/// as in grey-edge. `None` when fewer than [`AUTO_MIN_PIXELS`] pixels
/// are usable, or the matrix will not invert.
pub fn auto_neutral(
    pixels: &[[f32; 3]],
    width: usize,
    gains: [f32; 3],
    matrix: Matrix3,
    clip: f32,
    as_shot: [f32; 3],
) -> Option<[f32; 3]> {
    let inv = invert3(matrix)?;
    // The sensor clips at one in every channel; a develop that did
    // not rebuild its highlights clipped each lower, at `clip` over
    // its gain.
    let top: [f32; 3] = std::array::from_fn(|c| (clip / gains[c]).min(1.0));
    let camera: Vec<Option<[f32; 3]>> = pixels
        .iter()
        .map(|&p| {
            let v = to_camera(p, gains, &inv);
            (0..3)
                .all(|c| v[c].is_finite() && v[c] > AUTO_FLOOR && v[c] <= top[c] * AUTO_CLIP)
                .then_some(v)
        })
        .collect();
    let usable: Vec<[f32; 3]> = camera.iter().flatten().copied().collect();
    if usable.len() < AUTO_MIN_PIXELS {
        return None;
    }
    let edges = edges(&camera, width);
    let samples = if edges.len() >= AUTO_MIN_PIXELS {
        &edges
    } else {
        &usable
    };
    // The as-shot white as the sensor sees it.
    let shot = as_shot.map(|g| 1.0 / g);
    let prior = AUTO_PRIOR_SPREAD * AUTO_PRIOR_SPREAD;
    let first = weighted_mean(samples, |p| 1.0 / (1.0 + chroma2(p, shot) / prior))?;
    let spread = 2.0 * AUTO_GREY_SPREAD * AUTO_GREY_SPREAD;
    let light = weighted_mean(samples, |p| (-chroma2(p, first) / spread).exp()).unwrap_or(first);
    // Forward again, to the pixel the develop made of that light.
    Some(apply3(
        &matrix,
        std::array::from_fn(|c| light[c] * gains[c]),
    ))
}

/// Each channel's gradient on a grid `width` across, at every point
/// whose right and lower neighbors are there (`Some`) as well: the
/// length of its two differences. A point where a channel does not
/// change at all says nothing of the light's color and is left out.
fn edges(grid: &[Option<[f32; 3]>], width: usize) -> Vec<[f32; 3]> {
    if width < 2 {
        return Vec::new();
    }
    let rows = grid.len() / width;
    let mut out = Vec::new();
    for y in 0..rows.saturating_sub(1) {
        for x in 0..width - 1 {
            let i = y * width + x;
            let (Some(a), Some(right), Some(below)) = (grid[i], grid[i + 1], grid[i + width])
            else {
                continue;
            };
            let g: [f32; 3] = std::array::from_fn(|c| (right[c] - a[c]).hypot(below[c] - a[c]));
            if g.iter().all(|&v| v > 0.0) {
                out.push(g);
            }
        }
    }
    out
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

    const IDENTITY3: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    #[test]
    fn a_white_shift_lands_where_a_develop_at_the_target_would() {
        let frame = frame_with(vec![
            Calibration {
                illuminant: A,
                color_matrix: rec2020_camera_matrix(),
                forward_matrix: None,
            },
            Calibration {
                illuminant: D65,
                color_matrix: rec2020_camera_matrix(),
                forward_matrix: None,
            },
        ]);
        let profile = profile_from_frame(&frame).unwrap();
        let at = |cct| white_balance_at(&profile, TempTint { cct, duv: 0.0 }).unwrap();
        let (day, warm) = (at(5500.0), at(3200.0));
        let develop = |wb: &WhiteBalance, cam: [f32; 3]| {
            let g = wb.coefficients_f32();
            apply3(&matrix_rows(wb), std::array::from_fn(|c| cam[c] * g[c]))
        };
        let m = white_shift(
            day.coefficients_f32(),
            matrix_rows(&day),
            warm.coefficients_f32(),
            matrix_rows(&warm),
        )
        .unwrap();
        for cam in [[0.2f32, 0.5, 0.3], [0.05, 0.02, 0.4], [0.9, 0.9, 0.9]] {
            let shifted = apply3(&m, develop(&day, cam));
            let direct = develop(&warm, cam);
            for c in 0..3 {
                assert!(
                    (shifted[c] - direct[c]).abs() < 1e-5,
                    "{shifted:?} {direct:?}"
                );
            }
        }
        // To itself, nothing.
        let same = white_shift(
            day.coefficients_f32(),
            matrix_rows(&day),
            day.coefficients_f32(),
            matrix_rows(&day),
        )
        .unwrap();
        for (r, row) in same.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                assert!(
                    (v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-5,
                    "{same:?}"
                );
            }
        }
        // No inverse, no shift.
        assert!(white_shift([1.0; 3], [[1.0; 3]; 3], [1.0; 3], IDENTITY3).is_none());
    }

    /// [`auto_neutral`] on a picture developed with no gains and no
    /// matrix, the camera's own white at one: working space is
    /// camera space. One column, so no edges: the pixels' own path.
    fn plain(pixels: &[[f32; 3]], clip: f32) -> Option<[f32; 3]> {
        auto_neutral(pixels, 1, [1.0; 3], IDENTITY3, clip, [1.0; 3])
    }

    /// A grey ramp from dark to bright under a light of color `cast`,
    /// `n` pixels.
    fn cast_ramp(cast: [f32; 3], n: usize) -> Vec<[f32; 3]> {
        (0..n)
            .map(|i| {
                let level = 0.02 + 0.6 * i as f32 / n as f32;
                cast.map(|c| c * level)
            })
            .collect()
    }

    /// The light's color `px` names, as red and blue over green.
    fn ratios(px: [f32; 3]) -> [f32; 2] {
        [px[0] / px[1], px[2] / px[1]]
    }

    fn within(got: [f32; 3], cast: [f32; 3], tolerance: f32) -> bool {
        ratios(got)
            .iter()
            .zip(ratios(cast))
            .all(|(g, w)| (g / w - 1.0).abs() < tolerance)
    }

    #[test]
    fn auto_neutral_finds_the_cast_of_a_grey_ramp() {
        let cast = [1.3f32, 1.0, 0.75];
        let got = plain(&cast_ramp(cast, 2000), 1.0).unwrap();
        assert!(within(got, cast, 0.01), "{got:?}");
        // The gains it leads to are the cast's inverse.
        let g = neutral_gains(got, [1.0; 3], IDENTITY3).unwrap();
        assert!((g[0] as f32 * 1.3 - 1.0).abs() < 0.01, "{g:?}");
        assert!((g[2] as f32 * 0.75 - 1.0).abs() < 0.01, "{g:?}");
    }

    #[test]
    fn auto_neutral_is_not_dragged_by_a_big_blue_sky() {
        // Grey patches under a warm light, and a saturated blue sky
        // that is most of the frame. Plain grey-world lands between
        // the two; the weighting brings it back to the patches.
        let cast = [1.2f32, 1.0, 0.8];
        let mut pixels = cast_ramp(cast, 1200);
        for i in 0..2800 {
            let t = i as f32 / 2800.0;
            pixels.push([0.04 + 0.02 * t, 0.10 + 0.05 * t, 0.45 + 0.2 * t]);
        }
        let got = plain(&pixels, 1.0).unwrap();
        assert!(within(got, cast, 0.03), "{got:?}");
        // Plain grey-world would be far off: the test is of the
        // weighting, not of a sky too small to matter.
        let n = pixels.len() as f32;
        let mean = [0, 1, 2].map(|k| pixels.iter().map(|p| p[k]).sum::<f32>() / n);
        assert!(!within(mean, cast, 0.2), "{mean:?}");
    }

    /// The light as the sensor sees it in [`textured_land_under_sky`].
    const DAYLIGHT: [f32; 3] = [0.5, 1.0, 0.7];

    /// A frame 64 points across as the sensor saw it under
    /// [`DAYLIGHT`]: the lower 20 rows grey ground of every lightness,
    /// changing from point to point, and above it a pale sky, brighter
    /// and most of the frame, smooth but for a faint fall from the
    /// horizon up. The shape of a seascape or an air show, where the
    /// sky's area is what grey-world reads.
    fn textured_land_under_sky() -> Vec<[f32; 3]> {
        let mut frame = Vec::new();
        for y in 0..64 {
            for x in 0..64 {
                if y < 44 {
                    let level = 0.45 + 0.1 * y as f32 / 44.0;
                    let sky = [0.85, 1.0, 1.35];
                    frame.push(std::array::from_fn(|c| DAYLIGHT[c] * sky[c] * level));
                } else {
                    let level = 0.05 + 0.25 * ((x * 7 + y * 13) % 11) as f32 / 10.0;
                    frame.push(DAYLIGHT.map(|c| c * level));
                }
            }
        }
        frame
    }

    /// [`auto_neutral`] on `frame` read `width` across, at no gains
    /// and no matrix, with an as-shot white a little off [`DAYLIGHT`].
    fn on_land(frame: &[[f32; 3]], width: usize) -> [f32; 3] {
        auto_neutral(frame, width, [1.0; 3], IDENTITY3, 1.0, [2.1, 1.0, 1.35]).unwrap()
    }

    #[test]
    fn auto_neutral_reads_the_light_off_edges_not_a_big_sky() {
        let frame = textured_land_under_sky();
        // On the grid, the ground's edges name the light.
        let got = on_land(&frame, 64);
        assert!(within(got, DAYLIGHT, 0.02), "{got:?}");
        // Read as pixels alone, grey-world even with both passes, the
        // sky drags it blue: the light taken for bluer than it is, a
        // white set higher, a picture warmer than the scene.
        let pixels = on_land(&frame, 1);
        assert!(!within(pixels, DAYLIGHT, 0.05), "{pixels:?}");
        assert!(ratios(pixels)[1] > ratios(DAYLIGHT)[1], "{pixels:?}");
    }

    #[test]
    fn auto_neutral_leaves_out_edges_at_clipped_and_black_pixels() {
        // Glints dotted through the ground, the light itself a little
        // brighter than the sensor holds: green clipped at one, red
        // and blue still under. Off the ground each makes a strong
        // edge near grey but too red and too blue, close enough to
        // the light to pass both passes; with black noise beside
        // them. A point beside either has no edge to give, so the
        // answer is the rest of the ground's.
        let mut frame = textured_land_under_sky();
        let alone = on_land(&frame, 64);
        assert!(within(alone, DAYLIGHT, 0.02), "{alone:?}");
        for y in 44..64 {
            for x in (y % 3..64).step_by(6) {
                frame[y * 64 + x] = if x % 4 == 1 {
                    [0.004, 0.002, 0.003]
                } else {
                    [DAYLIGHT[0] * 1.15, 1.0, DAYLIGHT[2] * 1.15]
                };
            }
        }
        let got = on_land(&frame, 64);
        assert!(within(got, alone, 0.01), "{got:?} {alone:?}");
    }

    #[test]
    fn auto_neutral_leaves_out_clipped_and_black_pixels() {
        let cast = [0.8f32, 1.0, 1.3];
        let ramp = cast_ramp(cast, 1000);
        let alone = plain(&ramp, 1.0).unwrap();
        let mut pixels = ramp.clone();
        // Blown highlights of a colored lamp, and colored noise in
        // the shadows: plenty of both, none of it counted.
        for i in 0..800 {
            let t = i as f32 / 800.0;
            pixels.push([1.0, 0.4 + 0.3 * t, 0.2]);
            pixels.push([0.003, 0.0045 * t, 0.001 + 0.002 * t]);
            pixels.push([f32::NAN, 0.5, 0.5]);
        }
        let got = plain(&pixels, 1.0).unwrap();
        assert!(
            got.iter().zip(alone).all(|(a, b)| (a - b).abs() < 1e-6),
            "{got:?} {alone:?}"
        );
        // The clip is the develop's: a lamp at 0.6 is clipped in a
        // develop that clipped there, and is enough to read alone in
        // one that clipped at the sensor's white.
        let lamp: Vec<[f32; 3]> = (0..800)
            .map(|i| [0.6, 0.25 + 0.1 * i as f32 / 800.0, 0.12])
            .collect();
        assert!(plain(&lamp, 0.6).is_none());
        let lit = plain(&lamp, 1.0).unwrap();
        assert!(lit[0] > lit[1] && lit[1] > lit[2], "{lit:?}");
    }

    #[test]
    fn auto_neutral_wants_enough_of_the_picture() {
        let cast = [1.1f32, 1.0, 0.9];
        assert!(plain(&cast_ramp(cast, AUTO_MIN_PIXELS - 1), 1.0).is_none());
        assert!(plain(&cast_ramp(cast, AUTO_MIN_PIXELS), 1.0).is_some());
        // A frame that is all highlight or all shadow has too few.
        let mut dark = vec![[0.001f32, 0.002, 0.001]; 5000];
        dark.extend(vec![[0.99f32, 0.99, 0.99]; 5000]);
        dark.extend(cast_ramp(cast, 100));
        assert!(plain(&dark, 1.0).is_none());
        assert!(plain(&[], 1.0).is_none());
    }

    /// How wide [`camera_scene`] is: the patches are its first 30
    /// rows, the sky the 50 after.
    const SCENE_WIDTH: usize = 40;

    /// A scene as the sensor saw it: grey patches under a light the
    /// camera reads as (0.5, 1, 0.7), and a sky that is most of it.
    fn camera_scene() -> Vec<[f32; 3]> {
        let mut scene = cast_ramp([0.5, 1.0, 0.7], 1200);
        for i in 0..2000 {
            let t = i as f32 / 2000.0;
            scene.push([0.03 + 0.01 * t, 0.12 + 0.05 * t, 0.35 + 0.1 * t]);
        }
        scene
    }

    /// `scene` developed at `gains` through `matrix`, highlights
    /// rebuilt: the working-space picture and its clip.
    fn developed(scene: &[[f32; 3]], gains: [f32; 3], matrix: Matrix3) -> (Vec<[f32; 3]>, f32) {
        let pixels = scene
            .iter()
            .map(|c| apply3(&matrix, std::array::from_fn(|k| c[k] * gains[k])))
            .collect();
        (pixels, gains.iter().copied().fold(1.0, f32::max) * 0.98)
    }

    /// Two camera to working matrices a white apart, rows summing to
    /// one so a balanced grey stays grey.
    const WARM_MATRIX: Matrix3 = [[1.7, -0.5, -0.2], [-0.2, 1.4, -0.2], [0.0, -0.4, 1.4]];
    const COOL_MATRIX: Matrix3 = [[1.5, -0.4, -0.1], [-0.15, 1.3, -0.15], [0.05, -0.3, 1.25]];
    /// The camera's own gains for the scene: near, not at, its light.
    const AS_SHOT: [f32; 3] = [1.9, 1.0, 1.35];

    #[test]
    fn auto_neutral_does_not_depend_on_the_white_developed_at() {
        let scene = camera_scene();
        let answer = |gains: [f32; 3], matrix: Matrix3| {
            let (pixels, clip) = developed(&scene, gains, matrix);
            let px = auto_neutral(&pixels, SCENE_WIDTH, gains, matrix, clip, AS_SHOT).unwrap();
            neutral_gains(px, gains, matrix).unwrap().map(|g| g as f32)
        };
        // A tungsten white and a shade white: the same light.
        let warm = answer([1.2, 1.0, 2.6], WARM_MATRIX);
        let cool = answer([2.8, 1.0, 1.0], COOL_MATRIX);
        for k in 0..3 {
            assert!((warm[k] / cool[k] - 1.0).abs() < 0.01, "{warm:?} {cool:?}");
        }
        // And the light is the patches', not the sky's.
        assert!((warm[0] * 0.5 - 1.0).abs() < 0.03, "{warm:?}");
        assert!((warm[2] * 0.7 - 1.0).abs() < 0.03, "{warm:?}");
    }

    #[test]
    fn auto_neutral_on_its_own_answer_is_neutral() {
        let scene = camera_scene();
        let (pixels, clip) = developed(&scene, AS_SHOT, WARM_MATRIX);
        let px = auto_neutral(&pixels, SCENE_WIDTH, AS_SHOT, WARM_MATRIX, clip, AS_SHOT).unwrap();
        let gains = neutral_gains(px, AS_SHOT, WARM_MATRIX)
            .unwrap()
            .map(|g| g as f32);
        // Developed at the white it found, it finds grey.
        let (again, clip) = developed(&scene, gains, WARM_MATRIX);
        let px = auto_neutral(&again, SCENE_WIDTH, gains, WARM_MATRIX, clip, AS_SHOT).unwrap();
        assert!(within(px, [1.0; 3], 0.01), "{px:?}");
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
