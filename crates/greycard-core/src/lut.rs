//! Color lookup tables: `.cube` and HaldCLUT PNG read into one
//! [`Lut3d`], sampled tetrahedrally.
//!
//! A LUT is made for a particular input encoding — nearly always
//! display-referred sRGB — so it cannot be applied to a scene-referred
//! picture as it stands. [`Look`] is the whole stage a consumer wants:
//! the working space's linear color taken to the table's primaries,
//! clipped into them, encoded, looked up, decoded, brought back and
//! blended by a strength. That order is notes §78's, and the viewport
//! shader mirrors it step for step.
//!
//! What this module deliberately does not do: decide where LUT files
//! live, or what a look means in an edit. Those belong to consumers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rawcolor::RgbSpace;

use crate::color::{CAT, WORKING_SPACE};
use crate::error::{Error, Result};

/// The largest table this build reads, per axis.
///
/// A table is cubed: 144 is 3.0 million entries, 36 MB of floats here
/// and 24 MB as half floats on a GPU, and it is what a HaldCLUT of
/// level 12 comes to. Bigger than that is refused by name rather than
/// quietly resampled.
pub const MAX_SIZE: usize = 144;

/// What a table's input and output are encoded in.
///
/// The default is [`Encoding::Srgb`]: a look LUT is a display-referred
/// thing, and every collection worth reading — the Pat David film
/// stocks, RawTherapee's, what a grading application writes — is made
/// against sRGB's curve.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Encoding {
    /// sRGB's piecewise curve.
    #[default]
    Srgb,
    /// Rec.709's camera curve. Not sRGB's: the toe and the exponent
    /// differ, and a table made against a broadcast chain says so.
    Rec709,
    /// None at all: the table is indexed by linear light.
    Linear,
    /// A plain power. The number is the display gamma, so 2.2 means
    /// the encoded value is the linear one to the power 1/2.2.
    Gamma(f32),
}

impl Encoding {
    /// Linear light to the table's input.
    pub fn encode(self, v: f32) -> f32 {
        match self {
            Encoding::Srgb => {
                if v <= 0.003_130_8 {
                    12.92 * v
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055
                }
            }
            Encoding::Rec709 => {
                if v < 0.018 {
                    4.5 * v
                } else {
                    1.099 * v.powf(0.45) - 0.099
                }
            }
            Encoding::Linear => v,
            // Sign times the power of the magnitude: a plain `powf` of
            // a negative number is NaN, and a table is allowed to hand
            // one back. The shader does the same.
            Encoding::Gamma(g) => v.abs().powf(1.0 / g.max(1e-3)).copysign(v),
        }
    }

    /// The table's output back to linear light.
    pub fn decode(self, v: f32) -> f32 {
        match self {
            Encoding::Srgb => {
                if v <= 0.040_45 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            }
            Encoding::Rec709 => {
                if v < 0.081 {
                    v / 4.5
                } else {
                    ((v + 0.099) / 1.099).powf(1.0 / 0.45)
                }
            }
            Encoding::Linear => v,
            Encoding::Gamma(g) => v.abs().powf(g.max(1e-3)).copysign(v),
        }
    }

    /// The word a file declares it with, and what is shown.
    pub fn name(self) -> String {
        match self {
            Encoding::Srgb => "sRGB".into(),
            Encoding::Rec709 => "Rec.709".into(),
            Encoding::Linear => "linear".into(),
            Encoding::Gamma(g) => format!("gamma {g}"),
        }
    }

    /// A declaration's words: `srgb`, `linear`, `rec709`, or `gamma`
    /// and a number.
    fn from_words(words: &[&str]) -> Option<Self> {
        let first = words.first()?.to_ascii_lowercase();
        match first.as_str() {
            "srgb" | "s-rgb" => Some(Encoding::Srgb),
            "rec709" | "rec.709" | "bt709" | "bt.709" => Some(Encoding::Rec709),
            "linear" | "scene-linear" | "none" => Some(Encoding::Linear),
            "gamma" => words.get(1)?.parse().ok().map(Encoding::Gamma),
            _ => first.strip_prefix("gamma").and_then(|g| {
                let g = g.trim_start_matches(['-', '_']);
                g.parse().ok().map(Encoding::Gamma)
            }),
        }
    }
}

/// The primaries a table's input and output are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Primaries {
    #[default]
    Srgb,
    DisplayP3,
    AdobeRgb,
    Rec2020,
    ProPhoto,
}

impl Primaries {
    pub fn space(self) -> RgbSpace {
        match self {
            Primaries::Srgb => RgbSpace::SRGB,
            Primaries::DisplayP3 => RgbSpace::DISPLAY_P3,
            Primaries::AdobeRgb => RgbSpace::ADOBE_RGB,
            Primaries::Rec2020 => RgbSpace::REC2020,
            Primaries::ProPhoto => RgbSpace::PROPHOTO,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Primaries::Srgb => "sRGB",
            Primaries::DisplayP3 => "Display P3",
            Primaries::AdobeRgb => "Adobe RGB",
            Primaries::Rec2020 => "Rec.2020",
            Primaries::ProPhoto => "ProPhoto",
        }
    }

    fn from_word(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "srgb" | "rec709" | "rec.709" | "bt709" | "bt.709" => Some(Primaries::Srgb),
            "p3" | "displayp3" | "display-p3" | "dci-p3" => Some(Primaries::DisplayP3),
            "adobergb" | "adobe-rgb" | "argb" => Some(Primaries::AdobeRgb),
            "rec2020" | "rec.2020" | "bt2020" | "bt.2020" => Some(Primaries::Rec2020),
            "prophoto" | "prophotorgb" | "romm" => Some(Primaries::ProPhoto),
            _ => None,
        }
    }
}

/// A three-dimensional color lookup table.
///
/// The entries run red fastest, then green, then blue, which is what
/// both a `.cube` file and a HaldCLUT lay their values out in.
#[derive(Debug, Clone, PartialEq)]
pub struct Lut3d {
    /// The name the file gave itself, when it gave one.
    pub title: Option<String>,
    /// Nodes per axis, at least two.
    pub size: usize,
    /// `size^3` entries, red fastest.
    pub data: Vec<[f32; 3]>,
    /// The input value the first node stands for, per channel.
    pub domain_min: [f32; 3],
    /// The input value the last node stands for, per channel.
    pub domain_max: [f32; 3],
    /// What the input and the output are encoded in.
    pub encoding: Encoding,
    /// The primaries they are in.
    pub primaries: Primaries,
    /// The file was one-dimensional and was spread over a cube
    /// ([`Lut3d::from_cube`] says what that costs).
    pub from_1d: bool,
}

/// What a listing says about a file without reading its table.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub title: Option<String>,
    pub size: usize,
    pub encoding: Encoding,
    pub primaries: Primaries,
    /// Nodes per axis of the file as written: a HaldCLUT's level, a
    /// 1D table's length.
    pub kind: Kind,
}

/// Which sort of file a LUT came out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cube3d,
    Cube1d,
    /// A HaldCLUT PNG of this level; its cube is the level squared.
    Hald(usize),
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Cube3d => "3D .cube",
            Kind::Cube1d => "1D .cube",
            Kind::Hald(_) => "HaldCLUT",
        }
    }
}

/// The extensions a look file may have.
pub const EXTENSIONS: [&str; 2] = ["cube", "png"];

/// How many nodes a 1D table is spread over when it is turned into a
/// cube. A 1D table is a per-channel curve, and tetrahedral
/// interpolation is exact for one (see the tests), so the only loss is
/// resampling a curve longer than this onto this many nodes.
const FROM_1D_SIZE: usize = 64;

/// How much of a `.cube` a header read looks at.
///
/// Every keyword sits above the first row, so this is generous by a
/// long way; a listing of three hundred film stocks is 300 of these
/// rather than 300 whole files, which at 64 nodes an axis is the
/// difference between 19 MB and a gigabyte on the thread that draws
/// the panel. A file whose header somehow runs past it is read whole
/// rather than guessed at.
const HEADER_BYTES: usize = 64 * 1024;

/// The first [`HEADER_BYTES`] of a file as text, and whether that cut
/// it short.
///
/// A chunk that ends mid-line loses that line: the header is parsed
/// from whole lines, and half a row is not one.
fn read_head(path: &Path) -> Result<(String, bool)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; HEADER_BYTES];
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    let cut = filled == HEADER_BYTES;
    buf.truncate(filled);
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if cut {
        match text.rfind('\n') {
            Some(i) => text.truncate(i + 1),
            None => text.clear(),
        }
    }
    Ok((text, cut))
}

/// What a line of a `.cube` is.
enum Line<'a> {
    Blank,
    /// The text after the `#`.
    Comment(&'a str),
    /// A keyword and the rest of the line: the first word starts with
    /// a letter, whether or not this build knows it.
    Keyword(&'a str, &'a str),
    /// Anything else, which is where the table starts.
    Row,
}

fn classify(line: &str) -> Line<'_> {
    if line.is_empty() {
        return Line::Blank;
    }
    if let Some(comment) = line.strip_prefix('#') {
        return Line::Comment(comment);
    }
    let word = line.split_whitespace().next().unwrap_or_default();
    if word.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Line::Keyword(word, line[word.len()..].trim());
    }
    Line::Row
}

/// What a comment says about the encoding and the primaries, if
/// anything. The conventions are [`Lut3d::from_cube`]'s.
fn read_convention(
    comment: &str,
    encoding: &mut Option<Encoding>,
    primaries: &mut Option<Primaries>,
) {
    let words: Vec<&str> = comment
        .split([' ', '\t', ':', '=', ','])
        .filter(|w| !w.is_empty())
        .collect();
    let Some(key) = words.first() else { return };
    let rest = &words[1..];
    match key.to_ascii_lowercase().as_str() {
        "encoding" | "greycard_encoding" => {
            if let Some(e) = Encoding::from_words(rest) {
                *encoding = Some(e);
            }
        }
        "primaries" | "space" | "colorspace" | "greycard_space" => {
            if let Some(p) = rest.first().and_then(|w| Primaries::from_word(w)) {
                *primaries = Some(p);
            }
        }
        _ => {}
    }
}

/// A file's text without a byte order mark, which some editors put on
/// the front of a `.cube` and which is not part of its first keyword.
fn without_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

fn lut_err(what: impl std::fmt::Display) -> Error {
    Error::Lut(what.to_string())
}

impl Lut3d {
    /// The table that changes nothing, at `size` nodes an axis.
    pub fn identity(size: usize) -> Self {
        let size = size.clamp(2, MAX_SIZE);
        let last = (size - 1) as f32;
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push([r as f32 / last, g as f32 / last, b as f32 / last]);
                }
            }
        }
        Lut3d {
            title: None,
            size,
            data,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            encoding: Encoding::default(),
            primaries: Primaries::default(),
            from_1d: false,
        }
    }

    /// Whether this table changes nothing.
    pub fn is_identity(&self) -> bool {
        *self == Lut3d::identity(self.size)
    }

    /// A table from a file, by its extension: `.cube` or a HaldCLUT
    /// `.png`.
    pub fn load(path: &Path) -> Result<Self> {
        match extension(path).as_deref() {
            Some("cube") => {
                let text = std::fs::read_to_string(path)?;
                Self::from_cube(&text)
            }
            Some("png") => Self::from_hald(&std::fs::read(path)?),
            Some(other) => Err(lut_err(format!(
                "{other:?} is not a LUT this build reads; it reads .cube and HaldCLUT .png"
            ))),
            None => Err(lut_err("the file has no extension to read it by")),
        }
    }

    /// A file's header alone, for a listing: no table is built, and
    /// for a `.cube` only the first [`HEADER_BYTES`] are read.
    pub fn info(path: &Path) -> Result<Info> {
        match extension(path).as_deref() {
            Some("cube") => {
                let (head, cut) = read_head(path)?;
                match cube_header(&head) {
                    // A header longer than the first chunk is the one
                    // case where the whole file is worth reading: a
                    // listing that said "no size line" would be wrong.
                    Err(e) if cut => {
                        log::debug!(
                            "look {}: {e} in the first chunk; reading it whole",
                            path.display()
                        );
                        cube_header(&std::fs::read_to_string(path)?)
                    }
                    other => other,
                }
            }
            Some("png") => {
                let (w, h) = image::ImageReader::open(path)?
                    .into_dimensions()
                    .map_err(|e| lut_err(format!("the PNG could not be read: {e}")))?;
                let level = hald_level(w, h)?;
                Ok(Info {
                    title: None,
                    size: level * level,
                    encoding: Encoding::default(),
                    primaries: Primaries::default(),
                    kind: Kind::Hald(level),
                })
            }
            Some(other) => Err(lut_err(format!(
                "{other:?} is not a LUT this build reads; it reads .cube and HaldCLUT .png"
            ))),
            None => Err(lut_err("the file has no extension to read it by")),
        }
    }

    /// One node, by its place on each axis.
    #[inline]
    pub fn node(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.data[(b * self.size + g) * self.size + r]
    }

    /// Where an input value sits on the grid, in nodes, clamped to it.
    #[inline]
    fn position(&self, rgb: [f32; 3]) -> [f32; 3] {
        let last = (self.size - 1) as f32;
        [0, 1, 2].map(|k| {
            let span = self.domain_max[k] - self.domain_min[k];
            let t = (rgb[k] - self.domain_min[k]) / span;
            (t * last).clamp(0.0, last)
        })
    }

    /// The table at a color in its own encoding, tetrahedrally.
    ///
    /// Tetrahedral interpolation splits each cell into six tetrahedra
    /// sharing the cell's black-to-white diagonal, so a color on that
    /// diagonal is a straight blend of the two neutral corners and
    /// comes out neutral exactly. Trilinear mixes all eight corners
    /// and does not (the tests hold both down). It is also what every
    /// grading application resolves a `.cube` with, so a LUT looks
    /// here as it looked where it was made.
    pub fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let p = self.position(rgb);
        let top = self.size - 2;
        let i = p.map(|v| (v.floor() as usize).min(top));
        let f = [0, 1, 2].map(|k| p[k] - i[k] as f32);
        let (ir, ig, ib) = (i[0], i[1], i[2]);
        let at = |dr: usize, dg: usize, db: usize| self.node(ir + dr, ig + dg, ib + db);
        let (fr, fg, fb) = (f[0], f[1], f[2]);
        let c000 = at(0, 0, 0);
        // Which tetrahedron the point is in is the order of the three
        // fractions; each is the base corner plus three edges, and
        // every branch's three edges join black to white, so equal
        // fractions — a neutral — give `c000 + f * (c111 - c000)`.
        let (er, eg, eb) = if fr > fg {
            if fg > fb {
                (
                    (at(1, 0, 0), c000),
                    (at(1, 1, 0), at(1, 0, 0)),
                    (at(1, 1, 1), at(1, 1, 0)),
                )
            } else if fr > fb {
                (
                    (at(1, 0, 0), c000),
                    (at(1, 1, 1), at(1, 0, 1)),
                    (at(1, 0, 1), at(1, 0, 0)),
                )
            } else {
                (
                    (at(1, 0, 1), at(0, 0, 1)),
                    (at(1, 1, 1), at(1, 0, 1)),
                    (at(0, 0, 1), c000),
                )
            }
        } else if fb > fg {
            (
                (at(1, 1, 1), at(0, 1, 1)),
                (at(0, 1, 1), at(0, 0, 1)),
                (at(0, 0, 1), c000),
            )
        } else if fb > fr {
            (
                (at(1, 1, 1), at(0, 1, 1)),
                (at(0, 1, 0), c000),
                (at(0, 1, 1), at(0, 1, 0)),
            )
        } else {
            (
                (at(1, 1, 0), at(0, 1, 0)),
                (at(0, 1, 0), c000),
                (at(1, 1, 1), at(1, 1, 0)),
            )
        };
        [0, 1, 2].map(|k| {
            c000[k] + fr * (er.0[k] - er.1[k]) + fg * (eg.0[k] - eg.1[k]) + fb * (eb.0[k] - eb.1[k])
        })
    }

    /// The table at a color, trilinearly: the eight corners of the
    /// cell by volume. Here to be tested against [`Lut3d::sample`],
    /// which is what the pipeline uses.
    pub fn sample_trilinear(&self, rgb: [f32; 3]) -> [f32; 3] {
        let p = self.position(rgb);
        let top = self.size - 2;
        let i = p.map(|v| (v.floor() as usize).min(top));
        let f = [0, 1, 2].map(|k| p[k] - i[k] as f32);
        let mut out = [0.0f32; 3];
        for db in 0..2 {
            for dg in 0..2 {
                for dr in 0..2 {
                    let w = [(dr, f[0]), (dg, f[1]), (db, f[2])]
                        .into_iter()
                        .map(|(d, t)| if d == 1 { t } else { 1.0 - t })
                        .product::<f32>();
                    let n = self.node(i[0] + dr, i[1] + dg, i[2] + db);
                    for k in 0..3 {
                        out[k] += w * n[k];
                    }
                }
            }
        }
        out
    }

    /// The table at a color in its own encoding, blended toward it by
    /// `strength`: nothing at zero, the whole table at one.
    ///
    /// This is the table's own blend, in the table's own space. The
    /// pipeline's is [`Look::at`], which blends in working linear
    /// after decoding, as notes §78 has it.
    pub fn apply(&self, rgb: [f32; 3], strength: f32) -> [f32; 3] {
        let s = strength.clamp(0.0, 1.0);
        if s <= 0.0 {
            return rgb;
        }
        let out = self.sample(rgb);
        [0, 1, 2].map(|k| rgb[k] + s * (out[k] - rgb[k]))
    }

    /// A table from the text of a `.cube` file.
    ///
    /// Understood: `TITLE`, `DOMAIN_MIN`, `DOMAIN_MAX`, `LUT_3D_SIZE`,
    /// `LUT_1D_SIZE`, comments, blank lines, and the rows themselves.
    ///
    /// The format says nothing about what its numbers are encoded in,
    /// so two comment conventions of this build's own are honored,
    /// case and punctuation aside:
    ///
    /// ```text
    /// # encoding: srgb        (or linear, rec709, gamma 2.2)
    /// # primaries: srgb       (or rec2020, p3, adobergb, prophoto)
    /// ```
    ///
    /// `GREYCARD_ENCODING` and `GREYCARD_SPACE` are read as the same
    /// two. Nothing else is: a file that says neither is sRGB in both,
    /// which is what a look LUT almost always is. A declaration must
    /// sit above the table; one below it is a comment, so that a
    /// header read ([`Lut3d::info`], which stops at the first row)
    /// cannot disagree with this about what a file says it is.
    ///
    /// A keyword this build has no use for — `LUT_IN_VIDEO_RANGE` and
    /// whatever else a grading application writes — is passed over
    /// with a line in the debug log, and a byte order mark on the
    /// front of the file is stripped.
    ///
    /// A 1D file is a per-channel curve and is spread over a cube of
    /// [`FROM_1D_SIZE`] nodes, or its own length where that is
    /// shorter. Tetrahedral interpolation is exact for a curve, so the
    /// only loss is resampling one longer than that.
    pub fn from_cube(text: &str) -> Result<Self> {
        let mut title = None;
        let mut size_3d = None;
        let mut size_1d = None;
        let mut domain_min = [0.0f32; 3];
        let mut domain_max = [1.0f32; 3];
        let mut encoding = None;
        let mut primaries = None;
        let mut rows: Vec<[f32; 3]> = Vec::new();

        for (n, raw) in without_bom(text).lines().enumerate() {
            let line = raw.trim();
            let number = n + 1;
            match classify(line) {
                Line::Blank => {}
                // A declaration sits above the table. One below it is
                // a comment and nothing more — not because reading it
                // would be hard, but because the listing stops at the
                // first row, and a header read and a full read must
                // not disagree about what a file says it is.
                Line::Comment(comment) if rows.is_empty() => {
                    read_convention(comment, &mut encoding, &mut primaries)
                }
                Line::Comment(_) => {}
                Line::Keyword(word, rest) => match word.to_ascii_uppercase().as_str() {
                    "TITLE" => {
                        title = Some(rest.trim_matches('"').trim().to_string())
                            .filter(|t| !t.is_empty());
                    }
                    "LUT_3D_SIZE" => {
                        size_3d = Some(read_size(rest.split_whitespace().next(), number, word)?)
                    }
                    "LUT_1D_SIZE" => {
                        size_1d = Some(read_size(rest.split_whitespace().next(), number, word)?)
                    }
                    "DOMAIN_MIN" => {
                        domain_min = read_triple(rest.split_whitespace(), number, word)?
                    }
                    "DOMAIN_MAX" => {
                        domain_max = read_triple(rest.split_whitespace(), number, word)?
                    }
                    "LUT_3D_INPUT_RANGE" | "LUT_1D_INPUT_RANGE" => {
                        // IRIDAS's older spelling of the domain, one pair
                        // for all three channels.
                        let pair = read_pair(rest.split_whitespace(), number, word)?;
                        domain_min = [pair[0]; 3];
                        domain_max = [pair[1]; 3];
                    }
                    // A keyword this build has no use for —
                    // `LUT_IN_VIDEO_RANGE` and the rest of what a
                    // grading application writes. Passing over it is
                    // right; calling its first word "not a number" was
                    // not.
                    _ => log::debug!("line {number}: {word} is not a keyword this build reads"),
                },
                Line::Row => {
                    rows.push(read_triple(line.split_whitespace(), number, "a table row")?)
                }
            }
        }

        if let (Some(a), Some(b)) = (size_3d, size_1d) {
            return Err(lut_err(format!(
                "the file declares both LUT_3D_SIZE {a} and LUT_1D_SIZE {b}; a .cube is one or the other"
            )));
        }
        for k in 0..3 {
            if domain_max[k] <= domain_min[k] {
                return Err(lut_err(format!(
                    "DOMAIN_MAX {} is not above DOMAIN_MIN {} on the {} axis",
                    domain_max[k],
                    domain_min[k],
                    ["red", "green", "blue"][k]
                )));
            }
        }
        let encoding = encoding.unwrap_or_default();
        let primaries = primaries.unwrap_or_default();

        if let Some(size) = size_3d {
            check_size(size, "LUT_3D_SIZE")?;
            let want = size * size * size;
            if rows.len() != want {
                return Err(lut_err(format!(
                    "LUT_3D_SIZE {size} asks for {want} rows, the file has {}",
                    rows.len()
                )));
            }
            return Ok(Lut3d {
                title,
                size,
                data: rows,
                domain_min,
                domain_max,
                encoding,
                primaries,
                from_1d: false,
            });
        }
        if let Some(length) = size_1d {
            if length < 2 {
                return Err(lut_err(format!(
                    "LUT_1D_SIZE {length}: a table needs at least two entries"
                )));
            }
            if rows.len() != length {
                return Err(lut_err(format!(
                    "LUT_1D_SIZE {length} asks for {length} rows, the file has {}",
                    rows.len()
                )));
            }
            return Ok(Self::from_curve(
                title, &rows, domain_min, domain_max, encoding, primaries,
            ));
        }
        Err(lut_err(
            "no LUT_3D_SIZE or LUT_1D_SIZE line: the file does not say how big its table is",
        ))
    }

    /// A per-channel curve spread over a cube.
    fn from_curve(
        title: Option<String>,
        curve: &[[f32; 3]],
        domain_min: [f32; 3],
        domain_max: [f32; 3],
        encoding: Encoding,
        primaries: Primaries,
    ) -> Self {
        let size = curve.len().clamp(2, FROM_1D_SIZE);
        let last = (size - 1) as f32;
        let at = |k: usize, t: f32| -> f32 {
            let x = (t * (curve.len() - 1) as f32).clamp(0.0, (curve.len() - 1) as f32);
            let i = (x.floor() as usize).min(curve.len() - 2);
            let f = x - i as f32;
            curve[i][k] + f * (curve[i + 1][k] - curve[i][k])
        };
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push([
                        at(0, r as f32 / last),
                        at(1, g as f32 / last),
                        at(2, b as f32 / last),
                    ]);
                }
            }
        }
        Lut3d {
            title,
            size,
            data,
            domain_min,
            domain_max,
            encoding,
            primaries,
            from_1d: true,
        }
    }

    /// A table from the bytes of a HaldCLUT PNG.
    ///
    /// A HaldCLUT of level `L` is a square image `L^3` pixels on a
    /// side holding a cube of `L^2` nodes an axis, laid out row by
    /// row with red running fastest — the same order a `.cube` has.
    /// Nothing in the image says what it is encoded in, and every
    /// collection that ships them is display-referred sRGB, so that is
    /// what one is read as.
    pub fn from_hald(bytes: &[u8]) -> Result<Self> {
        let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .map_err(|e| lut_err(format!("the PNG could not be read: {e}")))?;
        let level = hald_level(image.width(), image.height())?;
        let size = level * level;
        let rgb = image.to_rgb32f();
        let data: Vec<[f32; 3]> = rgb.pixels().map(|p| [p.0[0], p.0[1], p.0[2]]).collect();
        debug_assert_eq!(data.len(), size * size * size);
        Ok(Lut3d {
            title: None,
            size,
            data,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            encoding: Encoding::default(),
            primaries: Primaries::default(),
            from_1d: false,
        })
    }
}

/// The level of a HaldCLUT image of this size, or what is wrong with
/// it.
fn hald_level(width: u32, height: u32) -> Result<usize> {
    if width == 0 || width != height {
        return Err(lut_err(format!(
            "the image is {width} by {height}: a HaldCLUT is square"
        )));
    }
    let level = (width as f64).cbrt().round() as u32;
    if level < 2 || level * level * level != width {
        return Err(lut_err(format!(
            "the image is {width} wide, which is not a whole number cubed: a HaldCLUT of level L is L^3 wide"
        )));
    }
    check_size(level as usize * level as usize, "the HaldCLUT's level")?;
    Ok(level as usize)
}

fn check_size(size: usize, what: &str) -> Result<()> {
    if size < 2 {
        return Err(lut_err(format!(
            "{what} {size}: a table needs at least two nodes an axis"
        )));
    }
    if size > MAX_SIZE {
        return Err(lut_err(format!(
            "{what} gives {size} nodes an axis, more than the {MAX_SIZE} this build reads"
        )));
    }
    Ok(())
}

fn read_size(word: Option<&str>, line: usize, what: &str) -> Result<usize> {
    let Some(word) = word else {
        return Err(lut_err(format!(
            "line {line}: {what} with no number after it"
        )));
    };
    word.parse::<usize>().map_err(|_| {
        lut_err(format!(
            "line {line}: {what} {word:?} is not a whole number"
        ))
    })
}

fn read_triple<'a>(
    words: impl Iterator<Item = &'a str>,
    line: usize,
    what: &str,
) -> Result<[f32; 3]> {
    let values = read_numbers(words, line)?;
    match values.len() {
        3 => Ok([values[0], values[1], values[2]]),
        n => Err(lut_err(format!(
            "line {line}: {what} has {n} numbers where it needs 3"
        ))),
    }
}

fn read_pair<'a>(
    words: impl Iterator<Item = &'a str>,
    line: usize,
    what: &str,
) -> Result<[f32; 2]> {
    let values = read_numbers(words, line)?;
    match values.len() {
        2 => Ok([values[0], values[1]]),
        n => Err(lut_err(format!(
            "line {line}: {what} has {n} numbers where it needs 2"
        ))),
    }
}

fn read_numbers<'a>(words: impl Iterator<Item = &'a str>, line: usize) -> Result<Vec<f32>> {
    let mut out = Vec::new();
    for word in words {
        let v: f32 = word
            .parse()
            .map_err(|_| lut_err(format!("line {line}: {word:?} is not a number")))?;
        if !v.is_finite() {
            return Err(lut_err(format!(
                "line {line}: {word:?} is not a finite number"
            )));
        }
        out.push(v);
    }
    Ok(out)
}

/// A `.cube`'s header, up to but not including its table.
///
/// It stops at the first table row and at nothing earlier: a file may
/// put its `# encoding:` line under the size, and a listing that gave
/// up before reading it would tell the panel sRGB about a table
/// rendered as linear. Only the first [`HEADER_BYTES`] of the file
/// reach here, which is what makes the stop worth anything.
fn cube_header(text: &str) -> Result<Info> {
    let mut title = None;
    let mut size_3d = None;
    let mut size_1d = None;
    let mut encoding = None;
    let mut primaries = None;
    for (n, raw) in without_bom(text).lines().enumerate() {
        let line = raw.trim();
        match classify(line) {
            Line::Blank => {}
            Line::Comment(comment) => read_convention(comment, &mut encoding, &mut primaries),
            Line::Keyword(word, rest) => match word.to_ascii_uppercase().as_str() {
                "TITLE" => {
                    title =
                        Some(rest.trim_matches('"').trim().to_string()).filter(|t| !t.is_empty());
                }
                "LUT_3D_SIZE" => {
                    size_3d = Some(read_size(rest.split_whitespace().next(), n + 1, word)?)
                }
                "LUT_1D_SIZE" => {
                    size_1d = Some(read_size(rest.split_whitespace().next(), n + 1, word)?)
                }
                _ => {}
            },
            Line::Row => break,
        }
    }
    let encoding = encoding.unwrap_or_default();
    let primaries = primaries.unwrap_or_default();
    if let Some(size) = size_3d {
        check_size(size, "LUT_3D_SIZE")?;
        return Ok(Info {
            title,
            size,
            encoding,
            primaries,
            kind: Kind::Cube3d,
        });
    }
    if let Some(length) = size_1d {
        return Ok(Info {
            title,
            size: length.clamp(2, FROM_1D_SIZE),
            encoding,
            primaries,
            kind: Kind::Cube1d,
        });
    }
    Err(lut_err(
        "no LUT_3D_SIZE or LUT_1D_SIZE line: the file does not say how big its table is",
    ))
}

fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_string_lossy().to_ascii_lowercase())
}

/// Every look file in a directory, by its stem, sorted.
///
/// A file that is not a LUT is passed over; what is wrong with it is
/// the caller's to say, so its error comes back with it.
pub fn list_dir(dir: &Path) -> Vec<(String, PathBuf, Result<Info>)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf, Result<Info>)> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| extension(p).is_some_and(|x| EXTENSIONS.contains(&x.as_str())))
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let info = Lut3d::info(&path);
            Some((name, path, info))
        })
        .collect();
    out.sort_by_key(|(name, _, _)| name.to_lowercase());
    out
}

/// A look ready to run on the working space's linear color: a table, a
/// strength, and the matrices either side of it.
///
/// The stage, in order (notes §78): the working space's linear color
/// through `to_lut` into the table's primaries, clipped into them —
/// an sRGB table has nothing to say about a Rec.2020 green, so the
/// clip is the gamut map, and it is a clip, not a soft one — encoded,
/// looked up tetrahedrally, decoded, brought back through `from_lut`,
/// and blended with what came in by the strength.
///
/// The viewport shader does the same arithmetic with the same
/// matrices, which is why they are readable here.
#[derive(Debug, Clone)]
pub struct Look {
    pub lut: Arc<Lut3d>,
    /// 0 for nothing, 1 for the whole table.
    pub strength: f32,
    to_lut: [[f32; 3]; 3],
    from_lut: [[f32; 3]; 3],
}

impl Look {
    /// A look from a table and a strength. The matrices are solved
    /// once, here.
    pub fn new(lut: Arc<Lut3d>, strength: f32) -> Result<Self> {
        let space = lut.primaries.space();
        let to_lut = matrix(&WORKING_SPACE, &space)?;
        let from_lut = matrix(&space, &WORKING_SPACE)?;
        Ok(Look {
            lut,
            strength: strength.clamp(0.0, 1.0),
            to_lut,
            from_lut,
        })
    }

    /// The working space to the table's primaries, linear, rows.
    pub fn to_lut(&self) -> &[[f32; 3]; 3] {
        &self.to_lut
    }

    /// The table's primaries back to the working space, linear, rows.
    pub fn from_lut(&self) -> &[[f32; 3]; 3] {
        &self.from_lut
    }

    /// Whether this look would change any pixel.
    pub fn is_off(&self) -> bool {
        self.strength <= 0.0
    }

    /// One working-space linear color through the look.
    #[inline]
    pub fn at(&self, c: [f32; 3]) -> [f32; 3] {
        if self.is_off() {
            return c;
        }
        let e = apply_matrix(&self.to_lut, c).map(|v| self.lut.encoding.encode(v.clamp(0.0, 1.0)));
        let o = self.lut.sample(e).map(|v| self.lut.encoding.decode(v));
        let back = apply_matrix(&self.from_lut, o);
        [0, 1, 2].map(|k| c[k] + self.strength * (back[k] - c[k]))
    }
}

fn matrix(from: &RgbSpace, to: &RgbSpace) -> Result<[[f32; 3]; 3]> {
    Ok(from
        .to_space_matrix(to, CAT)
        .ok_or_else(|| lut_err("the table's primaries do not reach the working space"))?
        .rows
        .map(|row| row.map(|v| v as f32)))
}

#[inline]
fn apply_matrix(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|k| m[k][0] * c[0] + m[k][1] * c[1] + m[k][2] * c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `.cube`'s text from a function of the node's position.
    fn cube(size: usize, header: &str, f: impl Fn([f32; 3]) -> [f32; 3]) -> String {
        let last = (size - 1) as f32;
        let mut text = String::new();
        text.push_str(header);
        text.push_str(&format!("LUT_3D_SIZE {size}\n"));
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let v = f([r as f32 / last, g as f32 / last, b as f32 / last]);
                    text.push_str(&format!("{} {} {}\n", v[0], v[1], v[2]));
                }
            }
        }
        text
    }

    fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() <= tol)
    }

    #[test]
    fn an_identity_cube_changes_nothing_at_its_nodes_or_between_them() {
        let lut = Lut3d::from_cube(&cube(9, "", |c| c)).unwrap();
        assert_eq!(lut.size, 9);
        assert!(lut.is_identity());
        for v in [0.0, 0.125, 0.25, 0.3, 0.5, 0.625, 0.77, 1.0] {
            let c = [v, v * 0.5, 1.0 - v];
            assert!(
                close(lut.sample(c), c, 1e-6),
                "{c:?} -> {:?}",
                lut.sample(c)
            );
            assert!(close(lut.sample_trilinear(c), c, 1e-6));
        }
    }

    #[test]
    fn a_channel_swap_cube_swaps_channels() {
        // Blue, red, green out: a permutation is exact everywhere,
        // nodes and between, under either interpolation.
        let lut = Lut3d::from_cube(&cube(5, "TITLE \"swap\"\n", |c| [c[2], c[0], c[1]])).unwrap();
        assert_eq!(lut.title.as_deref(), Some("swap"));
        for c in [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.25, 0.5, 0.75],
            [0.13, 0.87, 0.41],
        ] {
            assert!(close(lut.sample(c), [c[2], c[0], c[1]], 1e-5), "{c:?}");
        }
    }

    #[test]
    fn a_curve_cube_is_exact_at_its_nodes_and_straight_between_them() {
        // A square law: a node is exact, and the midpoint of a cell is
        // the mean of the cell's ends, not the square of the midpoint.
        let size = 5;
        let lut = Lut3d::from_cube(&cube(size, "", |c| c.map(|v| v * v))).unwrap();
        for i in 0..size {
            let v = i as f32 / (size - 1) as f32;
            assert!(close(lut.sample([v; 3]), [v * v; 3], 1e-6), "node {i}");
        }
        let a = 0.25f32;
        let b = 0.5f32;
        let mid = (a + b) * 0.5;
        let want = (a * a + b * b) * 0.5;
        let got = lut.sample([mid; 3]);
        assert!(close(got, [want; 3], 1e-5), "{got:?} wanted {want}");
        assert!(want > mid * mid, "the straight line sits above the curve");
    }

    /// The table that holds this down is neutral-preserving and not
    /// separable: red picks up `g*b - r*r`, which is zero on the
    /// neutral axis and not multilinear, so trilinear cannot
    /// reproduce it and drags a grey off neutral. Tetrahedral splits
    /// the cell along the black-to-white diagonal, so a neutral is a
    /// blend of two neutral corners and stays one.
    #[test]
    fn tetrahedral_keeps_a_neutral_where_trilinear_does_not() {
        let lut = Lut3d::from_cube(&cube(5, "", |c| {
            [c[0] + 0.4 * (c[1] * c[2] - c[0] * c[0]), c[1], c[2]]
        }))
        .unwrap();
        let mut worst_tetra = 0.0f32;
        let mut worst_tri = 0.0f32;
        for i in 0..=40 {
            let v = i as f32 / 40.0;
            let spread = |c: [f32; 3]| {
                let hi = c[0].max(c[1]).max(c[2]);
                let lo = c[0].min(c[1]).min(c[2]);
                hi - lo
            };
            worst_tetra = worst_tetra.max(spread(lut.sample([v; 3])));
            worst_tri = worst_tri.max(spread(lut.sample_trilinear([v; 3])));
        }
        assert!(worst_tetra < 1e-6, "tetrahedral kept it: {worst_tetra}");
        assert!(worst_tri > 1e-3, "trilinear did not: {worst_tri}");
    }

    #[test]
    fn a_domain_moves_where_the_nodes_sit() {
        let text = cube(3, "DOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n", |c| c);
        let lut = Lut3d::from_cube(&text).unwrap();
        assert_eq!(lut.domain_max, [2.0; 3]);
        // The table's own values still run 0..1, but an input of 2 now
        // reaches the last node and an input of 1 the middle one.
        assert!(close(lut.sample([2.0; 3]), [1.0; 3], 1e-6));
        assert!(close(lut.sample([1.0; 3]), [0.5; 3], 1e-6));
        // Past the domain the table clamps rather than extrapolates.
        assert!(close(lut.sample([3.0; 3]), [1.0; 3], 1e-6));
    }

    #[test]
    fn a_comment_declares_the_encoding_and_the_primaries() {
        let plain = Lut3d::from_cube(&cube(2, "", |c| c)).unwrap();
        assert_eq!(plain.encoding, Encoding::Srgb);
        assert_eq!(plain.primaries, Primaries::Srgb);
        let said = Lut3d::from_cube(&cube(
            2,
            "# encoding: linear\n# primaries: rec2020\n",
            |c| c,
        ))
        .unwrap();
        assert_eq!(said.encoding, Encoding::Linear);
        assert_eq!(said.primaries, Primaries::Rec2020);
        let gamma = Lut3d::from_cube(&cube(2, "# GREYCARD_ENCODING gamma 2.2\n", |c| c)).unwrap();
        assert_eq!(gamma.encoding, Encoding::Gamma(2.2));
        // A comment that says nothing this build knows is a comment.
        let other =
            Lut3d::from_cube(&cube(2, "# Created by a grading application\n", |c| c)).unwrap();
        assert_eq!(other.encoding, Encoding::Srgb);
    }

    /// A file written by a grading application carries keywords this
    /// build has no use for and may carry a byte order mark; neither
    /// is a reason to refuse it. A convention under the size is still
    /// read, by the listing as well as by the reader.
    #[test]
    fn a_bom_and_an_unknown_keyword_are_passed_over() {
        let body = cube(2, "", |c| c);
        let odd = format!(
            "\u{feff}TITLE \"Odd\"\nLUT_IN_VIDEO_RANGE false\nWHATEVER 1 2 3\n# encoding: linear\n{body}"
        );
        let lut = Lut3d::from_cube(&odd).expect("it reads");
        assert_eq!(lut.title.as_deref(), Some("Odd"));
        assert_eq!(lut.size, 2);
        assert_eq!(lut.encoding, Encoding::Linear);
        // And the listing agrees with the reader about all of it,
        // the declaration that sits under the size included.
        let info = cube_header(&odd).expect("the header reads");
        assert_eq!(info.title.as_deref(), Some("Odd"));
        assert_eq!(info.size, 2);
        assert_eq!(info.encoding, Encoding::Linear);

        // A declaration below the table is a comment to both of them,
        // so neither can tell the other something it does not know.
        let late = format!("{body}# encoding: linear\n");
        assert_eq!(Lut3d::from_cube(&late).unwrap().encoding, Encoding::Srgb);
        assert_eq!(cube_header(&late).unwrap().encoding, Encoding::Srgb);
    }

    /// The listing stops at the first row and nowhere earlier: a
    /// declaration between the size and the table is not missed.
    #[test]
    fn a_header_read_stops_at_the_first_row() {
        let text = "LUT_3D_SIZE 2\n# primaries: rec2020\n# encoding: gamma 2.6\n0 0 0\n# encoding: linear\n";
        let info = cube_header(text).unwrap();
        assert_eq!(info.primaries, Primaries::Rec2020);
        // The one under the table is past where a header read looks,
        // which is the point of stopping there.
        assert_eq!(info.encoding, Encoding::Gamma(2.6));
    }

    /// A gamma encoding of a negative value is a number, not a NaN:
    /// a table may hand one back, and the shader does the same.
    #[test]
    fn a_gamma_encoding_keeps_a_negative_negative() {
        let g = Encoding::Gamma(2.2);
        for v in [-0.5f32, -0.01, -2.0] {
            assert!(g.encode(v).is_finite(), "encode {v}");
            assert!(g.decode(v).is_finite(), "decode {v}");
            assert!(g.encode(v) < 0.0 && g.decode(v) < 0.0, "{v}");
            assert!((g.decode(g.encode(v)) - v).abs() < 1e-5, "{v}");
        }
        assert_eq!(g.encode(0.0), 0.0);
    }

    #[test]
    fn every_encoding_round_trips() {
        for e in [
            Encoding::Srgb,
            Encoding::Rec709,
            Encoding::Linear,
            Encoding::Gamma(2.2),
        ] {
            for v in [0.0, 0.005, 0.02, 0.18, 0.5, 1.0] {
                let back = e.decode(e.encode(v));
                assert!((back - v).abs() < 1e-5, "{} at {v}: {back}", e.name());
            }
        }
    }

    #[test]
    fn a_one_dimensional_cube_reads_as_a_cube() {
        let mut text = String::from("TITLE \"curve\"\nLUT_1D_SIZE 5\n");
        for i in 0..5 {
            let v = i as f32 / 4.0;
            text.push_str(&format!("{} {} {}\n", v * v, v, v.sqrt()));
        }
        let lut = Lut3d::from_cube(&text).unwrap();
        assert!(lut.from_1d);
        assert_eq!(lut.size, 5);
        // Each channel takes its own curve, and nothing crosses.
        assert!(close(
            lut.sample([0.5, 0.5, 0.5]),
            [0.25, 0.5, 0.5f32.sqrt()],
            1e-5
        ));
        assert!(close(lut.sample([1.0, 0.0, 0.25]), [1.0, 0.0, 0.5], 1e-5));
    }

    /// A curve spread over a cube is separable, and tetrahedral
    /// interpolation is exact for a separable function: within a cell
    /// only one edge of the tetrahedron moves each channel. So the
    /// expansion costs nothing between the nodes either.
    #[test]
    fn a_spread_curve_is_exact_between_its_nodes() {
        let mut text = String::from("LUT_1D_SIZE 9\n");
        for i in 0..9 {
            let v = i as f32 / 8.0;
            text.push_str(&format!("{} {} {}\n", v * v, v * v, v * v));
        }
        let lut = Lut3d::from_cube(&text).unwrap();
        // At a point between nodes, every channel follows its own
        // straight line, whatever the other two are doing.
        let want = |t: f32| {
            let x = t * 8.0;
            let i = x.floor() as usize;
            let f = x - i as f32;
            let a = (i as f32 / 8.0).powi(2);
            let b = ((i + 1) as f32 / 8.0).powi(2);
            a + f * (b - a)
        };
        for c in [[0.3, 0.7, 0.1], [0.9, 0.2, 0.55], [0.06, 0.06, 0.94]] {
            let got = lut.sample(c);
            let wanted = c.map(want);
            assert!(close(got, wanted, 1e-5), "{c:?}: {got:?} wanted {wanted:?}");
        }
    }

    #[test]
    fn a_cube_says_what_is_wrong_with_it() {
        let says = |text: &str, what: &str| {
            let e = Lut3d::from_cube(text).expect_err("it is refused");
            let msg = e.to_string();
            assert!(msg.contains(what), "{msg:?} does not mention {what:?}");
        };
        says("# nothing here\n", "LUT_3D_SIZE");
        says("LUT_3D_SIZE 2\n0 0 0\n", "asks for 8 rows, the file has 1");
        says("LUT_3D_SIZE 1\n0 0 0\n", "at least two nodes");
        says("LUT_3D_SIZE 512\n", "more than the 144");
        says("LUT_3D_SIZE two\n", "is not a whole number");
        says("LUT_3D_SIZE\n", "with no number after it");
        says("LUT_3D_SIZE 2\nLUT_1D_SIZE 4\n", "one or the other");
        says(
            "LUT_3D_SIZE 2\n0 0 0\n0 0 blue\n",
            "line 3: \"blue\" is not a number",
        );
        says("LUT_3D_SIZE 2\n0 0\n", "has 2 numbers where it needs 3");
        says(
            "DOMAIN_MIN 1 0 0\nDOMAIN_MAX 0 1 1\nLUT_3D_SIZE 2\n",
            "red axis",
        );
        says("LUT_1D_SIZE 1\n0 0 0\n", "at least two entries");
    }

    /// A HaldCLUT of the same function as a `.cube` reads to the same
    /// table, node for node.
    #[test]
    fn a_hald_png_reads_to_the_table_its_pixels_hold() {
        let level = 3usize;
        let size = level * level; // 9 nodes an axis
        let width = (level * level * level) as u32; // 27 pixels a side
        let f = |c: [f32; 3]| [c[2], c[0], c[1]];
        let last = (size - 1) as f32;
        let mut px = image::RgbImage::new(width, width);
        let mut i = 0u32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let v = f([r as f32 / last, g as f32 / last, b as f32 / last]);
                    let level8 = v.map(|x| (x * 255.0).round() as u8);
                    px.put_pixel(i % width, i / width, image::Rgb(level8));
                    i += 1;
                }
            }
        }
        assert_eq!(i, width * width);
        let mut png = Vec::new();
        {
            use image::ImageEncoder;
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(px.as_raw(), width, width, image::ExtendedColorType::Rgb8)
                .unwrap();
        }
        let lut = Lut3d::from_hald(&png).unwrap();
        assert_eq!(lut.size, size);
        assert_eq!(lut.encoding, Encoding::Srgb);
        for (r, g, b) in [(0, 0, 0), (8, 0, 0), (3, 5, 7), (8, 8, 8)] {
            let want = f([r as f32 / last, g as f32 / last, b as f32 / last]);
            let got = lut.node(r, g, b);
            assert!(close(got, want, 1.0 / 255.0), "{r},{g},{b}: {got:?}");
        }
    }

    #[test]
    fn a_hald_png_says_what_is_wrong_with_it() {
        let png = |w: u32, h: u32| {
            let px = image::RgbImage::new(w, h);
            let mut out = Vec::new();
            use image::ImageEncoder;
            image::codecs::png::PngEncoder::new(&mut out)
                .write_image(px.as_raw(), w, h, image::ExtendedColorType::Rgb8)
                .unwrap();
            out
        };
        let e = Lut3d::from_hald(&png(27, 26)).expect_err("not square");
        assert!(e.to_string().contains("square"), "{e}");
        let e = Lut3d::from_hald(&png(26, 26)).expect_err("not a cube");
        assert!(e.to_string().contains("whole number cubed"), "{e}");
        let e = Lut3d::from_hald(b"not a png").expect_err("not a png");
        assert!(e.to_string().contains("could not be read"), "{e}");
    }

    #[test]
    fn strength_blends_toward_the_table() {
        let lut = Lut3d::from_cube(&cube(2, "", |_| [1.0, 0.0, 0.0])).unwrap();
        let c = [0.5, 0.5, 0.5];
        assert_eq!(lut.apply(c, 0.0), c);
        assert!(close(lut.apply(c, 1.0), [1.0, 0.0, 0.0], 1e-6));
        assert!(close(lut.apply(c, 0.5), [0.75, 0.25, 0.25], 1e-6));
        // Out of range is held to it rather than extrapolated.
        assert_eq!(lut.apply(c, -1.0), c);
        assert!(close(lut.apply(c, 2.0), [1.0, 0.0, 0.0], 1e-6));
    }

    #[test]
    fn a_look_on_an_identity_table_gives_back_what_it_was_given() {
        let look = Look::new(Arc::new(Lut3d::identity(17)), 1.0).unwrap();
        for c in [[0.18; 3], [0.5, 0.2, 0.05], [0.9, 0.9, 0.9], [0.0; 3]] {
            let got = look.at(c);
            assert!(close(got, c, 2e-3), "{c:?} -> {got:?}");
        }
    }

    /// The gamut map is a clip: a Rec.2020 green that an sRGB table
    /// has no entry for comes back on sRGB's edge, not outside it.
    #[test]
    fn an_srgb_table_clips_what_it_cannot_hold() {
        let look = Look::new(Arc::new(Lut3d::identity(33)), 1.0).unwrap();
        let green = [0.0, 1.0, 0.0]; // Rec.2020's green primary.
        let out = look.at(green);
        // Through the identity it is sRGB's green, which in Rec.2020
        // is short of the primary and has red and blue in it.
        assert!(out[1] < 0.95, "{out:?}");
        assert!(out[0] > 0.0 && out[2] > 0.0, "{out:?}");
        // And nothing is negative, which is what the clip is for.
        assert!(out.iter().all(|v| *v > -1e-3), "{out:?}");
    }

    #[test]
    fn a_looks_strength_blends_in_working_linear() {
        let lut = Arc::new(Lut3d::from_cube(&cube(2, "", |_| [0.0, 0.0, 0.0])).unwrap());
        let c = [0.18, 0.18, 0.18];
        let off = Look::new(lut.clone(), 0.0).unwrap();
        assert_eq!(off.at(c), c);
        assert!(off.is_off());
        let half = Look::new(lut.clone(), 0.5).unwrap();
        assert!(close(half.at(c), [0.09; 3], 1e-4), "{:?}", half.at(c));
        let full = Look::new(lut, 1.0).unwrap();
        assert!(close(full.at(c), [0.0; 3], 1e-4));
    }

    #[test]
    fn a_looks_matrices_undo_each_other() {
        let look = Look::new(Arc::new(Lut3d::identity(2)), 1.0).unwrap();
        let c = [0.3, 0.6, 0.2];
        let there = apply_matrix(look.to_lut(), c);
        let back = apply_matrix(look.from_lut(), there);
        assert!(close(back, c, 1e-5), "{back:?}");
    }

    #[test]
    fn a_header_read_gives_a_listing_what_it_needs() {
        let dir = std::env::temp_dir().join(format!("greycard-lut-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Some Look.cube");
        std::fs::write(
            &path,
            cube(5, "TITLE \"Some Look\"\n# encoding: linear\n", |c| c),
        )
        .unwrap();
        let info = Lut3d::info(&path).unwrap();
        assert_eq!(info.title.as_deref(), Some("Some Look"));
        assert_eq!(info.size, 5);
        assert_eq!(info.encoding, Encoding::Linear);
        assert_eq!(info.kind, Kind::Cube3d);
        // And the directory lists it.
        let listed = list_dir(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, "Some Look");
        // A file that is not a LUT at all is listed with its error,
        // not dropped silently.
        std::fs::write(dir.join("broken.cube"), "not a lut\n").unwrap();
        let listed = list_dir(&dir);
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|(n, _, i)| n == "broken" && i.is_err()));
        // And something that is neither is not listed at all.
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        assert_eq!(list_dir(&dir).len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A header read looks at the first chunk of the file, not all of
    /// it: a directory of film stocks is a listing, not a gigabyte.
    /// A header that somehow runs past the chunk is read whole rather
    /// than reported missing.
    #[test]
    fn a_header_read_looks_at_the_first_chunk_only() {
        let dir = std::env::temp_dir().join(format!("greycard-lut-head-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        // A 33-node table is 1.2 MB; its header is four lines.
        let path = dir.join("Big.cube");
        std::fs::write(&path, cube(33, "TITLE \"Big\"\n", |c| c)).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > HEADER_BYTES as u64);
        let (head, cut) = read_head(&path).unwrap();
        assert!(cut, "the read stopped short");
        assert!(head.len() <= HEADER_BYTES);
        assert!(head.ends_with('\n'), "a half line is dropped");
        let info = Lut3d::info(&path).unwrap();
        assert_eq!(info.title.as_deref(), Some("Big"));
        assert_eq!(info.size, 33);

        // A header buried under more comments than the chunk holds is
        // still found, by reading the file whole.
        let padded = dir.join("Padded.cube");
        let mut text = String::new();
        while text.len() < HEADER_BYTES + 1024 {
            text.push_str("# a long preamble of no consequence whatever\n");
        }
        text.push_str(&cube(2, "TITLE \"Padded\"\n", |c| c));
        std::fs::write(&padded, &text).unwrap();
        let info = Lut3d::info(&padded).expect("it is found the slow way");
        assert_eq!(info.title.as_deref(), Some("Padded"));
        assert_eq!(info.size, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_table_loads_from_a_path_by_its_extension() {
        let dir = std::env::temp_dir().join(format!("greycard-lut-load-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Swap.cube");
        std::fs::write(&path, cube(3, "", |c| [c[2], c[0], c[1]])).unwrap();
        let lut = Lut3d::load(&path).unwrap();
        assert!(close(lut.sample([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0], 1e-5));
        let other = dir.join("Swap.icc");
        std::fs::write(&other, b"x").unwrap();
        let e = Lut3d::load(&other).expect_err("not a LUT");
        assert!(e.to_string().contains("reads .cube and HaldCLUT"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
