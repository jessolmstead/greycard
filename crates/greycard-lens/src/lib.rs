//! Lens profiles: the lensfun database (lensfun.github.io, CC BY-SA
//! 3.0), read from the user's cache or the system's copy, fetched on
//! first use, and matched to a file's camera and lens; a match gives
//! the engine a [`LensCorrection`] for the picture at hand, its
//! calibrations interpolated to the focal length, aperture and
//! distance the file records.
//!
//! The engine never sees the database: `greycard-core` has the
//! models, this crate has the data and the matching, and a consumer
//! (the editor, the CLI) joins them.

use std::path::PathBuf;

pub mod db;
pub mod matching;
pub mod store;

pub use db::{Camera, Database, Lens};
pub use store::{Progress, Store};

use db::{DistortionModel, Lens as DbLens};
use greycard_core::develop::lens::{
    ChromaticAberration, Distortion, LensCorrection, Scale, Vignetting,
};
use greycard_core::raw::{RawFrame, Shot};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no cache directory: XDG_CACHE_HOME is not set and the platform names none")]
    NoCacheDir,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("fetching {url}: {message}")]
    Http { url: String, message: String },
    #[error("lens database: {0}")]
    Xml(String),
    #[error("no lens database in {0}")]
    Empty(PathBuf),
    #[error("no source answered for the lens database")]
    NoSource,
}

/// The diagonal of a full frame, millimeters: what a crop factor is
/// against.
const FULL_FRAME_DIAGONAL: f32 = 43.266_6;

/// A lens found for a file, with the body when that was found too.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Profile<'a> {
    pub lens: &'a Lens,
    pub camera: Option<&'a Camera>,
}

/// Which corrections a consumer wants of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wanted {
    pub distortion: bool,
    pub chromatic_aberration: bool,
    pub vignetting: bool,
}

impl Wanted {
    pub const ALL: Self = Self {
        distortion: true,
        chromatic_aberration: true,
        vignetting: true,
    };
}

/// What the database made of a file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lookup<'a> {
    pub camera: Option<&'a Camera>,
    pub lens: Option<&'a Lens>,
    /// Whether the lens was taken from the body, a compact's fixed
    /// lens for a file that names none.
    pub from_body: bool,
}

impl Database {
    /// The file's camera and lens, as far as they are in the database.
    pub fn lookup(&self, frame: &RawFrame) -> Lookup<'_> {
        self.lookup_shot(&frame.make, &frame.model, &frame.shot, true)
    }

    /// The same for what a file says of its body and its lens, for a
    /// raw or for a picture that is not one. `raw` says whether the
    /// picture is the sensor's data or one the camera rendered, which
    /// decides a compact's fixed lens (see [`Database::fixed_lens`]).
    pub fn lookup_shot(&self, make: &str, model: &str, shot: &Shot, raw: bool) -> Lookup<'_> {
        let camera = self.camera(make, model);
        let named = shot
            .lens_model
            .as_deref()
            .and_then(|name| self.lens(camera, shot.lens_make.as_deref(), name, shot.focal_length));
        // A file that names no lens may be a compact's, whose lens is
        // its body's.
        let fixed = shot
            .lens_model
            .is_none()
            .then(|| camera.and_then(|c| self.fixed_lens(c, shot.focal_length, raw)))
            .flatten();
        let lens = named.or(fixed);
        // Debug, not info: a develop looks the lens up more than
        // once, and the editor says what it found once, on the open.
        match (lens, shot.lens_model.as_deref()) {
            (Some(l), None) => {
                log::debug!("{make} {model}: no lens named, the body's {}", l.name())
            }
            (Some(l), Some(name)) => log::debug!(
                "lens {name}: {}{}",
                l.name(),
                if camera.is_some() {
                    ""
                } else {
                    " (body not in the database)"
                }
            ),
            (None, Some(name)) => log::debug!("lens {name} on {make} {model}: no match"),
            (None, None) => log::debug!("{make} {model}: the file names no lens"),
        }
        Lookup {
            camera,
            lens,
            from_body: fixed.is_some(),
        }
    }

    /// The profile for a file: its lens found, with its body when
    /// that was.
    pub fn profile(&self, frame: &RawFrame) -> Option<Profile<'_>> {
        self.profile_for(&frame.make, &frame.model, &frame.shot, true)
    }

    /// The profile for what a file says of its body and its lens.
    pub fn profile_for(
        &self,
        make: &str,
        model: &str,
        shot: &Shot,
        raw: bool,
    ) -> Option<Profile<'_>> {
        let l = self.lookup_shot(make, model, shot, raw);
        l.lens.map(|lens| Profile {
            lens,
            camera: l.camera,
        })
    }
}

impl Profile<'_> {
    pub fn name(&self) -> &str {
        self.lens.name()
    }

    /// The crop factor of the sensor the picture was made on: the
    /// body's, or the lens's own format when the body is unknown.
    pub fn crop_factor(&self) -> f32 {
        self.camera.map_or(self.lens.crop_factor, |c| c.crop_factor)
    }

    /// How far the picture's corner reaches in the calibration's half
    /// diagonals: the vignetting's radius scale, one when the two
    /// sensors are one size.
    pub fn reach(&self) -> f32 {
        self.lens.crop_factor / self.crop_factor()
    }

    /// Whether the calibration was made on a sensor smaller than the
    /// picture's, so the picture's corners are past where the profile
    /// was measured and its vignetting is held at its edge there. A
    /// percent over is the database's rounding (a full-frame lens
    /// calibrated at 1.005), not a smaller sensor.
    pub fn measured_on_smaller_sensor(&self) -> bool {
        self.reach() > 1.01
    }

    /// Which corrections the profile can make.
    pub fn offers(&self) -> Wanted {
        Wanted {
            distortion: !self.lens.distortion.is_empty(),
            chromatic_aberration: !self.lens.tca.is_empty(),
            vignetting: !self.lens.vignetting.is_empty(),
        }
    }

    /// The engine's correction for a picture `width` by `height`
    /// pixels of the whole sensor, at the focal length, aperture and
    /// distance of `shot` (the calibrations' nearest when a value is
    /// not recorded), for the corrections `wanted` that the profile
    /// has. Nothing where the profile has nothing.
    pub fn correction(
        &self,
        width: usize,
        height: usize,
        shot: &Shot,
        wanted: Wanted,
    ) -> LensCorrection {
        let (w, h) = (width as f32, height as f32);
        let aspect = w.max(h) / w.min(h).max(1.0);
        let crop = self.crop_factor();
        let lens = self.lens;
        // The picture's radius in half shorter sides to the
        // calibration's: the two sensors' shorter sides, from their
        // crop factors and shapes.
        let shorter =
            |crop: f32, aspect: f32| FULL_FRAME_DIAGONAL / crop / (1.0 + aspect * aspect).sqrt();
        let radius_scale = shorter(crop, aspect) / shorter(lens.crop_factor, lens.aspect_ratio);
        // lensfun's `pa` radius is one at the calibration sensor's
        // corner whatever its shape: the half diagonals, from the crop
        // factors alone.
        let vignetting_scale = self.reach();
        let focal = shot
            .focal_length
            .unwrap_or_else(|| lens.focal_range().map_or(50.0, |(lo, hi)| (lo + hi) / 2.0));
        let distortion = if wanted.distortion {
            distortion_at(lens, focal).map(|(model, real_focal)| match model {
                DistortionModel::Poly3 { k1 } => Distortion::Poly3 { k1 },
                DistortionModel::Poly5 { k1, k2 } => Distortion::Poly5 { k1, k2 },
                DistortionModel::Ptlens { a, b, c } => Distortion::Ptlens { a, b, c },
                DistortionModel::Acm { k } => Distortion::Acm {
                    k,
                    // Half the shorter side in millimeters over the
                    // focal length, for a radius already taken to the
                    // calibration's units.
                    focal_scale: shorter(crop, aspect) / 2.0 / real_focal / radius_scale,
                },
            })
        } else {
            None
        };
        let chromatic_aberration = wanted
            .chromatic_aberration
            .then(|| tca_at(lens, focal))
            .flatten();
        let vignetting = wanted
            .vignetting
            .then(|| {
                vignetting_at(
                    lens,
                    focal,
                    shot.f_number,
                    shot.focus_distance.unwrap_or(DEFAULT_DISTANCE),
                )
            })
            .flatten();
        LensCorrection {
            distortion,
            chromatic_aberration,
            vignetting,
            radius_scale,
            vignetting_scale,
            scale: Scale::Auto,
        }
    }
}

/// The focus distance taken when the file records none, meters: far
/// enough that the vignetting is the lens's at infinity, near enough
/// that a calibration at ten meters is nearer than one at a thousand.
const DEFAULT_DISTANCE: f32 = 10.0;

/// The distortion model at a focal length: the calibration there, or
/// the two about it blended when they are of one model, or the nearer
/// otherwise. With it the real focal length, the nominal when not
/// measured.
fn distortion_at(lens: &DbLens, focal: f32) -> Option<(DistortionModel, f32)> {
    let cal = &lens.distortion;
    let (lo, hi) = bracket(cal.iter().map(|c| c.focal), focal)?;
    let (a, b) = (&cal[lo], &cal[hi]);
    let real = |c: &db::DistortionCalibration| c.real_focal.unwrap_or(c.focal);
    if lo == hi {
        return Some((a.model, real(a)));
    }
    let t = (focal - a.focal) / (b.focal - a.focal);
    let mix = |x: f32, y: f32| x + (y - x) * t;
    let model = match (a.model, b.model) {
        (DistortionModel::Poly3 { k1: x }, DistortionModel::Poly3 { k1: y }) => {
            DistortionModel::Poly3 { k1: mix(x, y) }
        }
        (DistortionModel::Poly5 { k1: x1, k2: x2 }, DistortionModel::Poly5 { k1: y1, k2: y2 }) => {
            DistortionModel::Poly5 {
                k1: mix(x1, y1),
                k2: mix(x2, y2),
            }
        }
        (
            DistortionModel::Ptlens {
                a: xa,
                b: xb,
                c: xc,
            },
            DistortionModel::Ptlens {
                a: ya,
                b: yb,
                c: yc,
            },
        ) => DistortionModel::Ptlens {
            a: mix(xa, ya),
            b: mix(xb, yb),
            c: mix(xc, yc),
        },
        (DistortionModel::Acm { k: x }, DistortionModel::Acm { k: y }) => DistortionModel::Acm {
            k: std::array::from_fn(|i| mix(x[i], y[i])),
        },
        _ => {
            return Some(if t < 0.5 {
                (a.model, real(a))
            } else {
                (b.model, real(b))
            });
        }
    };
    Some((model, mix(real(a), real(b))))
}

fn tca_at(lens: &DbLens, focal: f32) -> Option<ChromaticAberration> {
    let cal = &lens.tca;
    let (lo, hi) = bracket(cal.iter().map(|c| c.focal), focal)?;
    let (a, b) = (&cal[lo], &cal[hi]);
    if lo == hi {
        return Some(ChromaticAberration {
            red: a.red,
            blue: a.blue,
        });
    }
    let t = (focal - a.focal) / (b.focal - a.focal);
    let mix = |x: [f32; 3], y: [f32; 3]| std::array::from_fn(|i| x[i] + (y[i] - x[i]) * t);
    Some(ChromaticAberration {
        red: mix(a.red, b.red),
        blue: mix(a.blue, b.blue),
    })
}

/// The vignetting at a focal length, aperture and distance: at each
/// of the two focal lengths about it, the calibrations at the
/// distance nearest (by ratio), blended over aperture; then the two
/// blended over focal length. Without an aperture the widest is
/// taken, since that is where a lens vignettes most and where a file
/// that says nothing was most likely made.
fn vignetting_at(
    lens: &DbLens,
    focal: f32,
    aperture: Option<f32>,
    distance: f32,
) -> Option<Vignetting> {
    let cal = &lens.vignetting;
    let mut focals: Vec<f32> = cal.iter().map(|c| c.focal).collect();
    focals.dedup();
    let (lo, hi) = bracket(focals.iter().copied(), focal)?;
    let at_focal = |f: f32| -> Option<[f32; 3]> {
        let here: Vec<_> = cal.iter().filter(|c| c.focal == f).collect();
        // The distance nearest by ratio, then only the entries at it.
        let nearest = here.iter().map(|c| c.distance).min_by(|a, b| {
            let d = |x: f32| (x.max(0.01) / distance.max(0.01)).ln().abs();
            d(*a).total_cmp(&d(*b))
        })?;
        let mut here: Vec<_> = here.into_iter().filter(|c| c.distance == nearest).collect();
        here.sort_by(|a, b| a.aperture.total_cmp(&b.aperture));
        here.dedup_by(|a, b| a.aperture == b.aperture);
        let aperture = aperture.unwrap_or(here[0].aperture);
        let (a, b) = bracket(here.iter().map(|c| c.aperture), aperture)?;
        if a == b {
            return Some(here[a].k);
        }
        let t = (aperture - here[a].aperture) / (here[b].aperture - here[a].aperture);
        Some(std::array::from_fn(|i| {
            here[a].k[i] + (here[b].k[i] - here[a].k[i]) * t
        }))
    };
    let ka = at_focal(focals[lo])?;
    if lo == hi {
        return Some(Vignetting { k: ka });
    }
    let kb = at_focal(focals[hi])?;
    let t = (focal - focals[lo]) / (focals[hi] - focals[lo]);
    Some(Vignetting {
        k: std::array::from_fn(|i| ka[i] + (kb[i] - ka[i]) * t),
    })
}

/// The indices of the two values about `x` in a sorted list: the same
/// index twice when `x` is at or beyond an end, or on a value. None
/// for an empty list.
fn bracket(values: impl Iterator<Item = f32>, x: f32) -> Option<(usize, usize)> {
    let values: Vec<f32> = values.collect();
    let n = values.len();
    if n == 0 {
        return None;
    }
    if x <= values[0] {
        return Some((0, 0));
    }
    if x >= values[n - 1] {
        return Some((n - 1, n - 1));
    }
    let hi = values.iter().position(|v| *v >= x)?;
    if values[hi] == x {
        return Some((hi, hi));
    }
    Some((hi - 1, hi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::raw::{
        CfaPattern, LevelPattern, Levels, Orientation, Samples, SensorLayout,
    };

    fn db() -> Database {
        let mut db = Database::default();
        db.read_xml(db::SAMPLE).unwrap();
        db
    }

    fn frame(model: &str, lens: &str, focal: f32, f_number: f32) -> RawFrame {
        RawFrame {
            make: "Canon".into(),
            model: model.into(),
            other_names: Vec::new(),
            width: 60,
            height: 40,
            channels: 1,
            layout: SensorLayout::Cfa(CfaPattern::rggb()),
            samples: Samples::U16(vec![0; 60 * 40]),
            levels: Levels {
                black: LevelPattern::uniform(0.0, 1),
                white: LevelPattern::uniform(1000.0, 1),
            },
            as_shot_coefficients: None,
            calibrations: Vec::new(),
            crop: None,
            orientation: Orientation::Normal,
            shot: Shot {
                lens_make: Some("Canon".into()),
                lens_model: Some(lens.into()),
                focal_length: Some(focal),
                f_number: Some(f_number),
                ..Default::default()
            },
        }
    }

    #[test]
    fn brackets_find_the_neighbors() {
        let v = [24.0, 50.0, 105.0];
        assert_eq!(bracket(v.iter().copied(), 10.0), Some((0, 0)));
        assert_eq!(bracket(v.iter().copied(), 24.0), Some((0, 0)));
        assert_eq!(bracket(v.iter().copied(), 30.0), Some((0, 1)));
        assert_eq!(bracket(v.iter().copied(), 50.0), Some((1, 1)));
        assert_eq!(bracket(v.iter().copied(), 70.0), Some((1, 2)));
        assert_eq!(bracket(v.iter().copied(), 200.0), Some((2, 2)));
        assert_eq!(bracket(std::iter::empty(), 1.0), None);
    }

    #[test]
    fn a_profile_is_looked_up_and_its_calibrations_interpolated() {
        let db = db();
        let f = frame("EOS R6", "RF24-105mm F4 L IS USM", 37.0, 5.6);
        let p = db.profile(&f).unwrap();
        assert_eq!(p.name(), "Canon RF 24-105mm F4L IS USM");
        assert_eq!(p.camera.unwrap().models[1], "EOS R6");
        assert_eq!(p.crop_factor(), 1.0);
        assert_eq!(p.offers(), Wanted::ALL);
        let c = p.correction(6000, 4000, &f.shot, Wanted::ALL);
        // Same sensor, same shape: the units are the calibration's.
        assert!((c.radius_scale - 1.0).abs() < 1e-5 && (c.vignetting_scale - 1.0).abs() < 1e-5);
        // 37 mm is half way from 24 to 50: k1 half way from -0.02 to 0.01.
        assert!(
            matches!(c.distortion, Some(Distortion::Poly3 { k1 }) if (k1 + 0.005).abs() < 1e-6)
        );
        // CA linear entries at 24 and 105: 37 is 13/81 of the way.
        let ca = c.chromatic_aberration.unwrap();
        let t = 13.0 / 81.0;
        assert!((ca.red[0] - (1.0002 + (0.9999 - 1.0002) * t)).abs() < 1e-6);
        assert_eq!(ca.red[1], 0.0);
        // Vignetting at f/5.6 is between f/4 and f/8 at each focal,
        // then between the focals.
        let v = c.vignetting.unwrap();
        let at = |k4: f32, k8: f32| k4 + (k8 - k4) * (5.6 - 4.0) / 4.0;
        let expect = at(-1.0, -0.4) + (at(-0.6, -0.2) - at(-1.0, -0.4)) * t;
        assert!((v.k[0] - expect).abs() < 1e-5, "{} vs {expect}", v.k[0]);
        // Only what is wanted.
        let d = p.correction(
            6000,
            4000,
            &f.shot,
            Wanted {
                distortion: true,
                chromatic_aberration: false,
                vignetting: false,
            },
        );
        assert!(d.chromatic_aberration.is_none() && d.vignetting.is_none());
        assert!(d.distortion.is_some());
    }

    #[test]
    fn a_crop_body_rescales_a_full_frame_calibration() {
        let db = db();
        // The 50 on the R7 (crop 1.6): the picture's half shorter side
        // is 1.6 times smaller in millimeters, so a radius of one on it
        // is 1/1.6 in the calibration's units; likewise the diagonal.
        let f = frame("EOS R7", "RF50mm F1.8 STM", 50.0, 2.5);
        let p = db.profile(&f).unwrap();
        assert_eq!(p.crop_factor(), 1.6);
        let c = p.correction(6960, 4640, &f.shot, Wanted::ALL);
        assert!(
            (c.radius_scale - 1.0 / 1.6).abs() < 1e-4,
            "{}",
            c.radius_scale
        );
        assert!((c.vignetting_scale - 1.0 / 1.6).abs() < 1e-4);
        assert!(!p.measured_on_smaller_sensor() && c.vignetting_edge().is_none());
        // The other way, a calibration made at 1.6 on a full frame:
        // the picture's corner is 1.6 of the calibration's, and the
        // vignetting ends short of it.
        let mut small = p.lens.clone();
        small.crop_factor = 1.6;
        let full = db
            .profile(&frame("EOS R6", "RF50mm F1.8 STM", 50.0, 2.5))
            .unwrap();
        let q = Profile {
            lens: &small,
            camera: full.camera,
        };
        assert!(q.measured_on_smaller_sensor() && (q.reach() - 1.6).abs() < 1e-6);
        let e = q
            .correction(6000, 4000, &f.shot, Wanted::ALL)
            .vignetting_edge();
        assert!(e.is_some_and(|e| (e - 1.0 / 1.6).abs() < 1e-6), "{e:?}");
        // A percent over is rounding, not a smaller sensor.
        small.crop_factor = 1.005;
        assert!(
            !Profile {
                lens: &small,
                camera: full.camera
            }
            .measured_on_smaller_sensor()
        );
        // At f/2.5 exactly, the calibration's own values, the ten
        // meter one for a file without a distance.
        assert_eq!(c.vignetting.unwrap().k, [-0.2789, -0.7487, 0.3534]);
        // The lens's format when the body is not known.
        let unknown = frame("EOS R6 Mark II", "RF50mm F1.8 STM", 50.0, 1.8);
        let p = db.profile(&unknown).unwrap();
        assert!(p.camera.is_none());
        assert_eq!(p.crop_factor(), 1.0);
        // A square picture on a 3:2 calibration: the shorter side is
        // longer than a 3:2 sensor's of the same diagonal, so a radius
        // of one reaches further in the calibration's units.
        let c = p.correction(4000, 4000, &unknown.shot, Wanted::ALL);
        assert!(
            c.radius_scale > 1.0 && c.radius_scale < 1.3,
            "{}",
            c.radius_scale
        );
    }

    #[test]
    fn what_the_file_lacks_is_taken_from_the_calibrations() {
        let db = db();
        let mut f = frame("EOS R6", "RF50mm F1.8 STM", 50.0, 1.8);
        f.shot.f_number = None;
        f.shot.focal_length = None;
        let p = db.profile(&f).unwrap();
        let c = p.correction(6000, 4000, &f.shot, Wanted::ALL);
        // The widest aperture's vignetting.
        assert_eq!(c.vignetting.unwrap().k, [-1.8308, 1.8716, -0.8660]);
        assert!(matches!(c.distortion, Some(Distortion::Ptlens { .. })));
        // No lens name: nothing.
        f.shot.lens_model = None;
        assert!(db.profile(&f).is_none());
        assert!(db.lookup(&f).camera.is_some());
    }

    /// Compacts as lensfun has them: each body on a mount of its own
    /// that no `<mount>` entry defines, its lenses on that mount.
    const COMPACTS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<lensdatabase version="2">
    <camera>
        <maker>Canon</maker>
        <model>Canon PowerShot SX710 HS</model>
        <model lang="en">PowerShot SX710 HS</model>
        <mount>canonSX710HS</mount>
        <cropfactor>5.6</cropfactor>
    </camera>
    <lens>
        <maker>Canon</maker>
        <model>Canon PowerShot SX710 HS &amp; compatibles, with CHDK's DNG</model>
        <model lang="en">fixed lens, with CHDK's DNG</model>
        <mount>canonSX710HS</mount>
        <cropfactor>5.6</cropfactor>
        <aspect-ratio>4:3</aspect-ratio>
        <calibration>
            <distortion model="ptlens" focal="4.5" a="0.03396" b="-0.11175" c="-0.01779"/>
            <distortion model="ptlens" focal="135.0" a="0.00333" b="-0.02616" c="0.03388"/>
        </calibration>
    </lens>
    <camera>
        <maker>Canon</maker>
        <model>Canon PowerShot SX510 HS</model>
        <model lang="en">PowerShot SX510 HS</model>
        <mount>canonSX510HS</mount>
        <cropfactor>5.6</cropfactor>
    </camera>
    <lens>
        <maker>Canon</maker>
        <model>Canon PowerShot SX510 HS &amp; compatibles</model>
        <mount>canonSX510HS</mount>
        <cropfactor>5.6</cropfactor>
        <calibration>
            <distortion model="poly3" focal="4.3" k1="-0.0068"/>
            <distortion model="poly3" focal="129" k1="0"/>
        </calibration>
    </lens>
    <lens>
        <maker>Canon</maker>
        <model>Canon PowerShot SX510 HS &amp; compatibles, with CHDK's DNG</model>
        <mount>canonSX510HS</mount>
        <cropfactor>5.6</cropfactor>
        <calibration>
            <distortion model="ptlens" focal="4.3" a="0.094" b="-0.309" c="0.163"/>
            <distortion model="ptlens" focal="129" a="0" b="0" c="0"/>
        </calibration>
    </lens>
    <camera>
        <maker>Casio</maker>
        <model>QV-3500EX</model>
        <mount>casioQV3500</mount>
        <cropfactor>4.9</cropfactor>
    </camera>
    <lens>
        <maker>Casio</maker>
        <model>QV-3500EX &amp; compatibles (Standard)</model>
        <mount>casioQV3500</mount>
        <cropfactor>4.9</cropfactor>
        <calibration>
            <distortion model="ptlens" focal="7" a="0.01" b="-0.03" c="0"/>
            <distortion model="ptlens" focal="21" a="0" b="0" c="0"/>
        </calibration>
    </lens>
    <lens>
        <maker>Casio</maker>
        <model>QV-3500EX &amp; compatibles, with Raynox 0.66x</model>
        <mount>casioQV3500</mount>
        <cropfactor>4.9</cropfactor>
        <calibration>
            <distortion model="ptlens" focal="7" a="0.05" b="-0.1" c="0"/>
            <distortion model="ptlens" focal="10" a="0.04" b="-0.08" c="0"/>
        </calibration>
    </lens>
    <camera>
        <maker>Nikon</maker>
        <model>E995</model>
        <mount>nikon995</mount>
        <cropfactor>4.8</cropfactor>
    </camera>
    <lens>
        <maker>Nikon</maker>
        <model>E995 &amp; compatibles, with WC-E63</model>
        <mount>nikon995</mount>
        <cropfactor>4.8</cropfactor>
        <calibration>
            <distortion model="ptlens" focal="8" a="0.02" b="-0.05" c="0"/>
            <distortion model="ptlens" focal="10" a="0.01" b="-0.03" c="0"/>
        </calibration>
    </lens>
    <lens>
        <maker>Nikon</maker>
        <model>E995 &amp; compatibles, with TC-E2</model>
        <mount>nikon995</mount>
        <cropfactor>4.8</cropfactor>
        <calibration>
            <distortion model="ptlens" focal="20" a="0.03" b="-0.06" c="0"/>
            <distortion model="ptlens" focal="30" a="0.01" b="-0.02" c="0"/>
        </calibration>
    </lens>
    <camera>
        <maker>Canon</maker>
        <model>Canon EOS Odd</model>
        <mount>Canon Odd</mount>
        <cropfactor>1.6</cropfactor>
    </camera>
    <lens>
        <maker>Canon</maker>
        <model>Canon Odd 18-55mm</model>
        <mount>Canon Odd</mount>
        <mount>Canon RF</mount>
        <cropfactor>1.6</cropfactor>
        <calibration>
            <distortion model="poly3" focal="18" k1="-0.02"/>
            <distortion model="poly3" focal="55" k1="0.01"/>
        </calibration>
    </lens>
</lensdatabase>
"#;

    fn with_compacts() -> Database {
        let mut db = db();
        db.read_xml(COMPACTS).unwrap();
        db
    }

    fn unnamed(focal: Option<f32>) -> Shot {
        Shot {
            focal_length: focal,
            f_number: Some(4.0),
            ..Default::default()
        }
    }

    /// The name of the lens a raw (or a rendered picture) from a body
    /// that names none is given.
    fn fixed(
        db: &Database,
        make: &str,
        model: &str,
        focal: Option<f32>,
        raw: bool,
    ) -> Option<String> {
        db.lookup_shot(make, model, &unnamed(focal), raw)
            .lens
            .map(|l| l.name().to_string())
    }

    #[test]
    fn a_compact_with_one_lens_gets_it_with_no_lens_named() {
        let db = with_compacts();
        let l = db.lookup_shot(
            "Canon",
            "Canon PowerShot SX710 HS",
            &unnamed(Some(5.5)),
            true,
        );
        assert_eq!(
            l.lens.unwrap().name(),
            "Canon PowerShot SX710 HS & compatibles, with CHDK's DNG"
        );
        assert!(l.from_body && l.camera.is_some());
        let mut f = frame("PowerShot SX710 HS", "", 5.5, 4.0);
        f.shot.lens_model = None;
        let p = db.profile(&f).unwrap();
        assert_eq!(p.crop_factor(), 5.6);
        assert!(
            p.correction(4608, 3456, &f.shot, Wanted::ALL)
                .distortion
                .is_some()
        );
        // Without a focal length the one lens is still the one.
        assert!(fixed(&db, "Canon", "PowerShot SX710 HS", None, true).is_some());
        // Past what it was calibrated for it is not taken.
        assert!(fixed(&db, "Canon", "PowerShot SX710 HS", Some(300.0), true).is_none());
        // A JPEG from the body is corrected already: the CHDK profile
        // is not for it.
        assert!(fixed(&db, "Canon", "PowerShot SX710 HS", Some(5.5), false).is_none());
    }

    #[test]
    fn a_raw_gets_the_chdk_profile_and_a_rendered_picture_the_plain_one() {
        let db = with_compacts();
        assert_eq!(
            fixed(&db, "Canon", "PowerShot SX510 HS", Some(4.3), true).as_deref(),
            Some("Canon PowerShot SX510 HS & compatibles, with CHDK's DNG")
        );
        assert_eq!(
            fixed(&db, "Canon", "PowerShot SX510 HS", Some(4.3), false).as_deref(),
            Some("Canon PowerShot SX510 HS & compatibles")
        );
        // A raw from a body with no CHDK entry gets the plain one.
        assert_eq!(
            fixed(&db, "Casio", "QV-3500EX", Some(14.0), true).as_deref(),
            Some("QV-3500EX & compatibles (Standard)")
        );
    }

    #[test]
    fn an_interchangeable_body_with_no_lens_named_gets_none() {
        let db = with_compacts();
        assert!(!db.is_fixed_mount("Canon RF") && !db.is_fixed_mount("Canon EF"));
        assert!(db.is_fixed_mount("canonSX710HS"));
        let l = db.lookup_shot("Canon", "EOS R6", &unnamed(Some(50.0)), true);
        assert!(l.camera.is_some() && l.lens.is_none() && !l.from_body);
        // A mount with no entry of its own, but with a lens that sits
        // on a defined mount too, is not a fixed lens's.
        assert!(!db.is_fixed_mount("Canon Odd"));
        assert!(fixed(&db, "Canon", "EOS Odd", Some(18.0), true).is_none());
    }

    #[test]
    fn a_fixed_mount_with_two_lenses_picks_by_focal_length_then_the_plain_body() {
        let db = with_compacts();
        // The converter's range is inside the plain one's: where both
        // fit, the plain body.
        for f in [Some(8.0), None] {
            assert_eq!(
                fixed(&db, "Casio", "QV-3500EX", f, true).as_deref(),
                Some("QV-3500EX & compatibles (Standard)")
            );
        }
        // Two converters and no plain body: the focal length alone
        // tells them apart, and where it cannot, no guess.
        assert_eq!(
            fixed(&db, "Nikon", "E995", Some(25.0), true).as_deref(),
            Some("E995 & compatibles, with TC-E2")
        );
        assert_eq!(
            fixed(&db, "Nikon", "E995", Some(9.0), true).as_deref(),
            Some("E995 & compatibles, with WC-E63")
        );
        assert!(fixed(&db, "Nikon", "E995", None, true).is_none());
    }

    #[test]
    fn a_named_lens_wins_over_the_body() {
        let db = with_compacts();
        // A lens named is matched by name, and a name that matches
        // nothing is not replaced by the body's.
        let mut shot = unnamed(Some(50.0));
        shot.lens_model = Some("RF50mm F1.8 STM".into());
        let l = db.lookup_shot("Canon", "PowerShot SX710 HS", &shot, true);
        assert_eq!(l.lens.unwrap().name(), "Canon RF 50mm F1.8 STM");
        assert!(!l.from_body);
        shot.lens_model = Some("4.5-135.0 mm".into());
        shot.focal_length = Some(5.5);
        assert!(
            db.lookup_shot("Canon", "PowerShot SX710 HS", &shot, true)
                .lens
                .is_none()
        );
    }
}
