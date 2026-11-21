//! Adobe lens profiles (`.lcp`): the XMP files Camera Raw and Adobe's
//! Lens Profile Creator write, read from the user's own folder. None
//! is shipped; Adobe's license keeps its own files on the user's
//! machine, and a profile the user made with the creator is theirs.
//!
//! A profile holds one entry a sample: the focal length, focus
//! distance, and aperture it was measured at, and Adobe's camera model
//! for it (the "Adobe Camera Model" of the DNG and LCP documents): a
//! rectilinear distortion, a vignetting, and a chromatic model for red
//! and for blue against green. Each is a polynomial in units of the
//! focal length, `x = (u - u0) / (fx D)`, `D` the picture's longer
//! side in pixels:
//!
//! * distortion `x_d = x (1 + k1 r² + k2 r⁴ + k3 r⁶) + 2x (t1 y + t2 x)
//!   + t2 r²`, and `y_d` likewise with `t1 r²`;
//! * vignetting, the light kept, `1 + a1 r² + a2 r⁴ + a3 r⁶`;
//! * a channel's place from the green's, the distortion's polynomial
//!   again times a scale factor.
//!
//! [`Lcp::correction`] puts them into the engine's
//! [`LensCorrection`], the one the lensfun path fills: the distortion
//! as `acm` with the units folded into its coefficients, the
//! vignetting as `pa` with the same, and the chromatic models fitted
//! to the engine's radial `[v, c, b]`. What does not fit is said at
//! [`Lcp::correction`]. [`Sample`] keeps Adobe's model as the files
//! state it, evaluated in pixels: the reference the mapping is tested
//! against.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use greycard_core::develop::lens::{
    ChromaticAberration, Distortion, LensCorrection, Scale, Vignetting,
};
use greycard_core::raw::Shot;

use crate::{DEFAULT_DISTANCE, Error, FULL_FRAME_DIAGONAL, Wanted, bracket};

/// The file extension of a lens profile.
pub const EXTENSION: &str = "lcp";

/// One of Adobe's polynomial models: the distortion's, or a channel's
/// against the green.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    /// The focal length over the picture's longer side, as the file
    /// gives it.
    pub focal_x: f32,
    pub focal_y: f32,
    /// The optical center as fractions of the width and the height.
    pub center: (f32, f32),
    /// `k1`, `k2`, `k3`.
    pub radial: [f32; 3],
    /// `t1` (with y), `t2` (with x).
    pub tangential: [f32; 2],
    /// A channel's magnification against the green; one for the
    /// distortion.
    pub scale: f32,
}

/// Adobe's vignetting model: the light kept, by the radius in focal
/// lengths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VignetteModel {
    pub focal_x: f32,
    pub focal_y: f32,
    pub center: (f32, f32),
    pub k: [f32; 3],
}

/// One sample of a profile: where it was measured, and what.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Millimeters.
    pub focal: f32,
    /// Meters, when the file says.
    pub distance: Option<f32>,
    /// The f-number, when the file says.
    pub aperture: Option<f32>,
    pub distortion: Option<Model>,
    pub vignetting: Option<VignetteModel>,
    pub red: Option<Model>,
    pub blue: Option<Model>,
}

/// A lens profile: the lens, the body it was made on, the sensor's
/// size and shape, and its samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Lcp {
    /// The lens's name: the file's pretty name, else its plain one.
    pub lens: String,
    pub make: Option<String>,
    pub model: Option<String>,
    /// Whether it was made on raws, not on the camera's JPEGs.
    pub raw: bool,
    /// The crop factor of the sensor it was made on.
    pub crop_factor: f32,
    /// The longer side over the shorter of the pictures it was made
    /// on; three by two when the file does not say.
    pub aspect: f32,
    pub samples: Vec<Sample>,
}

impl Model {
    /// Where the light for an undistorted point landed, both in focal
    /// lengths about the center: the polynomial as Adobe states it.
    pub fn distorted(&self, x: f32, y: f32) -> (f32, f32) {
        let [k1, k2, k3] = self.radial;
        let [t1, t2] = self.tangential;
        let r2 = x * x + y * y;
        let f = 1.0 + r2 * (k1 + r2 * (k2 + r2 * k3)) + 2.0 * (t1 * y + t2 * x);
        (
            self.scale * (x * f + t2 * r2),
            self.scale * (y * f + t1 * r2),
        )
    }

    /// The pixel's place in focal lengths about the center, for a
    /// picture `w` by `h`, and back.
    fn focal_of(self, (x, y): (f32, f32), w: f32, h: f32) -> (f32, f32) {
        let d = w.max(h);
        (
            (x - self.center.0 * w) / (self.focal_x * d),
            (y - self.center.1 * h) / (self.focal_y * d),
        )
    }

    fn pixels_of(self, (x, y): (f32, f32), w: f32, h: f32) -> (f32, f32) {
        let d = w.max(h);
        (
            x * self.focal_x * d + self.center.0 * w,
            y * self.focal_y * d + self.center.1 * h,
        )
    }
}

impl VignetteModel {
    /// The light kept at a pixel of a picture `w` by `h`.
    pub fn falloff_at(&self, x: f32, y: f32, w: f32, h: f32) -> f32 {
        let d = w.max(h);
        let xf = (x - self.center.0 * w) / (self.focal_x * d);
        let yf = (y - self.center.1 * h) / (self.focal_y * d);
        let r2 = xf * xf + yf * yf;
        1.0 + r2 * (self.k[0] + r2 * (self.k[1] + r2 * self.k[2]))
    }
}

impl Sample {
    /// The reference: where a channel's light came from for an output
    /// pixel of a picture `w` by `h` made on the profile's own sensor,
    /// in pixels, as Adobe's model states it: the distortion, then
    /// the channel's model on the green's place.
    pub fn source_of(&self, channel: usize, x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
        let Some(d) = &self.distortion else {
            return (x, y);
        };
        let green = d.distorted_pixels((x, y), w, h);
        let model = match channel {
            0 => self.red.as_ref(),
            2 => self.blue.as_ref(),
            _ => None,
        };
        match model {
            Some(m) => m.distorted_pixels(green, w, h),
            None => green,
        }
    }

    /// The reference's gain at a pixel: one over the light kept.
    pub fn gain_at(&self, x: f32, y: f32, w: f32, h: f32) -> f32 {
        self.vignetting
            .map_or(1.0, |v| 1.0 / v.falloff_at(x, y, w, h))
    }
}

impl Model {
    fn distorted_pixels(&self, p: (f32, f32), w: f32, h: f32) -> (f32, f32) {
        let (x, y) = self.focal_of(p, w, h);
        self.pixels_of(self.distorted(x, y), w, h)
    }
}

impl Lcp {
    /// A profile from the file's text.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let doc = roxmltree::Document::parse(text).map_err(|e| Error::Xml(e.to_string()))?;
        let entries: Vec<roxmltree::Node> = doc
            .descendants()
            .filter(|n| n.has_tag_name_local("CameraProfiles"))
            .flat_map(|n| n.descendants().filter(|c| c.has_tag_name_local("li")))
            .map(resource)
            .collect();
        if entries.is_empty() {
            return Err(Error::Xml("no camera profiles in the file".into()));
        }
        // The lens, the body, and the sensor are the same in every
        // entry; the first that says, says.
        let first = |name: &str| entries.iter().find_map(|e| prop(*e, name));
        let lens = first("LensPrettyName")
            .or_else(|| first("Lens"))
            .ok_or_else(|| Error::Xml("the profile names no lens".into()))?
            .to_string();
        let crop_factor = first("SensorFormatFactor")
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|c| *c > 0.0)
            .unwrap_or(1.0);
        let dims = |name: &str| first(name).and_then(|s| s.parse::<f32>().ok());
        let aspect = match (dims("ImageWidth"), dims("ImageLength")) {
            (Some(w), Some(h)) if w > 0.0 && h > 0.0 => w.max(h) / w.min(h),
            _ => 1.5,
        };
        let raw = first("CameraRawProfile").is_none_or(|s| s.eq_ignore_ascii_case("true"));
        // A focal length in the file's units when a model leaves it
        // out: the focal length over the longer side, in millimeters.
        let longer = FULL_FRAME_DIAGONAL / crop_factor * aspect / (1.0 + aspect * aspect).sqrt();
        let samples: Vec<Sample> = entries.iter().filter_map(|e| sample(*e, longer)).collect();
        if samples.is_empty() {
            return Err(Error::Xml("no rectilinear samples in the profile".into()));
        }
        Ok(Self {
            lens,
            make: first("Make").map(str::to_string),
            model: first("Model").map(str::to_string),
            raw,
            crop_factor,
            aspect,
            samples,
        })
    }

    /// A profile from a file: a regular file (after any link) of at
    /// most [`LARGEST`] bytes, read no further than that.
    pub fn read(path: &Path) -> Result<Self, Error> {
        let refuse =
            |why: String| Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, why));
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            return Err(refuse("not a regular file".into()));
        }
        let too_large = || {
            refuse(format!(
                "larger than {LARGEST} bytes, too large for a lens profile"
            ))
        };
        if meta.len() > LARGEST {
            return Err(too_large());
        }
        let mut text = String::new();
        std::fs::File::open(path)?
            .take(LARGEST + 1)
            .read_to_string(&mut text)?;
        if text.len() as u64 > LARGEST {
            return Err(too_large());
        }
        Self::parse(&text)
    }

    /// The focal lengths sampled, shortest and longest.
    pub fn focal_range(&self) -> (f32, f32) {
        self.samples
            .iter()
            .fold((f32::INFINITY, 0.0), |(lo, hi), s| {
                (lo.min(s.focal), hi.max(s.focal))
            })
    }

    /// Which corrections the profile can make.
    pub fn offers(&self) -> Wanted {
        Wanted {
            distortion: self.samples.iter().any(|s| s.distortion.is_some()),
            chromatic_aberration: self
                .samples
                .iter()
                .any(|s| s.red.is_some() || s.blue.is_some()),
            vignetting: self.samples.iter().any(|s| s.vignetting.is_some()),
        }
    }

    /// Whether the profile was made on the body of `make` and `model`:
    /// its model, or its make and model together, the same letters and
    /// digits as the body's model, or as its make and model together.
    pub fn made_on(&self, make: &str, model: &str) -> bool {
        made_on(self.make.as_deref(), self.model.as_deref(), make, model)
    }

    /// A word in the log, once a profile, when its optical center sits
    /// more than a pixel off the middle of a picture `w` by `h`, which
    /// the engine's models, about the center, cannot follow.
    fn say_off_center(&self, w: f32, h: f32) {
        let off = self
            .samples
            .iter()
            .flat_map(|s| {
                [
                    s.distortion.map(|m| m.center),
                    s.vignetting.map(|m| m.center),
                ]
            })
            .flatten()
            .map(|(x, y)| ((x - 0.5) * w).hypot((y - 0.5) * h))
            .fold(0.0f32, f32::max);
        if off > 1.0 {
            warn_once(format!(
                "lens profile for {}: its optical center is {off:.1} px off the middle, corrected about the middle",
                self.lens
            ));
        }
    }

    /// The engine's correction for a picture `width` by `height` of
    /// the whole sensor of a body with `crop_factor`, at the focal
    /// length, aperture, and distance of `shot`, for the corrections
    /// `wanted`, the picture standing as the sensor does (see
    /// `LensCorrection::oriented` for one turned).
    ///
    /// Each sample is first put in the engine's units: the distortion
    /// and the channels in half the calibration sensor's shorter side,
    /// the vignetting in half its diagonal. Then the samples are
    /// interpolated: the distortion and the channels between the two
    /// focal lengths about the shot's (on a log scale) and, at each,
    /// between the two focus distances about the shot's (in diopters);
    /// the vignetting likewise over focal length and, at each, at the
    /// focus distance nearest the shot's, between the two apertures
    /// about it (in stops). A value the file does not record is the
    /// samples' middle focal length (on the same log scale), ten
    /// meters, and the widest aperture, as the lensfun path takes
    /// them. Adobe publishes no
    /// rule of its own; this is the one the lensfun path follows, on
    /// the scales the samples are spaced on.
    ///
    /// Not carried over: an optical center off the middle (the
    /// engine's models are about the picture's center; one more than
    /// a pixel off is said once in the log), a vertical focal length
    /// unlike the horizontal (the horizontal is taken), the main
    /// model's scale factor (the engine's own scale fills the frame),
    /// the green channel's own distortion model (the main distortion
    /// moves all three), and the chromatic models' tangential terms.
    /// The chromatic models' sixth-order radial polynomial is fitted
    /// by least squares, by area, to the engine's `v + c r + b r²` out
    /// to the picture's corner; a model with only `k1` fits exactly.
    pub fn correction(
        &self,
        width: usize,
        height: usize,
        crop_factor: f32,
        shot: &Shot,
        wanted: Wanted,
    ) -> LensCorrection {
        let (w, h) = (width as f32, height as f32);
        let aspect = w.max(h) / w.min(h).max(1.0);
        let crop = if crop_factor > 0.0 {
            crop_factor
        } else {
            self.crop_factor
        };
        self.say_off_center(w, h);
        let shorter =
            |crop: f32, aspect: f32| FULL_FRAME_DIAGONAL / crop / (1.0 + aspect * aspect).sqrt();
        let radius_scale = shorter(crop, aspect) / shorter(self.crop_factor, self.aspect);
        let (lo, hi) = self.focal_range();
        let focal = shot.focal_length.unwrap_or((lo * hi).sqrt());
        let distance = shot.focus_distance.unwrap_or(DEFAULT_DISTANCE);
        let a = self.aspect;
        // Half the calibration's shorter side in focal lengths, for a
        // focal length `fx` of the longer side.
        let half_shorter = |fx: f32| 1.0 / (2.0 * a * fx);
        // And half its diagonal.
        let half_diagonal = |fx: f32| (1.0 + 1.0 / (a * a)).sqrt() / (2.0 * fx);

        let distortion = wanted
            .distortion
            .then(|| {
                let points: Vec<Point<5>> = self
                    .samples
                    .iter()
                    .filter_map(|s| {
                        let m = s.distortion?;
                        let u = half_shorter(m.focal_x);
                        let [k1, k2, k3] = m.radial;
                        let [t1, t2] = m.tangential;
                        Some(Point {
                            focal: s.focal,
                            other: diopters(s.distance),
                            v: [
                                k1 * u.powi(2),
                                k2 * u.powi(4),
                                k3 * u.powi(6),
                                t1 * u,
                                t2 * u,
                            ],
                        })
                    })
                    .collect();
                interpolate(&points, focal, diopters(Some(distance)))
            })
            .flatten()
            .map(|k| Distortion::Acm {
                k,
                focal_scale: 1.0,
            });

        // The picture's corner in the calibration's half shorter
        // sides: how far the channels' fit must hold.
        let corner = radius_scale * (1.0 + aspect * aspect).sqrt();
        let chromatic_aberration = wanted
            .chromatic_aberration
            .then(|| {
                let channel = |pick: fn(&Sample) -> Option<Model>| -> Option<[f32; 3]> {
                    let points: Vec<Point<4>> = self
                        .samples
                        .iter()
                        .filter_map(|s| {
                            let m = pick(s)?;
                            let u = half_shorter(m.focal_x);
                            let [k1, k2, k3] = m.radial;
                            Some(Point {
                                focal: s.focal,
                                other: diopters(s.distance),
                                v: [m.scale, k1 * u.powi(2), k2 * u.powi(4), k3 * u.powi(6)],
                            })
                        })
                        .collect();
                    interpolate(&points, focal, diopters(Some(distance)))
                        .map(|p| fit_radial(p, corner))
                };
                let red = channel(|s| s.red);
                let blue = channel(|s| s.blue);
                (red.is_some() || blue.is_some()).then(|| ChromaticAberration {
                    red: red.unwrap_or(ChromaticAberration::IDENTITY.red),
                    blue: blue.unwrap_or(ChromaticAberration::IDENTITY.blue),
                })
            })
            .flatten();

        let vignetting = wanted
            .vignetting
            .then(|| {
                let points: Vec<(Point<3>, Option<f32>)> = self
                    .samples
                    .iter()
                    .filter_map(|s| {
                        let m = s.vignetting?;
                        let t = half_diagonal(m.focal_x);
                        let point = Point {
                            focal: s.focal,
                            other: s.aperture.map_or(0.0, stops),
                            v: [m.k[0] * t.powi(2), m.k[1] * t.powi(4), m.k[2] * t.powi(6)],
                        };
                        Some((point, s.distance))
                    })
                    .collect();
                interpolate_nearest_distance(&points, focal, distance, shot.f_number.map(stops))
            })
            .flatten()
            .map(|k| Vignetting { k });

        LensCorrection {
            distortion,
            chromatic_aberration,
            vignetting,
            radius_scale,
            vignetting_scale: self.crop_factor / crop,
            scale: Scale::Auto,
        }
    }
}

/// A sample's values in the engine's units, where it was measured: the
/// focal length, and the second axis (diopters or stops).
#[derive(Debug, Clone, Copy)]
struct Point<const N: usize> {
    focal: f32,
    other: f32,
    v: [f32; N],
}

/// A focus distance in diopters, nought for infinity or none said.
fn diopters(d: Option<f32>) -> f32 {
    d.filter(|d| *d > 0.0).map_or(0.0, |d| 1.0 / d)
}

/// An f-number in stops.
fn stops(n: f32) -> f32 {
    2.0 * n.max(0.1).log2()
}

/// The values at `focal` and `other`: between the two focal lengths
/// about it on a log scale, and at each between the two values of the
/// second axis about `other`.
fn interpolate<const N: usize>(points: &[Point<N>], focal: f32, other: f32) -> Option<[f32; N]> {
    let mut focals: Vec<f32> = points.iter().map(|p| p.focal).collect();
    focals.sort_by(f32::total_cmp);
    focals.dedup();
    let (lo, hi) = bracket(focals.iter().copied(), focal)?;
    let at = |f: f32| -> Option<[f32; N]> {
        let mut here: Vec<&Point<N>> = points.iter().filter(|p| p.focal == f).collect();
        here.sort_by(|a, b| a.other.total_cmp(&b.other));
        here.dedup_by(|a, b| a.other == b.other);
        let (a, b) = bracket(here.iter().map(|p| p.other), other)?;
        let t = if a == b {
            0.0
        } else {
            (other - here[a].other) / (here[b].other - here[a].other)
        };
        Some(mix(here[a].v, here[b].v, t))
    };
    let va = at(focals[lo])?;
    if lo == hi {
        return Some(va);
    }
    let vb = at(focals[hi])?;
    let t = (focal.max(0.1).ln() - focals[lo].ln()) / (focals[hi].ln() - focals[lo].ln());
    Some(mix(va, vb, t))
}

/// [`interpolate`] over focal length and aperture, at each focal
/// length taking only the samples at the focus distance nearest
/// `distance` by ratio. No aperture is the widest.
fn interpolate_nearest_distance<const N: usize>(
    points: &[(Point<N>, Option<f32>)],
    focal: f32,
    distance: f32,
    aperture: Option<f32>,
) -> Option<[f32; N]> {
    let mut focals: Vec<f32> = points.iter().map(|(p, _)| p.focal).collect();
    focals.sort_by(f32::total_cmp);
    focals.dedup();
    let (lo, hi) = bracket(focals.iter().copied(), focal)?;
    let at = |f: f32| -> Option<[f32; N]> {
        let here: Vec<&(Point<N>, Option<f32>)> =
            points.iter().filter(|(p, _)| p.focal == f).collect();
        let far = |d: Option<f32>| d.filter(|d| *d > 0.0).unwrap_or(1000.0);
        let off = |d: Option<f32>| (far(d).max(0.01) / distance.max(0.01)).ln().abs();
        let nearest = here
            .iter()
            .map(|(_, d)| *d)
            .min_by(|a, b| off(*a).total_cmp(&off(*b)))?;
        let at_distance: Vec<Point<N>> = here
            .iter()
            .filter(|(_, d)| far(*d) == far(nearest))
            .map(|(p, _)| *p)
            .collect();
        let widest = at_distance
            .iter()
            .map(|p| p.other)
            .fold(f32::INFINITY, f32::min);
        interpolate(&at_distance, f, aperture.unwrap_or(widest))
    };
    let va = at(focals[lo])?;
    if lo == hi {
        return Some(va);
    }
    let vb = at(focals[hi])?;
    let t = (focal.max(0.1).ln() - focals[lo].ln()) / (focals[hi].ln() - focals[lo].ln());
    Some(mix(va, vb, t))
}

fn mix<const N: usize>(a: [f32; N], b: [f32; N], t: f32) -> [f32; N] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// A channel's `s (1 + a1 r² + a2 r⁴ + a3 r⁶)`, `[s, a1, a2, a3]`, as
/// the engine's `v + c r + b r²` over `0..corner`: least squares, each
/// radius weighed by the ring of picture it stands for.
fn fit_radial([s, a1, a2, a3]: [f32; 4], corner: f32) -> [f32; 3] {
    if a2 == 0.0 && a3 == 0.0 {
        return [s, 0.0, s * a1];
    }
    let f = |r: f64| {
        let r2 = r * r;
        s as f64 * (1.0 + r2 * (a1 as f64 + r2 * (a2 as f64 + r2 * a3 as f64)))
    };
    // Normal equations for the basis 1, r, r².
    let mut m = [[0.0f64; 3]; 3];
    let mut y = [0.0f64; 3];
    let n = 256;
    for i in 0..n {
        let r = (i as f64 + 0.5) / n as f64 * corner.max(1e-3) as f64;
        let basis = [1.0, r, r * r];
        for j in 0..3 {
            for k in 0..3 {
                m[j][k] += r * basis[j] * basis[k];
            }
            y[j] += r * basis[j] * f(r);
        }
    }
    let x = solve3(m, y);
    [x[0] as f32, x[1] as f32, x[2] as f32]
}

/// `m x = y` for a 3 by 3 `m`, by Cramer's rule.
fn solve3(m: [[f64; 3]; 3], y: [f64; 3]) -> [f64; 3] {
    let det = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(m);
    std::array::from_fn(|c| {
        let mut mc = m;
        for r in 0..3 {
            mc[r][c] = y[r];
        }
        det(mc) / d
    })
}

/// The element that holds a resource's properties: its own
/// `rdf:Description` child when it has one (the attribute form), else
/// itself (`rdf:parseType="Resource"`).
fn resource<'a, 'i>(node: roxmltree::Node<'a, 'i>) -> roxmltree::Node<'a, 'i> {
    node.children()
        .find(|c| c.has_tag_name_local("Description"))
        .unwrap_or(node)
}

/// A property of a resource by its local name, as an attribute or as
/// a child element's text.
fn prop<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == name)
        .map(|a| a.value())
        .or_else(|| {
            node.children()
                .find(|c| c.has_tag_name_local(name))
                .and_then(|c| c.text())
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn number(node: roxmltree::Node, name: &str) -> Option<f32> {
    prop(node, name)?
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children()
        .find(|c| c.has_tag_name_local(name))
        .map(resource)
}

trait Local {
    fn has_tag_name_local(&self, name: &str) -> bool;
}

impl Local for roxmltree::Node<'_, '_> {
    fn has_tag_name_local(&self, name: &str) -> bool {
        self.is_element() && self.tag_name().name() == name
    }
}

/// One entry's sample: its rectilinear model, or none for an entry
/// with only a fisheye model or none at all. `longer` is the sensor's
/// longer side in millimeters, for a focal length the model leaves
/// out.
fn sample(entry: roxmltree::Node, longer: f32) -> Option<Sample> {
    let focal = number(entry, "FocalLength").filter(|f| *f > 0.0)?;
    let model = child(entry, "PerspectiveModel")?;
    let default_fx = focal / longer;
    let center = |n: roxmltree::Node| {
        (
            number(n, "ImageXCenter").unwrap_or(0.5),
            number(n, "ImageYCenter").unwrap_or(0.5),
        )
    };
    let focal_x = |n: roxmltree::Node, inherit: f32| {
        number(n, "FocalLengthX")
            .filter(|f| *f > 0.0)
            .unwrap_or(inherit)
    };
    let main_fx = focal_x(model, default_fx);
    let main_fy = number(model, "FocalLengthY")
        .filter(|f| *f > 0.0)
        .unwrap_or(main_fx);
    let polynomial = |n: roxmltree::Node| -> Option<Model> {
        let radial = [
            number(n, "RadialDistortParam1"),
            number(n, "RadialDistortParam2"),
            number(n, "RadialDistortParam3"),
        ];
        let scale = number(n, "ScaleFactor");
        if radial.iter().all(Option::is_none) && scale.is_none() {
            return None;
        }
        let fx = focal_x(n, main_fx);
        Some(Model {
            focal_x: fx,
            focal_y: number(n, "FocalLengthY").filter(|f| *f > 0.0).unwrap_or(fx),
            center: center(n),
            radial: radial.map(|v| v.unwrap_or(0.0)),
            tangential: [
                number(n, "TangentialDistortParam1").unwrap_or(0.0),
                number(n, "TangentialDistortParam2").unwrap_or(0.0),
            ],
            scale: scale.filter(|s| *s > 0.0).unwrap_or(1.0),
        })
    };
    // The main model's scale factor magnifies the whole picture, which
    // the engine's own scale does after the correction; it is not
    // carried over, here or in the reference.
    let distortion = polynomial(model).map(|m| Model {
        focal_y: main_fy,
        scale: 1.0,
        ..m
    });
    let vignetting = child(model, "VignetteModel").and_then(|n| {
        let k = [
            number(n, "VignetteModelParam1"),
            number(n, "VignetteModelParam2"),
            number(n, "VignetteModelParam3"),
        ];
        if k.iter().all(Option::is_none) {
            return None;
        }
        let fx = focal_x(n, main_fx);
        Some(VignetteModel {
            focal_x: fx,
            focal_y: number(n, "FocalLengthY").filter(|f| *f > 0.0).unwrap_or(fx),
            center: center(n),
            k: k.map(|v| v.unwrap_or(0.0)),
        })
    });
    let red = child(model, "ChromaticRedGreenModel").and_then(polynomial);
    let blue = child(model, "ChromaticBlueGreenModel").and_then(polynomial);
    Some(Sample {
        focal,
        distance: number(entry, "FocusDistance").filter(|d| *d > 0.0),
        // APEX: the aperture value is two log2 of the f-number.
        aperture: number(entry, "ApertureValue").map(|av| 2f32.powf(av / 2.0)),
        distortion,
        vignetting,
        red,
        blue,
    })
}

/// Whether a profile whose body is `own_make` and `own_model` was made
/// on the body of `make` and `model`: see [`Lcp::made_on`].
fn made_on(own_make: Option<&str>, own_model: Option<&str>, make: &str, model: &str) -> bool {
    let Some(own) = own_model else {
        return false;
    };
    let own_make = own_make.unwrap_or("");
    let body = [fold(model), fold(&format!("{make}{model}"))];
    [fold(own), fold(&format!("{own_make}{own}"))]
        .iter()
        .any(|o| !o.is_empty() && body.contains(o))
}

/// A name as its letters and digits alone, in lower case: how a
/// profile's camera is matched to a body's.
pub fn fold(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// `message` as a warning, the first time it is said: a develop runs
/// many times a session, and a profile that is missing is missing in
/// each.
pub(crate) fn warn_once(message: String) {
    static SAID: LazyLock<Mutex<std::collections::HashSet<String>>> =
        LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));
    if SAID
        .lock()
        .map(|mut s| s.insert(message.clone()))
        .unwrap_or(true)
    {
        log::warn!("{message}");
    }
}

/// A profile in the folder, as a picker lists it: what the head of the
/// file says, not its samples, which are read only for a profile
/// chosen or picked ([`load`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    /// The file's name with its extension, under the folder with its
    /// folders joined by `/`: what an edit stores.
    pub file: String,
    pub path: PathBuf,
    /// The lens, as the file names it.
    pub lens: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub raw: bool,
}

impl Listed {
    /// Whether the profile was made on the body of `make` and `model`,
    /// as [`Lcp::made_on`] says.
    pub fn made_on(&self, make: &str, model: &str) -> bool {
        made_on(self.make.as_deref(), self.model.as_deref(), make, model)
    }
}

/// How much of a file's head a listing reads for its row. Camera Raw
/// and Adobe's creator write the lens, the body, and the raw flag in
/// the first entry, in its first kilobyte or two.
const HEAD: u64 = 16 * 1024;
/// The largest file read as a profile. Adobe's largest are some
/// hundreds of kilobytes; a file past this is no lens profile.
const LARGEST: u64 = 16 * 1024 * 1024;
/// How many profiles read whole are kept: the chosen, the picked, and
/// the develop's, of a few frames.
const KEEP: usize = 16;

/// A row's fields from the head of a profile: the lens (its pretty
/// name, else its plain one), the body's make and model, and whether it
/// was made on raws, each the first value given, as [`Lcp::parse`]
/// takes them. None when the head names no lens.
fn header_of(head: &str) -> Option<(String, Option<String>, Option<String>, bool)> {
    let lens = field(head, "LensPrettyName").or_else(|| field(head, "Lens"))?;
    let raw = field(head, "CameraRawProfile").is_none_or(|s| s.eq_ignore_ascii_case("true"));
    Some((lens, field(head, "Make"), field(head, "Model"), raw))
}

/// The first value of the property `name` in `text` that is not
/// empty: an attribute (`stCamera:Make="..."`) or an element
/// (`<stCamera:Make>...</...>`), under any prefix, its entities
/// undone. A value the head cuts off is not taken.
fn field(text: &str, name: &str) -> Option<String> {
    let mut found: Vec<(usize, String)> = Vec::new();
    for (open, close) in [("=\"", '"'), ("='", '\''), (">", '<')] {
        let pattern = format!(":{name}{open}");
        for (at, _) in text.match_indices(&pattern) {
            let rest = &text[at + pattern.len()..];
            let Some(end) = rest.find(close) else {
                break;
            };
            let value = unescape(rest[..end].trim());
            if !value.is_empty() {
                found.push((at, value));
                break;
            }
        }
    }
    found.into_iter().min_by_key(|(at, _)| *at).map(|(_, v)| v)
}

/// XML's five named entities and its numeric ones, undone.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';') else {
            break;
        };
        let entity = &rest[1..end];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A row as last read, with the length and clock of the file it was
/// read from: a listing reads again only what changed.
struct Header {
    len: u64,
    stamp: Option<SystemTime>,
    /// `None` for a file whose head names no lens, said once.
    row: Option<(String, Option<String>, Option<String>, bool)>,
}

static HEADERS: LazyLock<Mutex<HashMap<PathBuf, Header>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The listing's row for the file at `path`, from its first [`HEAD`]
/// bytes, read once while its length and clock stand.
fn listed(file: String, path: PathBuf) -> Option<Listed> {
    let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file())?;
    let stat = (meta.len(), meta.modified().ok());
    let kept = HEADERS.lock().ok().and_then(|h| {
        h.get(&path)
            .filter(|h| (h.len, h.stamp) == stat)
            .map(|h| h.row.clone())
    });
    let row = match kept {
        Some(row) => row,
        None => {
            let mut head = Vec::new();
            let read = std::fs::File::open(&path)
                .and_then(|f| std::io::Read::read_to_end(&mut f.take(HEAD), &mut head));
            let row = match &read {
                Ok(_) => header_of(&String::from_utf8_lossy(&head)),
                Err(e) => {
                    warn_once(format!("lens profile {}: {e}", path.display()));
                    None
                }
            };
            if read.is_ok() && row.is_none() {
                warn_once(format!(
                    "lens profile {}: no lens named in its first {HEAD} bytes; not listed",
                    path.display()
                ));
            }
            if let Ok(mut headers) = HEADERS.lock() {
                headers.insert(
                    path.clone(),
                    Header {
                        len: stat.0,
                        stamp: stat.1,
                        row: row.clone(),
                    },
                );
            }
            row
        }
    };
    let (lens, make, model, raw) = row?;
    Some(Listed {
        file,
        path,
        lens,
        make,
        model,
        raw,
    })
}

/// A folder the walk read, and its clock as it found it: a profile
/// added, removed, or renamed in it moves the clock.
#[derive(Clone)]
struct Folder {
    path: PathBuf,
    stamp: Option<SystemTime>,
}

impl Folder {
    fn now(path: &Path) -> Folder {
        Folder {
            path: path.to_path_buf(),
            stamp: std::fs::metadata(path).and_then(|m| m.modified()).ok(),
        }
    }
}

/// Every profile in `dir` and its subfolders, by name: its path under
/// `dir` with folders joined by `/`, a file at the top by its bare file
/// name. The folder is walked as the camera profiles' is
/// (`greycard_core::profile_folder`), so Adobe's LensProfiles tree, one
/// folder a maker, can be copied or linked in whole. Only each file's
/// head is read, once while it stands; a file whose head names no lens
/// is passed over with a word in the log. Walked now, not held.
pub fn list_in(dir: &Path) -> Vec<Listed> {
    list_within(dir, &mut Vec::new())
}

/// The row of the profile named `file` in `dir`, read from its head as
/// a listing reads it: for a profile an edit names that the listing
/// held does not have yet.
pub fn row_in(dir: &Path, file: &str) -> Option<Listed> {
    listed(file.to_string(), path_in(dir, file)?)
}

fn list_within(dir: &Path, folders: &mut Vec<Folder>) -> Vec<Listed> {
    let kind = greycard_core::profile_folder::Kind {
        extension: EXTENSION,
        what: "lens profiles",
        named_with_extension: true,
    };
    let found = greycard_core::profile_folder::walk(
        dir,
        kind,
        greycard_core::profile_folder::BOUNDS,
        &mut |folder| folders.push(Folder::now(folder)),
    );
    let seen: std::collections::HashSet<PathBuf> = found.iter().map(|(_, p)| p.clone()).collect();
    let mut out: Vec<Listed> = found
        .into_iter()
        .filter_map(|(file, path)| listed(file, path))
        .collect();
    // The heads of files no longer there are let go.
    if let Ok(mut headers) = HEADERS.lock() {
        headers.retain(|p, _| !p.starts_with(dir) || seen.contains(p));
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

/// The folder's profiles as last walked.
#[derive(Clone, Debug)]
pub struct Listing {
    /// Changes when, and only when, a look finds a different list.
    pub generation: u64,
    pub entries: Arc<Vec<Listed>>,
}

/// A listing and the folders its walk read.
struct Held {
    listing: Listing,
    folders: Vec<Folder>,
}

static HELD: LazyLock<Mutex<HashMap<PathBuf, Held>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The listing of `dir` held from the last look, without a look at
/// the disk: what an open lists from, so it never waits on a walk.
/// None before the first look has ended.
pub fn held_in(dir: &Path) -> Option<Listing> {
    HELD.lock().ok()?.get(dir).map(|h| h.listing.clone())
}

/// The listing of `dir` brought up to date, now, on this thread: the
/// folder walked again when one of its folders' clocks has moved (a
/// profile added, removed, or renamed), when nothing is held, or when
/// `force`; else each listed file's head looked at again, which reads
/// only the files rewritten in place. Its generation moves only when
/// the list differs.
pub fn refresh_in(dir: &Path, force: bool) -> Listing {
    let held = HELD
        .lock()
        .ok()
        .and_then(|h| h.get(dir).map(|h| (h.listing.clone(), h.folders.clone())));
    let (entries, folders) = match held {
        Some((listing, folders))
            if !force
                && folders
                    .iter()
                    .all(|f| Folder::now(&f.path).stamp == f.stamp) =>
        {
            let entries: Vec<Listed> = listing
                .entries
                .iter()
                .filter_map(|l| listed(l.file.clone(), l.path.clone()))
                .collect();
            (entries, folders)
        }
        _ => {
            let mut folders = Vec::new();
            (list_within(dir, &mut folders), folders)
        }
    };
    let Ok(mut all) = HELD.lock() else {
        return Listing {
            generation: 0,
            entries: Arc::new(entries),
        };
    };
    let listing = match all.get(dir) {
        Some(h) if *h.listing.entries == entries => h.listing.clone(),
        _ => Listing {
            generation: GENERATION.fetch_add(1, Ordering::Relaxed) + 1,
            entries: Arc::new(entries),
        },
    };
    all.insert(
        dir.to_path_buf(),
        Held {
            listing: listing.clone(),
            folders,
        },
    );
    listing
}

/// What a look hands its listing to when it ends.
pub type Then = Box<dyn FnOnce(&Listing) + Send>;

/// The look asked for next, and whether a thread is looking.
struct Looks {
    running: bool,
    /// The folder, whether it is to be walked whatever its clocks say,
    /// and what the listing is handed to.
    next: Option<(PathBuf, bool, Then)>,
}

static LOOKS: Mutex<Looks> = Mutex::new(Looks {
    running: false,
    next: None,
});

/// The looking thread's mark taken down when a look or its `then`
/// panics, so a later look starts a thread again, and the panic said in
/// the log. (A thread that ends as it should takes it down itself, in
/// the step that finds nothing asked.)
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        if std::thread::panicking() {
            log::error!("lens profiles: a look panicked; the next look starts afresh");
            LOOKS.lock().unwrap_or_else(|e| e.into_inner()).running = false;
        }
    }
}

/// A look at `dir` ([`refresh_in`], walking it whatever its folders'
/// clocks say when `force` is set) off the caller's thread, `then`
/// handed the listing when it ends. One look runs at a time; one asked
/// meanwhile runs after it, and of several asked meanwhile only the
/// last `then` is kept (an open since has its own), walking if any of
/// them asked to.
pub fn look_in(dir: &Path, force: bool, then: Then) {
    let mut looks = LOOKS.lock().unwrap_or_else(|e| e.into_inner());
    let force = force
        || looks
            .next
            .as_ref()
            .is_some_and(|(d, f, _)| *f && d.as_path() == dir);
    looks.next = Some((dir.to_path_buf(), force, then));
    if looks.running {
        return;
    }
    looks.running = true;
    drop(looks);
    let spawned = std::thread::Builder::new()
        .name("lens profiles".into())
        .spawn(|| {
            let _running = Running;
            loop {
                // Idle in the same locked step that finds nothing
                // asked, so no request is left without a thread.
                let next = {
                    let mut looks = LOOKS.lock().unwrap_or_else(|e| e.into_inner());
                    match looks.next.take() {
                        Some(next) => next,
                        None => {
                            looks.running = false;
                            return;
                        }
                    }
                };
                let (dir, force, then) = next;
                then(&refresh_in(&dir, force));
            }
        });
    if let Err(e) = spawned {
        log::warn!("lens profiles: no thread to list them on: {e}");
        let mut looks = LOOKS.lock().unwrap_or_else(|e| e.into_inner());
        looks.running = false;
        looks.next = None;
    }
}

/// Where the profile of this name is in `dir`: a file name ending in
/// `.lcp` in any case, at the top or under folders joined by `/`, as
/// [`list_in`] names it. None for a name that is not a plain path
/// under `dir` or not a lens profile's.
pub fn path_in(dir: &Path, file: &str) -> Option<PathBuf> {
    let is_lcp = Path::new(file)
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case(EXTENSION));
    let path = is_lcp
        .then(|| greycard_core::profile_folder::join(dir, file))
        .flatten();
    if path.is_none() {
        log::warn!("lens profile {file:?} is not a lens profile's name in the profile directory");
    }
    path
}

/// The profile in a file, read once and kept while the file's length
/// and time stay as they were, [`KEEP`] of them at most. None, with a
/// word in the log, for one that is not there or will not read.
pub fn load(path: &Path) -> Option<Arc<Lcp>> {
    type Kept = HashMap<PathBuf, (u64, Option<SystemTime>, Option<Arc<Lcp>>)>;
    static KEPT: LazyLock<Mutex<Kept>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file());
    let Some(meta) = meta else {
        warn_once(format!("lens profile {}: not there", path.display()));
        return None;
    };
    let stat = (meta.len(), meta.modified().ok());
    if let Ok(kept) = KEPT.lock()
        && let Some((len, time, p)) = kept.get(path)
        && (*len, *time) == stat
    {
        return p.clone();
    }
    let read = match Lcp::read(path) {
        Ok(p) => Some(Arc::new(p)),
        Err(e) => {
            warn_once(format!("lens profile {}: {e}", path.display()));
            None
        }
    };
    if let Ok(mut kept) = KEPT.lock() {
        if kept.len() >= KEEP && !kept.contains_key(path) {
            kept.clear();
        }
        kept.insert(path.to_path_buf(), (stat.0, stat.1, read.clone()));
    }
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile written by hand in the element form Camera Raw
    /// writes: two focal lengths, two apertures at the first, and the
    /// chromatic models at 24 mm only.
    pub(crate) const HAND: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
    xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile">
   <photoshop:CameraProfiles>
    <rdf:Seq>
     <rdf:li rdf:parseType="Resource">
      <stCamera:Author>greycard tests</stCamera:Author>
      <stCamera:Make>Testmaker</stCamera:Make>
      <stCamera:Model>Testmaker One</stCamera:Model>
      <stCamera:CameraRawProfile>True</stCamera:CameraRawProfile>
      <stCamera:Lens>TM 24-70mm</stCamera:Lens>
      <stCamera:LensPrettyName>Testmaker 24-70mm F4</stCamera:LensPrettyName>
      <stCamera:SensorFormatFactor>1</stCamera:SensorFormatFactor>
      <stCamera:ImageWidth>6000</stCamera:ImageWidth>
      <stCamera:ImageLength>4000</stCamera:ImageLength>
      <stCamera:FocalLength>24</stCamera:FocalLength>
      <stCamera:FocusDistance>10</stCamera:FocusDistance>
      <stCamera:ApertureValue>4</stCamera:ApertureValue>
      <stCamera:PerspectiveModel rdf:parseType="Resource">
       <stCamera:Version>2</stCamera:Version>
       <stCamera:FocalLengthX>0.7</stCamera:FocalLengthX>
       <stCamera:FocalLengthY>0.7</stCamera:FocalLengthY>
       <stCamera:RadialDistortParam1>-0.1</stCamera:RadialDistortParam1>
       <stCamera:RadialDistortParam2>0.05</stCamera:RadialDistortParam2>
       <stCamera:RadialDistortParam3>-0.01</stCamera:RadialDistortParam3>
       <stCamera:TangentialDistortParam1>0.001</stCamera:TangentialDistortParam1>
       <stCamera:TangentialDistortParam2>-0.002</stCamera:TangentialDistortParam2>
       <stCamera:ChromaticRedGreenModel rdf:parseType="Resource">
        <stCamera:FocalLengthX>0.7</stCamera:FocalLengthX>
        <stCamera:FocalLengthY>0.7</stCamera:FocalLengthY>
        <stCamera:ScaleFactor>1.0004</stCamera:ScaleFactor>
        <stCamera:RadialDistortParam1>0.002</stCamera:RadialDistortParam1>
       </stCamera:ChromaticRedGreenModel>
       <stCamera:ChromaticBlueGreenModel rdf:parseType="Resource">
        <stCamera:FocalLengthX>0.7</stCamera:FocalLengthX>
        <stCamera:FocalLengthY>0.7</stCamera:FocalLengthY>
        <stCamera:ScaleFactor>0.9997</stCamera:ScaleFactor>
        <stCamera:RadialDistortParam1>-0.001</stCamera:RadialDistortParam1>
       </stCamera:ChromaticBlueGreenModel>
       <stCamera:VignetteModel rdf:parseType="Resource">
        <stCamera:FocalLengthX>0.7</stCamera:FocalLengthX>
        <stCamera:FocalLengthY>0.7</stCamera:FocalLengthY>
        <stCamera:VignetteModelParam1>-0.8</stCamera:VignetteModelParam1>
        <stCamera:VignetteModelParam2>0.3</stCamera:VignetteModelParam2>
        <stCamera:VignetteModelParam3>-0.05</stCamera:VignetteModelParam3>
       </stCamera:VignetteModel>
      </stCamera:PerspectiveModel>
     </rdf:li>
     <rdf:li>
      <rdf:Description
        stCamera:FocalLength="24" stCamera:FocusDistance="10"
        stCamera:ApertureValue="6">
       <stCamera:PerspectiveModel>
        <rdf:Description stCamera:Version="2"
          stCamera:FocalLengthX="0.7" stCamera:FocalLengthY="0.7">
         <stCamera:VignetteModel>
          <rdf:Description stCamera:FocalLengthX="0.7"
            stCamera:FocalLengthY="0.7"
            stCamera:VignetteModelParam1="-0.4"
            stCamera:VignetteModelParam2="0.1"
            stCamera:VignetteModelParam3="0"/>
         </stCamera:VignetteModel>
        </rdf:Description>
       </stCamera:PerspectiveModel>
      </rdf:Description>
     </rdf:li>
     <rdf:li rdf:parseType="Resource">
      <stCamera:FocalLength>70</stCamera:FocalLength>
      <stCamera:FocusDistance>10</stCamera:FocusDistance>
      <stCamera:ApertureValue>4</stCamera:ApertureValue>
      <stCamera:PerspectiveModel rdf:parseType="Resource">
       <stCamera:FocalLengthX>2.0</stCamera:FocalLengthX>
       <stCamera:FocalLengthY>2.0</stCamera:FocalLengthY>
       <stCamera:RadialDistortParam1>0.2</stCamera:RadialDistortParam1>
       <stCamera:VignetteModel rdf:parseType="Resource">
        <stCamera:FocalLengthX>2.0</stCamera:FocalLengthX>
        <stCamera:VignetteModelParam1>-2.0</stCamera:VignetteModelParam1>
       </stCamera:VignetteModel>
      </stCamera:PerspectiveModel>
     </rdf:li>
     <rdf:li rdf:parseType="Resource">
      <stCamera:FocalLength>50</stCamera:FocalLength>
      <stCamera:FisheyeModel rdf:parseType="Resource">
       <stCamera:RadialDistortParam1>0.5</stCamera:RadialDistortParam1>
      </stCamera:FisheyeModel>
     </rdf:li>
    </rdf:Seq>
   </photoshop:CameraProfiles>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;

    fn hand() -> Lcp {
        Lcp::parse(HAND).unwrap()
    }

    fn shot(focal: f32, f_number: Option<f32>) -> Shot {
        Shot {
            focal_length: Some(focal),
            f_number,
            focus_distance: Some(10.0),
            ..Default::default()
        }
    }

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol * (1.0 + a.abs().max(b.abs()))
    }

    #[test]
    fn the_profile_reads_in_both_of_the_files_forms() {
        let p = hand();
        assert_eq!(p.lens, "Testmaker 24-70mm F4");
        assert_eq!(p.model.as_deref(), Some("Testmaker One"));
        assert!(p.raw);
        assert_eq!(p.crop_factor, 1.0);
        assert!((p.aspect - 1.5).abs() < 1e-6);
        // The fisheye entry is not a rectilinear sample.
        assert_eq!(p.samples.len(), 3);
        assert_eq!(p.focal_range(), (24.0, 70.0));
        assert_eq!(p.offers(), Wanted::ALL);
        let a = &p.samples[0];
        assert_eq!(a.aperture, Some(4.0));
        let d = a.distortion.unwrap();
        assert_eq!(d.radial, [-0.1, 0.05, -0.01]);
        assert_eq!(d.tangential, [0.001, -0.002]);
        assert_eq!(a.red.unwrap().scale, 1.0004);
        // The attribute form: the vignetting alone, at f/8.
        let b = &p.samples[1];
        assert!(b.distortion.is_none() && b.red.is_none());
        assert_eq!(b.vignetting.unwrap().k, [-0.4, 0.1, 0.0]);
        assert!((b.aperture.unwrap() - 8.0).abs() < 1e-5);
        // A model that leaves out its focal length takes the main one.
        assert_eq!(p.samples[2].vignetting.unwrap().focal_x, 2.0);
        assert!(Lcp::parse("<x/>").is_err());
    }

    /// The mapped correction moves every channel where Adobe's model,
    /// evaluated in pixels as the files state it, does: exactly for
    /// the distortion and the vignetting, and for the chromatic
    /// models, which have only `k1` here, exactly too.
    #[test]
    fn the_correction_matches_adobes_model_on_its_own_sensor() {
        let p = hand();
        let (w, h) = (6000.0, 4000.0);
        let reference = &p.samples[0];
        let c = p.correction(6000, 4000, 1.0, &shot(24.0, Some(4.0)), Wanted::ALL);
        assert!((c.radius_scale - 1.0).abs() < 1e-6);
        assert!((c.vignetting_scale - 1.0).abs() < 1e-6);
        for &(x, y) in &[
            (0.0, 0.0),
            (6000.0, 4000.0),
            (3000.0, 0.0),
            (100.0, 3900.0),
            (4500.0, 1200.0),
            (3000.0, 2000.0),
        ] {
            for channel in 0..3 {
                let ours = c.source_of(channel, x, y, w, h, 1.0);
                let adobe = reference.source_of(channel, x, y, w, h);
                assert!(
                    (ours.0 - adobe.0).abs() < 0.05 && (ours.1 - adobe.1).abs() < 0.05,
                    "channel {channel} at ({x}, {y}): {ours:?} against {adobe:?}"
                );
            }
            let ours = c.gain_at(x, y, w, h);
            let adobe = reference.gain_at(x, y, w, h);
            assert!(
                close(ours, adobe, 1e-4),
                "gain at ({x}, {y}): {ours} against {adobe}"
            );
        }
    }

    #[test]
    fn a_sixth_order_channel_is_fitted_close_over_the_picture() {
        let mut p = hand();
        let red = p.samples[0].red.as_mut().unwrap();
        red.radial = [0.003, -0.002, 0.001];
        let reference = p.samples[0];
        let c = p.correction(6000, 4000, 1.0, &shot(24.0, Some(4.0)), Wanted::ALL);
        let (w, h) = (6000.0, 4000.0);
        let mut worst = 0.0f32;
        for i in 0..=20 {
            let t = i as f32 / 20.0;
            let (x, y) = (3000.0 + 3000.0 * t, 2000.0 + 2000.0 * t);
            let ours = c.source_of(0, x, y, w, h, 1.0);
            let adobe = reference.source_of(0, x, y, w, h);
            worst = worst.max((ours.0 - adobe.0).hypot(ours.1 - adobe.1));
        }
        // A tenth of a pixel on a 24 MP frame.
        assert!(worst < 0.1, "{worst}");
    }

    #[test]
    fn samples_are_interpolated_over_focal_length_and_aperture() {
        let p = hand();
        let at = |f: f32, n: Option<f32>| p.correction(6000, 4000, 1.0, &shot(f, n), Wanted::ALL);
        let k = |c: &LensCorrection| match c.distortion {
            Some(Distortion::Acm { k, .. }) => k,
            other => panic!("{other:?}"),
        };
        // The distortion's k1 in half shorter sides at either end:
        // k1 (1 / (2 a fx))².
        let k24 = -0.1 * (1.0f32 / (2.0 * 1.5 * 0.7)).powi(2);
        let k70 = 0.2 * (1.0f32 / (2.0 * 1.5 * 2.0)).powi(2);
        assert!(close(k(&at(24.0, None))[0], k24, 1e-5));
        assert!(close(k(&at(70.0, None))[0], k70, 1e-5));
        // Between, on a log scale of focal length.
        let t = (40f32.ln() - 24f32.ln()) / (70f32.ln() - 24f32.ln());
        assert!(close(k(&at(40.0, None))[0], k24 + (k70 - k24) * t, 1e-5));
        // Past the ends, the ends.
        assert!(close(k(&at(10.0, None))[0], k24, 1e-5));
        // The vignetting at 24 mm between f/4 and f/8, in stops: f/5.6
        // is half way.
        let t24 = (1.0f32 + 1.0 / 2.25).sqrt() / (2.0 * 0.7);
        let v = |c: &LensCorrection| c.vignetting.unwrap().k[0];
        let k4 = -0.8 * t24 * t24;
        let k8 = -0.4 * t24 * t24;
        assert!(close(v(&at(24.0, Some(4.0))), k4, 1e-5));
        assert!(close(v(&at(24.0, Some(8.0))), k8, 1e-5));
        let half = v(&at(24.0, Some(4.0 * 2f32.sqrt())));
        assert!(close(half, (k4 + k8) / 2.0, 1e-4), "{half}");
        // No aperture recorded: the widest.
        assert!(close(v(&at(24.0, None)), k4, 1e-5));
        // The chromatic models are at 24 mm only: everywhere theirs.
        assert_eq!(
            at(70.0, None).chromatic_aberration,
            at(24.0, None).chromatic_aberration
        );
        // Only what is wanted.
        let d = p.correction(
            6000,
            4000,
            1.0,
            &shot(24.0, None),
            Wanted {
                distortion: true,
                chromatic_aberration: false,
                vignetting: false,
            },
        );
        assert!(
            d.distortion.is_some() && d.chromatic_aberration.is_none() && d.vignetting.is_none()
        );
    }

    #[test]
    fn a_crop_body_reads_a_full_frame_profile_in_its_units() {
        let p = hand();
        let s = shot(24.0, Some(4.0));
        let full = p.correction(6000, 4000, 1.0, &s, Wanted::ALL);
        let crop = p.correction(6000, 4000, 1.6, &s, Wanted::ALL);
        assert!(close(crop.radius_scale, 1.0 / 1.6, 1e-5));
        assert!(close(crop.vignetting_scale, 1.0 / 1.6, 1e-5));
        // The crop picture's corner is the full frame's point 1/1.6 of
        // the way out: the same displacement, in the same millimeters.
        let (w, h) = (6000.0, 4000.0);
        let (cx, cy) = (w / 2.0, h / 2.0);
        let shown = crop.source_of(1, w, h, w, h, 1.0);
        let inner = (cx + cx / 1.6, cy + cy / 1.6);
        let there = full.source_of(1, inner.0, inner.1, w, h, 1.0);
        let moved_crop = ((shown.0 - w) / 1.0, (shown.1 - h) / 1.0);
        let moved_full = ((there.0 - inner.0) * 1.6, (there.1 - inner.1) * 1.6);
        assert!(
            (moved_crop.0 - moved_full.0).abs() < 0.05
                && (moved_crop.1 - moved_full.1).abs() < 0.05,
            "{moved_crop:?} against {moved_full:?}"
        );
        assert!(close(
            crop.gain_at(w, h, w, h),
            full.gain_at(inner.0, inner.1, w, h),
            1e-4
        ));
    }

    /// A portrait frame, the sensor turned a quarter clockwise before
    /// the correction, is corrected as Adobe's model corrects the
    /// sensor: the tangential terms turned with it.
    #[test]
    fn a_turned_frame_is_corrected_as_adobes_model_corrects_the_sensor() {
        use greycard_core::raw::Orientation;
        let p = hand();
        let reference = p.samples[0];
        let c = p
            .correction(4000, 6000, 1.0, &shot(24.0, Some(4.0)), Wanted::ALL)
            .oriented(Orientation::Rotate90);
        // `develop::orient`'s source for a turned pixel: (x, y) on the
        // frame is (y, 4000 - x) on the sensor.
        let to_sensor = |(x, y): (f32, f32)| (y, 4000.0 - x);
        for &(x, y) in &[
            (0.0f32, 0.0f32),
            (4000.0, 6000.0),
            (300.0, 5000.0),
            (2600.0, 900.0),
        ] {
            for channel in 0..3 {
                let ours = c.source_of(channel, x, y, 4000.0, 6000.0, 1.0);
                let ours = to_sensor(ours);
                let (sx, sy) = to_sensor((x, y));
                let adobe = reference.source_of(channel, sx, sy, 6000.0, 4000.0);
                assert!(
                    (ours.0 - adobe.0).abs() < 0.05 && (ours.1 - adobe.1).abs() < 0.05,
                    "channel {channel} at ({x}, {y}): {ours:?} against {adobe:?}"
                );
            }
        }
    }

    /// A profile made on another body is used only where the database
    /// knows this body's sensor; one made on this body always is.
    #[test]
    fn a_profile_for_another_body_wants_the_bodys_size() {
        let dir = std::env::temp_dir().join(format!("greycard-lcp-body-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hand.lcp");
        std::fs::write(&path, HAND).unwrap();
        let p = Lcp::read(&path).unwrap();
        assert!(p.made_on("Testmaker", "One") && p.made_on("Testmaker", "Testmaker One"));
        assert!(!p.made_on("Testmaker", "Two") && !p.made_on("Canon", "EOS R7"));
        let s = shot(24.0, Some(4.0));
        let picked = |make, model, db| {
            crate::picked_correction(
                db,
                (make, model, &s, true),
                crate::Pick::Adobe(&path),
                (6000, 4000),
                greycard_core::raw::Orientation::Normal,
            )
        };
        assert!(picked("Testmaker", "Testmaker One", None).is_some());
        assert!(picked("Canon", "EOS R7", None).is_none());
        let mut db = crate::Database::default();
        db.read_xml(crate::db::SAMPLE).unwrap();
        let c = picked("Canon", "EOS R7", Some(&db)).unwrap();
        assert!(close(c.radius_scale, 1.0 / 1.6, 1e-5));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    #[test]
    fn a_folder_lists_its_profiles_and_keeps_what_it_read() {
        let dir = std::env::temp_dir().join(format!("greycard-lcp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.LCP"), HAND).unwrap();
        std::fs::write(dir.join("a.lcp"), "not a profile").unwrap();
        std::fs::write(dir.join("c.dcp"), HAND).unwrap();
        let listed = list_in(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file, "b.LCP");
        assert_eq!(listed[0].lens, "Testmaker 24-70mm F4");
        let path = path_in(&dir, "b.LCP").unwrap();
        assert!(Arc::ptr_eq(&load(&path).unwrap(), &load(&path).unwrap()));
        assert!(path_in(&dir, "../b.LCP").is_none());
        assert!(load(&dir.join("gone.lcp")).is_none());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Adobe's LensProfiles tree, one folder a maker, is read whole: a
    /// profile in a subfolder is named by its path under the folder, a
    /// file at the top by its bare name as before, one file name in two
    /// folders is two profiles, and a hidden folder is passed over.
    #[test]
    fn the_subfolders_are_read() {
        let dir = std::env::temp_dir().join(format!("greycard-lcp-tree-{}", std::process::id()));
        for sub in ["Canon", "Sigma/Art", "Nikon", ".Trash"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        for file in [
            "Top.lcp",
            "Canon/EF 24-70.lcp",
            "Sigma/Art/EF 24-70.lcp",
            ".Trash/Gone.lcp",
        ] {
            std::fs::write(dir.join(file), HAND).unwrap();
        }
        std::fs::write(dir.join("Nikon/notes.txt"), "not a profile").unwrap();
        let names: Vec<String> = list_in(&dir).into_iter().map(|l| l.file).collect();
        assert_eq!(
            names,
            ["Canon/EF 24-70.lcp", "Sigma/Art/EF 24-70.lcp", "Top.lcp"]
        );
        for name in &names {
            let path = path_in(&dir, name).unwrap();
            assert!(load(&path).is_some(), "{name} reads by its name");
        }
        for bad in [
            "",
            "Canon/../Top.lcp",
            "Canon//EF.lcp",
            "Canon\\EF.lcp",
            "/Top.lcp",
            "./Top.lcp",
        ] {
            assert!(path_in(&dir, bad).is_none(), "{bad:?} is refused");
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A link back up the tree is walked once; a link to a folder
    /// outside is read as the folder it names.
    #[cfg(unix)]
    #[test]
    fn a_link_loop_is_walked_once() {
        let tag = std::process::id();
        let dir = std::env::temp_dir().join(format!("greycard-lcp-loop-{tag}"));
        let outside = std::env::temp_dir().join(format!("greycard-lcp-loop-outside-{tag}"));
        std::fs::create_dir_all(dir.join("Canon")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(dir.join("Canon/A.lcp"), HAND).unwrap();
        std::fs::write(outside.join("B.lcp"), HAND).unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("Canon/up")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("Linked")).unwrap();
        let names: Vec<String> = list_in(&dir).into_iter().map(|l| l.file).collect();
        assert_eq!(names, ["Canon/A.lcp", "Linked/B.lcp"]);
        greycard_core::testing::remove_dir_retry(&dir);
        greycard_core::testing::remove_dir_retry(&outside);
    }

    /// The head of a profile names its row as the whole file does, in
    /// the element form and the attribute form, its entities undone.
    #[test]
    fn the_head_names_the_row_as_the_whole_file_does() {
        let attributes = HAND
            .replace("<stCamera:Author>greycard tests</stCamera:Author>", "")
            .replace(
                "<rdf:li rdf:parseType=\"Resource\">",
                "<rdf:li rdf:parseType=\"Resource\" stCamera:Make=\"A &amp; B\" \
                 stCamera:Model='Two'>",
            );
        for text in [HAND.to_string(), attributes] {
            let whole = Lcp::parse(&text).unwrap();
            let head = header_of(&text).unwrap();
            assert_eq!(
                head,
                (whole.lens, whole.make, whole.model, whole.raw),
                "{text}"
            );
        }
        assert_eq!(
            header_of(&HAND.replace(
                "True</stCamera:CameraRawProfile>",
                "False</stCamera:CameraRawProfile>"
            ))
            .map(|h| h.3),
            Some(false)
        );
        assert_eq!(header_of("<x:xmpmeta/>"), None, "no lens, no row");
        assert_eq!(
            unescape("a &lt;&#65;&#x42;&gt; &bogus; &"),
            "a <AB> &bogus; &"
        );
    }

    /// A listing reads only each file's head: a profile whose samples
    /// are cut off is listed by its head (and will not load), and one
    /// whose head is past what the listing reads is not listed, though
    /// it loads by its name.
    #[test]
    fn a_listing_reads_only_the_head() {
        let dir = std::env::temp_dir().join(format!("greycard-lcp-head-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cut = &HAND[..HAND.find("<stCamera:PerspectiveModel").unwrap()];
        std::fs::write(dir.join("Cut.lcp"), cut).unwrap();
        let late = HAND.replacen(
            "<x:xmpmeta",
            &format!("<!-- {} --><x:xmpmeta", "x".repeat(HEAD as usize)),
            1,
        );
        std::fs::write(dir.join("Late.lcp"), late).unwrap();
        let listed = list_in(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(
            (listed[0].file.as_str(), listed[0].lens.as_str()),
            ("Cut.lcp", "Testmaker 24-70mm F4")
        );
        assert!(listed[0].made_on("Testmaker", "One"));
        assert!(load(&listed[0].path).is_none(), "its samples are cut off");
        assert!(row_in(&dir, "Late.lcp").is_none());
        assert!(load(&path_in(&dir, "Late.lcp").unwrap()).is_some());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// One look test at a time: the looks' queue is the process's, and
    /// a look asked while another waits takes its place.
    static LOOK_TESTS: Mutex<()> = Mutex::new(());

    /// The listing is held between looks: none before the first; a look
    /// with nothing changed keeps its generation; a profile added in a
    /// subfolder, or one rewritten in place, is a new one; and a look
    /// off the caller's thread hands its listing on.
    #[test]
    fn the_listing_is_held_between_looks() {
        let _one = LOOK_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("greycard-lcp-held-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Canon")).unwrap();
        std::fs::write(dir.join("Canon/A.lcp"), HAND).unwrap();
        assert!(held_in(&dir).is_none());
        let first = refresh_in(&dir, false);
        assert_eq!(first.entries.len(), 1);
        assert_eq!(held_in(&dir).unwrap().generation, first.generation);
        assert_eq!(refresh_in(&dir, false).generation, first.generation);
        assert_eq!(refresh_in(&dir, true).generation, first.generation);

        // Past the folder's clock's step, so the file added moves it.
        greycard_core::testing::a_moment(&dir.join("Canon"));
        std::fs::write(dir.join("Canon/B.lcp"), HAND).unwrap();
        let added = refresh_in(&dir, false);
        assert_ne!(added.generation, first.generation);
        assert_eq!(added.entries.len(), 2);

        // Rewritten in place under another lens, at another length, the
        // folder's clock standing: its head is read again.
        let renamed = HAND.replace("Testmaker 24-70mm F4", "Testmaker 24-70mm F4 II");
        std::fs::write(dir.join("Canon/B.lcp"), &renamed).unwrap();
        let rewritten = refresh_in(&dir, false);
        assert_ne!(rewritten.generation, added.generation);
        assert!(rewritten.entries.iter().any(|l| l.lens.ends_with(" II")));

        let (tx, rx) = std::sync::mpsc::channel();
        look_in(
            &dir,
            false,
            Box::new(move |listing| tx.send(listing.generation).unwrap()),
        );
        let looked = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        assert_eq!(looked, rewritten.generation);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A profile added where the folder's clock does not move (FAT, a
    /// share's cache) is found by a look that walks whatever the clocks
    /// say, and only by one.
    #[cfg(unix)]
    #[test]
    fn a_forced_look_finds_what_the_clocks_hide() {
        let _one = LOOK_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("greycard-lcp-forced-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Canon")).unwrap();
        std::fs::write(dir.join("Canon/A.lcp"), HAND).unwrap();
        assert_eq!(refresh_in(&dir, false).entries.len(), 1);
        let canon = dir.join("Canon");
        let clock = std::fs::metadata(&canon).unwrap().modified().unwrap();
        std::fs::write(canon.join("B.lcp"), HAND).unwrap();
        // The folder's clock put back, as a file system that keeps it.
        std::fs::File::open(&canon)
            .unwrap()
            .set_modified(clock)
            .unwrap();
        assert_eq!(refresh_in(&dir, false).entries.len(), 1, "the clocks stand");
        let (tx, rx) = std::sync::mpsc::channel();
        look_in(
            &dir,
            true,
            Box::new(move |listing| tx.send(listing.entries.len()).unwrap()),
        );
        let found = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        assert_eq!(found, 2, "walked");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A look whose `then` panics leaves the looks able to run: the
    /// next one asked starts a thread and hands its listing on.
    #[test]
    fn a_look_that_panics_does_not_stop_the_next() {
        let _one = LOOK_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("greycard-lcp-panic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("A.lcp"), HAND).unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        look_in(
            &dir,
            false,
            Box::new(move |_| {
                let _told = tx;
                panic!("a test's look");
            }),
        );
        // The sender dropped as the panic unwinds: the look is over.
        assert!(rx.recv_timeout(std::time::Duration::from_secs(30)).is_err());
        let started = std::time::Instant::now();
        while LOOKS.lock().unwrap_or_else(|e| e.into_inner()).running {
            assert!(started.elapsed().as_secs() < 30, "the mark stays up");
            std::thread::yield_now();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        look_in(
            &dir,
            false,
            Box::new(move |listing| tx.send(listing.entries.len()).unwrap()),
        );
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap(),
            1
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A file is read as a profile only when it is a regular file of a
    /// lens profile's size, and named as one.
    #[test]
    fn only_a_regular_file_of_a_profiles_size_is_read() {
        let dir = std::env::temp_dir().join(format!("greycard-lcp-read-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Folder.lcp")).unwrap();
        let said = Lcp::read(&dir.join("Folder.lcp")).unwrap_err().to_string();
        assert!(said.contains("not a regular file"), "{said}");
        let huge = std::fs::File::create(dir.join("Huge.lcp")).unwrap();
        huge.set_len(LARGEST + 1).unwrap();
        let said = Lcp::read(&dir.join("Huge.lcp")).unwrap_err().to_string();
        assert!(said.contains("too large"), "{said}");
        assert!(path_in(&dir, "Canon/Lens.txt").is_none());
        assert!(path_in(&dir, "Canon/Lens").is_none());
        assert_eq!(
            path_in(&dir, "Canon/Lens.LCP"),
            Some(dir.join("Canon").join("Lens.LCP"))
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }
}
