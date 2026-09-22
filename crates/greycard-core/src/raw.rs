//! The decoder boundary: what a RAW file is once the container format is out
//! of the way.
//!
//! A [`RawFrame`] is sensor samples plus everything needed to interpret them.
//! Nothing here is developed: no levels applied, no white balance, no
//! demosaic, no matrix. That is the point of the boundary. A decoder backend
//! (see [`crate::decode`]) produces this; the engine does the rest.

use crate::error::{Error, Result};

/// One color filter of a color filter array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CfaColor {
    Red,
    Green,
    Blue,
    /// Emerald, cyan, magenta, yellow, white: anything a 3x3 matrix does not
    /// model. Carries the backend's own code for diagnostics.
    Other(u8),
}

impl CfaColor {
    /// Index of this color in an RGB triple, if it has one.
    pub fn rgb_index(self) -> Option<usize> {
        match self {
            CfaColor::Red => Some(0),
            CfaColor::Green => Some(1),
            CfaColor::Blue => Some(2),
            CfaColor::Other(_) => None,
        }
    }
}

/// A repeating color filter pattern, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfaPattern {
    pub width: usize,
    pub height: usize,
    pub colors: Vec<CfaColor>,
}

impl CfaPattern {
    pub fn new(width: usize, height: usize, colors: Vec<CfaColor>) -> Result<Self> {
        if width == 0 || height == 0 || colors.len() != width * height {
            return Err(Error::Unsupported(format!(
                "CFA pattern {width}x{height} with {} entries",
                colors.len()
            )));
        }
        Ok(Self {
            width,
            height,
            colors,
        })
    }

    /// The common 2x2 Bayer layout.
    pub fn rggb() -> Self {
        use CfaColor::*;
        Self {
            width: 2,
            height: 2,
            colors: vec![Red, Green, Green, Blue],
        }
    }

    /// Color of the filter over sensor position (`row`, `col`).
    #[inline]
    pub fn color_at(&self, row: usize, col: usize) -> CfaColor {
        self.colors[(row % self.height) * self.width + (col % self.width)]
    }

    /// The pattern as seen from an origin moved right by `dx` and down by
    /// `dy`, for use after cropping.
    pub fn shifted(&self, dx: usize, dy: usize) -> Self {
        let colors = (0..self.height)
            .flat_map(|r| (0..self.width).map(move |c| (r, c)))
            .map(|(r, c)| self.color_at(r + dy, c + dx))
            .collect();
        Self {
            width: self.width,
            height: self.height,
            colors,
        }
    }

    /// True when every filter is red, green or blue.
    pub fn is_rgb(&self) -> bool {
        self.colors.iter().all(|c| c.rgb_index().is_some())
    }
}

/// The pattern as a person would name it: `2x2 RGGB`, or `6x6
/// (X-Trans)` for the one six-by-six layout in the wild. A `{:?}` of a
/// 36-entry pattern is no use in a message a user reads.
impl std::fmt::Display for CfaPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)?;
        if self.width == 6 && self.height == 6 {
            return write!(f, " (X-Trans)");
        }
        f.write_str(" ")?;
        for color in &self.colors {
            f.write_str(match color {
                CfaColor::Red => "R",
                CfaColor::Green => "G",
                CfaColor::Blue => "B",
                CfaColor::Other(_) => "?",
            })?;
        }
        Ok(())
    }
}

/// Fujifilm's 6x6 X-Trans layout, for the tests that check what is
/// said about a mosaic this build does not develop.
#[cfg(test)]
pub(crate) fn xtrans() -> CfaPattern {
    use CfaColor::{Blue as B, Green as G, Red as R};
    CfaPattern::new(
        6,
        6,
        vec![
            G, B, G, G, R, G, //
            R, G, R, B, G, B, //
            G, B, G, G, R, G, //
            G, R, G, G, B, G, //
            B, G, B, R, G, R, //
            G, R, G, G, B, G, //
        ],
    )
    .unwrap()
}

/// How the samples in a frame are laid out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensorLayout {
    /// One sample per pixel, color given by the pattern.
    Cfa(CfaPattern),
    /// Three interleaved camera-RGB samples per pixel, already demosaiced
    /// (linear DNG, some medium format backs, sRAW).
    Linear,
    /// One sample per pixel, no color.
    Monochrome,
}

/// A black or white level that repeats over the sensor.
///
/// DNG lets black level vary per CFA position (`BlackLevelRepeatDim`) and per
/// component. `values` is row-major over the repeat, `cpp` entries per cell.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelPattern {
    pub width: usize,
    pub height: usize,
    pub cpp: usize,
    pub values: Vec<f32>,
}

impl LevelPattern {
    pub fn new(width: usize, height: usize, cpp: usize, values: Vec<f32>) -> Result<Self> {
        if width == 0 || height == 0 || cpp == 0 || values.len() != width * height * cpp {
            return Err(Error::Unsupported(format!(
                "level pattern {width}x{height}x{cpp} with {} values",
                values.len()
            )));
        }
        Ok(Self {
            width,
            height,
            cpp,
            values,
        })
    }

    /// The same level everywhere.
    pub fn uniform(value: f32, cpp: usize) -> Self {
        Self {
            width: 1,
            height: 1,
            cpp,
            values: vec![value; cpp],
        }
    }

    #[inline]
    pub fn at(&self, row: usize, col: usize, channel: usize) -> f32 {
        let cell = (row % self.height) * self.width + (col % self.width);
        self.values[cell * self.cpp + channel]
    }

    /// The pattern as seen from an origin moved right by `dx` and down by
    /// `dy`, for use after cropping.
    pub fn shifted(&self, dx: usize, dy: usize) -> Self {
        let mut values = Vec::with_capacity(self.values.len());
        for r in 0..self.height {
            for c in 0..self.width {
                for ch in 0..self.cpp {
                    values.push(self.at(r + dy, c + dx, ch));
                }
            }
        }
        Self {
            width: self.width,
            height: self.height,
            cpp: self.cpp,
            values,
        }
    }
}

/// Sensor black and white levels in the units of the samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Levels {
    pub black: LevelPattern,
    pub white: LevelPattern,
}

/// One illuminant's color calibration, as the file carries it.
///
/// Matrices are row-major `f32` with three columns (XYZ) and one row per
/// camera channel, as in DNG `ColorMatrix`. Nothing is interpreted here;
/// [`crate::color`] turns these into a profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    /// EXIF `LightSource` code (17 = A, 21 = D65, ...).
    pub illuminant: u16,
    /// XYZ to camera.
    pub color_matrix: Vec<f32>,
    /// White-balanced camera to XYZ (D50), if the file has one.
    pub forward_matrix: Option<Vec<f32>>,
}

/// An axis-aligned region of the sensor in sample coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// Orientation the image should be shown in, from the file's metadata.
/// Variants follow the EXIF `Orientation` tag in tag order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Normal,
    FlipHorizontal,
    Rotate180,
    FlipVertical,
    Transpose,
    Rotate90,
    Transverse,
    Rotate270,
}

impl Orientation {
    /// From the EXIF `Orientation` tag's value, 1 to 8; none for
    /// anything else. The enum is declared in tag order.
    pub fn from_exif(code: u16) -> Option<Self> {
        Some(match code {
            1 => Self::Normal,
            2 => Self::FlipHorizontal,
            3 => Self::Rotate180,
            4 => Self::FlipVertical,
            5 => Self::Transpose,
            6 => Self::Rotate90,
            7 => Self::Transverse,
            8 => Self::Rotate270,
            _ => return None,
        })
    }

    /// The EXIF `Orientation` tag's value for this, 1 to 8.
    pub fn to_exif(self) -> u16 {
        self as u16 + 1
    }

    /// This orientation with `quarters` more quarter turns clockwise
    /// on the screen after it.
    ///
    /// The eight tags are the square's eight symmetries, and every
    /// one of them is a quarter turn or three after an optional
    /// mirror: a turn on top only moves which quarter, and never
    /// whether the picture is mirrored. So the four with no mirror
    /// (1, 6, 3, 8 in tag order, which is Normal, Rotate90,
    /// Rotate180, Rotate270) are a cycle, the four with one
    /// (2, 7, 4, 5: FlipHorizontal, Transverse, FlipVertical,
    /// Transpose) are the other, and a turn steps along whichever
    /// cycle the tag is in.
    pub fn turned(self, quarters: i32) -> Self {
        use Orientation::*;
        // Which cycle this tag is in, and where in it.
        let (mirrored, at) = match self {
            Normal => (false, 0),
            Rotate90 => (false, 1),
            Rotate180 => (false, 2),
            Rotate270 => (false, 3),
            FlipHorizontal => (true, 0),
            Transverse => (true, 1),
            FlipVertical => (true, 2),
            Transpose => (true, 3),
        };
        match (mirrored, (at + quarters).rem_euclid(4)) {
            (false, 0) => Normal,
            (false, 1) => Rotate90,
            (false, 2) => Rotate180,
            (false, _) => Rotate270,
            (true, 0) => FlipHorizontal,
            (true, 1) => Transverse,
            (true, 2) => FlipVertical,
            (true, _) => Transpose,
        }
    }

    /// The quarter turns clockwise that take `from` to this, when
    /// there are any: none when the two are mirrored differently,
    /// which no quarter turn can undo.
    pub fn turns_from(self, from: Orientation) -> Option<u8> {
        (0..4u8).find(|q| from.turned(i32::from(*q)) == self)
    }
}

/// Sensor samples in whatever width the file stores them.
#[derive(Debug, Clone, PartialEq)]
pub enum Samples {
    U16(Vec<u16>),
    F32(Vec<f32>),
}

impl Samples {
    pub fn len(&self) -> usize {
        match self {
            Samples::U16(v) => v.len(),
            Samples::F32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copy the samples out as `f32`.
    pub fn to_f32(&self) -> Vec<f32> {
        match self {
            Samples::U16(v) => v.iter().map(|&s| s as f32).collect(),
            Samples::F32(v) => v.clone(),
        }
    }
}

/// A decoded but undeveloped image.
#[derive(Debug, Clone, PartialEq)]
pub struct RawFrame {
    pub make: String,
    pub model: String,
    /// Full sensor dimensions of `samples`.
    pub width: usize,
    pub height: usize,
    /// Samples per pixel: 1 for CFA and monochrome, 3 for linear.
    pub channels: usize,
    pub layout: SensorLayout,
    pub samples: Samples,
    pub levels: Levels,
    /// Camera white balance gains, RGB, normalized so green is 1. `None`
    /// when the file does not record them.
    pub as_shot_coefficients: Option<[f64; 3]>,
    /// Every calibration the file carries, in no particular order.
    pub calibrations: Vec<Calibration>,
    /// The region the camera considers the picture; the rest is masked
    /// pixels and border. `None` means the whole frame.
    pub crop: Option<Rect>,
    pub orientation: Orientation,
    /// What the file says the frame was shot at, for a consumer that
    /// corrects the lens, expects a noise level, or tells the user.
    /// The engine reads none of it: it measures the noise it acts on
    /// from the frame itself.
    pub shot: Shot,
}

/// What the file says about the taking: the lens, and what the camera
/// and the lens were set to. Every field is optional, since every one
/// is a tag a camera may leave out.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shot {
    pub lens_make: Option<String>,
    /// The lens's name as the camera wrote it (`LensModel`), or as the
    /// decoder's own lens table names it when the tag is missing.
    pub lens_model: Option<String>,
    /// Focal length, millimeters.
    pub focal_length: Option<f32>,
    /// The aperture, as an f-number.
    pub f_number: Option<f32>,
    /// The focus distance, meters, when the file records one.
    pub focus_distance: Option<f32>,
    /// The ISO speed the file records.
    pub iso: Option<u32>,
    /// The shutter, in seconds.
    pub exposure_time: Option<f32>,
}

/// What the frame was shot at, in words, for a display to show: the
/// body and its lens, then the exposure triangle. Each is what the
/// file said, with whatever it left out left out, so either can come
/// back empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShotSummary {
    /// "Canon EOS R5 · RF50mm F1.2 L USM".
    pub camera: String,
    /// "50 mm · f/1.2 · 1/250 s · ISO 400".
    pub exposure: String,
}

impl Shot {
    /// The shot in words: what the body and the lens were, and what
    /// they were set to. `make` and `model` are the file's camera tags.
    pub fn summary(&self, make: &str, model: &str) -> ShotSummary {
        let focal = self.focal_length.map(focal_text).unwrap_or_default();
        let aperture = self.f_number.map(aperture_text).unwrap_or_default();
        let shutter = self
            .exposure_time
            .and_then(shutter_text)
            .unwrap_or_default();
        let iso = self.iso.map(|i| format!("ISO {i}")).unwrap_or_default();
        ShotSummary {
            camera: join([
                camera_name(make, model).as_str(),
                // The lens's maker is left out: it is almost always
                // the body's, and a third party's name is in the
                // lens's own already.
                self.lens_model.as_deref().unwrap_or_default().trim(),
            ]),
            exposure: join([
                focal.as_str(),
                aperture.as_str(),
                shutter.as_str(),
                iso.as_str(),
            ]),
        }
    }
}

/// The parts that said something, a middle dot between them.
fn join<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

/// The camera in one name. A model usually repeats its maker ("Canon
/// EOS R5") and a maker often says more than the body does ("NIKON
/// CORPORATION"), so when the two start with the same word the model
/// speaks for both.
pub fn camera_name(make: &str, model: &str) -> String {
    let (make, model) = (make.trim(), model.trim());
    fn first(s: &str) -> &str {
        s.split_whitespace().next().unwrap_or_default()
    }
    if model.is_empty() {
        make.to_string()
    } else if make.is_empty() || first(model).eq_ignore_ascii_case(first(make)) {
        model.to_string()
    } else {
        format!("{make} {model}")
    }
}

/// A focal length: "50 mm".
pub fn focal_text(mm: f32) -> String {
    format!("{} mm", number(mm))
}

/// The f-numbers a barrel and a camera display are marked with, a
/// third of a stop apart. Each is a rounding of the exact `2^(k/6)`:
/// the mark reading 11 is 11.314, which is what an APEX aperture
/// decodes to, so the window below has to be wider than that 2.8%.
/// The marks are 12% apart, so no value falls in two windows.
const F_NUMBERS: [f32; 37] = [
    1.0, 1.1, 1.2, 1.4, 1.6, 1.8, 2.0, 2.2, 2.5, 2.8, 3.2, 3.5, 4.0, 4.5, 5.0, 5.6, 6.3, 7.1, 8.0,
    9.0, 10.0, 11.0, 13.0, 14.0, 16.0, 18.0, 20.0, 22.0, 25.0, 29.0, 32.0, 36.0, 40.0, 45.0, 51.0,
    57.0, 64.0,
];

/// An aperture: "f/1.2", by the mark it sits on. A file that stores
/// the aperture as an APEX value gives back 5.657 and 11.314 for what
/// the lens and every camera call f/5.6 and f/11, so a value within
/// 3% of a mark is that mark; anything else is a number.
pub fn aperture_text(f_number: f32) -> String {
    let marked = F_NUMBERS
        .iter()
        .copied()
        .find(|&f| (f - f_number).abs() <= 0.03 * f_number);
    format!("f/{}", number(marked.unwrap_or(f_number)))
}

/// A shutter speed: "1/250 s" under a second, "30 s" at or over one.
/// A time no fraction says honestly (0.6 s is not 1/2 s) is given the
/// way the camera shows it, in tenths.
pub fn shutter_text(seconds: f32) -> Option<String> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    if seconds >= 1.0 {
        return Some(format!("{} s", number(seconds)));
    }
    let n = (1.0 / seconds).round();
    let honest = n >= 2.0 && (1.0 / n - seconds).abs() <= 0.05 * seconds;
    Some(if honest {
        format!("1/{n:.0} s")
    } else {
        format!("{} s", number(seconds))
    })
}

/// A number with a decimal only where it has one to show.
fn number(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{:.0}", v.round())
    } else {
        format!("{v:.1}")
    }
}

impl RawFrame {
    /// Check the invariants a decoder must uphold.
    pub fn validate(&self) -> Result<()> {
        let expected = self.width * self.height * self.channels;
        if self.samples.len() != expected {
            return Err(Error::Unsupported(format!(
                "{}x{}x{} frame with {} samples",
                self.width,
                self.height,
                self.channels,
                self.samples.len()
            )));
        }
        let want_channels = match self.layout {
            SensorLayout::Cfa(_) | SensorLayout::Monochrome => 1,
            SensorLayout::Linear => 3,
        };
        if self.channels != want_channels {
            return Err(Error::Unsupported(format!(
                "{:?} layout with {} samples per pixel",
                self.layout, self.channels
            )));
        }
        if self.levels.black.cpp != self.channels || self.levels.white.cpp != self.channels {
            return Err(Error::Unsupported(
                "level pattern does not match samples per pixel".into(),
            ));
        }
        if let Some(c) = self.crop
            && (c.width == 0
                || c.height == 0
                || c.x + c.width > self.width
                || c.y + c.height > self.height)
        {
            return Err(Error::Unsupported(format!(
                "crop {c:?} outside {}x{}",
                self.width, self.height
            )));
        }
        Ok(())
    }

    /// The crop to develop: the recommended one, or the whole frame.
    pub fn effective_crop(&self) -> Rect {
        self.crop.unwrap_or(Rect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        })
    }

    /// Compare the samples in the crop against the declared white level.
    pub fn white_check(&self) -> WhiteCheck {
        let crop = self.effective_crop();
        let cpp = self.channels;
        let white = &self.levels.white;
        let declared = white.values.iter().copied().fold(f32::INFINITY, f32::min);
        let sample = |i: usize| match &self.samples {
            Samples::U16(v) => v[i] as f32,
            Samples::F32(v) => v[i],
        };
        let mut brightest = 0.0f32;
        let mut above = 0usize;
        let mut total = 0usize;
        for y in crop.y..crop.y + crop.height {
            for x in crop.x..crop.x + crop.width {
                let base = (y * self.width + x) * cpp;
                for ch in 0..cpp {
                    let s = sample(base + ch);
                    brightest = brightest.max(s);
                    if s > white.at(y, x, ch) {
                        above += 1;
                    }
                    total += 1;
                }
            }
        }
        WhiteCheck {
            declared,
            brightest,
            above,
            total,
        }
    }
}

/// Where the samples sit against the declared white level, over the crop.
///
/// A white level is metadata, and metadata can be wrong. This reports what
/// the data do: the brightest sample and how many sit above the level
/// declared for their position. It cannot say on its own that the level is
/// wrong, because Canon declares its white below the ADC ceiling on
/// purpose (the specular white marks where linearity ends, and blown
/// highlights sit above it in every file), so a population above the
/// level is normal there. What it gives is the numbers to look at when a
/// render disagrees with another tool's: rawler 0.8's white for the EOS R6
/// Mark II family, 12735 for a sensor that clips at 16383 and declares
/// 14888, was found by exactly that.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteCheck {
    /// The lowest white level declared over the pattern.
    pub declared: f32,
    /// The brightest sample in the crop.
    pub brightest: f32,
    /// Samples above the white level declared for their position.
    pub above: usize,
    /// Samples examined.
    pub total: usize,
}

impl WhiteCheck {
    /// The share of samples above the declared level.
    pub fn fraction_above(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.above as f64 / self.total as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutter_reads_as_a_photographer_says_it() {
        assert_eq!(shutter_text(1.0 / 250.0).unwrap(), "1/250 s");
        assert_eq!(shutter_text(0.5).unwrap(), "1/2 s");
        assert_eq!(shutter_text(1.0 / 8000.0).unwrap(), "1/8000 s");
        assert_eq!(shutter_text(0.005).unwrap(), "1/200 s");
        // A third of a second as a camera records it.
        assert_eq!(shutter_text(1.0 / 3.0).unwrap(), "1/3 s");
        // And as an APEX value rounds it: 1/3 is 11% away, so tenths.
        assert_eq!(shutter_text(0.3).unwrap(), "0.3 s");
        // 0.6 s is not 1/2 s, and 0.4 s sits between 1/2 and 1/3.
        assert_eq!(shutter_text(0.6).unwrap(), "0.6 s");
        assert_eq!(shutter_text(0.4).unwrap(), "0.4 s");
        assert_eq!(shutter_text(0.8).unwrap(), "0.8 s");
        // A hair under a second still counts up, not "1.0 s".
        assert_eq!(shutter_text(0.98).unwrap(), "1 s");
        // A second and over counts up.
        assert_eq!(shutter_text(1.0).unwrap(), "1 s");
        assert_eq!(shutter_text(1.3).unwrap(), "1.3 s");
        assert_eq!(shutter_text(30.0).unwrap(), "30 s");
        // Nothing to say.
        assert_eq!(shutter_text(0.0), None);
        assert_eq!(shutter_text(-1.0), None);
        assert_eq!(shutter_text(f32::NAN), None);
    }

    #[test]
    fn an_aperture_lands_on_the_mark_it_was_set_to() {
        assert_eq!(aperture_text(1.2), "f/1.2");
        assert_eq!(aperture_text(5.6), "f/5.6");
        assert_eq!(aperture_text(2.0), "f/2");
        // What an APEX aperture decodes to: the marks named 5.6, 11
        // and 22.
        assert_eq!(aperture_text(5.656_854), "f/5.6");
        assert_eq!(aperture_text(11.313_708), "f/11");
        assert_eq!(aperture_text(22.627_417), "f/22");
        // Nowhere near a mark: as it is.
        assert_eq!(aperture_text(6.0), "f/6");
        assert_eq!(aperture_text(3.0), "f/3");
    }

    #[test]
    fn a_focal_length_keeps_the_half_millimeter() {
        assert_eq!(focal_text(50.0), "50 mm");
        assert_eq!(focal_text(10.5), "10.5 mm");
        assert_eq!(focal_text(400.0), "400 mm");
    }

    #[test]
    fn camera_name_says_the_maker_once() {
        assert_eq!(camera_name("Canon", "Canon EOS R5"), "Canon EOS R5");
        assert_eq!(camera_name("NIKON CORPORATION", "NIKON D850"), "NIKON D850");
        assert_eq!(camera_name("SONY", "ILCE-7RM5"), "SONY ILCE-7RM5");
        assert_eq!(camera_name("FUJIFILM", " X-T5 "), "FUJIFILM X-T5");
        assert_eq!(camera_name("", "X-T5"), "X-T5");
        assert_eq!(camera_name("Canon", ""), "Canon");
        assert_eq!(camera_name("", ""), "");
    }

    #[test]
    fn a_summary_is_the_camera_then_the_triangle() {
        let shot = Shot {
            lens_model: Some("RF50mm F1.2 L USM".into()),
            lens_make: Some("Canon".into()),
            focal_length: Some(50.0),
            f_number: Some(1.2),
            exposure_time: Some(1.0 / 250.0),
            iso: Some(400),
            focus_distance: Some(2.0),
        };
        let s = shot.summary("Canon", "Canon EOS R5");
        assert_eq!(s.camera, "Canon EOS R5 \u{b7} RF50mm F1.2 L USM");
        assert_eq!(
            s.exposure,
            "50 mm \u{b7} f/1.2 \u{b7} 1/250 s \u{b7} ISO 400"
        );
    }

    #[test]
    fn a_summary_drops_what_the_file_left_out() {
        let shot = Shot {
            f_number: Some(8.0),
            iso: Some(100),
            ..Default::default()
        };
        let s = shot.summary("SONY", "ILCE-7RM5");
        assert_eq!(s.camera, "SONY ILCE-7RM5");
        assert_eq!(s.exposure, "f/8 \u{b7} ISO 100");
        // A file that says nothing says nothing.
        assert_eq!(Shot::default().summary("", ""), ShotSummary::default());
    }

    #[test]
    fn exif_orientation_codes_map_in_tag_order() {
        assert_eq!(Orientation::from_exif(1), Some(Orientation::Normal));
        assert_eq!(Orientation::from_exif(6), Some(Orientation::Rotate90));
        assert_eq!(Orientation::from_exif(8), Some(Orientation::Rotate270));
        assert_eq!(Orientation::from_exif(0), None);
        assert_eq!(Orientation::from_exif(9), None);
        for code in 1..=8u16 {
            assert_eq!(Orientation::from_exif(code).unwrap() as u16 + 1, code);
        }
    }

    #[test]
    fn a_quarter_turn_steps_the_tag_along_its_own_cycle() {
        use Orientation::*;
        // Four turns are none, and a turn never mirrors a frame that
        // was not mirrored, nor unmirrors one that was.
        for tag in [
            Normal,
            FlipHorizontal,
            Rotate180,
            FlipVertical,
            Transpose,
            Rotate90,
            Transverse,
            Rotate270,
        ] {
            assert_eq!(tag.turned(0), tag);
            assert_eq!(tag.turned(4), tag);
            assert_eq!(tag.turned(1).turned(3), tag);
            assert_eq!(tag.turned(-1), tag.turned(3));
            for q in 0..4 {
                assert_eq!(tag.turned(q).turns_from(tag), Some(q as u8));
            }
        }
        // A frame the camera called upright, turned right, is the
        // tag a camera held on its side would have written.
        assert_eq!(Normal.turned(1), Rotate90);
        assert_eq!(Normal.turned(-1), Rotate270);
        assert_eq!(Rotate90.turned(1), Rotate180);
        assert_eq!(FlipHorizontal.turned(1), Transverse);
        assert_eq!(Transpose.turned(1), FlipHorizontal);
        // Mirrored and not are different cycles, and no turn crosses.
        assert_eq!(Normal.turns_from(FlipHorizontal), None);
        assert_eq!(Transpose.turns_from(Rotate90), None);
        // The tag values, which is what an XMP carries.
        assert_eq!(Normal.to_exif(), 1);
        assert_eq!(Rotate90.to_exif(), 6);
        assert_eq!(Rotate270.to_exif(), 8);
        for code in 1..=8u16 {
            assert_eq!(Orientation::from_exif(code).unwrap().to_exif(), code);
        }
    }

    #[test]
    fn cfa_pattern_shifts_with_the_origin() {
        let p = CfaPattern::rggb();
        assert_eq!(p.color_at(0, 0), CfaColor::Red);
        assert_eq!(p.color_at(1, 1), CfaColor::Blue);
        assert_eq!(p.color_at(2, 3), CfaColor::Green);
        let s = p.shifted(1, 0);
        assert_eq!(s.color_at(0, 0), CfaColor::Green);
        assert_eq!(s.color_at(0, 1), CfaColor::Red);
        let s = p.shifted(1, 1);
        assert_eq!(s.color_at(0, 0), CfaColor::Blue);
    }

    #[test]
    fn a_pattern_prints_as_a_person_would_name_it() {
        assert_eq!(CfaPattern::rggb().to_string(), "2x2 RGGB");
        assert_eq!(
            CfaPattern::rggb().shifted(1, 1).to_string(),
            "2x2 BGGR",
            "the colors come out in order"
        );
        assert_eq!(xtrans().to_string(), "6x6 (X-Trans)");
        // Anything a 3x3 matrix does not model is a question mark; the
        // backend's own code stays in the Debug.
        let odd = CfaPattern::new(
            2,
            2,
            vec![
                CfaColor::Red,
                CfaColor::Green,
                CfaColor::Blue,
                CfaColor::Other(3),
            ],
        )
        .unwrap();
        assert_eq!(odd.to_string(), "2x2 RGB?");
    }

    #[test]
    fn level_pattern_indexes_by_cell_then_channel() {
        let l = LevelPattern::new(2, 2, 1, vec![10.0, 11.0, 12.0, 13.0]).unwrap();
        assert_eq!(l.at(0, 1, 0), 11.0);
        assert_eq!(l.at(3, 2, 0), 12.0);
        assert_eq!(l.shifted(1, 1).at(0, 0, 0), 13.0);
        let u = LevelPattern::uniform(7.0, 3);
        assert_eq!(u.at(9, 9, 2), 7.0);
        assert!(LevelPattern::new(2, 2, 1, vec![0.0; 3]).is_err());
    }

    /// A 200x200 frame with the given samples, black 100, white 1000, and a
    /// crop that leaves out a 10-sample border.
    fn frame_with(samples: Vec<u16>) -> RawFrame {
        RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            width: 200,
            height: 200,
            channels: 1,
            layout: SensorLayout::Cfa(CfaPattern::rggb()),
            samples: Samples::U16(samples),
            levels: Levels {
                black: LevelPattern::uniform(100.0, 1),
                white: LevelPattern::uniform(1000.0, 1),
            },
            as_shot_coefficients: None,
            calibrations: Vec::new(),
            crop: Some(Rect {
                x: 10,
                y: 10,
                width: 180,
                height: 180,
            }),
            orientation: Orientation::Normal,
            shot: Shot::default(),
        }
    }

    #[test]
    fn white_check_counts_samples_above_the_level_inside_the_crop() {
        // Everything at the declared level: clipped there, nothing above.
        let clipped = frame_with(vec![1000; 200 * 200]).white_check();
        assert_eq!(clipped.declared, 1000.0);
        assert_eq!(clipped.brightest, 1000.0);
        assert_eq!(clipped.above, 0);
        assert_eq!(clipped.total, 180 * 180);
        assert_eq!(clipped.fraction_above(), 0.0);

        // Three samples above it.
        let mut samples = vec![500u16; 200 * 200];
        for i in [50 * 200 + 50, 60 * 200 + 120, 150 * 200 + 30] {
            samples[i] = 1200;
        }
        let hot = frame_with(samples).white_check();
        assert_eq!(hot.above, 3);
        assert_eq!(hot.brightest, 1200.0);

        // The same samples outside the crop are ignored.
        let mut border = vec![500u16; 200 * 200];
        for row in border.chunks_mut(200).take(10) {
            row.fill(1300);
        }
        let ignored = frame_with(border).white_check();
        assert_eq!(ignored.above, 0);
        assert_eq!(ignored.brightest, 500.0);
    }
}
