//! Lens corrections on the working image: vignetting undone, the two
//! outer channels brought back onto the green (lateral chromatic
//! aberration), and the geometry straightened, in one resample.
//!
//! The models are the ones the lensfun database publishes its
//! calibrations in (lensfun.github.io, the "Lens calibration models"
//! page of its manual), with the same names and the same parameters,
//! so a database entry goes straight into a [`LensCorrection`]:
//!
//! * distortion: `poly3`, `poly5`, `ptlens` and Adobe's `acm`, each
//!   giving the distorted radius as a function of the undistorted one;
//! * lateral CA: `linear` and `poly3`, the red and blue radii as
//!   functions of the green;
//! * vignetting: `pa` (Pablo d'Angelo's), and Adobe's, which is the
//!   same polynomial.
//!
//! Radii are normalized as lensfun normalizes them: for distortion and
//! CA, one is half the picture's shorter side; for vignetting, one is
//! half its diagonal. A calibration made on a sensor of another size
//! is used through [`LensCorrection::radius_scale`], which the
//! consumer sets from the two sensors' crop factors and shapes.
//!
//! A calibration made on a smaller sensor than the picture's knows
//! nothing past its own corner. The vignetting is held there: its
//! polynomial, fit only out to the corner, bends back up past it (see
//! [`Vignetting::falloff`]). The distortion and CA polynomials are
//! evaluated as they are: holding a displacement at a radius would put
//! a kink in the picture's geometry there, and all but a handful of
//! the database's distortions (fisheyes, phones) keep moving outward
//! past the corner of a full-frame calibration on a 44 by 33 sensor.
//!
//! Where the geometry changes the picture is resampled once, cubic,
//! from the working image, each channel at its own place; the
//! vignetting gain is taken at the place sampled. A correction that
//! only changes the gain leaves every pixel where it is.

use rayon::prelude::*;

use crate::image::WorkingImage;

/// A distortion model: the distorted radius (where the light landed)
/// for an undistorted one (where it should have), radii in half
/// shorter sides unless the model says otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Distortion {
    /// `r_d = r (1 - k1 + k1 r²)`: the radius one is held.
    Poly3 { k1: f32 },
    /// `r_d = r (1 + k1 r² + k2 r⁴)`.
    Poly5 { k1: f32, k2: f32 },
    /// `r_d = r (a r³ + b r² + c r + 1 - a - b - c)`: the radius one is
    /// held. PanoTools' model.
    Ptlens { a: f32, b: f32, c: f32 },
    /// Adobe's camera model, in units of the focal length:
    /// `x_d = x (1 + k1 r² + k2 r⁴ + k3 r⁶) + 2x (k4 y + k5 x) + k5 r²`,
    /// and `y_d` likewise with `k4 r²`. `focal_scale` turns a radius in
    /// half shorter sides of the picture into focal lengths: half the
    /// sensor's shorter side in millimeters over the real focal length.
    Acm { k: [f32; 5], focal_scale: f32 },
}

impl Distortion {
    /// Where the light for an undistorted point landed, both about the
    /// center in half shorter sides.
    #[inline]
    pub fn distorted(&self, x: f32, y: f32) -> (f32, f32) {
        let r2 = x * x + y * y;
        match *self {
            Distortion::Poly3 { k1 } => {
                let f = 1.0 - k1 + k1 * r2;
                (x * f, y * f)
            }
            Distortion::Poly5 { k1, k2 } => {
                let f = 1.0 + k1 * r2 + k2 * r2 * r2;
                (x * f, y * f)
            }
            Distortion::Ptlens { a, b, c } => {
                let r = r2.sqrt();
                let f = a * r2 * r + b * r2 + c * r + 1.0 - a - b - c;
                (x * f, y * f)
            }
            Distortion::Acm { k, focal_scale } => {
                let (xf, yf) = (x * focal_scale, y * focal_scale);
                let r2 = xf * xf + yf * yf;
                let radial = 1.0 + k[0] * r2 + k[1] * r2 * r2 + k[2] * r2 * r2 * r2;
                let tangent = 2.0 * (k[3] * yf + k[4] * xf);
                let xd = xf * radial + xf * tangent + k[4] * r2;
                let yd = yf * radial + yf * tangent + k[3] * r2;
                (xd / focal_scale, yd / focal_scale)
            }
        }
    }

    /// Whether the model moves nothing.
    pub fn is_identity(&self) -> bool {
        match *self {
            Distortion::Poly3 { k1 } => k1 == 0.0,
            Distortion::Poly5 { k1, k2 } => k1 == 0.0 && k2 == 0.0,
            Distortion::Ptlens { a, b, c } => a == 0.0 && b == 0.0 && c == 0.0,
            Distortion::Acm { k, .. } => k.iter().all(|v| *v == 0.0),
        }
    }
}

/// Lateral chromatic aberration: red and blue each a radial scaling of
/// the green, `r_c = r (v + c r + b r²)`, radii in half shorter sides.
/// The linear model is `v` alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChromaticAberration {
    /// `[v, c, b]` for red.
    pub red: [f32; 3],
    /// `[v, c, b]` for blue.
    pub blue: [f32; 3],
}

impl ChromaticAberration {
    pub const IDENTITY: Self = Self {
        red: [1.0, 0.0, 0.0],
        blue: [1.0, 0.0, 0.0],
    };

    /// Where a channel's light landed for a point where the green's
    /// did.
    #[inline]
    pub fn shifted(&self, channel: usize, x: f32, y: f32) -> (f32, f32) {
        let [v, c, b] = match channel {
            0 => self.red,
            2 => self.blue,
            _ => return (x, y),
        };
        let r = (x * x + y * y).sqrt();
        let f = v + c * r + b * r * r;
        (x * f, y * f)
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// This aberration with each channel's radius scaled again by
    /// `red` and `blue`, as a correction by hand on top of a
    /// profile's: `m (v + c r + b r²)` is the same polynomial with
    /// every coefficient times `m`, so the two fold into one model and
    /// one resample.
    pub fn scaled(self, red: f32, blue: f32) -> Self {
        Self {
            red: self.red.map(|k| k * red),
            blue: self.blue.map(|k| k * blue),
        }
    }
}

/// Vignetting: the light that reached the sensor as a fraction of what
/// left the lens, `1 + k1 r² + k2 r⁴ + k3 r⁶`, `r` in half diagonals
/// of the sensor the calibration was made on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vignetting {
    pub k: [f32; 3],
}

impl Vignetting {
    /// The radius the model was fit out to: the calibration sensor's
    /// corner, one half diagonal (lensfun's `pa` convention).
    pub const CALIBRATED_RADIUS: f32 = 1.0;

    /// The fall-off at a radius: what a pixel there recorded of the
    /// scene, one at the center. Past [`Self::CALIBRATED_RADIUS`] it is
    /// the fall-off there. A sixth-order fit is a fit only where it was
    /// measured, and the database's bend back up past the corner: the
    /// Sigma 50 Art's at f/5.6 falls to 0.71 at the corner and climbs
    /// to 1.10 by 1.37, which would brighten a bigger sensor's corners
    /// less than the calibration's, or darken them.
    #[inline]
    pub fn falloff(&self, r: f32) -> f32 {
        let r = r.min(Self::CALIBRATED_RADIUS);
        let r2 = r * r;
        1.0 + self.k[0] * r2 + self.k[1] * r2 * r2 + self.k[2] * r2 * r2 * r2
    }

    pub fn is_identity(&self) -> bool {
        self.k == [0.0; 3]
    }
}

/// How much of the corrected picture to show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scale {
    /// The smallest magnification that leaves no edge empty.
    Auto,
    /// A magnification: one shows the corrected picture at the
    /// source's scale, whatever that leaves empty at the edges.
    Fixed(f32),
}

/// A correction for one picture: the models and the units they are in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LensCorrection {
    pub distortion: Option<Distortion>,
    pub chromatic_aberration: Option<ChromaticAberration>,
    pub vignetting: Option<Vignetting>,
    /// What a radius in half shorter sides of this picture is in the
    /// distortion and CA models' units: one when the models were made
    /// on a sensor of this size and shape.
    pub radius_scale: f32,
    /// The same for the vignetting model, in half diagonals.
    pub vignetting_scale: f32,
    pub scale: Scale,
}

impl Default for LensCorrection {
    fn default() -> Self {
        Self {
            distortion: None,
            chromatic_aberration: None,
            vignetting: None,
            radius_scale: 1.0,
            vignetting_scale: 1.0,
            scale: Scale::Auto,
        }
    }
}

/// What the correction did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LensStats {
    /// The magnification applied.
    pub scale: f32,
    /// Whether the picture was resampled, or only its gain changed.
    pub resampled: bool,
    /// The gain at the corners, when the vignetting was corrected.
    pub corner_gain: Option<f32>,
}

impl LensCorrection {
    /// Where the vignetting calibration ends on this picture, as a
    /// fraction of the way from its center to its corner, when that is
    /// short of the corner: the calibration was made on a smaller
    /// sensor, and the gain past it is held at its value there.
    pub fn vignetting_edge(&self) -> Option<f32> {
        (self.vignetting.is_some() && self.vignetting_scale > Vignetting::CALIBRATED_RADIUS)
            .then(|| Vignetting::CALIBRATED_RADIUS / self.vignetting_scale)
    }

    /// Whether the geometry changes: the distortion or the CA moves
    /// something.
    pub fn moves(&self) -> bool {
        self.distortion.is_some_and(|d| !d.is_identity())
            || self.chromatic_aberration.is_some_and(|c| !c.is_identity())
    }

    /// Whether the correction does anything at all.
    pub fn is_identity(&self) -> bool {
        !self.moves() && !self.vignetting.is_some_and(|v| !v.is_identity())
    }

    /// The source position, pixels with the origin at the top left, of
    /// a channel's light for an output pixel position, at
    /// magnification `scale`, for a picture `w` by `h`.
    #[inline]
    pub fn source_of(
        &self,
        channel: usize,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        scale: f32,
    ) -> (f32, f32) {
        let half = w.min(h) / 2.0;
        let (cx, cy) = (w / 2.0, h / 2.0);
        // To half shorter sides about the center, magnified, in the
        // models' units.
        let k = self.radius_scale;
        let (mut u, mut v) = ((x - cx) / half / scale * k, (y - cy) / half / scale * k);
        if let Some(d) = &self.distortion {
            (u, v) = d.distorted(u, v);
        }
        if let Some(c) = &self.chromatic_aberration {
            (u, v) = c.shifted(channel, u, v);
        }
        (cx + u / k * half, cy + v / k * half)
    }

    /// The vignetting gain at a source position.
    #[inline]
    pub fn gain_at(&self, x: f32, y: f32, w: f32, h: f32) -> f32 {
        let Some(vig) = &self.vignetting else {
            return 1.0;
        };
        let (dx, dy) = (x - w / 2.0, y - h / 2.0);
        let r = (dx * dx + dy * dy).sqrt() / (w * w + h * h).sqrt() * 2.0 * self.vignetting_scale;
        1.0 / vig.falloff(r).max(MIN_FALLOFF)
    }

    /// The magnification in force for a picture `w` by `h`: the fixed
    /// one, or the smallest at which every edge pixel of the output
    /// has its green light on the source.
    pub fn scale_for(&self, w: f32, h: f32) -> f32 {
        match self.scale {
            Scale::Fixed(s) => s.max(MIN_SCALE),
            Scale::Auto => {
                if !self.moves() {
                    return 1.0;
                }
                let fits = |s: f32| self.edge_fits(w, h, s);
                // Larger shows less of the source, so a scale that
                // fits stays fitting as it grows; bisect on that.
                let (mut lo, mut hi) = (MIN_SCALE, MAX_SCALE);
                if !fits(hi) {
                    return hi;
                }
                if fits(lo) {
                    return lo;
                }
                for _ in 0..40 {
                    let mid = (lo + hi) / 2.0;
                    if fits(mid) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                hi
            }
        }
    }

    /// Whether every pixel along the output's edge has its green light
    /// on the source at magnification `scale`.
    fn edge_fits(&self, w: f32, h: f32, scale: f32) -> bool {
        let inside = |p: (f32, f32)| p.0 >= 0.0 && p.1 >= 0.0 && p.0 <= w && p.1 <= h;
        let steps = 256;
        (0..=steps).all(|i| {
            let t = i as f32 / steps as f32;
            [(t * w, 0.0), (t * w, h), (0.0, t * h), (w, t * h)]
                .into_iter()
                .all(|(x, y)| inside(self.source_of(1, x, y, w, h, scale)))
        })
    }
}

/// A fall-off below this is taken as this: no gain past a hundred.
const MIN_FALLOFF: f32 = 0.01;
const MIN_SCALE: f32 = 0.25;
const MAX_SCALE: f32 = 4.0;

/// The picture corrected: the same size, resampled where the geometry
/// moves, its gain changed where the vignetting says.
pub fn correct(image: &WorkingImage, correction: &LensCorrection) -> (WorkingImage, LensStats) {
    let (w, h) = (image.width as f32, image.height as f32);
    let scale = correction.scale_for(w, h);
    let corner_gain = correction
        .vignetting
        .map(|_| correction.gain_at(0.0, 0.0, w, h));
    if !correction.moves() {
        let mut out = image.clone();
        if correction.vignetting.is_some() {
            out.data
                .par_chunks_mut(image.width * 3)
                .enumerate()
                .for_each(|(y, row)| vignette_row(row, y, correction, (w, h)));
        }
        return (
            out,
            LensStats {
                scale,
                resampled: false,
                corner_gain,
            },
        );
    }
    let mut out = WorkingImage::new(image.width, image.height);
    let vignetted = correction.vignetting.is_some();
    out.data
        .par_chunks_mut(image.width * 3)
        .enumerate()
        .for_each(|(y, row)| resample_row(row, y, image, correction, (w, h), scale, vignetted));
    (
        out,
        LensStats {
            scale,
            resampled: true,
            corner_gain,
        },
    )
}

/// Row `y` of the picture brightened by the vignetting's gain.
///
/// This and [`resample_row`] are out of line so the row arrives as an
/// argument: inside rayon's closure it comes out of the enumerate tuple,
/// a reference loaded from memory that carries no promise it is distinct
/// from anything else, and every write to it had the correction's
/// fields read again for the next pixel.
#[inline(never)]
fn vignette_row(row: &mut [f32], y: usize, correction: &LensCorrection, (w, h): (f32, f32)) {
    for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        let g = correction.gain_at(x as f32 + 0.5, y as f32 + 0.5, w, h);
        for v in px {
            *v *= g;
        }
    }
}

/// Row `y` of the corrected picture, resampled from `image`.
#[inline(never)]
fn resample_row(
    row: &mut [f32],
    y: usize,
    image: &WorkingImage,
    correction: &LensCorrection,
    (w, h): (f32, f32),
    scale: f32,
    vignetted: bool,
) {
    for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        let (ox, oy) = (x as f32 + 0.5, y as f32 + 0.5);
        for (channel, v) in px.iter_mut().enumerate() {
            let (sx, sy) = correction.source_of(channel, ox, oy, w, h, scale);
            if sx < 0.0 || sy < 0.0 || sx >= w || sy >= h {
                *v = 0.0;
                continue;
            }
            let mut s = sample_cubic(image, channel, sx, sy);
            if vignetted {
                s *= correction.gain_at(sx, sy, w, h);
            }
            *v = s;
        }
    }
}

/// Catmull-Rom's weights for the four taps about a position `t` (0 to
/// 1) past the second.
#[inline]
fn weights(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

/// One channel of the image at a position, pixel centers at halves,
/// edges clamped, Catmull-Rom.
#[inline]
pub fn sample_cubic(image: &WorkingImage, channel: usize, x: f32, y: f32) -> f32 {
    let (w, h) = (image.width as i64, image.height as i64);
    let fx = (x - 0.5).floor();
    let fy = (y - 0.5).floor();
    let wx = weights(x - 0.5 - fx);
    let wy = weights(y - 0.5 - fy);
    let mut out = 0f32;
    for (j, wyj) in wy.iter().enumerate() {
        let sy = (fy as i64 + j as i64 - 1).clamp(0, h - 1) as usize;
        for (i, wxi) in wx.iter().enumerate() {
            let sx = (fx as i64 + i as i64 - 1).clamp(0, w - 1) as usize;
            out += wxi * wyj * image.data[(sy * image.width + sx) * 3 + channel];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scaled_aberration_is_the_scale_after_the_model() {
        let ca = ChromaticAberration {
            red: [1.001, -0.0004, 0.0002],
            blue: [0.9985, 0.0003, -0.0001],
        };
        let folded = ca.scaled(1.002, 0.997);
        for (x, y) in [(0.3, 0.1), (-0.7, 0.5), (0.9, -0.9)] {
            for (channel, m) in [(0, 1.002f32), (2, 0.997)] {
                let (sx, sy) = ca.shifted(channel, x, y);
                let (fx, fy) = folded.shifted(channel, x, y);
                assert!((fx - sx * m).abs() < 1e-6 && (fy - sy * m).abs() < 1e-6);
            }
            assert_eq!(folded.shifted(1, x, y), (x, y));
        }
        assert!(ChromaticAberration::IDENTITY.scaled(1.0, 1.0).is_identity());
    }

    /// A picture whose red is x, green is y, blue one: any sample
    /// says where it came from.
    fn ramp(w: usize, h: usize) -> WorkingImage {
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let k = (y * w + x) * 3;
                image.data[k] = x as f32;
                image.data[k + 1] = y as f32;
                image.data[k + 2] = 1.0;
            }
        }
        image
    }

    #[test]
    fn nothing_asked_is_nothing_done() {
        let c = LensCorrection::default();
        assert!(c.is_identity() && !c.moves());
        let image = ramp(64, 48);
        let (out, stats) = correct(&image, &c);
        assert_eq!(out, image);
        assert!(!stats.resampled && stats.scale == 1.0 && stats.corner_gain.is_none());
        // Zero coefficients are the identity too.
        let zero = LensCorrection {
            distortion: Some(Distortion::Ptlens {
                a: 0.0,
                b: 0.0,
                c: 0.0,
            }),
            chromatic_aberration: Some(ChromaticAberration::IDENTITY),
            vignetting: Some(Vignetting { k: [0.0; 3] }),
            ..Default::default()
        };
        assert!(zero.is_identity());
        assert_eq!(correct(&image, &zero).0, image);
    }

    #[test]
    fn the_models_hold_their_fixed_radii_and_move_the_rest() {
        // poly3 and ptlens hold r = 1; poly5 does not.
        let p3 = Distortion::Poly3 { k1: -0.05 };
        let (x, _) = p3.distorted(1.0, 0.0);
        assert!((x - 1.0).abs() < 1e-6);
        let (x, y) = p3.distorted(0.5, 0.0);
        // Barrel (k1 < 0): inside the unit radius the light landed
        // further out than it should have.
        assert!(x > 0.5 && y == 0.0, "{x}");
        let pt = Distortion::Ptlens {
            a: 0.01,
            b: -0.03,
            c: 0.02,
        };
        let (x, _) = pt.distorted(1.0, 0.0);
        assert!((x - 1.0).abs() < 1e-6);
        let p5 = Distortion::Poly5 { k1: 0.1, k2: 0.0 };
        let (x, _) = p5.distorted(1.0, 0.0);
        assert!((x - 1.1).abs() < 1e-6);
        // Radial: a point off the axes moves along its own ray.
        let (x, y) = pt.distorted(0.3, 0.4);
        assert!((x / y - 0.75).abs() < 1e-5);
        // ACM with only k1: radial in focal units; the same k1 scales
        // with the focal scale squared.
        let acm = Distortion::Acm {
            k: [0.1, 0.0, 0.0, 0.0, 0.0],
            focal_scale: 0.5,
        };
        let (x, _) = acm.distorted(1.0, 0.0);
        assert!((x - (1.0 + 0.1 * 0.25)).abs() < 1e-6, "{x}");
        // CA: red and blue scale about the green.
        let ca = ChromaticAberration {
            red: [1.001, 0.0, 0.0],
            blue: [0.999, 0.0, 0.0],
        };
        assert_eq!(ca.shifted(1, 0.5, 0.5), (0.5, 0.5));
        assert!(ca.shifted(0, 0.5, 0.0).0 > 0.5 && ca.shifted(2, 0.5, 0.0).0 < 0.5);
        // Vignetting: dark corners, nothing at the center.
        let v = Vignetting {
            k: [-0.5, 0.0, 0.0],
        };
        assert_eq!(v.falloff(0.0), 1.0);
        assert!((v.falloff(1.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn vignetting_alone_changes_the_gain_and_moves_nothing() {
        let image = ramp(60, 40);
        let c = LensCorrection {
            vignetting: Some(Vignetting {
                k: [-0.5, 0.0, 0.0],
            }),
            ..Default::default()
        };
        assert!(!c.moves() && !c.is_identity());
        let (out, stats) = correct(&image, &c);
        assert!(!stats.resampled);
        // The corner is at the half diagonal, r = 1: gain 2.
        assert!((stats.corner_gain.unwrap() - 2.0).abs() < 1e-5);
        // The center pixel is untouched, near enough: r is a fraction
        // of a pixel over the half diagonal.
        let k = (20 * 60 + 30) * 3;
        assert!((out.data[k + 2] - 1.0).abs() < 1e-3, "{}", out.data[k + 2]);
        // A corner pixel's blue rises toward 2, and stays a fixed
        // multiple of the source: nothing moved.
        let corner = out.data[2];
        assert!(corner > 1.9 && corner < 2.0, "{corner}");
        assert_eq!(out.data[0], 0.0 * corner);
        assert!((out.data[(39 * 60 + 59) * 3] - 59.0 * corner).abs() < 1e-3);
        // The scale for the vignetting says the calibration's sensor
        // was bigger: a smaller r, less gain.
        let smaller = LensCorrection {
            vignetting_scale: 0.5,
            ..c
        };
        let (out2, _) = correct(&image, &smaller);
        assert!(out2.data[2] < corner && out2.data[2] > 1.0);
    }

    #[test]
    fn vignetting_is_held_at_the_calibrated_corner() {
        // The Sigma 50mm f/1.4 DG HSM Art at f/5.6, a full-frame
        // calibration, on a GFX 100S II: the picture's corner is 1.27
        // of the calibration's half diagonal.
        let k = [-0.3879, -0.0701, 0.1637];
        let v = Vignetting { k };
        let poly = |r: f32| {
            let s = r * r;
            1.0 + k[0] * s + k[1] * s * s + k[2] * s * s * s
        };
        // The polynomial itself climbs back past the corner, to more
        // than one by 1.37; the model does not.
        assert!(poly(1.2) > poly(1.0) && poly(1.37) > 1.0);
        let edge = v.falloff(1.0);
        assert!((edge - poly(1.0)).abs() < 1e-6 && edge < 0.72);
        for r in [1.0001, 1.1, 1.2722, 1.37, 2.0] {
            assert_eq!(v.falloff(r), edge, "{r}");
        }
        // Across a picture whose corner is past the calibration's, the
        // gain never falls from the center out along the diagonal, and
        // is the edge's from where the calibration ends to the corner.
        let c = LensCorrection {
            vignetting: Some(v),
            vignetting_scale: 1.2722,
            ..Default::default()
        };
        let (w, h) = (4000.0, 3000.0);
        let steps = 400;
        let mut last = 0.0;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let g = c.gain_at(w / 2.0 * (1.0 - t), h / 2.0 * (1.0 - t), w, h);
            assert!(g >= last, "gain fell at {t}: {g} after {last}");
            last = g;
        }
        let at_edge = 1.0 / edge;
        assert!((last - at_edge).abs() < 1e-5);
        let t = c.vignetting_edge().unwrap();
        assert!((t - 1.0 / 1.2722).abs() < 1e-6);
        let past = t + 0.01;
        let g = c.gain_at(w / 2.0 * (1.0 - past), h / 2.0 * (1.0 - past), w, h);
        assert!((g - at_edge).abs() < 1e-5);
        // The same calibration on its own sensor, or a smaller one:
        // no edge short of the corner.
        assert!(
            LensCorrection {
                vignetting_scale: 1.0,
                ..c
            }
            .vignetting_edge()
            .is_none()
        );
        assert!(
            LensCorrection {
                vignetting: None,
                ..c
            }
            .vignetting_edge()
            .is_none()
        );
    }

    #[test]
    fn distortion_samples_where_the_model_says_at_the_scale_that_fits() {
        let image = ramp(90, 60);
        // Pincushion: the light landed closer to the center than it
        // should have outside r = 1, so the output's corners would be
        // empty at scale one; the auto scale magnifies.
        let c = LensCorrection {
            distortion: Some(Distortion::Poly3 { k1: 0.05 }),
            ..Default::default()
        };
        let (w, h) = (90.0, 60.0);
        let s = c.scale_for(w, h);
        assert!(s > 1.0 && s < 1.2, "{s}");
        assert!(c.edge_fits(w, h, s) && !c.edge_fits(w, h, s * 0.99));
        let (out, stats) = correct(&image, &c);
        assert!(stats.resampled && (stats.scale - s).abs() < 1e-6);
        // Away from the edges the ramp reads the source position the
        // model gives, and the blue plane stays one.
        for (x, y) in [(45usize, 30usize), (20, 12), (70, 50)] {
            let (sx, sy) = c.source_of(1, x as f32 + 0.5, y as f32 + 0.5, w, h, s);
            let k = (y * 90 + x) * 3;
            assert!(
                (out.data[k] - (sx - 0.5)).abs() < 0.05
                    && (out.data[k + 1] - (sy - 0.5)).abs() < 0.05,
                "{x},{y}: {:?} vs {sx},{sy}",
                &out.data[k..k + 3]
            );
            assert!((out.data[k + 2] - 1.0).abs() < 1e-4);
        }
        // Nothing empty anywhere at the auto scale.
        assert!(out.data.chunks(3).all(|p| p[2] > 0.99));
        // At a fixed scale of one the corners are empty.
        let fixed = LensCorrection {
            scale: Scale::Fixed(1.0),
            ..c
        };
        let (out, stats) = correct(&image, &fixed);
        assert_eq!(stats.scale, 1.0);
        assert_eq!(out.data[2], 0.0);
        // Barrel lets the auto scale show more than the source.
        let barrel = LensCorrection {
            distortion: Some(Distortion::Poly3 { k1: -0.05 }),
            ..Default::default()
        };
        let s = barrel.scale_for(w, h);
        assert!(s < 1.0 && s > 0.9, "{s}");
        // The center is fixed whatever the scale.
        let (cx, cy) = barrel.source_of(1, w / 2.0, h / 2.0, w, h, s);
        assert!((cx - w / 2.0).abs() < 1e-4 && (cy - h / 2.0).abs() < 1e-4);
    }

    #[test]
    fn chromatic_aberration_moves_red_and_blue_apart_from_green() {
        let image = ramp(80, 60);
        let c = LensCorrection {
            chromatic_aberration: Some(ChromaticAberration {
                red: [1.01, 0.0, 0.0],
                blue: [0.99, 0.0, 0.0],
            }),
            scale: Scale::Fixed(1.0),
            ..Default::default()
        };
        assert!(c.moves());
        let (out, _) = correct(&image, &c);
        // At the middle of the right edge, r = 40/30 along x: red is
        // read one percent further out, blue one percent in; green
        // stays put.
        let (x, y) = (70usize, 30usize);
        let k = (y * 80 + x) * 3;
        let dx = (x as f32 + 0.5) - 40.0;
        assert!(
            (out.data[k] - (40.0 + dx * 1.01 - 0.5)).abs() < 0.05,
            "{}",
            out.data[k]
        );
        assert!((out.data[k + 1] - (y as f32)).abs() < 0.05);
        // Blue is a flat plane; its sample is one wherever it is read.
        assert!((out.data[k + 2] - 1.0).abs() < 1e-4);
        // The radius scale changes where a radius of one falls, so the
        // same coefficients move a point a different amount only when
        // the model is not linear.
        let quad = LensCorrection {
            chromatic_aberration: Some(ChromaticAberration {
                red: [1.0, 0.0, 0.01],
                blue: [1.0, 0.0, 0.0],
            }),
            radius_scale: 2.0,
            scale: Scale::Fixed(1.0),
            ..Default::default()
        };
        let one = LensCorrection {
            radius_scale: 1.0,
            ..quad
        };
        let (w, h) = (80.0, 60.0);
        let a = quad.source_of(0, 70.5, 30.5, w, h, 1.0).0;
        let b = one.source_of(0, 70.5, 30.5, w, h, 1.0).0;
        assert!(a > b && b > 70.5, "{a} {b}");
    }

    #[test]
    fn the_cubic_reads_a_ramp_exactly() {
        let image = ramp(32, 24);
        assert_eq!(sample_cubic(&image, 0, 10.5, 20.5), 10.0);
        assert_eq!(sample_cubic(&image, 1, 10.5, 20.5), 20.0);
        let s = sample_cubic(&image, 0, 10.25, 20.75);
        assert!((s - 9.75).abs() < 1e-4);
        assert!((sample_cubic(&image, 1, 10.25, 20.75) - 20.25).abs() < 1e-4);
    }
}
