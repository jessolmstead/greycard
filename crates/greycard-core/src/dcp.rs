//! DCP camera profiles: the file, its tags, and the hue/saturation/value
//! map that is the accurate half of one.
//!
//! A DCP is a TIFF whose magic number is `RC` (0x4352) instead of 42 and
//! whose single IFD holds DNG's profile tags: the matrix pair, the
//! forward matrices, a hue/saturation/value map per illuminant, a look
//! table, a tone curve and a baseline exposure offset. rawler's TIFF
//! reader refuses that magic and then goes looking for the sub-IFDs and
//! image data a profile does not have, so the reader here is our own:
//! one flat IFD, the handful of types DNG uses, bounds checked.
//!
//! What this module does not do is decide how a profile is rendered.
//! The matrices and the map are the camera's colorimetry and belong to
//! the profile stage ([`crate::develop::profile`]); the look table and
//! the tone curve are Adobe's rendering, so they are parsed, kept here,
//! and applied by nothing. They are a look for a later line (notes
//! §78).
//!
//! The map's interpolation, the illuminant weighting and the encoding
//! follow the DNG 1.7.1.0 specification. RawTherapee's
//! `rtengine/dcp.cc` (GPL-3.0-or-later; Jacques Desmis, Oliver Duis and
//! the RawTherapee developers) was read alongside it for what real
//! files carry and where the specification is quiet, but no code was
//! copied: what follows was written from the specification.

use std::path::Path;
use std::sync::Arc;

use rawcolor::{Calibration, CalibrationIlluminant, CameraProfile, Mat3};

use crate::color::Profile;
use crate::error::{Error, Result};

/// The magic number in a DCP's header, where a TIFF carries 42.
const MAGIC: u16 = 0x4352;

/// Tags, DNG's numbering.
mod tag {
    pub const UNIQUE_CAMERA_MODEL: u16 = 50708;
    pub const COLOR_MATRIX_1: u16 = 50721;
    pub const COLOR_MATRIX_2: u16 = 50722;
    pub const CALIBRATION_ILLUMINANT_1: u16 = 50778;
    pub const CALIBRATION_ILLUMINANT_2: u16 = 50779;
    pub const PROFILE_CALIBRATION_SIGNATURE: u16 = 50932;
    pub const PROFILE_NAME: u16 = 50936;
    pub const PROFILE_HUE_SAT_MAP_DIMS: u16 = 50937;
    pub const PROFILE_HUE_SAT_MAP_DATA_1: u16 = 50938;
    pub const PROFILE_HUE_SAT_MAP_DATA_2: u16 = 50939;
    pub const PROFILE_TONE_CURVE: u16 = 50940;
    pub const PROFILE_EMBED_POLICY: u16 = 50941;
    pub const PROFILE_COPYRIGHT: u16 = 50942;
    pub const FORWARD_MATRIX_1: u16 = 50964;
    pub const FORWARD_MATRIX_2: u16 = 50965;
    pub const PROFILE_LOOK_TABLE_DIMS: u16 = 50981;
    pub const PROFILE_LOOK_TABLE_DATA: u16 = 50982;
    pub const PROFILE_HUE_SAT_MAP_ENCODING: u16 = 51107;
    pub const PROFILE_LOOK_TABLE_ENCODING: u16 = 51108;
    pub const BASELINE_EXPOSURE_OFFSET: u16 = 51109;
}

/// Nothing sane is this large; a file that claims to be is refused
/// before it is read.
const MAX_FILE: u64 = 128 << 20;

/// What a profile says about being carried into a converted file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbedPolicy {
    /// 0: embed it wherever it goes.
    #[default]
    AllowCopying,
    /// 1: embed it, but it may not be moved on to another file.
    EmbedIfUsed,
    /// 2: do not embed it.
    EmbedNever,
    /// 3: no restriction at all.
    NoRestrictions,
    /// Anything else the file says.
    Other(u32),
}

impl EmbedPolicy {
    fn from_code(code: u32) -> Self {
        match code {
            0 => EmbedPolicy::AllowCopying,
            1 => EmbedPolicy::EmbedIfUsed,
            2 => EmbedPolicy::EmbedNever,
            3 => EmbedPolicy::NoRestrictions,
            other => EmbedPolicy::Other(other),
        }
    }

    fn code(self) -> u32 {
        match self {
            EmbedPolicy::AllowCopying => 0,
            EmbedPolicy::EmbedIfUsed => 1,
            EmbedPolicy::EmbedNever => 2,
            EmbedPolicy::NoRestrictions => 3,
            EmbedPolicy::Other(code) => code,
        }
    }
}

/// How a table's values are encoded: what curve the RGB is on before it
/// becomes HSV.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    /// 0: linear ProPhoto.
    #[default]
    Linear,
    /// 1: sRGB's transfer curve over ProPhoto's primaries.
    Srgb,
}

impl Encoding {
    fn from_code(code: u32) -> Self {
        match code {
            1 => Encoding::Srgb,
            _ => Encoding::Linear,
        }
    }
}

/// A hue/saturation/value map: a lattice of (hue shift in degrees,
/// saturation scale, value scale) over HSV of ProPhoto.
///
/// Hue wraps over `hue_divisions` steps of 360 degrees; saturation and
/// value run 0 to 1 over `sat_divisions - 1` and `val_divisions - 1`
/// steps and clamp outside. A `val_divisions` of 1 is a two dimensional
/// map, which acts the same at every value.
#[derive(Debug, Clone, PartialEq)]
pub struct HueSatMap {
    pub hue_divisions: usize,
    pub sat_divisions: usize,
    pub val_divisions: usize,
    pub encoding: Encoding,
    /// Value slowest, then hue, saturation fastest.
    pub data: Vec<[f32; 3]>,
}

/// The most nodes an axis of a table may have.
///
/// Real profiles use 90 hues and 30 saturations; dcamprof's largest
/// are 90x30x30. The cap is here so a file that claims four million
/// nodes an axis is turned away rather than overflowing the product of
/// the three, and it is far above anything a profile has reason to
/// carry.
pub const MAX_DIVISIONS: usize = 1024;

impl HueSatMap {
    /// A map of these dimensions that changes nothing.
    ///
    /// Panics above [`MAX_DIVISIONS`] on an axis: this is for building
    /// a map in hand, and a dimension that large is a mistake in the
    /// calling code, not in a file. A map read from a file goes
    /// through [`Dcp::from_bytes`], which refuses it instead.
    pub fn identity(hue_divisions: usize, sat_divisions: usize, val_divisions: usize) -> Self {
        assert!(
            hue_divisions <= MAX_DIVISIONS
                && sat_divisions <= MAX_DIVISIONS
                && val_divisions <= MAX_DIVISIONS,
            "a hue/sat map axis may have at most {MAX_DIVISIONS} nodes"
        );
        Self {
            hue_divisions,
            sat_divisions,
            val_divisions,
            encoding: Encoding::Linear,
            data: vec![[0.0, 1.0, 1.0]; hue_divisions * sat_divisions * val_divisions],
        }
    }

    fn check(&self) -> Result<()> {
        if self.hue_divisions == 0 || self.sat_divisions < 2 || self.val_divisions == 0 {
            return Err(Error::Profile(format!(
                "hue/sat map dimensions {}x{}x{} are not usable",
                self.hue_divisions, self.sat_divisions, self.val_divisions
            )));
        }
        if self.hue_divisions > MAX_DIVISIONS
            || self.sat_divisions > MAX_DIVISIONS
            || self.val_divisions > MAX_DIVISIONS
        {
            return Err(Error::Profile(format!(
                "hue/sat map dimensions {}x{}x{} are past the {MAX_DIVISIONS} a table may have \
                 on an axis",
                self.hue_divisions, self.sat_divisions, self.val_divisions
            )));
        }
        // Checked, not because the cap above leaves room to overflow,
        // but so that the multiply cannot be the thing that decides.
        let want = self
            .hue_divisions
            .checked_mul(self.sat_divisions)
            .and_then(|n| n.checked_mul(self.val_divisions))
            .ok_or_else(|| {
                Error::Profile(format!(
                    "hue/sat map dimensions {}x{}x{} do not multiply",
                    self.hue_divisions, self.sat_divisions, self.val_divisions
                ))
            })?;
        if self.data.len() != want {
            return Err(Error::Profile(format!(
                "hue/sat map of {}x{}x{} carries {} entries, not {want}",
                self.hue_divisions,
                self.sat_divisions,
                self.val_divisions,
                self.data.len()
            )));
        }
        Ok(())
    }

    /// Whether the map leaves every color alone.
    pub fn is_identity(&self) -> bool {
        self.data
            .iter()
            .all(|e| e[0] == 0.0 && e[1] == 1.0 && e[2] == 1.0)
    }

    #[inline]
    fn at(&self, h: usize, s: usize, v: usize) -> [f32; 3] {
        self.data[(v * self.hue_divisions + h) * self.sat_divisions + s]
    }

    /// The entry for a color: hue in degrees, saturation and value in 0
    /// to 1, trilinear between the lattice's nodes and wrapping in hue.
    pub fn lookup(&self, hue: f32, sat: f32, val: f32) -> [f32; 3] {
        let h = hue.rem_euclid(360.0) * (self.hue_divisions as f32 / 360.0);
        let h0 = (h.floor() as usize) % self.hue_divisions;
        let h1 = (h0 + 1) % self.hue_divisions;
        let hf = h - h.floor();

        let (s0, s1, sf) = span(sat, self.sat_divisions);
        let (v0, v1, vf) = span(val, self.val_divisions);

        std::array::from_fn(|c| {
            let plane = |v: usize| {
                let lo = lerp(self.at(h0, s0, v)[c], self.at(h1, s0, v)[c], hf);
                let hi = lerp(self.at(h0, s1, v)[c], self.at(h1, s1, v)[c], hf);
                lerp(lo, hi, sf)
            };
            let near = plane(v0);
            if v1 == v0 {
                near
            } else {
                lerp(near, plane(v1), vf)
            }
        })
    }

    /// `weight` of `self` and the rest of `other`, entry by entry: how
    /// DNG blends the two illuminants' maps, by the same weight the
    /// matrices are blended with.
    pub fn blend(&self, other: &HueSatMap, weight: f32) -> Result<HueSatMap> {
        if self.hue_divisions != other.hue_divisions
            || self.sat_divisions != other.sat_divisions
            || self.val_divisions != other.val_divisions
        {
            return Err(Error::Profile(
                "the two illuminants' hue/sat maps have different dimensions".into(),
            ));
        }
        let w = weight.clamp(0.0, 1.0);
        Ok(HueSatMap {
            data: self
                .data
                .iter()
                .zip(&other.data)
                .map(|(a, b)| std::array::from_fn(|c| b[c] + (a[c] - b[c]) * w))
                .collect(),
            ..self.clone()
        })
    }
}

/// Where a value sits between two nodes of a 0-to-1 axis, clamped.
#[inline]
fn span(v: f32, divisions: usize) -> (usize, usize, f32) {
    if divisions < 2 {
        return (0, 0, 0.0);
    }
    let scaled = v.clamp(0.0, 1.0) * (divisions - 1) as f32;
    let i = (scaled.floor() as usize).min(divisions - 2);
    (i, i + 1, scaled - i as f32)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Both illuminants' maps. `secondary` is absent when the file carries
/// one map, which then acts at every temperature.
#[derive(Debug, Clone, PartialEq)]
pub struct HueSatMaps {
    pub primary: HueSatMap,
    pub secondary: Option<HueSatMap>,
}

/// The look table: the same lattice as a hue/sat map, and Adobe's
/// rendering rather than the camera's colorimetry. Parsed, kept, and
/// applied by nothing here.
pub type LookTable = HueSatMap;

/// One illuminant's worth of a DCP.
#[derive(Debug, Clone, PartialEq)]
pub struct DcpCalibration {
    /// The EXIF LightSource code the file gives.
    pub illuminant: u16,
    /// `ColorMatrix`, row major: XYZ (D50) to camera.
    pub color_matrix: [f64; 9],
    /// `ForwardMatrix`, row major: white-balanced camera to XYZ (D50).
    pub forward_matrix: Option<[f64; 9]>,
    pub hue_sat_map: Option<HueSatMap>,
}

/// What a profile says about itself: everything but its tables.
///
/// What a listing of the profile directory needs, and all it reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DcpInfo {
    pub name: Option<String>,
    pub unique_camera_model: Option<String>,
    pub copyright: Option<String>,
    pub embed_policy: EmbedPolicy,
}

/// A camera profile as a DCP file holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Dcp {
    pub name: Option<String>,
    /// `UniqueCameraModel`: which camera the profile was made for.
    pub unique_camera_model: Option<String>,
    /// `ProfileCopyright`: shown beside the profile's name, so a
    /// profile whose licence says something says it.
    pub copyright: Option<String>,
    /// `ProfileCalibrationSignature`: whose calibration the profile
    /// expects a file's `CameraCalibration` matrices to be. Read and
    /// used by nothing: the engine builds no `CameraCalibration` of its
    /// own and leaves a file's alone, so there is nothing to match
    /// against. It is kept because a reader that does honor it would
    /// want it, and because dropping a tag on the way through
    /// [`Dcp::to_bytes`] would be a lie.
    pub calibration_signature: Option<String>,
    /// `ProfileEmbedPolicy`: what the profile's maker allows a
    /// converter to do with it. Read and honored by nothing, because
    /// nothing here embeds a profile in anything yet; the linear DNG
    /// writer carries the file's own tags and no profile. It becomes
    /// load bearing the day an export embeds one.
    pub embed_policy: EmbedPolicy,
    /// The lower-temperature calibration.
    pub primary: DcpCalibration,
    /// The higher-temperature calibration, when the profile has two.
    pub secondary: Option<DcpCalibration>,
    /// `BaselineExposureOffset`, stops. Read and reported; nothing here
    /// applies it.
    pub baseline_exposure_offset: Option<f32>,
    /// The look table, kept apart as a look.
    pub look: Option<LookTable>,
    /// `ProfileToneCurve` as (input, output) pairs, kept apart as a look.
    pub tone_curve: Option<Vec<[f32; 2]>>,
}

impl Dcp {
    /// Read a DCP from a file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let len = std::fs::metadata(path)?.len();
        if len > MAX_FILE {
            return Err(Error::Profile(format!(
                "{}: {len} bytes is too large for a camera profile",
                path.display()
            )));
        }
        let bytes = std::fs::read(path)?;
        Self::from_bytes(&bytes).map_err(|e| at_path(path, e))
    }

    /// Read a DCP from its bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let ifd = Ifd::parse(bytes)?;

        let dims = ifd.u32s(tag::PROFILE_HUE_SAT_MAP_DIMS)?;
        let map_encoding = ifd
            .u32s(tag::PROFILE_HUE_SAT_MAP_ENCODING)?
            .and_then(|v| v.first().copied())
            .map_or(Encoding::Linear, Encoding::from_code);
        let map = |tag| -> Result<Option<HueSatMap>> {
            let Some(data) = ifd.floats(tag)? else {
                return Ok(None);
            };
            let Some(dims) = &dims else {
                return Err(Error::Profile(
                    "a hue/sat map without ProfileHueSatMapDims".into(),
                ));
            };
            Ok(Some(table(dims, data, map_encoding)?))
        };

        let matrix = |tag| -> Result<Option<[f64; 9]>> {
            match ifd.floats(tag)? {
                None => Ok(None),
                Some(v) if v.len() == 9 => Ok(Some(std::array::from_fn(|i| v[i] as f64))),
                Some(v) => Err(Error::Profile(format!(
                    "tag {tag} is a matrix of {} values, not 9",
                    v.len()
                ))),
            }
        };

        let illuminant = |tag| -> Result<Option<u16>> {
            Ok(ifd
                .u32s(tag)?
                .and_then(|v| v.first().copied())
                .map(|v| v as u16))
        };

        let one = matrix(tag::COLOR_MATRIX_1)?.ok_or_else(|| {
            Error::Profile("no ColorMatrix1: this is not a usable camera profile".into())
        })?;
        let first = DcpCalibration {
            illuminant: illuminant(tag::CALIBRATION_ILLUMINANT_1)?.unwrap_or(0),
            color_matrix: one,
            forward_matrix: matrix(tag::FORWARD_MATRIX_1)?,
            hue_sat_map: map(tag::PROFILE_HUE_SAT_MAP_DATA_1)?,
        };
        let second = match matrix(tag::COLOR_MATRIX_2)? {
            None => None,
            Some(m) => Some(DcpCalibration {
                illuminant: illuminant(tag::CALIBRATION_ILLUMINANT_2)?.unwrap_or(0),
                color_matrix: m,
                forward_matrix: matrix(tag::FORWARD_MATRIX_2)?,
                hue_sat_map: map(tag::PROFILE_HUE_SAT_MAP_DATA_2)?,
            }),
        };

        // The pair in DNG's order, the cooler illuminant second, so the
        // weighting and the maps line up with what rawcolor does with
        // them.
        let (primary, secondary) = match second {
            Some(second) if cct_of(second.illuminant) < cct_of(first.illuminant) => {
                (second, Some(first))
            }
            second => (first, second),
        };

        let look = match (
            ifd.u32s(tag::PROFILE_LOOK_TABLE_DIMS)?,
            ifd.floats(tag::PROFILE_LOOK_TABLE_DATA)?,
        ) {
            (Some(dims), Some(data)) => {
                let encoding = ifd
                    .u32s(tag::PROFILE_LOOK_TABLE_ENCODING)?
                    .and_then(|v| v.first().copied())
                    .map_or(Encoding::Linear, Encoding::from_code);
                Some(table(&dims, data, encoding)?)
            }
            _ => None,
        };

        let tone_curve = match ifd.floats(tag::PROFILE_TONE_CURVE)? {
            None => None,
            Some(v) if v.len() >= 4 && v.len() % 2 == 0 => Some(v.as_chunks::<2>().0.to_vec()),
            Some(v) => {
                return Err(Error::Profile(format!(
                    "ProfileToneCurve of {} values is not a list of pairs",
                    v.len()
                )));
            }
        };

        Ok(Dcp {
            name: ifd.ascii(tag::PROFILE_NAME)?,
            unique_camera_model: ifd.ascii(tag::UNIQUE_CAMERA_MODEL)?,
            copyright: ifd.ascii(tag::PROFILE_COPYRIGHT)?,
            calibration_signature: ifd.ascii(tag::PROFILE_CALIBRATION_SIGNATURE)?,
            embed_policy: ifd
                .u32s(tag::PROFILE_EMBED_POLICY)?
                .and_then(|v| v.first().copied())
                .map_or(EmbedPolicy::default(), EmbedPolicy::from_code),
            primary,
            secondary,
            baseline_exposure_offset: ifd
                .floats(tag::BASELINE_EXPOSURE_OFFSET)?
                .and_then(|v| v.first().copied()),
            look,
            tone_curve,
        })
    }

    /// What to call it: its `ProfileName`, else the camera it names.
    pub fn title(&self) -> Option<&str> {
        self.name
            .as_deref()
            .or(self.unique_camera_model.as_deref())
            .filter(|s| !s.is_empty())
    }

    /// What a profile says about itself, without reading its tables.
    ///
    /// A directory of profiles is listed with this: the file is opened
    /// and its directory walked, but the hue map and the look table —
    /// a megabyte of floats in a profile that carries a 90x30x30 look —
    /// are never decoded. Reading a profile in full is for the one
    /// that is chosen.
    pub fn info(path: impl AsRef<Path>) -> Result<DcpInfo> {
        let path = path.as_ref();
        let len = std::fs::metadata(path)?.len();
        if len > MAX_FILE {
            return Err(Error::Profile(format!(
                "{}: {len} bytes is too large for a camera profile",
                path.display()
            )));
        }
        let bytes = std::fs::read(path)?;
        Self::info_from_bytes(&bytes).map_err(|e| at_path(path, e))
    }

    /// [`Dcp::info`] from bytes already in hand.
    pub fn info_from_bytes(bytes: &[u8]) -> Result<DcpInfo> {
        let ifd = Ifd::parse(bytes)?;
        // Enough of a check to keep a file that is not a profile out of
        // a listing, without decoding the matrix itself.
        if ifd.entry(tag::COLOR_MATRIX_1).is_none() {
            return Err(Error::Profile(
                "no ColorMatrix1: this is not a usable camera profile".into(),
            ));
        }
        Ok(DcpInfo {
            name: ifd.ascii(tag::PROFILE_NAME)?,
            unique_camera_model: ifd.ascii(tag::UNIQUE_CAMERA_MODEL)?,
            copyright: ifd.ascii(tag::PROFILE_COPYRIGHT)?,
            embed_policy: ifd
                .u32s(tag::PROFILE_EMBED_POLICY)?
                .and_then(|v| v.first().copied())
                .map_or(EmbedPolicy::default(), EmbedPolicy::from_code),
        })
    }

    /// The engine's profile: the matrix pair, the forward matrices and
    /// the map. The look table and the tone curve stay behind.
    ///
    /// A calibration this build cannot use — an illuminant code it does
    /// not know, a matrix that does not invert — is dropped and the
    /// other one carries the profile, as `color::profile_from_frame`
    /// does with a file's own calibrations. Only a profile with none
    /// left is refused.
    pub fn profile(&self) -> Result<Profile> {
        let build = |c: &DcpCalibration| -> Option<Calibration> {
            let illuminant = CalibrationIlluminant::from_exif_code(c.illuminant)?;
            let mut calibration = Calibration::new(illuminant, mat3(&c.color_matrix)?);
            // A forward matrix that will not invert is dropped on its
            // own: the color matrix alone still develops the picture.
            if let Some(forward) = c.forward_matrix.as_ref().and_then(mat3) {
                calibration = calibration.with_forward_matrix(forward);
            }
            Some(calibration)
        };

        let usable: Vec<(&DcpCalibration, Calibration)> = std::iter::once(&self.primary)
            .chain(self.secondary.as_ref())
            .filter_map(|c| build(c).map(|k| (c, k)))
            .collect();
        let (camera, dropped) = match usable.as_slice() {
            [] => {
                return Err(Error::Profile(format!(
                    "no usable calibration: illuminant {} is not one this build knows, or its \
                     matrix does not invert",
                    self.primary.illuminant
                )));
            }
            [(_, one)] => (CameraProfile::single(*one), self.secondary.is_some()),
            [(_, warm), (_, cool)] => (CameraProfile::dual(*warm, *cool), false),
            _ => unreachable!("a DCP carries at most two calibrations"),
        };
        if dropped {
            log::warn!(
                "{}: one calibration was dropped, its illuminant or its matrix being unusable",
                self.title().unwrap_or("camera profile")
            );
        }

        // The maps of the calibrations that survived, in the order the
        // matrices ended up in.
        let mut maps = usable.iter().filter_map(|(c, _)| c.hue_sat_map.as_ref());
        let maps = maps.next().map(|primary| HueSatMaps {
            primary: primary.clone(),
            // A profile with one map has it act at every temperature,
            // which is what DNG says and what a dropped calibration
            // leaves behind.
            secondary: maps.next().cloned(),
        });

        Ok(Profile {
            camera,
            name: self.title().map(str::to_string),
            unique_camera_model: self.unique_camera_model.clone(),
            copyright: self.copyright.clone(),
            embed_policy: self.embed_policy,
            baseline_exposure_offset: self.baseline_exposure_offset,
            maps: maps.map(Arc::new),
        })
    }

    /// The file's bytes: what [`Dcp::from_bytes`] reads, written back.
    ///
    /// Little endian, one IFD, the tags in order. It is here so the
    /// reader can be tested against files this build makes, and for the
    /// profile maker a chart shot will want (notes §78).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = IfdWriter::default();
        if let Some(s) = &self.unique_camera_model {
            w.ascii(tag::UNIQUE_CAMERA_MODEL, s);
        }
        w.srationals(tag::COLOR_MATRIX_1, &self.primary.color_matrix);
        w.short(tag::CALIBRATION_ILLUMINANT_1, self.primary.illuminant);
        if let Some(c) = &self.secondary {
            w.srationals(tag::COLOR_MATRIX_2, &c.color_matrix);
            w.short(tag::CALIBRATION_ILLUMINANT_2, c.illuminant);
        }
        if let Some(s) = &self.calibration_signature {
            w.ascii(tag::PROFILE_CALIBRATION_SIGNATURE, s);
        }
        if let Some(s) = &self.name {
            w.ascii(tag::PROFILE_NAME, s);
        }
        // Data2 without Data1 is not a file anyone else reads: Adobe's
        // reader hands back nothing at all when the first table is
        // missing. A profile carrying only the cooler illuminant's map
        // writes it as Data1, which is the single-map form, and is how
        // this build reads such a file back in.
        let secondary_map = self.secondary.as_ref().and_then(|c| c.hue_sat_map.as_ref());
        let (first, second) = match (&self.primary.hue_sat_map, secondary_map) {
            (Some(first), second) => (Some(first), second),
            (None, second) => (second, None),
        };
        if let Some(m) = first {
            w.longs(tag::PROFILE_HUE_SAT_MAP_DIMS, &dims(m));
            w.longs(
                tag::PROFILE_HUE_SAT_MAP_ENCODING,
                &[u32::from(m.encoding == Encoding::Srgb)],
            );
            w.floats(tag::PROFILE_HUE_SAT_MAP_DATA_1, &flat(m));
        }
        if let Some(m) = second {
            w.floats(tag::PROFILE_HUE_SAT_MAP_DATA_2, &flat(m));
        }
        if let Some(c) = &self.tone_curve {
            let points: Vec<f32> = c.iter().flatten().copied().collect();
            w.floats(tag::PROFILE_TONE_CURVE, &points);
        }
        w.longs(tag::PROFILE_EMBED_POLICY, &[self.embed_policy.code()]);
        if let Some(s) = &self.copyright {
            w.ascii(tag::PROFILE_COPYRIGHT, s);
        }
        if let Some(m) = &self.primary.forward_matrix {
            w.srationals(tag::FORWARD_MATRIX_1, m);
        }
        if let Some(m) = self
            .secondary
            .as_ref()
            .and_then(|c| c.forward_matrix.as_ref())
        {
            w.srationals(tag::FORWARD_MATRIX_2, m);
        }
        if let Some(look) = &self.look {
            w.longs(tag::PROFILE_LOOK_TABLE_DIMS, &dims(look));
            w.floats(tag::PROFILE_LOOK_TABLE_DATA, &flat(look));
            w.longs(
                tag::PROFILE_LOOK_TABLE_ENCODING,
                &[u32::from(look.encoding == Encoding::Srgb)],
            );
        }
        if let Some(offset) = self.baseline_exposure_offset {
            w.srationals(tag::BASELINE_EXPOSURE_OFFSET, &[offset as f64]);
        }
        w.finish()
    }
}

/// A profile's complaint with the file it came from in front of it.
fn at_path(path: &Path, e: Error) -> Error {
    match e {
        Error::Profile(msg) => Error::Profile(format!("{}: {msg}", path.display())),
        other => other,
    }
}

fn dims(map: &HueSatMap) -> [u32; 3] {
    [
        map.hue_divisions as u32,
        map.sat_divisions as u32,
        map.val_divisions as u32,
    ]
}

fn flat(map: &HueSatMap) -> Vec<f32> {
    map.data.iter().flatten().copied().collect()
}

/// A lattice from its dimensions and its data.
fn table(dims: &[u32], data: Vec<f32>, encoding: Encoding) -> Result<HueSatMap> {
    if dims.len() != 3 {
        return Err(Error::Profile(format!(
            "a table's dimensions came as {} values, not 3",
            dims.len()
        )));
    }
    if !data.len().is_multiple_of(3) {
        return Err(Error::Profile(format!(
            "a table of {} values is not a list of triples",
            data.len()
        )));
    }
    let map = HueSatMap {
        hue_divisions: dims[0] as usize,
        sat_divisions: dims[1] as usize,
        val_divisions: dims[2] as usize,
        encoding,
        data: data.as_chunks::<3>().0.to_vec(),
    };
    map.check()?;
    Ok(map)
}

/// A temperature for an illuminant code, for ordering the pair. An
/// unknown code sorts warm, where a missing tag's zero belongs.
fn cct_of(code: u16) -> f64 {
    CalibrationIlluminant::from_exif_code(code).map_or(0.0, |i| i.cct())
}

fn mat3(flat: &[f64; 9]) -> Option<Mat3> {
    let m = Mat3::new(std::array::from_fn(|r| {
        std::array::from_fn(|c| flat[r * 3 + c])
    }));
    m.inverse().map(|_| m)
}

/// One flat IFD, read in place: every entry's tag, type, count and
/// where its bytes are.
struct Ifd<'a> {
    bytes: &'a [u8],
    little: bool,
    entries: Vec<Entry>,
}

#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    kind: u16,
    count: usize,
    /// Where the value's bytes start in the file.
    offset: usize,
}

/// Bytes per value of each TIFF type this reader knows; 0 for the ones
/// it does not.
fn type_size(kind: u16) -> usize {
    match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

impl<'a> Ifd<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() < 8 {
            return Err(Error::Profile(
                "the file is too short to be a camera profile".into(),
            ));
        }
        let little = match &bytes[0..2] {
            b"II" => true,
            b"MM" => false,
            _ => {
                return Err(Error::Profile(
                    "not a camera profile: it does not begin with a TIFF byte order mark".into(),
                ));
            }
        };
        let u16_at = |at: usize| -> u16 {
            let b = [bytes[at], bytes[at + 1]];
            if little {
                u16::from_le_bytes(b)
            } else {
                u16::from_be_bytes(b)
            }
        };
        let u32_at = |at: usize| -> u32 {
            let b = [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
            if little {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            }
        };
        let magic = u16_at(2);
        if magic != MAGIC {
            return Err(Error::Profile(format!(
                "not a camera profile: its magic number is {magic}, not {MAGIC}; a DNG or a TIFF \
                 is not a DCP"
            )));
        }
        // Every offset the file gives is checked against the file's own
        // length before it is read, and added to with `checked_add`, so
        // that a 32-bit build cannot be talked round the check.
        let start = u32_at(4) as usize;
        if start.checked_add(2).is_none_or(|end| end > bytes.len()) {
            return Err(Error::Profile(
                "the profile's directory is past the end of the file".into(),
            ));
        }
        let count = u16_at(start) as usize;
        if count == 0 {
            return Err(Error::Profile("the profile's directory is empty".into()));
        }
        if count
            .checked_mul(12)
            .and_then(|n| n.checked_add(start + 2))
            .is_none_or(|end| end > bytes.len())
        {
            return Err(Error::Profile(format!(
                "the profile claims {count} tags, which do not fit in {} bytes",
                bytes.len()
            )));
        }
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let at = start + 2 + i * 12;
            let tag = u16_at(at);
            let kind = u16_at(at + 2);
            let count = u32_at(at + 4) as usize;
            let size = type_size(kind);
            // A type this reader does not know is left alone; its
            // length is unknown, so it cannot even be bounds checked.
            if size == 0 {
                continue;
            }
            let len = size.saturating_mul(count);
            let offset = if len <= 4 {
                at + 8
            } else {
                u32_at(at + 8) as usize
            };
            if offset.saturating_add(len) > bytes.len() {
                return Err(Error::Profile(format!(
                    "tag {tag} points past the end of the file"
                )));
            }
            entries.push(Entry {
                tag,
                kind,
                count,
                offset,
            });
        }
        Ok(Ifd {
            bytes,
            little,
            entries,
        })
    }

    fn entry(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }

    fn u16_at(&self, at: usize) -> u16 {
        let b = [self.bytes[at], self.bytes[at + 1]];
        if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }
    }

    fn u32_at(&self, at: usize) -> u32 {
        let b = [
            self.bytes[at],
            self.bytes[at + 1],
            self.bytes[at + 2],
            self.bytes[at + 3],
        ];
        if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    }

    fn i32_at(&self, at: usize) -> i32 {
        self.u32_at(at) as i32
    }

    fn f64_at(&self, at: usize) -> f64 {
        let (hi, lo) = if self.little {
            (self.u32_at(at + 4), self.u32_at(at))
        } else {
            (self.u32_at(at), self.u32_at(at + 4))
        };
        f64::from_bits(((hi as u64) << 32) | lo as u64)
    }

    /// An ASCII tag, without its terminator.
    fn ascii(&self, tag: u16) -> Result<Option<String>> {
        let Some(e) = self.entry(tag) else {
            return Ok(None);
        };
        if e.kind != 2 {
            return Err(Error::Profile(format!("tag {tag} is not text")));
        }
        let raw = &self.bytes[e.offset..e.offset + e.count];
        let raw = raw.split(|b| *b == 0).next().unwrap_or(raw);
        Ok(Some(String::from_utf8_lossy(raw).into_owned()))
    }

    /// An integer tag, whatever integer type it is stored in.
    fn u32s(&self, tag: u16) -> Result<Option<Vec<u32>>> {
        let Some(e) = self.entry(tag) else {
            return Ok(None);
        };
        let size = type_size(e.kind);
        let values = (0..e.count)
            .map(|i| {
                let at = e.offset + i * size;
                match e.kind {
                    1 | 6 | 7 => Ok(self.bytes[at] as u32),
                    3 | 8 => Ok(self.u16_at(at) as u32),
                    4 | 9 => Ok(self.u32_at(at)),
                    _ => Err(Error::Profile(format!("tag {tag} is not a number"))),
                }
            })
            .collect::<Result<Vec<u32>>>()?;
        Ok(Some(values))
    }

    /// A real-valued tag: float, double, or either rational.
    fn floats(&self, tag: u16) -> Result<Option<Vec<f32>>> {
        let Some(e) = self.entry(tag) else {
            return Ok(None);
        };
        let size = type_size(e.kind);
        let values = (0..e.count)
            .map(|i| {
                let at = e.offset + i * size;
                match e.kind {
                    11 => Ok(f32::from_bits(self.u32_at(at))),
                    12 => Ok(self.f64_at(at) as f32),
                    5 => {
                        let d = self.u32_at(at + 4);
                        Ok(if d == 0 {
                            0.0
                        } else {
                            self.u32_at(at) as f32 / d as f32
                        })
                    }
                    10 => {
                        let d = self.i32_at(at + 4);
                        Ok(if d == 0 {
                            0.0
                        } else {
                            self.i32_at(at) as f32 / d as f32
                        })
                    }
                    3 | 8 => Ok(self.u16_at(at) as f32),
                    4 | 9 => Ok(self.u32_at(at) as f32),
                    _ => Err(Error::Profile(format!(
                        "tag {tag} is of type {}, which is not a number",
                        e.kind
                    ))),
                }
            })
            .collect::<Result<Vec<f32>>>()?;
        Ok(Some(values))
    }
}

/// The other direction: tags in, a little-endian DCP out.
#[derive(Default)]
struct IfdWriter {
    /// Tag, type, count, and the value's bytes.
    entries: Vec<(u16, u16, u32, Vec<u8>)>,
}

impl IfdWriter {
    fn push(&mut self, tag: u16, kind: u16, count: usize, bytes: Vec<u8>) {
        self.entries.push((tag, kind, count as u32, bytes));
    }

    fn ascii(&mut self, tag: u16, value: &str) {
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        let count = bytes.len();
        self.push(tag, 2, count, bytes);
    }

    fn short(&mut self, tag: u16, value: u16) {
        self.push(tag, 3, 1, value.to_le_bytes().to_vec());
    }

    fn longs(&mut self, tag: u16, values: &[u32]) {
        let bytes = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.push(tag, 4, values.len(), bytes);
    }

    fn floats(&mut self, tag: u16, values: &[f32]) {
        let bytes = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.push(tag, 11, values.len(), bytes);
    }

    /// Reals as SRATIONAL over 10000, which is what DNG matrices use.
    fn srationals(&mut self, tag: u16, values: &[f64]) {
        let mut bytes = Vec::with_capacity(values.len() * 8);
        for v in values {
            bytes.extend(((v * 10_000.0).round() as i32).to_le_bytes());
            bytes.extend(10_000i32.to_le_bytes());
        }
        self.push(tag, 10, values.len(), bytes);
    }

    fn finish(mut self) -> Vec<u8> {
        self.entries.sort_by_key(|e| e.0);
        let count = self.entries.len();
        let heap_start = 8 + 2 + count * 12 + 4;
        let mut out = Vec::new();
        out.extend(b"II");
        out.extend(MAGIC.to_le_bytes());
        out.extend(8u32.to_le_bytes());
        out.extend((count as u16).to_le_bytes());
        let mut heap: Vec<u8> = Vec::new();
        for (tag, kind, n, bytes) in &self.entries {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(n.to_le_bytes());
            if bytes.len() <= 4 {
                let mut inline = bytes.clone();
                inline.resize(4, 0);
                out.extend(inline);
            } else {
                out.extend(((heap_start + heap.len()) as u32).to_le_bytes());
                heap.extend(bytes);
                // Values start on an even offset, as TIFF asks.
                if heap.len() % 2 == 1 {
                    heap.push(0);
                }
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(heap);
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::color::tests::rec2020_camera_matrix;

    const STANDARD_A: u16 = 17;
    const D65: u16 = 21;

    /// The Rec.2020 camera of the color tests, as a DCP's matrix.
    pub(crate) fn rec2020_matrix() -> [f64; 9] {
        let flat = rec2020_camera_matrix();
        std::array::from_fn(|i| flat[i] as f64)
    }

    /// A dual-illuminant profile for that camera, with `map1` and
    /// `map2` if they are given.
    pub(crate) fn test_dcp(map1: Option<HueSatMap>, map2: Option<HueSatMap>) -> Dcp {
        Dcp {
            name: Some("Test Profile".into()),
            unique_camera_model: Some("Test Cam".into()),
            copyright: Some("public domain".into()),
            calibration_signature: None,
            embed_policy: EmbedPolicy::AllowCopying,
            primary: DcpCalibration {
                illuminant: STANDARD_A,
                color_matrix: rec2020_matrix(),
                forward_matrix: None,
                hue_sat_map: map1,
            },
            secondary: Some(DcpCalibration {
                illuminant: D65,
                color_matrix: rec2020_matrix(),
                forward_matrix: None,
                hue_sat_map: map2,
            }),
            baseline_exposure_offset: Some(-0.25),
            look: None,
            tone_curve: None,
        }
    }

    /// A 4x2x1 map whose entries are all different, so an
    /// interpolation that reads the wrong node shows.
    pub(crate) fn stepped_map() -> HueSatMap {
        let mut map = HueSatMap::identity(4, 2, 1);
        for (i, e) in map.data.iter_mut().enumerate() {
            *e = [i as f32, 1.0 + i as f32 * 0.1, 1.0 + i as f32 * 0.01];
        }
        map
    }

    #[test]
    fn a_profile_survives_the_round_trip_through_a_file() {
        let mut dcp = test_dcp(Some(stepped_map()), Some(HueSatMap::identity(4, 2, 1)));
        dcp.primary.forward_matrix = Some(rec2020_matrix());
        dcp.secondary.as_mut().unwrap().forward_matrix = Some(rec2020_matrix());
        dcp.tone_curve = Some(vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]]);
        dcp.look = Some({
            let mut look = HueSatMap::identity(2, 2, 2);
            look.encoding = Encoding::Srgb;
            look.data[3] = [10.0, 0.5, 0.9];
            look
        });
        let back = Dcp::from_bytes(&dcp.to_bytes()).unwrap();

        assert_eq!(back.name.as_deref(), Some("Test Profile"));
        assert_eq!(back.unique_camera_model.as_deref(), Some("Test Cam"));
        assert_eq!(back.copyright.as_deref(), Some("public domain"));
        assert_eq!(back.embed_policy, EmbedPolicy::AllowCopying);
        assert_eq!(back.primary.illuminant, STANDARD_A);
        assert_eq!(back.secondary.as_ref().unwrap().illuminant, D65);
        assert_eq!(back.look, dcp.look);
        assert_eq!(back.tone_curve, dcp.tone_curve);
        assert_eq!(
            back.baseline_exposure_offset, dcp.baseline_exposure_offset,
            "the baseline exposure offset is read, and applied by nothing"
        );
        assert_eq!(back.primary.hue_sat_map, dcp.primary.hue_sat_map);
        // The matrices go through SRATIONAL over 10000, so they come
        // back to four decimals, not bit for bit.
        for (a, b) in back
            .primary
            .color_matrix
            .iter()
            .zip(&dcp.primary.color_matrix)
        {
            assert!((a - b).abs() < 1e-4, "{a} is not {b}");
        }
        assert!(back.primary.forward_matrix.is_some());
        assert!(back.secondary.unwrap().forward_matrix.is_some());
    }

    #[test]
    fn the_pair_comes_back_in_temperature_order_whichever_way_it_was_written() {
        let mut dcp = test_dcp(None, None);
        dcp.primary.illuminant = D65;
        dcp.secondary.as_mut().unwrap().illuminant = STANDARD_A;
        let back = Dcp::from_bytes(&dcp.to_bytes()).unwrap();
        assert_eq!(back.primary.illuminant, STANDARD_A);
        assert_eq!(back.secondary.unwrap().illuminant, D65);
    }

    #[test]
    fn what_is_not_a_dcp_says_so() {
        // A TIFF, which is what a DNG is: right byte order, wrong magic.
        let mut tiff = vec![b'I', b'I'];
        tiff.extend(42u16.to_le_bytes());
        tiff.extend(8u32.to_le_bytes());
        tiff.extend([0u8; 16]);
        let msg = Dcp::from_bytes(&tiff).unwrap_err().to_string();
        assert!(msg.contains("not a camera profile"), "{msg}");
        assert!(msg.contains("magic number"), "{msg}");

        let msg = Dcp::from_bytes(b"this is a text file, not a profile at all")
            .unwrap_err()
            .to_string();
        assert!(msg.contains("byte order mark"), "{msg}");

        let msg = Dcp::from_bytes(b"II").unwrap_err().to_string();
        assert!(msg.contains("too short"), "{msg}");

        // A DCP with a directory but no color matrix.
        let mut bare = IfdWriter::default();
        bare.ascii(tag::PROFILE_NAME, "nothing in here");
        let msg = Dcp::from_bytes(&bare.finish()).unwrap_err().to_string();
        assert!(msg.contains("ColorMatrix1"), "{msg}");

        // A file whose tag points past its own end.
        let mut broken = test_dcp(Some(stepped_map()), None).to_bytes();
        broken.truncate(broken.len() - 32);
        let msg = Dcp::from_bytes(&broken).unwrap_err().to_string();
        assert!(msg.contains("past the end"), "{msg}");
    }

    #[test]
    fn a_map_reads_its_own_nodes_and_interpolates_between_them() {
        let map = stepped_map();
        // At a node: hue 0 is index 0, saturation 0 the first of the
        // pair, so the first entry.
        assert_eq!(map.lookup(0.0, 0.0, 0.5), [0.0, 1.0, 1.0]);
        // Saturation 1 is the second entry of the same hue.
        assert_eq!(map.lookup(0.0, 1.0, 0.5), [1.0, 1.1, 1.01]);
        // Hue 90 is the next hue's pair.
        assert_eq!(map.lookup(90.0, 0.0, 0.5), [2.0, 1.2, 1.02]);
        // Halfway between two hue nodes, at a saturation node: the
        // mean of the two.
        let mid = map.lookup(45.0, 0.0, 0.5);
        assert!((mid[0] - 1.0).abs() < 1e-6, "{mid:?}");
        assert!((mid[1] - 1.1).abs() < 1e-6, "{mid:?}");
        // And in both axes at once: a quarter of the way in hue,
        // three quarters in saturation.
        let both = map.lookup(22.5, 0.75, 0.5);
        let want: [f32; 3] = std::array::from_fn(|c| {
            let lo = map.data[0][c] + (map.data[2][c] - map.data[0][c]) * 0.25;
            let hi = map.data[1][c] + (map.data[3][c] - map.data[1][c]) * 0.25;
            lo + (hi - lo) * 0.75
        });
        for (got, want) in both.iter().zip(want) {
            assert!((got - want).abs() < 1e-5, "{both:?} is not {want:?}");
        }
        // Hue wraps: 350 degrees is most of the way from the last
        // node back to the first.
        let wrapped = map.lookup(350.0, 0.0, 0.5);
        let t = 350.0 * 4.0 / 360.0 - 3.0;
        let want = map.data[6][0] + (map.data[0][0] - map.data[6][0]) * t;
        assert!(
            (wrapped[0] - want).abs() < 1e-5,
            "{wrapped:?} is not {want}"
        );
        // Saturation and value clamp outside 0..1 rather than wrap.
        assert_eq!(map.lookup(0.0, 3.0, 9.0), map.lookup(0.0, 1.0, 1.0));
        assert_eq!(map.lookup(0.0, -1.0, -1.0), map.lookup(0.0, 0.0, 0.0));
    }

    #[test]
    fn a_three_dimensional_map_interpolates_in_value_too() {
        let mut map = HueSatMap::identity(2, 2, 3);
        for (i, e) in map.data.iter_mut().enumerate() {
            e[2] = i as f32;
        }
        // Value 0 is the first slice, 1 the last, 0.5 the middle one.
        assert_eq!(map.lookup(0.0, 0.0, 0.0)[2], 0.0);
        assert_eq!(map.lookup(0.0, 0.0, 0.5)[2], 4.0);
        assert_eq!(map.lookup(0.0, 0.0, 1.0)[2], 8.0);
        // And a quarter of the way is halfway into the first pair.
        assert!((map.lookup(0.0, 0.0, 0.25)[2] - 2.0).abs() < 1e-5);
    }

    #[test]
    fn the_illuminants_are_weighted_as_the_matrices_are() {
        let warm = stepped_map();
        let cool = HueSatMap::identity(4, 2, 1);
        let profile = test_dcp(Some(warm.clone()), Some(cool.clone()))
            .profile()
            .unwrap();

        // The illuminants' own temperatures, which are what the
        // weighting is built on: about 2856 K and 6504 K.
        let warm_cct = profile.camera.primary.illuminant.cct();
        let cool_cct = profile.camera.secondary.unwrap().illuminant.cct();
        assert!((warm_cct - 2856.0).abs() < 20.0, "{warm_cct}");
        assert!((cool_cct - 6504.0).abs() < 20.0, "{cool_cct}");

        // At or below the warm illuminant, the warm map alone.
        assert_eq!(profile.map_at_cct(2000.0).unwrap().unwrap(), warm);
        assert_eq!(profile.map_at_cct(warm_cct).unwrap().unwrap(), warm);
        // At or above the cool one, the cool map alone.
        assert_eq!(profile.map_at_cct(cool_cct).unwrap().unwrap(), cool);
        assert_eq!(profile.map_at_cct(20000.0).unwrap().unwrap(), cool);

        // Between them, DNG's blend in reciprocal temperature: the
        // midpoint in mired is half of each.
        let mired = (1.0 / warm_cct + 1.0 / cool_cct) / 2.0;
        let between = profile.map_at_cct(1.0 / mired).unwrap().unwrap();
        for (i, e) in between.data.iter().enumerate() {
            let want: [f32; 3] = std::array::from_fn(|c| (warm.data[i][c] + cool.data[i][c]) / 2.0);
            for c in 0..3 {
                assert!((e[c] - want[c]).abs() < 1e-5, "{e:?} is not {want:?}");
            }
        }
    }

    #[test]
    fn one_map_acts_at_every_temperature() {
        let map = stepped_map();
        let profile = test_dcp(Some(map.clone()), None).profile().unwrap();
        assert_eq!(profile.map_at_cct(2000.0).unwrap().unwrap(), map);
        assert_eq!(profile.map_at_cct(9000.0).unwrap().unwrap(), map);

        // Written against the cool illuminant alone, it still acts.
        let profile = test_dcp(None, Some(map.clone())).profile().unwrap();
        assert_eq!(profile.map_at_cct(2000.0).unwrap().unwrap(), map);
    }

    /// A file whose dimensions would overflow their own product, or
    /// simply claim more nodes than any profile has, is turned away
    /// rather than indexed outside its data.
    #[test]
    fn a_table_too_large_to_be_real_is_refused() {
        let dcp = test_dcp(Some(stepped_map()), None);
        let mut bytes = dcp.to_bytes();
        // Rewrite ProfileHueSatMapDims in place: 4194304 on each axis
        // is 2^66 entries, which a usize does not hold.
        let huge = 4_194_304u32;
        let dims = bytes
            .windows(2)
            .position(|w| w == tag::PROFILE_HUE_SAT_MAP_DIMS.to_le_bytes())
            .expect("the dims tag is in the file");
        let at = u32::from_le_bytes(bytes[dims + 8..dims + 12].try_into().unwrap()) as usize;
        for i in 0..3 {
            bytes[at + i * 4..at + i * 4 + 4].copy_from_slice(&huge.to_le_bytes());
        }
        let msg = Dcp::from_bytes(&bytes).unwrap_err().to_string();
        assert!(msg.contains("past the"), "{msg}");
        assert!(msg.contains(&MAX_DIVISIONS.to_string()), "{msg}");

        // And one just past the cap, which multiplies perfectly well
        // and is still not a profile anyone made.
        let past = (MAX_DIVISIONS + 1) as u32;
        bytes[at..at + 4].copy_from_slice(&past.to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&2u32.to_le_bytes());
        bytes[at + 8..at + 12].copy_from_slice(&1u32.to_le_bytes());
        let msg = Dcp::from_bytes(&bytes).unwrap_err().to_string();
        assert!(msg.contains("past the"), "{msg}");

        // Back inside the cap, the same file fails on its size alone,
        // which is the check that would have been indexing past the
        // data had the dimensions got through.
        bytes[at..at + 4].copy_from_slice(&64u32.to_le_bytes());
        let msg = Dcp::from_bytes(&bytes).unwrap_err().to_string();
        assert!(msg.contains("entries, not"), "{msg}");
    }

    /// The same cap, for a map built in hand rather than read.
    #[test]
    #[should_panic(expected = "at most")]
    fn an_identity_map_past_the_cap_is_a_mistake_in_the_caller() {
        HueSatMap::identity(MAX_DIVISIONS + 1, 2, 1);
    }

    /// A calibration this build cannot use is dropped, not the profile.
    #[test]
    fn an_unusable_calibration_is_dropped_and_the_other_carries_on() {
        // An illuminant code nothing knows on the cool side.
        let mut dcp = test_dcp(Some(stepped_map()), Some(HueSatMap::identity(4, 2, 1)));
        dcp.secondary.as_mut().unwrap().illuminant = 250;
        let profile = dcp.profile().unwrap();
        assert!(profile.camera.secondary.is_none(), "one calibration left");
        assert_eq!(
            profile.camera.primary.illuminant,
            CalibrationIlluminant::StandardA
        );
        // The map that survives is the surviving calibration's, and it
        // acts at every temperature.
        assert_eq!(profile.map_at_cct(2000.0).unwrap().unwrap(), stepped_map());
        assert_eq!(profile.map_at_cct(9000.0).unwrap().unwrap(), stepped_map());

        // A matrix that does not invert, on the warm side this time:
        // the cool calibration and its map carry the profile.
        let cool = HueSatMap::identity(4, 2, 1);
        let mut dcp = test_dcp(Some(stepped_map()), Some(cool.clone()));
        dcp.primary.color_matrix = [1.0; 9];
        let profile = dcp.profile().unwrap();
        assert!(profile.camera.secondary.is_none());
        assert_eq!(
            profile.camera.primary.illuminant,
            CalibrationIlluminant::D65
        );
        assert_eq!(profile.map_at_cct(5000.0).unwrap().unwrap(), cool);

        // Neither usable is no profile at all.
        let mut dcp = test_dcp(None, None);
        dcp.primary.illuminant = 250;
        dcp.secondary.as_mut().unwrap().color_matrix = [1.0; 9];
        let msg = dcp.profile().unwrap_err().to_string();
        assert!(msg.contains("no usable calibration"), "{msg}");
    }

    /// A profile whose only map belongs to the cool illuminant writes
    /// it as Data1, which is the single-map form every reader knows.
    #[test]
    fn a_lone_second_map_is_written_as_the_first() {
        let map = stepped_map();
        let bytes = test_dcp(None, Some(map.clone())).to_bytes();
        let back = Dcp::from_bytes(&bytes).unwrap();
        assert_eq!(back.primary.hue_sat_map.as_ref(), Some(&map));
        assert!(
            back.secondary
                .as_ref()
                .is_none_or(|c| c.hue_sat_map.is_none())
        );
        // It still acts at every temperature, which is what it meant.
        let profile = back.profile().unwrap();
        assert_eq!(profile.map_at_cct(2000.0).unwrap().unwrap(), map);
        assert_eq!(profile.map_at_cct(9000.0).unwrap().unwrap(), map);
    }

    /// The listing reads a profile's name and camera without touching
    /// its tables.
    #[test]
    fn the_header_reads_on_its_own() {
        let mut dcp = test_dcp(Some(stepped_map()), None);
        dcp.look = Some(HueSatMap::identity(4, 2, 2));
        let info = Dcp::info_from_bytes(&dcp.to_bytes()).unwrap();
        assert_eq!(info.name.as_deref(), Some("Test Profile"));
        assert_eq!(info.unique_camera_model.as_deref(), Some("Test Cam"));
        assert_eq!(info.copyright.as_deref(), Some("public domain"));
        assert_eq!(info.embed_policy, EmbedPolicy::AllowCopying);

        // And it turns away what a full read would have.
        let mut bare = IfdWriter::default();
        bare.ascii(tag::PROFILE_NAME, "nothing in here");
        let msg = Dcp::info_from_bytes(&bare.finish())
            .unwrap_err()
            .to_string();
        assert!(msg.contains("ColorMatrix1"), "{msg}");
    }

    #[test]
    fn a_profile_with_no_map_asks_for_none() {
        let profile = test_dcp(None, None).profile().unwrap();
        assert!(profile.map_at_cct(5000.0).unwrap().is_none());
        assert_eq!(profile.name.as_deref(), Some("Test Profile"));
        assert_eq!(profile.baseline_exposure_offset, Some(-0.25));
    }

    /// A real Adobe or RawTherapee profile, when one is pointed at:
    /// `GREYCARD_DCP=/path/to/camera.dcp cargo test -- --ignored`.
    /// None is committed; RawTherapee's are under `rtdata/dcpprofiles`.
    #[test]
    #[ignore = "needs GREYCARD_DCP pointing at a DCP file"]
    fn a_real_profile_reads() {
        let path = std::env::var("GREYCARD_DCP").expect("GREYCARD_DCP");
        let dcp = Dcp::load(&path).unwrap();
        println!(
            "{path}: name {:?}, camera {:?}, copyright {:?}, policy {:?}, offset {:?}",
            dcp.name,
            dcp.unique_camera_model,
            dcp.copyright,
            dcp.embed_policy,
            dcp.baseline_exposure_offset
        );
        if let Some(m) = &dcp.primary.hue_sat_map {
            println!(
                "  map {}x{}x{}, encoding {:?}",
                m.hue_divisions, m.sat_divisions, m.val_divisions, m.encoding
            );
        }
        if let Some(l) = &dcp.look {
            println!(
                "  look {}x{}x{}, encoding {:?}",
                l.hue_divisions, l.sat_divisions, l.val_divisions, l.encoding
            );
        }
        println!(
            "  tone curve points: {:?}",
            dcp.tone_curve.as_ref().map(Vec::len)
        );
        let profile = dcp.profile().unwrap();
        assert!(profile.camera.secondary.is_some(), "a real DCP is dual");
        // The file reads the same after being written back.
        let again = Dcp::from_bytes(&dcp.to_bytes()).unwrap();
        assert_eq!(again.primary.hue_sat_map, dcp.primary.hue_sat_map);
        assert_eq!(again.look, dcp.look);
    }
}
