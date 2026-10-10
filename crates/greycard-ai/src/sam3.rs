//! People: SAM 3 (Meta, the SAM License) asked for a person or a part of one
//! by name, a phrase from a fixed table, and on a picture with more
//! than one person, the person.
//!
//! Three graphs, run as the trial in `tools/ai/sam3_trial` runs them
//! (`sam3onnx.py`): the image encoder once a picture (the preview) or
//! a crop of it, the decoder once a phrase on an encoding, and in
//! place of the language encoder a table of each phrase's two decoder
//! inputs, computed once (`table.py`) and shipped as a file of the
//! model. The decoder returns the sixteen best queries whatever their
//! score; the cut is ours ([`CUT`]).
//!
//! A part is asked one of three ways ([`Route`]): on the whole
//! preview; or for the iris, the teeth and the lips, by cascade, each
//! step a crop of the full-resolution picture encoded at the model's
//! square: faces on the preview, the eyes (or the mouth) in a crop of
//! each face, the phrase in a crop of each eye (or the mouth). Each
//! crop's mask is refined at the crop's own resolution and handed back
//! with its place ([`Piece`]), for [`draw`] to put into the raster.
//!
//! A person is a face paired with the `person` instance that holds it,
//! and is known by a [`Signature`] of colors (clothes, hair and skin,
//! in three bands down the body) and the face's size, never by the face
//! itself. Colors cannot tell people apart in a group, so [`choose`]
//! decides alone only on the picture the shape was made on and on a
//! picture of one person; elsewhere it asks, with a guess.
//!
//! The picture is squashed to the square with Pillow's bilinear filter
//! ([`resample`]), so the encoder sees the bytes the trial's Python fed
//! it. Ported from Pillow's `src/libImaging/Resample.c`
//! (`precompute_coeffs`, `normalize_coeffs_8bpc`,
//! `ImagingResampleHorizontal_8bpc` and `ImagingResampleVertical_8bpc`,
//! as of Pillow 12.3): the filter, its fixed point and its two 8-bit
//! passes are theirs. Pillow's notice and license (MIT-CMU):
//!
//! > The Python Imaging Library (PIL) is
//! >
//! > Copyright © 1997-2011 by Secret Labs AB
//! >
//! > Copyright © 1995-2011 by Fredrik Lundh and contributors
//! >
//! > Pillow is the friendly PIL fork. It is
//! >
//! > Copyright © 2010 by Jeffrey 'Alex' Clark and contributors
//! >
//! > Like PIL, Pillow is licensed under the open source MIT-CMU License:
//! >
//! > By obtaining, using, and/or copying this software and/or its
//! > associated documentation, you agree that you have read, understood,
//! > and will comply with the following terms and conditions:
//! >
//! > Permission to use, copy, modify and distribute this software and
//! > its documentation for any purpose and without fee is hereby
//! > granted, provided that the above copyright notice appears in all
//! > copies, and that both that copyright notice and this permission
//! > notice appear in supporting documentation, and that the name of
//! > Secret Labs AB or the author not be used in advertising or
//! > publicity pertaining to distribution of the software without
//! > specific, written prior permission.
//! >
//! > SECRET LABS AB AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD
//! > TO THIS SOFTWARE, INCLUDING ALL IMPLIED WARRANTIES OF
//! > MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL SECRET LABS AB OR THE
//! > AUTHOR BE LIABLE FOR ANY SPECIAL, INDIRECT OR CONSEQUENTIAL DAMAGES
//! > OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR
//! > PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
//! > TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
//! > PERFORMANCE OF THIS SOFTWARE.

use std::collections::VecDeque;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor};
use serde::{Deserialize, Serialize};

use crate::image::{Mask, Rgb8};
use crate::refine::refine;
use crate::registry::SAM3;
use crate::runtime::{self, Loaded, Provider};
use crate::store::Store;

/// The model's square.
pub const SIDE: usize = 1008;
/// The side of each query's mask.
pub const MASK: usize = 288;
/// The queries the decoder returns, best first.
pub const QUERIES: usize = 16;
/// A phrase's tokens.
pub const CONTEXT: usize = 32;
/// The width of a phrase's features.
pub const WIDTH: usize = 256;
/// SAM 3's own cut: an instance is found when its score is over this.
pub const CUT: f32 = 0.5;

/// The decoder's four maps of the encoder's six, and their shapes.
const MAPS: [(&str, [usize; 4]); 4] = [
    ("vision_pos_enc_2", [1, 256, 72, 72]),
    ("backbone_fpn_0", [1, 256, 288, 288]),
    ("backbone_fpn_1", [1, 256, 144, 144]),
    ("backbone_fpn_2", [1, 256, 72, 72]),
];

/// The phrases the routes and the people ask for themselves.
pub const FACE: &str = "face";
pub const EYES: &str = "eyes";
pub const MOUTH: &str = "mouth";
pub const PERSON: &str = "person";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Model(#[from] runtime::Error),
    #[error("reading the phrase table: {0}")]
    Io(#[from] std::io::Error),
    #[error("the phrase table is malformed: {0}")]
    Table(String),
    #[error("\"{0}\" is not a preset")]
    NotPreset(String),
}

impl From<ort::Error> for Error {
    fn from(e: ort::Error) -> Self {
        Error::Model(e.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// One phrase's decoder inputs, as the language encoder gave them.
#[derive(Debug, Clone, PartialEq)]
pub struct Phrase {
    pub text: String,
    /// `language_mask`, 1 × `CONTEXT`: true for a padding token.
    pub mask: Vec<bool>,
    /// `language_features`, `CONTEXT` × 1 × `WIDTH`.
    pub features: Vec<f32>,
}

/// The phrase table: the only phrases the decoder can be asked.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Presets {
    phrases: Vec<Phrase>,
}

impl Presets {
    /// The table as `table.py` writes it: little-endian, a u32 count,
    /// then per phrase a u16 byte length and the UTF-8 phrase,
    /// `CONTEXT` mask bytes (0 or 1) and `CONTEXT` × `WIDTH` f32.
    pub fn read(path: &std::path::Path) -> Result<Self> {
        Self::parse(&std::fs::read(path)?)
    }

    /// The table in the model's store.
    pub fn of(store: &Store) -> Result<Self> {
        Self::read(&store.path(&SAM3, &SAM3.files[2]))
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let short = || Error::Table(format!("{} bytes end mid-phrase", bytes.len()));
        let mut at = 0usize;
        let mut take = |n: usize| -> Result<&[u8]> {
            let s = bytes.get(at..at + n).ok_or_else(short)?;
            at += n;
            Ok(s)
        };
        let count = u32::from_le_bytes(take(4)?.try_into().expect("four bytes")) as usize;
        let mut phrases = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            let n = u16::from_le_bytes(take(2)?.try_into().expect("two bytes")) as usize;
            let text = std::str::from_utf8(take(n)?)
                .map_err(|e| Error::Table(e.to_string()))?
                .to_string();
            let mask = take(CONTEXT)?;
            if mask.iter().any(|&b| b > 1) {
                return Err(Error::Table(format!("{text}: a mask byte is not 0 or 1")));
            }
            let mask = mask.iter().map(|&b| b == 1).collect();
            let features: Vec<f32> = take(CONTEXT * WIDTH * 4)?
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&c| f32::from_le_bytes(c))
                .collect();
            if features.iter().any(|v| !v.is_finite()) {
                return Err(Error::Table(format!("{text}: a feature is not finite")));
            }
            phrases.push(Phrase {
                text,
                mask,
                features,
            });
        }
        if at != bytes.len() {
            return Err(Error::Table(format!(
                "{} bytes after the last phrase",
                bytes.len() - at
            )));
        }
        Ok(Self { phrases })
    }

    /// The table as [`Presets::parse`] reads it.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = (self.phrases.len() as u32).to_le_bytes().to_vec();
        for p in &self.phrases {
            out.extend((p.text.len() as u16).to_le_bytes());
            out.extend(p.text.as_bytes());
            out.extend(p.mask.iter().map(|&m| m as u8));
            out.extend(p.features.iter().flat_map(|v| v.to_le_bytes()));
        }
        out
    }

    pub fn new(phrases: Vec<Phrase>) -> Self {
        Self { phrases }
    }

    pub fn phrase(&self, text: &str) -> Option<&Phrase> {
        self.phrases.iter().find(|p| p.text == text)
    }

    /// The phrase, or the error the UI shows: a phrase not in the
    /// table is never sent to a text encoder.
    pub fn get(&self, text: &str) -> Result<&Phrase> {
        self.phrase(text)
            .ok_or_else(|| Error::NotPreset(text.to_string()))
    }

    pub fn texts(&self) -> impl Iterator<Item = &str> {
        self.phrases.iter().map(|p| p.text.as_str())
    }
}

/// A picture encoded: the four maps the decoder takes, ready to feed.
/// About 117 MB.
pub struct Encoding {
    maps: Vec<(&'static str, DynValue)>,
}

/// One query: SAM 3's score for it (its own times the presence), its
/// box in fractions of the picture encoded (x0, y0, x1, y1), and its
/// mask over that picture, `MASK` square, a sigmoid.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub score: f32,
    pub bbox: [f32; 4],
    pub mask: Mask,
}

/// What a phrase found: the presence score and every query, best
/// first, over the cut or not.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub presence: f32,
    pub instances: Vec<Instance>,
}

impl Found {
    /// The instances over `cut`, best first.
    pub fn over(&self, cut: f32) -> impl Iterator<Item = &Instance> {
        self.instances.iter().filter(move |i| i.score > cut)
    }
}

/// What the routes need of a model: [`Sam3`], or a stand-in in a test.
pub trait Segment {
    type Encoding;
    fn encode(&mut self, image: &Rgb8) -> Result<Self::Encoding>;
    fn decode(&mut self, encoding: &Self::Encoding, phrase: &Phrase) -> Result<Found>;
}

/// The image encoder and the decoder.
pub struct Sam3 {
    encoder: Loaded,
    decoder: Loaded,
}

impl Sam3 {
    /// Both graphs from the store, each on the first provider that
    /// runs it, at `Level3` as the trial's bench ran them.
    pub fn load(store: &Store, providers: &[Provider]) -> Result<Self> {
        if !store.have(&SAM3) {
            return Err(runtime::Error::Missing(SAM3.name).into());
        }
        let level = GraphOptimizationLevel::Level3;
        let remembered = |file: usize| runtime::Remembered {
            store_root: store.root(),
            model: SAM3.id,
            hash: SAM3.files[file].sha256,
        };
        let encoder = runtime::open(
            &store.path(&SAM3, &SAM3.files[0]),
            level,
            providers,
            Some(remembered(0)),
            |s| {
                let input = Tensor::from_array(([3usize, SIDE, SIDE], vec![0u8; 3 * SIDE * SIDE]))?;
                s.run(ort::inputs!["image" => input])?;
                Ok(())
            },
        )?;
        let decoder = runtime::open(
            &store.path(&SAM3, &SAM3.files[1]),
            level,
            providers,
            Some(remembered(1)),
            |s| {
                let maps = MAPS
                    .iter()
                    .map(|&(name, dims)| {
                        let zeros = vec![0.0f32; dims.iter().product()];
                        Ok((name, Tensor::from_array((dims, zeros))?.into_dyn()))
                    })
                    .collect::<ort::Result<Vec<_>>>()?;
                let phrase = Phrase {
                    text: String::new(),
                    mask: vec![false; CONTEXT],
                    features: vec![0.0; CONTEXT * WIDTH],
                };
                s.run(decoder_inputs(&Encoding { maps }, &phrase)?)?;
                Ok(())
            },
        )?;
        Ok(Self { encoder, decoder })
    }

    /// The providers the encoder and the decoder settled on.
    pub fn providers(&self) -> (Provider, Provider) {
        (self.encoder.provider, self.decoder.provider)
    }

    /// The picture stretched to `SIDE` square (Pillow's bilinear,
    /// [`squash`]) and encoded; the decoder's four maps kept.
    pub fn encode(&mut self, image: &Rgb8) -> Result<Encoding> {
        let input = Tensor::from_array(([3usize, SIDE, SIDE], squash(image)))?;
        let out = self.encoder.session.run(ort::inputs!["image" => input])?;
        let mut maps = Vec::with_capacity(MAPS.len());
        for (name, dims) in MAPS {
            let (shape, data) = out[name].try_extract_tensor::<f32>()?;
            if data.len() != dims.iter().product::<usize>() {
                return Err(runtime::Error::Shape(format!("{name}: {shape}")).into());
            }
            // A card out of memory answers with nothing rather than an
            // error (the denoiser's lesson).
            if data.iter().any(|v| !v.is_finite()) {
                return Err(runtime::Error::Implausible(format!("{name} is not finite")).into());
            }
            maps.push((name, Tensor::from_array((dims, data.to_vec()))?.into_dyn()));
        }
        Ok(Encoding { maps })
    }

    /// `phrase` on an encoded picture: every query, best first.
    pub fn decode(&mut self, encoding: &Encoding, phrase: &Phrase) -> Result<Found> {
        run_decoder(&mut self.decoder.session, encoding, phrase)
    }
}

impl Segment for Sam3 {
    type Encoding = Encoding;
    fn encode(&mut self, image: &Rgb8) -> Result<Encoding> {
        Sam3::encode(self, image)
    }
    fn decode(&mut self, encoding: &Encoding, phrase: &Phrase) -> Result<Found> {
        Sam3::decode(self, encoding, phrase)
    }
}

fn decoder_inputs<'a>(
    encoding: &'a Encoding,
    phrase: &Phrase,
) -> ort::Result<Vec<(&'static str, SessionInputValue<'a>)>> {
    let mut inputs: Vec<(&str, SessionInputValue)> =
        encoding.maps.iter().map(|(n, v)| (*n, v.into())).collect();
    // The text alone: a box prompt, masked off.
    let owned: [(&str, DynValue); 5] = [
        (
            "language_mask",
            Tensor::from_array(([1usize, CONTEXT], phrase.mask.clone()))?.into_dyn(),
        ),
        (
            "language_features",
            Tensor::from_array(([CONTEXT, 1, WIDTH], phrase.features.clone()))?.into_dyn(),
        ),
        (
            "box_coords",
            Tensor::from_array(([1usize, 1, 4], vec![0f32; 4]))?.into_dyn(),
        ),
        (
            "box_labels",
            Tensor::from_array(([1usize, 1], vec![1i64]))?.into_dyn(),
        ),
        (
            "box_masks",
            Tensor::from_array(([1usize, 1], vec![true]))?.into_dyn(),
        ),
    ];
    inputs.extend(owned.into_iter().map(|(n, v)| (n, v.into())));
    Ok(inputs)
}

fn run_decoder(session: &mut Session, encoding: &Encoding, phrase: &Phrase) -> Result<Found> {
    if phrase.mask.len() != CONTEXT || phrase.features.len() != CONTEXT * WIDTH {
        return Err(Error::Table(format!("{}: the wrong size", phrase.text)));
    }
    let out = session.run(decoder_inputs(encoding, phrase)?)?;
    let (_, presence) = out["presence"].try_extract_tensor::<f32>()?;
    let (_, scores) = out["scores"].try_extract_tensor::<f32>()?;
    let (_, boxes) = out["boxes"].try_extract_tensor::<f32>()?;
    let (shape, masks) = out["masks"].try_extract_tensor::<f32>()?;
    let n = scores.len();
    if presence.len() != 1 || boxes.len() != 4 * n || masks.len() != n * MASK * MASK {
        return Err(runtime::Error::Shape(format!("masks {shape}, {n} scores")).into());
    }
    if !presence[0].is_finite()
        || scores.iter().any(|s| !s.is_finite())
        || boxes.iter().any(|b| !b.is_finite())
    {
        return Err(runtime::Error::Implausible("the scores are not finite".into()).into());
    }
    let side = MASK * MASK;
    let instances = (0..n)
        .map(|q| Instance {
            score: scores[q],
            bbox: std::array::from_fn(|k| boxes[4 * q + k]),
            mask: Mask::new(MASK, MASK, masks[q * side..(q + 1) * side].to_vec()),
        })
        .collect();
    Ok(Found {
        presence: presence[0],
        instances,
    })
}

// --- Pillow's bilinear resize -------------------------------------------

/// Pillow's fixed point for 8-bit resampling.
const PRECISION_BITS: u32 = 32 - 8 - 2;

/// Each output pixel's first input pixel and its weights, in Pillow's
/// fixed point.
struct Coefficients {
    bounds: Vec<(usize, usize)>,
    weights: Vec<i32>,
    ksize: usize,
}

/// Pillow's `precompute_coeffs` and `normalize_coeffs_8bpc` (see the
/// module's header): the triangle filter, widened by the scale when
/// shrinking so every input pixel counts, normalized, then rounded to
/// 22-bit fixed point.
fn coefficients(input: usize, output: usize) -> Coefficients {
    let scale = input as f64 / output as f64;
    let filterscale = scale.max(1.0);
    let support = filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut bounds = Vec::with_capacity(output);
    let mut weights = vec![0i32; output * ksize];
    let mut k = vec![0f64; ksize];
    for xx in 0..output {
        let center = (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        // C's (int) truncates toward zero.
        let xmin = ((center - support + 0.5) as i64).max(0) as usize;
        let xmax = ((center + support + 0.5) as i64).min(input as i64) as usize - xmin;
        let mut ww = 0.0;
        for (x, w) in k.iter_mut().enumerate().take(xmax) {
            let t = ((x + xmin) as f64 - center + 0.5) * ss;
            *w = (1.0 - t.abs()).max(0.0);
            ww += *w;
        }
        for x in 0..ksize {
            let w = if x < xmax && ww != 0.0 {
                k[x] / ww
            } else {
                0.0
            };
            let fixed = w * (1u32 << PRECISION_BITS) as f64;
            weights[xx * ksize + x] = if w < 0.0 {
                (fixed - 0.5) as i32
            } else {
                (fixed + 0.5) as i32
            };
        }
        bounds.push((xmin, xmax));
    }
    Coefficients {
        bounds,
        weights,
        ksize,
    }
}

fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// `image` resized to `width`×`height` exactly as Pillow's
/// `Image.resize(..., Image.BILINEAR)` does it (its
/// `ImagingResampleHorizontal_8bpc` and `ImagingResampleVertical_8bpc`,
/// see the module's header): a horizontal pass to 8 bits, then a
/// vertical one, each only where the size changes.
pub fn resample(image: &Rgb8, width: usize, height: usize) -> Rgb8 {
    let mut cur = image.clone();
    if width != cur.width {
        let c = coefficients(cur.width, width);
        let mut data = vec![0u8; width * cur.height * 3];
        for y in 0..cur.height {
            let row = &cur.data[y * cur.width * 3..(y + 1) * cur.width * 3];
            for xx in 0..width {
                let (xmin, n) = c.bounds[xx];
                let k = &c.weights[xx * c.ksize..xx * c.ksize + n];
                let mut ss = [1i32 << (PRECISION_BITS - 1); 3];
                for (x, &w) in k.iter().enumerate() {
                    let p = &row[(xmin + x) * 3..(xmin + x) * 3 + 3];
                    for (s, &v) in ss.iter_mut().zip(p) {
                        *s += v as i32 * w;
                    }
                }
                let o = (y * width + xx) * 3;
                for ch in 0..3 {
                    data[o + ch] = clip8(ss[ch]);
                }
            }
        }
        cur = Rgb8::new(width, cur.height, data);
    }
    if height != cur.height {
        let c = coefficients(cur.height, height);
        let w = cur.width;
        let mut data = vec![0u8; w * height * 3];
        for yy in 0..height {
            let (ymin, n) = c.bounds[yy];
            let k = &c.weights[yy * c.ksize..yy * c.ksize + n];
            for x in 0..w {
                let mut ss = [1i32 << (PRECISION_BITS - 1); 3];
                for (y, &wt) in k.iter().enumerate() {
                    let i = ((ymin + y) * w + x) * 3;
                    for (s, &v) in ss.iter_mut().zip(&cur.data[i..i + 3]) {
                        *s += v as i32 * wt;
                    }
                }
                let o = (yy * w + x) * 3;
                for ch in 0..3 {
                    data[o + ch] = clip8(ss[ch]);
                }
            }
        }
        cur = Rgb8::new(w, height, data);
    }
    cur
}

/// The encoder's input: the picture stretched to `SIDE` square by
/// [`resample`], as uint8 planes.
pub fn squash(image: &Rgb8) -> Vec<u8> {
    let sq = resample(image, SIDE, SIDE);
    let n = SIDE * SIDE;
    let mut planes = vec![0u8; 3 * n];
    for (i, p) in sq.data.as_chunks::<3>().0.iter().enumerate() {
        for c in 0..3 {
            planes[c * n + i] = p[c];
        }
    }
    planes
}

// --- The pure stages -----------------------------------------------------

/// A rectangle of the picture's pixels: `x0..x1` by `y0..y1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Rect {
    pub fn width(&self) -> usize {
        self.x1 - self.x0
    }
    pub fn height(&self) -> usize {
        self.y1 - self.y0
    }
    /// A box in fractions of this rectangle, in the picture's pixels.
    fn place(&self, b: [f32; 4]) -> [f32; 4] {
        let (w, h) = (self.width() as f32, self.height() as f32);
        [
            self.x0 as f32 + b[0] * w,
            self.y0 as f32 + b[1] * h,
            self.x0 as f32 + b[2] * w,
            self.y0 as f32 + b[3] * h,
        ]
    }
}

/// The trial's crop rule (`iris.py`): a square about `bbox` (in the
/// picture's pixels, x0 y0 x1 y1), `scale` times its longer side and
/// at least `min_side`, no larger than the picture, moved inside it.
/// Rounds as Python does, half to even.
pub fn square(bbox: [f32; 4], scale: f32, size: (usize, usize), min_side: f32) -> Rect {
    let [x0, y0, x1, y1] = bbox.map(f64::from);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let side = (min_side as f64)
        .max(scale as f64 * (x1 - x0).max(y1 - y0))
        .min(size.0 as f64)
        .min(size.1 as f64);
    let left = (cx - side / 2.0)
        .max(0.0)
        .min(size.0 as f64 - side)
        .round_ties_even() as usize;
    let top = (cy - side / 2.0)
        .max(0.0)
        .min(size.1 as f64 - side)
        .round_ties_even() as usize;
    let s = side as usize;
    Rect {
        x0: left,
        y0: top,
        x1: (left + s).min(size.0),
        y1: (top + s).min(size.1),
    }
}

/// The connected parts of `mask` over `cut` (four-connected, numbered
/// in the order a raster scan meets them, as `scipy.ndimage.label`
/// does), each boxed in the mask's cells, `[x0, y0, x1, y1)`; a part
/// under three cells across is dropped. "eyes" comes back as one
/// instance over both eyes; this splits it.
pub fn components(mask: &Mask, cut: f32) -> Vec<[usize; 4]> {
    let (w, h) = (mask.width, mask.height);
    let on: Vec<bool> = mask.data.iter().map(|&v| v > cut).collect();
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !on[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut b = [usize::MAX, usize::MAX, 0, 0];
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
            let mut visit = |j: usize| {
                if on[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        if b[2] - b[0] >= 3 {
            out.push(b);
        }
    }
    out
}

/// The instances over `cut` as one mask: the most of their sigmoids at
/// each cell, nothing where none is over the cut.
pub fn union(instances: &[Instance], cut: f32) -> Mask {
    let mut out = Mask::new(MASK, MASK, vec![0.0; MASK * MASK]);
    for i in instances.iter().filter(|i| i.score > cut) {
        let m = if i.mask.width == MASK && i.mask.height == MASK {
            std::borrow::Cow::Borrowed(&i.mask)
        } else {
            std::borrow::Cow::Owned(i.mask.resampled(MASK, MASK))
        };
        for (o, v) in out.data.iter_mut().zip(&m.data) {
            *o = o.max(*v);
        }
    }
    out
}

/// A binary dilation by a square of radius `r` cells.
fn dilate(on: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let mut rows = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            rows[y * w + x] = on[y * w + a..=y * w + b].iter().any(|&v| v);
        }
    }
    let mut out = vec![false; w * h];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            out[y * w + x] = (a..=b).any(|yy| rows[yy * w + x]);
        }
    }
    out
}

/// A mask over `rect` of the picture, at its own resolution: `rect`
/// is in the picture's full-resolution pixels; the mask may be at any
/// size (a crop's own pixels, or the preview's for the whole picture).
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub rect: Rect,
    pub mask: Mask,
}

/// `pieces` of a picture of `size` (full-resolution pixels) drawn into
/// a raster of `raster` (width, height), the most where they overlap,
/// nothing outside them. Each raster pixel is its piece averaged over
/// the whole pixel, counting the part outside the piece as nought, so a
/// pixel the piece's edge halves gets half: n × n bilinear samples
/// spread over the pixel (n up to eight a side where the piece is finer
/// than the raster), those outside the piece's rectangle adding
/// nothing, the sum over n².
pub fn draw(pieces: &[Piece], size: (usize, usize), raster: (usize, usize)) -> Vec<f32> {
    let (rw, rh) = raster;
    let mut out = vec![0.0f32; rw * rh];
    let (sx, sy) = (size.0 as f32 / rw as f32, size.1 as f32 / rh as f32);
    for p in pieces {
        let r = p.rect;
        if r.width() == 0 || r.height() == 0 {
            continue;
        }
        let (mx, my) = (
            p.mask.width as f32 / r.width() as f32,
            p.mask.height as f32 / r.height() as f32,
        );
        let n = ((sx * mx).max(sy * my).ceil() as usize).clamp(1, 8);
        let x0 = (r.x0 as f32 / sx).floor() as usize;
        let y0 = (r.y0 as f32 / sy).floor() as usize;
        let x1 = ((r.x1 as f32 / sx).ceil() as usize).min(rw);
        let y1 = ((r.y1 as f32 / sy).ceil() as usize).min(rh);
        for y in y0..y1 {
            for x in x0..x1 {
                let mut sum = 0.0;
                for j in 0..n {
                    let py = (y as f32 + (j as f32 + 0.5) / n as f32) * sy;
                    if py < r.y0 as f32 || py >= r.y1 as f32 {
                        continue;
                    }
                    for i in 0..n {
                        let px = (x as f32 + (i as f32 + 0.5) / n as f32) * sx;
                        if px < r.x0 as f32 || px >= r.x1 as f32 {
                            continue;
                        }
                        sum += sample(
                            &p.mask,
                            (px - r.x0 as f32) * mx - 0.5,
                            (py - r.y0 as f32) * my - 0.5,
                        );
                    }
                }
                let v = sum / (n * n) as f32;
                let o = &mut out[y * rw + x];
                *o = o.max(v);
            }
        }
    }
    out
}

/// `mask` at (fx, fy) in its pixel centers' coordinates, bilinearly,
/// held to its edge.
fn sample(mask: &Mask, fx: f32, fy: f32) -> f32 {
    let fx = fx.clamp(0.0, (mask.width - 1) as f32);
    let fy = fy.clamp(0.0, (mask.height - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(mask.width - 1), (y0 + 1).min(mask.height - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let top = mask.at(x0, y0) + (mask.at(x1, y0) - mask.at(x0, y0)) * tx;
    let bottom = mask.at(x0, y1) + (mask.at(x1, y1) - mask.at(x0, y1)) * tx;
    top + (bottom - top) * ty
}

// --- The routes ----------------------------------------------------------

/// How a part's phrase is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Route {
    /// The preview encoded once, the phrase decoded, the instances over
    /// the cut united.
    Whole,
    /// Faces on the preview; `eyes` in a 1.3 × crop of each face, split
    /// into its parts; the phrase in a 2.5 × crop of each eye.
    Eye,
    /// Faces on the preview; the best `mouth` in a 1.3 × crop of each
    /// face; the phrase in a 1.8 × crop of it.
    Mouth,
}

impl Route {
    pub fn name(self) -> &'static str {
        match self {
            Route::Whole => "whole",
            Route::Eye => "eye",
            Route::Mouth => "mouth",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Route::Whole, Route::Eye, Route::Mouth]
            .into_iter()
            .find(|r| r.name() == name)
    }
}

/// The crop rules, from the trial: (scale, least side in pixels).
const FACE_CROP: (f32, f32) = (1.3, 256.0);
const EYE_CROP: (f32, f32) = (2.5, 96.0);
const MOUTH_CROP: (f32, f32) = (1.8, 96.0);

/// A picture's encodings, kept so that a second phrase on the same
/// picture or person costs only decodes: the preview's, and the crops'
/// by their rectangle, the most recent `capacity` of them and at least
/// the one in use (each about 117 MB). All of them belong to one key,
/// the caller's name for the picture as rendered (the editor's base
/// develop stamp): asked under another key, they are dropped first, so
/// an encoding of one picture is never decoded for another.
pub struct Encodings<E = Encoding> {
    key: Option<u64>,
    preview: Option<E>,
    crops: VecDeque<(Rect, E)>,
    capacity: usize,
}

impl<E> Encodings<E> {
    pub fn new(capacity: usize) -> Self {
        Self {
            key: None,
            preview: None,
            crops: VecDeque::new(),
            capacity,
        }
    }

    pub fn clear(&mut self) {
        self.key = None;
        self.preview = None;
        self.crops.clear();
    }

    /// The key the kept encodings belong to.
    pub fn key(&self) -> Option<u64> {
        self.key
    }

    /// How many crops are kept.
    pub fn crops(&self) -> usize {
        self.crops.len()
    }

    pub fn has_preview(&self) -> bool {
        self.preview.is_some()
    }

    /// Drops everything kept under another key.
    fn keyed(&mut self, key: u64) {
        if self.key != Some(key) {
            self.clear();
            self.key = Some(key);
        }
    }

    fn preview_or(&mut self, key: u64, encode: impl FnOnce() -> Result<E>) -> Result<&E> {
        self.keyed(key);
        if self.preview.is_none() {
            self.preview = Some(encode()?);
        }
        Ok(self.preview.as_ref().expect("just set"))
    }

    fn crop_or(&mut self, key: u64, rect: Rect, encode: impl FnOnce() -> Result<E>) -> Result<&E> {
        self.keyed(key);
        if let Some(i) = self.crops.iter().position(|(r, _)| *r == rect) {
            let hit = self.crops.remove(i).expect("found");
            self.crops.push_back(hit);
        } else {
            // Room first, so the encodings held never pass the capacity.
            while self.crops.len() >= self.capacity.max(1) {
                self.crops.pop_front();
            }
            let e = encode()?;
            self.crops.push_back((rect, e));
        }
        Ok(&self.crops.back().expect("just pushed").1)
    }
}

/// What the routes see of the picture.
pub struct Picture<'a, E = Encoding> {
    /// The display rendering the masks are made on, at its preview
    /// size (2048 on the long side in the editor).
    pub preview: &'a Rgb8,
    /// The preview's luma ([`Rgb8::luma`]), the guide for the whole
    /// picture's mask.
    pub luma: &'a [f32],
    /// The picture's full resolution, the pixels `region` renders.
    pub size: (usize, usize),
    /// What names the picture as rendered, the preview and the regions
    /// alike (the editor's base develop stamp): the key `encodings`
    /// keeps its encodings under.
    pub key: u64,
    /// The same rendering of one rectangle of the picture, in its
    /// full-resolution pixels, at those pixels.
    pub region: &'a mut dyn FnMut(Rect) -> Rgb8,
    pub encodings: &'a mut Encodings<E>,
    /// Subject's matte of the same picture, over the whole of it at
    /// any size (the preview's, as a rule): the edge a Whole person
    /// takes inside their region grown a little ([`subject_edge`]).
    /// `None` keeps SAM's own edge.
    pub matte: Option<&'a Mask>,
}

impl<E> Picture<'_, E> {
    fn whole(&self) -> Rect {
        Rect {
            x0: 0,
            y0: 0,
            x1: self.size.0,
            y1: self.size.1,
        }
    }
}

fn preview_found<S: Segment>(
    sam: &mut S,
    picture: &mut Picture<S::Encoding>,
    phrase: &Phrase,
) -> Result<Found> {
    let preview = picture.preview;
    let enc = picture
        .encodings
        .preview_or(picture.key, || sam.encode(preview))?;
    sam.decode(enc, phrase)
}

fn crop_found<S: Segment>(
    sam: &mut S,
    picture: &mut Picture<S::Encoding>,
    rect: Rect,
    phrase: &Phrase,
) -> Result<Found> {
    let region = &mut *picture.region;
    let enc = picture
        .encodings
        .crop_or(picture.key, rect, || sam.encode(&region(rect)))?;
    sam.decode(enc, phrase)
}

/// The faces over the cut on the preview, best first, each face once
/// ([`distinct`]).
pub fn faces<S: Segment>(
    sam: &mut S,
    presets: &Presets,
    picture: &mut Picture<S::Encoding>,
) -> Result<Vec<Instance>> {
    let found = preview_found(sam, picture, presets.get(FACE)?)?;
    Ok(distinct(found.over(CUT).cloned().collect()))
}

/// Two face boxes overlapping by more than this (intersection over
/// union) are one face found twice.
const SAME_FACE: f32 = 0.5;

/// `instances`, best first, without any that overlaps a better one
/// kept by more than `SAME_FACE`: a face found twice would be two
/// people, and cost a picture of one person its decision.
pub fn distinct(mut instances: Vec<Instance>) -> Vec<Instance> {
    instances.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Instance> = Vec::with_capacity(instances.len());
    for i in instances {
        if kept.iter().all(|k| iou(k.bbox, i.bbox) <= SAME_FACE) {
            kept.push(i);
        }
    }
    kept
}

/// Two boxes' intersection over their union.
fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = w * h;
    let area = |r: [f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let union = area(a) + area(b) - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

/// `phrase` asked by `route` of `who` (a person [`people`] found on
/// this picture, or everyone): the masks, each over its place.
pub fn find<S: Segment>(
    sam: &mut S,
    presets: &Presets,
    picture: &mut Picture<S::Encoding>,
    route: Route,
    phrase: &str,
    who: Option<&Person>,
) -> Result<Vec<Piece>> {
    let phrase = presets.get(phrase)?;
    if route == Route::Whole {
        return whole(sam, presets, picture, phrase, who).map(|p| vec![p]);
    }
    let boxes: Vec<[f32; 4]> = match who {
        Some(p) => vec![p.face],
        None => faces(sam, presets, picture)?
            .iter()
            .map(|f| f.bbox)
            .collect(),
    };
    let size = picture.size;
    let all = picture.whole();
    let mut pieces = Vec::new();
    for face in boxes {
        let fcrop = square(all.place(face), FACE_CROP.0, size, FACE_CROP.1);
        let parts: Vec<([f32; 4], (f32, f32))> = if route == Route::Eye {
            let eyes = crop_found(sam, picture, fcrop, presets.get(EYES)?)?;
            let merged = union(&eyes.instances, CUT);
            // A face has two eyes: the two largest parts, left to right.
            let mut parts = components(&merged, CUT);
            parts.sort_by_key(|b| std::cmp::Reverse((b[2] - b[0]) * (b[3] - b[1])));
            parts.truncate(2);
            parts.sort_by_key(|b| b[0]);
            parts
                .into_iter()
                .map(|b| {
                    let cells = b.map(|v| v as f32 / MASK as f32);
                    (fcrop.place(cells), EYE_CROP)
                })
                .collect()
        } else {
            let mouths = crop_found(sam, picture, fcrop, presets.get(MOUTH)?)?;
            mouths
                .over(CUT)
                .next()
                .map(|m| (fcrop.place(m.bbox), MOUTH_CROP))
                .into_iter()
                .collect()
        };
        for (bbox, (scale, least)) in parts {
            let rect = square(bbox, scale, size, least);
            if let Some(piece) = crop_piece(sam, picture, rect, phrase)? {
                pieces.push(piece);
            }
        }
    }
    Ok(pieces)
}

/// The phrase on one crop, refined at the crop's resolution against
/// its own luma: radius a 128th of its side (at least 2), ε 1e-3,
/// which keeps the catchlight in an iris (1e-4 carves it out). `None`
/// where nothing is over the cut.
fn crop_piece<S: Segment>(
    sam: &mut S,
    picture: &mut Picture<S::Encoding>,
    rect: Rect,
    phrase: &Phrase,
) -> Result<Option<Piece>> {
    // The crop's own pixels are wanted twice, for the encoding and as
    // the refine's guide: rendered once when the encoding is made, and
    // with the encoding kept, only once something is found.
    let mut rendered = None;
    let region = &mut *picture.region;
    let enc = picture.encodings.crop_or(picture.key, rect, || {
        let crop = region(rect);
        let e = sam.encode(&crop);
        rendered = Some(crop);
        e
    })?;
    let found = sam.decode(enc, phrase)?;
    if found.over(CUT).next().is_none() {
        return Ok(None);
    }
    let crop = match rendered {
        Some(c) => c,
        None => (picture.region)(rect),
    };
    let mask = union(&found.instances, CUT);
    let radius = (crop.width.max(crop.height) / 128).max(2);
    let refined = refine(&mask, &crop.luma(), crop.width, crop.height, radius, 1e-3);
    Ok(Some(Piece {
        rect,
        mask: refined,
    }))
}

/// The whole route: the instances over the cut united, refined at the
/// preview as the Subject and Object masks are (radius a 256th of its
/// width, ε 1e-3). With a who, kept to them: to their own mask (grown
/// by three cells) where they have one of their own; for a face no
/// body went with, or whose body holds another face too (SAM merged
/// two people into it), whole instances rather than cut pixels
/// ([`near_face`]). The `person` phrase with a who is that person
/// whole ([`person_mask`]).
fn whole<S: Segment>(
    sam: &mut S,
    presets: &Presets,
    picture: &mut Picture<S::Encoding>,
    phrase: &Phrase,
    who: Option<&Person>,
) -> Result<Piece> {
    let found = preview_found(sam, picture, phrase)?;
    if phrase.text == PERSON {
        let faces = match who {
            Some(_) => faces(sam, presets, picture)?,
            None => Vec::new(),
        };
        // A head with no body keeps SAM's edge: Subject's matte about
        // it would take a collar of neck and shoulders the person is
        // not, cut off three cells under the chin.
        let (mask, bodied) = match who {
            Some(person) => person_mask(&found, &faces, person),
            None => (union(&found.instances, CUT), true),
        };
        let matte = picture.matte.filter(|_| bodied);
        let (filled, open) = fill_unsure_open(&mask.resampled(MASK, MASK));
        let solid = harden(&filled);
        let p = picture.preview;
        let radius = (p.width / 256).max(2);
        let sams = refine(&solid, picture.luma, p.width, p.height, radius, 1e-3);
        let mask = match matte {
            Some(matte) => {
                // Kept to SAM's edge: where someone else is, and the
                // gaps the fill left open.
                let mut keep = match who {
                    Some(person) => others(&found, &faces, person, &solid),
                    None => vec![false; MASK * MASK],
                };
                for (k, o) in keep.iter_mut().zip(open) {
                    *k |= o;
                }
                subject_edge(&solid, &keep, &sams, matte)
            }
            None => sams,
        };
        return Ok(Piece {
            rect: picture.whole(),
            mask,
        });
    }
    // A body over more than one face is a merged one: kept to it, one
    // person's Top would take the other's.
    let merged = match who {
        Some(Person {
            body: Some(body), ..
        }) => {
            let faces = faces(sam, presets, picture)?;
            holds(body, faces.iter().map(|f| f.bbox)) > 1
        }
        _ => false,
    };
    let mask = match who {
        None => union(&found.instances, CUT),
        Some(Person {
            body: Some(body),
            face,
            ..
        }) if !merged => {
            // Whole instances first, each to the body that holds most
            // of it: where two people overlap (one seated in front of
            // the other), the pixels of a neighbor's top inside this
            // body's outline are the neighbor's, not this person's. The
            // neighbors are the bodies other faces took, less any that
            // is this body found twice: an unpaired near-copy of her
            // own would otherwise take her own top from her.
            let own = body.resampled(MASK, MASK);
            let others: Vec<Mask> = people(sam, presets, picture)?
                .into_iter()
                .filter(|p| p.face != *face)
                .filter_map(|p| p.body)
                .map(|b| b.resampled(MASK, MASK))
                .filter(|b| mask_iou(b, &own) <= SAME_FACE)
                .collect();
            let kept: Vec<Instance> = found
                .over(CUT)
                .filter(|i| {
                    let m = i.mask.resampled(MASK, MASK);
                    let mine = overlap(&m, &own);
                    mine > 0 && others.iter().all(|o| overlap(&m, o) <= mine)
                })
                .cloned()
                .collect();
            let mut mask = union(&kept, CUT);
            let body = own;
            let on: Vec<bool> = body.data.iter().map(|&v| v > CUT).collect();
            let near = dilate(&on, MASK, MASK, 3);
            for (m, n) in mask.data.iter_mut().zip(near) {
                if !n {
                    *m = 0.0;
                }
            }
            mask
        }
        Some(person) => {
            let bodies = preview_found(sam, picture, presets.get(PERSON)?)?;
            let others: Vec<&Mask> = bodies.over(CUT).map(|b| &b.mask).collect();
            let kept = near_face(&found.instances, person.face, &others);
            union(&kept, CUT)
        }
    };
    let p = picture.preview;
    let radius = (p.width / 256).max(2);
    Ok(Piece {
        rect: picture.whole(),
        mask: refine(&mask, picture.luma, p.width, p.height, radius, 1e-3),
    })
}

/// A whole person, of `found` (the `person` instances) and `faces`
/// (every face on the picture): their own body, the instance paired
/// with their face, where it holds no other face; else the instance
/// over the cut that holds their face and no other; else, where SAM
/// merged them with someone into one instance, their share of it
/// ([`split`]); and for a face no instance holds, their face.
fn person_mask(found: &Found, faces: &[Instance], who: &Person) -> (Mask, bool) {
    let boxes = || faces.iter().map(|f| f.bbox);
    let [fx, fy] = center(who.face);
    let at = |m: &Mask| sample(m, fx * m.width as f32 - 0.5, fy * m.height as f32 - 0.5);
    if let Some(body) = &who.body
        && holds(body, boxes()) <= 1
    {
        return (body.clone(), true);
    }
    let holding: Vec<&Instance> = found.over(CUT).filter(|i| at(&i.mask) > CUT).collect();
    let alone = holding
        .iter()
        .filter(|i| holds(&i.mask, boxes()) <= 1)
        .max_by(|a, b| at(&a.mask).total_cmp(&at(&b.mask)));
    if let Some(alone) = alone {
        return (alone.mask.clone(), true);
    }
    let merged = who.body.as_ref().or_else(|| {
        holding
            .iter()
            .max_by(|a, b| at(&a.mask).total_cmp(&at(&b.mask)))
            .map(|i| &i.mask)
    });
    if let Some(merged) = merged {
        let merged = merged.resampled(MASK, MASK);
        let held: Vec<[f32; 4]> = boxes()
            .filter(|&f| holds(&merged, [f].into_iter()) > 0)
            .collect();
        return (split(&merged, &held, who.face), true);
    }
    // No instance holds the face: the face as the model found it.
    let face = faces
        .iter()
        .max_by(|a, b| iou(a.bbox, who.face).total_cmp(&iou(b.bbox, who.face)))
        .filter(|f| iou(f.bbox, who.face) > SAME_FACE)
        .map_or_else(|| box_mask(who.face), |f| f.mask.clone());
    (face, false)
}

/// A person's mask made solid: SAM 3's sigmoid over a body runs from
/// about 0.7 to 0.95 inside, lower over dark glasses or a shadowed
/// cheek, and the refine would carry that unevenness into the look. A
/// ramp about the cut, nought at 0.3 to one at 0.7, keeps the outline
/// where SAM put it and leaves the edge's softness to the refine.
fn harden(mask: &Mask) -> Mask {
    let mut out = mask.clone();
    for v in &mut out.data {
        *v = ((*v - CUT) / 0.4 + 0.5).clamp(0.0, 1.0);
    }
    out
}

/// A hole inside a mask, a region under the cut that the mask's cells
/// over it enclose, is unsure ground when no cell of it is at
/// `UNSURE` or under: dark glasses or a shadow in a face, which SAM 3
/// leaves at 0.13 to 0.45. A gap between an arm and the body reaches
/// background, 0.01 to 0.04 on the trial's frames, and stays.
const UNSURE: f32 = 0.1;

/// `mask` (`MASK` square) with each unsure hole and its rim filled.
#[cfg(test)]
fn fill_unsure(mask: &Mask) -> Mask {
    fill_unsure_open(mask).0
}

/// [`fill_unsure`], and the cells of the holes it left open: the
/// enclosed gaps that reach background, which Subject's edge must not
/// close ([`subject_edge`]).
fn fill_unsure_open(mask: &Mask) -> (Mask, Vec<bool>) {
    let (w, h) = (mask.width, mask.height);
    let mut seen = vec![false; w * h];
    let mut out = mask.clone();
    let mut open = vec![false; w * h];
    for start in 0..w * h {
        if seen[start] || mask.data[start] > CUT {
            continue;
        }
        // One region under the cut, 4-connected.
        let mut region = vec![start];
        seen[start] = true;
        let mut i = 0;
        let mut edge = false;
        while i < region.len() {
            let c = region[i];
            i += 1;
            let (x, y) = (c % w, c / w);
            edge |= x == 0 || y == 0 || x + 1 == w || y + 1 == h;
            let near = [
                (x > 0).then(|| c - 1),
                (x + 1 < w).then(|| c + 1),
                (y > 0).then(|| c - w),
                (y + 1 < h).then(|| c + w),
            ];
            for n in near.into_iter().flatten() {
                if !seen[n] && mask.data[n] <= CUT {
                    seen[n] = true;
                    region.push(n);
                }
            }
        }
        let least = region.iter().map(|&c| mask.data[c]).fold(1.0f32, f32::min);
        if !edge && least > UNSURE {
            // And its rim, two cells about it, which SAM leaves just
            // over the cut: else a ring of glasses' frame shows.
            for &c in &region {
                let (x, y) = (c % w, c / w);
                for ny in y.saturating_sub(2)..(y + 3).min(h) {
                    for nx in x.saturating_sub(2)..(x + 3).min(w) {
                        out.data[ny * w + nx] = 1.0;
                    }
                }
            }
        } else if !edge {
            for &c in &region {
                open[c] = true;
            }
        }
    }
    (out, open)
}

/// How far, in `MASK` cells, a Whole person's region is grown for
/// Subject's edge: Subject's matte is taken out to `GROW` cells past
/// SAM's outline and nothing beyond. SAM's cut runs within a cell or two
/// of the true edge at 288 cells (7 pixels a cell at 2048), and loose
/// hair against the sky that Subject keeps reaches a cell or two past
/// it; three takes that and stops before a neighbor's shoulder or a
/// held thing Subject also finds salient becomes this person's.
const GROW: f32 = 3.0;

/// How deep into the person's region, in cells, SAM's solid inside
/// comes up under Subject's matte as a floor, ramping from nothing at
/// SAM's outline: so a blotch Subject leaves in a body is filled, as
/// the fill fills SAM's, while at the edge Subject's matte alone says
/// where the person ends.
const FLOOR: f32 = 2.0;

/// The least share of a part of the person's inside Subject's matte
/// must hold (over the cut) for its edge to be taken there: a person
/// Subject does not see as salient, someone in the background, keeps
/// SAM's edge rather than losing theirs.
const COVER: f32 = 0.8;

/// The cells of `MASK` square instances that are someone else's, for a
/// whole person `who` whose region is `own`: every `person` instance
/// over the cut that does not hold `who`'s face, and the rest of one
/// that holds theirs and another's (a body SAM merged), and every
/// other face; less `own` itself. An instance that holds only their
/// face, or lies mostly inside `own`, is their own found again.
fn others(found: &Found, faces: &[Instance], who: &Person, own: &Mask) -> Vec<bool> {
    let mine: Vec<bool> = own.data.iter().map(|&v| v > CUT).collect();
    let boxes = || faces.iter().map(|f| f.bbox);
    let [fx, fy] = center(who.face);
    let mut out = vec![false; MASK * MASK];
    let mut add = |m: &Mask| {
        for (o, v) in out.iter_mut().zip(&m.data) {
            *o |= *v > CUT;
        }
    };
    for i in found.over(CUT) {
        let m = i.mask.resampled(MASK, MASK);
        let holds_theirs = sample(&m, fx * MASK as f32 - 0.5, fy * MASK as f32 - 0.5) > CUT;
        if holds_theirs && holds(&m, boxes()) <= 1 {
            continue;
        }
        let on = m.data.iter().filter(|&&v| v > CUT).count();
        let inside = m
            .data
            .iter()
            .zip(&mine)
            .filter(|&(&v, &o)| v > CUT && o)
            .count();
        if !holds_theirs && 2 * inside > on {
            continue;
        }
        // Filled as their own Whole person would be: glasses are theirs.
        add(&fill_unsure_open(&m).0);
    }
    for f in faces.iter().filter(|f| iou(f.bbox, who.face) <= SAME_FACE) {
        add(&f.mask.resampled(MASK, MASK));
    }
    for (o, m) in out.iter_mut().zip(&mine) {
        *o &= !m;
    }
    out
}

/// Each cell's distance, in cells, to the nearest cell `on` (nought on
/// one): a two-pass chamfer over the eight neighbors, steps of one and
/// √2. With no cell on, every distance is infinite.
fn cells_to(on: &[bool], w: usize, h: usize) -> Vec<f32> {
    const D: f32 = std::f32::consts::SQRT_2;
    let mut d: Vec<f32> = on
        .iter()
        .map(|&o| if o { 0.0 } else { f32::INFINITY })
        .collect();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mut v = d[i];
            if x > 0 {
                v = v.min(d[i - 1] + 1.0);
            }
            if y > 0 {
                v = v.min(d[i - w] + 1.0);
                if x > 0 {
                    v = v.min(d[i - w - 1] + D);
                }
                if x + 1 < w {
                    v = v.min(d[i - w + 1] + D);
                }
            }
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            let mut v = d[i];
            if x + 1 < w {
                v = v.min(d[i + 1] + 1.0);
            }
            if y + 1 < h {
                v = v.min(d[i + w] + 1.0);
                if x + 1 < w {
                    v = v.min(d[i + w + 1] + D);
                }
                if x > 0 {
                    v = v.min(d[i + w - 1] + D);
                }
            }
            d[i] = v;
        }
    }
    d
}

/// The four-connected parts of `on`, `w` wide: each cell's part, or
/// `None` off them, and how many there are.
fn label(on: &[bool], w: usize) -> (Vec<Option<usize>>, usize) {
    let h = on.len() / w;
    let mut out = vec![None; on.len()];
    let mut n = 0;
    let mut stack = Vec::new();
    for start in 0..on.len() {
        if !on[start] || out[start].is_some() {
            continue;
        }
        out[start] = Some(n);
        stack.push(start);
        while let Some(c) = stack.pop() {
            let (x, y) = (c % w, c / w);
            let near = [
                (x > 0).then(|| c - 1),
                (x + 1 < w).then(|| c + 1),
                (y > 0).then(|| c - w),
                (y + 1 < h).then(|| c + w),
            ];
            for j in near.into_iter().flatten() {
                if on[j] && out[j].is_none() {
                    out[j] = Some(n);
                    stack.push(j);
                }
            }
        }
        n += 1;
    }
    (out, n)
}

/// A whole person's mask with Subject's edge: `solid` is their region
/// as SAM made it and the fill and the ramp left it (`MASK` square),
/// `sams` that region refined at the preview's size (the edge SAM
/// alone gives), `matte` Subject's matte of the picture, and `keep`
/// the cells (`MASK` square) where SAM's edge stands whatever Subject
/// says: someone else's region ([`others`]) and the gaps the fill left
/// open. At the preview's size:
///
/// - out to `GROW` cells past SAM's outline, Subject's matte, fading
///   to nothing over the last cell; nothing beyond;
/// - inside, SAM's solid region under it as a floor, from nothing at
///   the outline to all of it `FLOOR` cells in;
/// - within a cell of `keep`, or as near it as to the person, SAM's
///   edge alone, blending into the above over the next `GROW - 1`
///   cells: where two people touch the line between them is the
///   person split's, a gap between them that Subject fills goes to
///   neither by its matte, and the fill's open gaps stay open.
///
/// A part of the region (four-connected) keeps SAM's edge, as if it
/// were `keep`, where Subject's matte holds less than `COVER` of its
/// inside, where it has no inside (no cell two in: a speck, a
/// sliver), or where most of the cells about it are kept (a speck SAM
/// gave this person inside someone else).
fn subject_edge(solid: &Mask, keep: &[bool], sams: &Mask, matte: &Mask) -> Mask {
    let (w, h) = (sams.width, sams.height);
    let n = MASK * MASK;
    let mut own: Vec<bool> = solid.data.iter().map(|&v| v > CUT).collect();
    let mut keep = keep.to_vec();
    // Subject's view of each cell, for the cover.
    let cells = matte.resampled(MASK, MASK);
    let inside = cells_to(&own.iter().map(|&o| !o).collect::<Vec<_>>(), MASK, MASK);
    let (parts, count) = label(&own, MASK);
    // Counted away from the outline, where SAM and Subject should both
    // be sure. A part with no cell two in, a speck or a sliver, has
    // too little inside to trust a matte grown three cells about it.
    let (mut held, mut deep) = (vec![0usize; count], vec![0usize; count]);
    for c in 0..n {
        if let Some(p) = parts[c]
            && inside[c] >= 2.0
        {
            deep[p] += 1;
            held[p] += (cells.data[c] > CUT) as usize;
        }
    }
    // The cells about each part, and how many of them are kept: a
    // speck SAM gave this person inside someone else is ringed by
    // them, and Subject, which holds them both, would make it solid.
    let (mut ring, mut ringed) = (vec![0usize; count], vec![0usize; count]);
    let mut stamp = vec![usize::MAX; n];
    for (c, part) in parts.iter().enumerate() {
        let Some(p) = *part else { continue };
        let (x, y) = ((c % MASK) as isize, (c / MASK) as isize);
        for (dx, dy) in [
            (-1, 0),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (1, -1),
            (-1, 1),
            (1, 1),
        ] {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= MASK as isize || ny >= MASK as isize {
                continue;
            }
            let j = ny as usize * MASK + nx as usize;
            if !own[j] && stamp[j] != p {
                stamp[j] = p;
                ring[p] += 1;
                ringed[p] += keep[j] as usize;
            }
        }
    }
    let trusted: Vec<bool> = (0..count)
        .map(|p| {
            deep[p] > 0 && held[p] as f32 >= COVER * deep[p] as f32 && 2 * ringed[p] <= ring[p]
        })
        .collect();
    for c in 0..n {
        if let Some(p) = parts[c]
            && !trusted[p]
        {
            own[c] = false;
            keep[c] = true;
        }
    }
    if !own.iter().any(|&o| o) {
        return sams.clone();
    }
    // The signed distance to SAM's outline, in cells: positive out,
    // negative in, nought half way between a cell in and one out.
    let out_d = cells_to(&own, MASK, MASK);
    let in_d = cells_to(&own.iter().map(|&o| !o).collect::<Vec<_>>(), MASK, MASK);
    let signed: Vec<f32> = (0..n)
        .map(|c| {
            if own[c] {
                0.5 - in_d[c]
            } else {
                out_d[c] - 0.5
            }
        })
        .collect();
    // Far from anything kept, as far as matters.
    let kept: Vec<f32> = cells_to(&keep, MASK, MASK)
        .into_iter()
        .map(|d| d.min(2.0 * GROW))
        .collect();
    let reach = 2.0 * GROW;
    let signed = Mask::new(
        MASK,
        MASK,
        signed.into_iter().map(|d| d.clamp(-reach, reach)).collect(),
    )
    .resampled(w, h);
    let kept = Mask::new(MASK, MASK, kept).resampled(w, h);
    let matte = if matte.width == w && matte.height == h {
        std::borrow::Cow::Borrowed(matte)
    } else {
        std::borrow::Cow::Owned(matte.resampled(w, h))
    };
    let data = (0..w * h)
        .map(|i| {
            let (d, r, s) = (signed.data[i], sams.data[i], matte.data[i]);
            let allow = (GROW + 0.5 - d).clamp(0.0, 1.0);
            let floor = (-d / FLOOR).clamp(0.0, 1.0);
            let theirs = s.min(allow).max(floor * r);
            // Away from what is kept, and nearer this person than it:
            // a gap between two people that Subject fills goes to
            // neither by Subject's matte, so they never share it.
            let k = kept.data[i];
            let away = ((k - 1.0) / (GROW - 1.0)).clamp(0.0, 1.0);
            let nearer = (k - 0.5 - d.max(0.0)).clamp(0.0, 1.0);
            let t = away.min(nearer);
            (t * theirs + (1.0 - t) * r).clamp(0.0, 1.0)
        })
        .collect();
    Mask::new(w, h, data)
}

/// `face`'s share of a `MASK` square instance SAM merged over the
/// faces `held` (boxes, fractions; `face` among them): each cell goes
/// to the face nearest it, across in face widths and down in a quarter
/// of face heights, so a body's share runs down from its own face
/// rather than across to a neighbor's, and a nearer, larger face
/// claims more.
fn split(merged: &Mask, held: &[[f32; 4]], face: [f32; 4]) -> Mask {
    let near = |f: [f32; 4], x: f32, y: f32| {
        let [cx, cy] = center(f);
        let (fw, fh) = ((f[2] - f[0]).max(1e-3), (f[3] - f[1]).max(1e-3));
        ((x - cx) / fw).hypot(0.25 * (y - cy) / fh)
    };
    let mut out = merged.clone();
    for (i, v) in out.data.iter_mut().enumerate() {
        let x = ((i % MASK) as f32 + 0.5) / MASK as f32;
        let y = ((i / MASK) as f32 + 0.5) / MASK as f32;
        let mine = near(face, x, y);
        if held.iter().any(|&f| f != face && near(f, x, y) < mine) {
            *v = 0.0;
        }
    }
    out
}

/// Two masks' cells over the cut: their intersection over their union.
fn mask_iou(a: &Mask, b: &Mask) -> f32 {
    let both = overlap(a, b);
    let on = |m: &Mask| m.data.iter().filter(|&&v| v > CUT).count();
    let either = on(a) + on(b) - both;
    if either > 0 {
        both as f32 / either as f32
    } else {
        0.0
    }
}

/// How many cells two masks of one size are both over the cut in.
fn overlap(a: &Mask, b: &Mask) -> usize {
    a.data
        .iter()
        .zip(&b.data)
        .filter(|&(&x, &y)| x > CUT && y > CUT)
        .count()
}

/// How many of `faces` (boxes, fractions) have their center inside
/// `body`, over the cut.
fn holds(body: &Mask, faces: impl Iterator<Item = [f32; 4]>) -> usize {
    let (s, t) = (body.width as f32, body.height as f32);
    faces
        .filter(|&f| {
            let [cx, cy] = center(f);
            sample(body, cx * s - 0.5, cy * t - 0.5) > CUT
        })
        .count()
}

/// The instances over the cut that belong to a face no body went with,
/// whole: those whose mask over the cut reaches into the region a body
/// under that face would fill (the signature's extent: from a quarter
/// face height above the face to seven below, three face widths
/// across), and does not reach further into someone else's body
/// (`others`, the `person` instances over the cut, less any that holds
/// this face) than into it. Cut to
/// a box instead, a top would be a collar and long hair would stop at
/// the jaw.
fn near_face(instances: &[Instance], face: [f32; 4], others: &[&Mask]) -> Vec<Instance> {
    let (fw, fh) = (face[2] - face[0], face[3] - face[1]);
    let cx = (face[0] + face[2]) / 2.0;
    let region = box_mask([
        cx - 1.5 * fw,
        face[1] + BANDS[0].0 * fh,
        cx + 1.5 * fw,
        face[1] + BANDS[2].1 * fh,
    ]);
    // A body that holds this face is not someone else's: it is the
    // body SAM merged over this person and another.
    let [fx, fy] = center(face);
    let others: Vec<Mask> = others
        .iter()
        .map(|m| m.resampled(MASK, MASK))
        .filter(|m| sample(m, fx * MASK as f32 - 0.5, fy * MASK as f32 - 0.5) <= CUT)
        .collect();
    instances
        .iter()
        .filter(|i| i.score > CUT)
        .filter(|i| {
            let m = i.mask.resampled(MASK, MASK);
            let overlap = |with: &Mask| {
                m.data
                    .iter()
                    .zip(&with.data)
                    .filter(|&(&a, &b)| a > CUT && b > CUT)
                    .count()
            };
            let mine = overlap(&region);
            mine > 0 && others.iter().all(|o| overlap(o) <= mine)
        })
        .cloned()
        .collect()
}

// --- People --------------------------------------------------------------

/// One person on a picture: a face over the cut, and the `person`
/// instance that holds it ([`people`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    /// The face's box, in fractions of the picture.
    pub face: [f32; 4],
    pub score: f32,
    /// The person's mask, `MASK` square over the picture, a sigmoid;
    /// `None` for a face no body went with.
    pub body: Option<Mask>,
    pub signature: Signature,
    /// The face's center, in units of the picture's width both ways
    /// (the masks' units).
    pub at: [f32; 2],
}

/// The people on the preview, best face first: every face over the cut
/// is a person, so a picture of two faces is never taken for a picture
/// of one. Each face goes with the `person` instance over the cut whose
/// mask is highest at the face's center (the smaller one where two are
/// about as high, so a baby in an adult's arms is the baby), or the
/// next such if a better face took it; no two faces share one. A face
/// no body is left for is measured from its face box alone: a head band
/// and no other, which no signature can be compared with, so the match
/// asks about them.
pub fn people<S: Segment>(
    sam: &mut S,
    presets: &Presets,
    picture: &mut Picture<S::Encoding>,
) -> Result<Vec<Person>> {
    let faces = faces(sam, presets, picture)?;
    if faces.is_empty() {
        return Ok(Vec::new());
    }
    let bodies = preview_found(sam, picture, presets.get(PERSON)?)?;
    let bodies: Vec<&Instance> = bodies.over(CUT).collect();
    let pairs = pair(&faces.iter().map(|f| f.bbox).collect::<Vec<_>>(), &bodies);
    let p = picture.preview;
    let aspect = p.height as f32 / p.width as f32;
    Ok(faces
        .iter()
        .zip(pairs)
        .map(|(face, body)| {
            let c = center(face.bbox);
            let body = body.map(|b| bodies[b].mask.clone());
            let signature = match &body {
                Some(mask) => signature(p, mask, face.bbox),
                None => signature(p, &box_mask(face.bbox), face.bbox),
            };
            Person {
                face: face.bbox,
                score: face.score,
                body,
                signature,
                at: [c[0], c[1] * aspect],
            }
        })
        .collect())
}

fn center(b: [f32; 4]) -> [f32; 2] {
    [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]
}

/// A box (fractions) as a `MASK` square mask: one inside, nought out.
fn box_mask(b: [f32; 4]) -> Mask {
    let inside = |v: usize, lo: f32, hi: f32| {
        let f = (v as f32 + 0.5) / MASK as f32;
        f >= lo && f < hi
    };
    Mask::new(
        MASK,
        MASK,
        (0..MASK * MASK)
            .map(|i| {
                let on = inside(i % MASK, b[0], b[2]) && inside(i / MASK, b[1], b[3]);
                if on { 1.0 } else { 0.0 }
            })
            .collect(),
    )
}

/// Two masks' values at a face's center this close are a tie, which
/// the smaller box wins: a sigmoid saturates, and an adult holding a
/// baby can be as sure of the baby's face as the baby's own mask is.
const TIE: f32 = 0.01;

/// Which body each face goes with, faces best first: of the bodies
/// whose mask is over the cut at the face's center that no better face
/// took, the highest there, or among those within `TIE` of it the one
/// with the smallest box. `None` where no body holds the face or every
/// one that does is taken.
fn pair(faces: &[[f32; 4]], bodies: &[&Instance]) -> Vec<Option<usize>> {
    let area = |b: usize| {
        let [x0, y0, x1, y1] = bodies[b].bbox;
        (x1 - x0) * (y1 - y0)
    };
    let mut taken = vec![false; bodies.len()];
    faces
        .iter()
        .map(|&f| {
            let [cx, cy] = center(f);
            let value = |b: &Instance| {
                let s = b.mask.width as f32;
                let t = b.mask.height as f32;
                sample(&b.mask, cx * s - 0.5, cy * t - 0.5)
            };
            let mut holds: Vec<(usize, f32)> = (0..bodies.len())
                .filter(|&b| !taken[b])
                .map(|b| (b, value(bodies[b])))
                .filter(|&(_, v)| v > CUT)
                .collect();
            holds.sort_by(|a, b| b.1.total_cmp(&a.1));
            let top = holds.first()?.1;
            let pick = holds
                .iter()
                .take_while(|&&(_, v)| v >= top - TIE)
                .map(|&(b, _)| b)
                .min_by(|&a, &b| area(a).total_cmp(&area(b)))
                .expect("the top one is within the tie");
            taken[pick] = true;
            Some(pick)
        })
        .collect()
}

// --- The signature and the match -----------------------------------------

/// What a [`Signature`]'s values are. Stored with them by name, so that
/// another kind (a face embedding, say) can take over without a change
/// of schema. A kind this build does not know is kept as it came, name
/// and values, so a shape written by a later build loads here and is
/// saved back unchanged; it is never compared, so the match asks.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SignatureKind {
    /// Colors in three bands down the body and the face's size
    /// ([`signature`]): per band the CIELAB mean's three, the
    /// deviation's three and the fill, then the size; 22 values.
    Colors1,
    /// A kind this build does not know, by its name.
    Unknown(String),
}

impl SignatureKind {
    pub fn name(&self) -> &str {
        match self {
            SignatureKind::Colors1 => "colors-1",
            SignatureKind::Unknown(name) => name,
        }
    }

    /// The kind called `name`, known or not.
    pub fn from_name(name: &str) -> Self {
        match name {
            "colors-1" => SignatureKind::Colors1,
            other => SignatureKind::Unknown(other.to_string()),
        }
    }

    /// How many values a signature of this kind has, if this build
    /// knows the kind.
    pub fn count(&self) -> Option<usize> {
        match self {
            SignatureKind::Colors1 => Some(3 * 7 + 1),
            SignatureKind::Unknown(_) => None,
        }
    }
}

impl Serialize for SignatureKind {
    fn serialize<Z: serde::Serializer>(&self, s: Z) -> std::result::Result<Z::Ok, Z::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for SignatureKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(SignatureKind::from_name(&String::deserialize(d)?))
    }
}

/// One band of a person: the mean and standard deviation of CIELAB
/// over their pixels in it, and the share of the band's rows, across
/// the person's own width in it, that is theirs. A band with too
/// little of the person in it (cut off by the frame, or hidden) is all
/// zeros, and is left out of a comparison; a band that is there always
/// has a fill over nought.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Band {
    pub mean: [f32; 3],
    pub std: [f32; 3],
    pub fill: f32,
}

impl Band {
    pub fn is_there(&self) -> bool {
        self.fill > 0.0
    }
}

/// A person as something to know them by again: a kind and its values.
/// Today the one kind made is [`SignatureKind::Colors1`]: clothes, hair
/// and skin as colors in three bands, head, upper and lower, and the
/// face's size over the picture's diagonal. Nothing in it is a face.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Stored", into = "Stored")]
pub struct Signature {
    kind: SignatureKind,
    values: Vec<f32>,
}

/// A signature as it is stored: the kind by name and the values.
#[derive(Serialize, Deserialize)]
struct Stored {
    kind: SignatureKind,
    values: Vec<f32>,
}

impl TryFrom<Stored> for Signature {
    type Error = String;
    fn try_from(s: Stored) -> std::result::Result<Self, String> {
        Signature::new(s.kind, s.values)
            .ok_or_else(|| "a signature with the wrong number of values".into())
    }
}

impl From<Signature> for Stored {
    fn from(s: Signature) -> Self {
        Stored {
            kind: s.kind,
            values: s.values,
        }
    }
}

impl Signature {
    /// A signature of `kind` from its values: for a kind this build
    /// knows, if they are its number and all finite; for another, as
    /// they are.
    pub fn new(kind: SignatureKind, values: Vec<f32>) -> Option<Self> {
        let fits = match kind.count() {
            Some(n) => values.len() == n && values.iter().all(|v| v.is_finite()),
            None => true,
        };
        fits.then_some(Self { kind, values })
    }

    /// A colors signature from its bands and the face's size, if every
    /// value is finite.
    pub fn colors(bands: [Band; 3], size: f32) -> Option<Self> {
        let mut values = Vec::with_capacity(22);
        for b in &bands {
            values.extend(b.mean);
            values.extend(b.std);
            values.push(b.fill);
        }
        values.push(size);
        Self::new(SignatureKind::Colors1, values)
    }

    pub fn kind(&self) -> &SignatureKind {
        &self.kind
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// A colors signature's bands, head, upper and lower.
    pub fn bands(&self) -> Option<[Band; 3]> {
        (self.kind == SignatureKind::Colors1).then(|| {
            std::array::from_fn(|k| {
                let b = &self.values[k * 7..(k + 1) * 7];
                Band {
                    mean: [b[0], b[1], b[2]],
                    std: [b[3], b[4], b[5]],
                    fill: b[6],
                }
            })
        })
    }

    /// A colors signature's face size, over the picture's diagonal.
    pub fn size(&self) -> Option<f32> {
        (self.kind == SignatureKind::Colors1).then(|| self.values[21])
    }

    /// A colors signature with no band: compared with nothing.
    fn blank() -> Self {
        Self::colors([Band::default(); 3], 0.0).expect("finite")
    }
}

/// sRGB's 8-bit values to linear, each.
fn linear_table() -> &'static [f32; 256] {
    static TABLE: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}

/// CIELAB of an sRGB pixel, D65 white.
pub fn lab(rgb: [u8; 3]) -> [f32; 3] {
    let t = linear_table();
    let [r, g, b] = rgb.map(|v| t[v as usize]);
    let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
    let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    let z = (0.0193339 * r + 0.119192 * g + 0.9503041 * b) / 1.08883;
    let f = |t: f32| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// The bands' rows, in face heights down from the top of the face:
/// the head (with the hair above it), the upper body below the neck,
/// and the lower body from the hips.
const BANDS: [(f32, f32); 3] = [(-0.25, 1.0), (1.25, 3.5), (4.0, 7.0)];
/// A band needs this many faces' areas of the person in it to count.
const LEAST: f32 = 0.5;

/// The colors signature of the person whose mask is `body` (any size,
/// over the picture) on `preview`, with their face at `face`
/// (fractions). The bands are measured from the face, in face heights,
/// not from the top and bottom of what the frame shows of the person,
/// so a head-and-shoulders frame and a full-length one of the same
/// person agree on the bands both show.
pub fn signature(preview: &Rgb8, body: &Mask, face: [f32; 4]) -> Signature {
    let (w, h) = (preview.width, preview.height);
    let mask = body.resampled(w, h);
    let on: Vec<bool> = mask.data.iter().map(|&v| v > CUT).collect();
    let fw = (face[2] - face[0]) * w as f32;
    let fh = (face[3] - face[1]) * h as f32;
    let diag = ((w * w + h * h) as f32).sqrt();
    let size = if diag > 0.0 {
        (fw * fw + fh * fh).sqrt() / diag
    } else {
        0.0
    };
    let top = face[1] * h as f32;
    let mut bands = [Band::default(); 3];
    for (band, (from, to)) in bands.iter_mut().zip(BANDS) {
        let a = (top + from * fh).max(0.0) as usize;
        let b = ((top + to * fh).min(h as f32).max(0.0) as usize).max(a);
        let mut sum = [0f64; 3];
        let mut sq = [0f64; 3];
        let (mut n, mut x0, mut x1) = (0usize, usize::MAX, 0usize);
        for y in a..b {
            for x in 0..w {
                let i = y * w + x;
                if !on[i] {
                    continue;
                }
                let px = &preview.data[i * 3..i * 3 + 3];
                let l = lab([px[0], px[1], px[2]]);
                for c in 0..3 {
                    sum[c] += l[c] as f64;
                    sq[c] += (l[c] as f64).powi(2);
                }
                (n, x0, x1) = (n + 1, x0.min(x), x1.max(x + 1));
            }
        }
        if n == 0 || (n as f32) < LEAST * fw * fh {
            continue;
        }
        let nf = n as f64;
        band.mean = std::array::from_fn(|c| (sum[c] / nf) as f32);
        band.std =
            std::array::from_fn(|c| (sq[c] / nf - (sum[c] / nf).powi(2)).max(0.0).sqrt() as f32);
        band.fill = (nf / ((b - a) * (x1 - x0)) as f64) as f32;
    }
    // Finite by construction from finite boxes; a box that is not
    // (or an empty preview) gives a signature compared with nothing.
    Signature::colors(bands, size).unwrap_or_else(Signature::blank)
}

/// The bands' weights, head, upper and lower, and the size's.
const BAND_WEIGHTS: [f32; 3] = [1.0, 1.5, 1.5];
const SIZE_WEIGHT: f32 = 0.5;
/// A band's fill and the face's size brought to Lab's scale: a fill
/// apart by a tenth counts as a color one unit apart (the fill moves
/// with the pose more than the colors do), and a face a fifth larger
/// or smaller (ln 1.2 = 0.18) as a color about nine apart.
const FILL_SCALE: f32 = 10.0;
const SIZE_SCALE: f32 = 50.0;

/// How far apart two signatures are, in about CIELAB's units: each
/// band both have (means, deviations and fill together) and the faces'
/// sizes (as a ratio), weighted and averaged. `None` where they cannot
/// be compared: of different kinds, or sharing neither the upper band
/// nor two bands of the three (a head alone put strangers as near as
/// the same person).
pub fn distance(a: &Signature, b: &Signature) -> Option<f32> {
    if a.kind != b.kind || a.kind.count().is_none() {
        return None;
    }
    let (ba, bb) = (a.bands()?, b.bands()?);
    let shared: Vec<bool> = ba
        .iter()
        .zip(&bb)
        .map(|(x, y)| x.is_there() && y.is_there())
        .collect();
    if !shared[1] && shared.iter().filter(|&&s| s).count() < 2 {
        return None;
    }
    let (mut total, mut weights) = (0.0, 0.0);
    for (((x, y), w), _) in ba
        .iter()
        .zip(&bb)
        .zip(BAND_WEIGHTS)
        .zip(&shared)
        .filter(|(_, s)| **s)
    {
        let mut d2 = 0.0;
        for c in 0..3 {
            d2 += (x.mean[c] - y.mean[c]).powi(2) + (x.std[c] - y.std[c]).powi(2);
        }
        d2 += (FILL_SCALE * (x.fill - y.fill)).powi(2);
        total += w * d2.sqrt();
        weights += w;
    }
    let (sa, sb) = (a.size()?, b.size()?);
    let ratio = (sa.max(1e-4) / sb.max(1e-4)).ln().abs();
    total += SIZE_WEIGHT * SIZE_SCALE * ratio;
    let d = total / (weights + SIZE_WEIGHT);
    d.is_finite().then_some(d)
}

/// The farthest a person may be from the signature and still be taken
/// for them on a picture of one person, and still be offered as the
/// guess on a picture of more. Set on the trial's thirteen portraits:
/// the same woman in two frames came to 4.80 (seated, framed alike) and
/// 7.44 (full length, then head and shoulders behind flowers); the
/// nearest two different people on different frames, 7.93. On groups
/// colors do not tell people apart at all (strangers inside one group
/// as near as 3.65), which is why nothing here decides alone there.
pub const BOUND: f32 = 7.5;
/// How much nearer the guess must be than everyone else on the picture
/// for the ask to carry it: the second's distance at least this times
/// the guess's. A person who cannot be compared is unknown, not far,
/// and leaves no guess.
pub const MARGIN: f32 = 1.5;
/// Two signatures of one kind whose values are each within this of the
/// other's are the same render of the same person: the picture the
/// shape was made on, where `at` breaks a tie. Read on the values, not
/// through [`distance`], so a person no distance can be had for (a
/// face no body went with) still resolves on their own picture. The
/// values are CIELAB's units but for the fills and the size, which are
/// fractions; the person's signature on the CPU and on WebGPU came at
/// most 0.008 apart in any value; of the nineteen people on the
/// trial's thirteen pictures, the two nearest in every value came 4.78
/// apart in one.
pub const SAME: f32 = 0.25;

/// Whether `a` and `b` are the same render of the same person
/// ([`SAME`]).
pub fn same_render(a: &Signature, b: &Signature) -> bool {
    // A kind this build does not know says nothing of what its values
    // mean (a later embedding's may all sit within `SAME`), and a
    // signature with no band could be anyone's.
    let shown = |s: &Signature| s.bands().is_some_and(|b| b.iter().any(Band::is_there));
    a.kind == b.kind
        && shown(a)
        && shown(b)
        && a.values.len() == b.values.len()
        && a.values
            .iter()
            .zip(&b.values)
            .all(|(x, y)| (x - y).abs() <= SAME)
}

/// Who a person's signature is on a picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Choice {
    /// The person at this index, and their distance (nought where it
    /// cannot be had, for the same render): decided alone.
    Person(usize, f32),
    /// Ask which person, highlighting `guess` if there is one.
    Ask { guess: Option<usize> },
    /// No one on the picture, or the one person on it is someone else.
    Nobody,
}

/// The person among `people` (each a signature and where their face
/// is, in width units) that `wanted` is. Decided alone only where it
/// can be: on the picture the shape was made on (the same render,
/// [`same_render`], with the face within about a face's width of `at`,
/// the nearest `at` among any tied there), or on a picture
/// of one person, who is them within `BOUND` or else nobody. On any
/// other picture of two or more people it asks, with the nearest as
/// the guess when they are within `BOUND` and clearly nearer than
/// everyone else (`MARGIN`). A signature it cannot compare (of another
/// kind, or too little of the person shown) is asked about, never
/// decided on.
pub fn choose(wanted: &Signature, at: [f32; 2], people: &[(Signature, [f32; 2])]) -> Choice {
    if people.is_empty() {
        return Choice::Nobody;
    }
    let d: Vec<Option<f32>> = people.iter().map(|(s, _)| distance(wanted, s)).collect();
    let mut known: Vec<(usize, f32)> = d
        .iter()
        .enumerate()
        .filter_map(|(i, d)| d.map(|d| (i, d)))
        .collect();
    known.sort_by(|a, b| a.1.total_cmp(&b.1));

    let gap = |i: usize| {
        let p = people[i].1;
        (p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2)
    };
    // The same render is only ever the picture the shape was made on,
    // where `at` is where the face is: within about a face's width
    // (the signature's face size, over the picture's diagonal, is at
    // most that in width units).
    let reach = wanted.size().unwrap_or(0.0).max(0.01).powi(2);
    if let Some(i) = (0..people.len())
        .filter(|&i| same_render(wanted, &people[i].0) && gap(i) <= reach)
        .min_by(|&a, &b| gap(a).total_cmp(&gap(b)))
    {
        return Choice::Person(i, d[i].unwrap_or(0.0));
    }
    if people.len() == 1 {
        return match d[0] {
            Some(d) if d <= BOUND => Choice::Person(0, d),
            Some(_) => Choice::Nobody,
            None => Choice::Ask { guess: None },
        };
    }
    let guess = match known[..] {
        [(i, d0), (_, d1), ..]
            if known.len() == people.len() && d0 <= BOUND && d1 >= MARGIN * d0 =>
        {
            Some(i)
        }
        _ => None,
    };
    Choice::Ask { guess }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_resample_is_pillows() {
        // Pillow 12.3's Image.resize(..., BILINEAR) of `pattern`,
        // planar: shrinking both ways, growing both ways, and one of
        // each.
        fn pattern(w: usize, h: usize) -> Rgb8 {
            let mut data = Vec::new();
            for y in 0..h {
                for x in 0..w {
                    data.extend([
                        ((x * 37 + y * 11) % 256) as u8,
                        ((x * 5 + y * 71 + 3) % 256) as u8,
                        ((255 + 512 - x * 29 - y * 13) % 256) as u8,
                    ]);
                }
            }
            Rgb8::new(w, h, data)
        }
        fn planar(img: &Rgb8) -> Vec<u8> {
            (0..3)
                .flat_map(|c| img.data.iter().skip(c).step_by(3).copied())
                .collect()
        }
        type Case = ((usize, usize), (usize, usize), Vec<u8>);
        let cases: [Case; 3] = [
            (
                (7, 5),
                (3, 3),
                vec![
                    37, 116, 195, 54, 133, 212, 71, 150, 177, 37, 48, 59, 149, 160, 171, 114, 125,
                    136, 224, 162, 100, 204, 142, 80, 184, 122, 60,
                ],
            ),
            (
                (2, 3),
                (5, 4),
                vec![
                    0, 4, 19, 33, 37, 7, 11, 26, 40, 44, 15, 19, 34, 48, 52, 22, 26, 41, 55, 59, 3,
                    3, 6, 8, 8, 47, 47, 50, 52, 52, 101, 101, 104, 106, 106, 145, 145, 148, 150,
                    150, 255, 252, 241, 229, 226, 247, 244, 233, 221, 218, 237, 234, 223, 211, 208,
                    229, 226, 215, 203, 200,
                ],
            ),
            (
                (9, 2),
                (4, 6),
                vec![
                    31, 106, 172, 61, 31, 106, 172, 61, 35, 110, 176, 65, 38, 113, 179, 68, 42,
                    117, 183, 72, 42, 117, 183, 72, 7, 17, 29, 39, 7, 17, 29, 39, 31, 41, 53, 63,
                    54, 64, 76, 86, 78, 88, 100, 110, 78, 88, 100, 110, 231, 172, 106, 47, 231,
                    172, 106, 47, 227, 168, 102, 43, 222, 163, 97, 38, 218, 159, 93, 34, 218, 159,
                    93, 34,
                ],
            ),
        ];
        for ((w, h), (nw, nh), want) in cases {
            let got = resample(&pattern(w, h), nw, nh);
            assert_eq!(planar(&got), want, "{w}x{h} to {nw}x{nh}");
        }
        // The same size is the same picture.
        let p = pattern(6, 4);
        assert_eq!(resample(&p, 6, 4), p);
        assert_eq!(squash(&p).len(), 3 * SIDE * SIDE);
    }

    #[test]
    fn square_follows_the_trials_crop_rule() {
        // A 40 × 20 box, 2.5 ×: a square of 100 about its center.
        let r = square([100.0, 200.0, 140.0, 220.0], 2.5, (1000, 800), 96.0);
        assert_eq!(
            r,
            Rect {
                x0: 70,
                y0: 160,
                x1: 170,
                y1: 260
            }
        );
        // The least side wins over a small box.
        let r = square([500.0, 500.0, 510.0, 504.0], 2.5, (1000, 800), 96.0);
        assert_eq!((r.width(), r.height()), (96, 96));
        assert_eq!((r.x0, r.y0), (457, 454));
        // Held inside the picture at a corner.
        let r = square([0.0, 780.0, 30.0, 800.0], 4.0, (1000, 800), 96.0);
        assert_eq!(
            r,
            Rect {
                x0: 0,
                y0: 680,
                x1: 120,
                y1: 800
            }
        );
        // No larger than the picture's shorter side.
        let r = square([0.0, 0.0, 1000.0, 800.0], 1.3, (1000, 800), 256.0);
        assert_eq!((r.width(), r.height()), (800, 800));
        assert_eq!((r.x0, r.y0), (100, 0));
        // Python rounds a half to even: 12.5 to 12, 13.5 to 14.
        let r = square([37.5, 38.5, 62.5, 63.5], 1.0, (200, 200), 25.0);
        assert_eq!((r.x0, r.y0), (38, 38));
        let r = square([24.5, 25.5, 49.5, 50.5], 1.0, (200, 200), 25.0);
        assert_eq!((r.x0, r.y0), (24, 26));
    }

    fn mask_of(w: usize, h: usize, on: impl Fn(usize, usize) -> bool) -> Mask {
        Mask::new(
            w,
            h,
            (0..w * h)
                .map(|i| if on(i % w, i / w) { 0.9 } else { 0.1 })
                .collect(),
        )
    }

    #[test]
    fn components_split_the_eyes_and_drop_specks() {
        let m = mask_of(40, 20, |x, y| {
            // Two eyes, a speck two cells across, and a diagonal pair
            // that is two parts under four-connectivity.
            ((5..12).contains(&x) && (8..12).contains(&y))
                || ((25..33).contains(&x) && (7..12).contains(&y))
                || ((18..20).contains(&x) && (2..6).contains(&y))
                || (x == 36 && y == 15)
                || (x == 37 && y == 16)
        });
        assert_eq!(components(&m, 0.5), vec![[25, 7, 33, 12], [5, 8, 12, 12]]);
        // Joined by a bridge, they are one part.
        let m = mask_of(40, 20, |x, y| {
            ((5..12).contains(&x) && (8..12).contains(&y))
                || ((25..33).contains(&x) && (7..12).contains(&y))
                || ((12..25).contains(&x) && y == 9)
        });
        assert_eq!(components(&m, 0.5), vec![[5, 7, 33, 12]]);
        assert!(components(&mask_of(10, 10, |_, _| false), 0.5).is_empty());
    }

    fn instance(score: f32, f: impl Fn(usize, usize) -> f32) -> Instance {
        Instance {
            score,
            bbox: [0.0, 0.0, 1.0, 1.0],
            mask: Mask::new(
                MASK,
                MASK,
                (0..MASK * MASK).map(|i| f(i % MASK, i / MASK)).collect(),
            ),
        }
    }

    #[test]
    fn union_is_the_most_over_the_instances_over_the_cut() {
        let a = instance(0.9, |x, _| if x < 100 { 0.8 } else { 0.0 });
        let b = instance(0.7, |x, _| if (50..150).contains(&x) { 0.6 } else { 0.1 });
        let c = instance(0.4, |_, _| 1.0);
        let u = union(&[a, b, c], CUT);
        assert_eq!(u.at(10, 5), 0.8);
        assert_eq!(u.at(60, 5), 0.8);
        assert_eq!(u.at(120, 5), 0.6);
        assert_eq!(u.at(200, 5), 0.1);
        assert!(
            union(&[instance(0.5, |_, _| 1.0)], CUT)
                .data
                .iter()
                .all(|&v| v == 0.0)
        );
    }

    #[test]
    fn the_table_reads_back_what_was_written() {
        let phrase = |text: &str, k: f32| Phrase {
            text: text.into(),
            mask: (0..CONTEXT).map(|i| i >= 4).collect(),
            features: (0..CONTEXT * WIDTH).map(|i| i as f32 * k - 1.5).collect(),
        };
        let table = Presets::new(vec![phrase("face", 1e-3), phrase("iris of the eye", -2e-4)]);
        let bytes = table.to_bytes();
        assert_eq!(
            bytes.len(),
            4 + 2 * (2 + CONTEXT + CONTEXT * WIDTH * 4) + 4 + 15
        );
        let back = Presets::parse(&bytes).expect("parse");
        assert_eq!(back, table);
        assert_eq!(
            back.phrase("iris of the eye").map(|p| p.features[1]),
            Some(-1.5 - 2e-4)
        );
        assert!(back.phrase("iris").is_none());
        assert!(matches!(back.get("dress"), Err(Error::NotPreset(t)) if t == "dress"));
        assert_eq!(
            back.texts().collect::<Vec<_>>(),
            ["face", "iris of the eye"]
        );
        // Cut short, or with bytes after it, or a mask byte not 0 or 1:
        // refused.
        assert!(matches!(
            Presets::parse(&bytes[..bytes.len() - 1]),
            Err(Error::Table(_))
        ));
        let mut long = bytes.clone();
        long.push(0);
        assert!(matches!(Presets::parse(&long), Err(Error::Table(_))));
        let mut bad = bytes.clone();
        bad[4 + 2 + 4] = 2;
        assert!(matches!(Presets::parse(&bad), Err(Error::Table(_))));
    }

    #[test]
    fn lab_is_cielab_under_d65() {
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 0.05);
        assert!(
            close(lab([255, 255, 255]), [100.0, 0.0, 0.0]),
            "{:?}",
            lab([255; 3])
        );
        assert!(close(lab([0, 0, 0]), [0.0, 0.0, 0.0]));
        // Published values for sRGB's primaries and a mid grey.
        assert!(
            close(lab([255, 0, 0]), [53.24, 80.09, 67.20]),
            "{:?}",
            lab([255, 0, 0])
        );
        assert!(
            close(lab([0, 0, 255]), [32.30, 79.19, -107.86]),
            "{:?}",
            lab([0, 0, 255])
        );
        assert!(
            close(lab([128, 128, 128]), [53.59, 0.0, 0.0]),
            "{:?}",
            lab([128; 3])
        );
    }

    /// A figure on grey, measured in its face's height (60 px) from the
    /// face's top: a face of skin 30 wide, a red top 60 wide from 1.25
    /// to 3.5 faces down, blue trousers from 4 to 7, all as the bands
    /// lie, at (`left`, `top`) on a `w` × `h` picture; and the face's
    /// box in fractions.
    fn figure(w: usize, h: usize, left: usize, top: usize) -> (Rgb8, Mask, [f32; 4]) {
        let mut data = vec![128u8; w * h * 3];
        let mut mask = vec![0.0f32; w * h];
        let parts: [(usize, usize, usize, usize, [u8; 3]); 3] = [
            (15, 45, 0, 60, [220, 180, 150]),
            (0, 60, 75, 210, [200, 30, 40]),
            (0, 60, 240, 420, [30, 50, 160]),
        ];
        for (x0, x1, y0, y1, c) in parts {
            for y in (top + y0..top + y1).filter(|&y| y < h) {
                for x in left + x0..left + x1 {
                    data[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
                    mask[y * w + x] = 1.0;
                }
            }
        }
        let face = [
            (left + 15) as f32 / w as f32,
            top as f32 / h as f32,
            (left + 45) as f32 / w as f32,
            (top + 60) as f32 / h as f32,
        ];
        (Rgb8::new(w, h, data), Mask::new(w, h, mask), face)
    }

    #[test]
    fn the_signature_reads_three_bands_from_the_face_down() {
        let (img, body, face) = figure(400, 500, 100, 50);
        let s = signature(&img, &body, face);
        assert_eq!(s.kind(), &SignatureKind::Colors1);
        let bands = s.bands().unwrap();
        let close = |a: [f32; 3], b: [u8; 3]| {
            let l = lab(b);
            a.iter().zip(&l).all(|(x, y)| (x - y).abs() < 1e-3)
        };
        assert!(close(bands[0].mean, [220, 180, 150]), "{:?}", bands[0]);
        assert!(close(bands[1].mean, [200, 30, 40]));
        assert!(close(bands[2].mean, [30, 50, 160]));
        for b in &bands {
            assert!(b.std.iter().all(|&v| v < 1e-2), "{b:?}");
        }
        // The head band starts a quarter face over the face: 60 of its
        // 75 rows are face, across the face's own width.
        assert!((bands[0].fill - 0.8).abs() < 1e-6, "{}", bands[0].fill);
        assert!((bands[1].fill - 1.0).abs() < 1e-6);
        // The face: 30 × 60 on 400 × 500.
        let size = (30f32 * 30.0 + 60.0 * 60.0).sqrt() / (400f32 * 400.0 + 500.0 * 500.0).sqrt();
        assert!((s.size().unwrap() - size).abs() < 1e-6);
        // The same figure, moved: the same signature.
        let (moved, body2, face2) = figure(400, 500, 250, 20);
        let s2 = signature(&moved, &body2, face2);
        assert!(distance(&s, &s2).unwrap() < 1e-3);
        // Half of the top in another color: half the band's deviation.
        let mut img3 = img.clone();
        for y in 125..260 {
            for x in 100..130 {
                img3.data[(y * 400 + x) * 3..(y * 400 + x) * 3 + 3].copy_from_slice(&[30, 50, 160]);
            }
        }
        let s3 = signature(&img3, &body, face);
        assert!(s3.bands().unwrap()[1].std[0] > 5.0);
        assert!(distance(&s, &s3).unwrap() > 10.0);
        // Framed at the waist: the trousers are not there, and the
        // bands both frames show agree.
        let (cut, body4, face4) = figure(400, 300, 100, 50);
        let b4 = signature(&cut, &body4, face4).bands().unwrap();
        assert!(b4[0].is_there() && b4[1].is_there());
        assert!(!b4[2].is_there(), "{:?}", b4[2]);
        let resized = Signature::colors(b4, size).unwrap();
        assert!(distance(&s, &resized).unwrap() < 1e-3);
        // Nobody: no band, and nothing to compare.
        let empty = signature(&img, &Mask::new(4, 4, vec![0.0; 16]), face);
        assert_eq!(empty.bands().unwrap(), [Band::default(); 3]);
        assert_eq!(distance(&s, &empty), None);
    }

    #[test]
    fn a_signature_carries_its_kind() {
        let s = sig([45.0, 60.0, 40.0], 0.05);
        assert_eq!(Some(s.values().len()), SignatureKind::Colors1.count());
        let back = Signature::new(s.kind().clone(), s.values().to_vec());
        assert_eq!(back.as_ref(), Some(&s));
        assert_eq!(Signature::new(SignatureKind::Colors1, vec![0.0; 5]), None);
        assert_eq!(
            Signature::new(SignatureKind::Colors1, vec![f32::NAN; 22]),
            None
        );
        assert_eq!(
            SignatureKind::from_name(s.kind().name()),
            SignatureKind::Colors1
        );
        // Stored as its kind's name and its values; the wrong number of
        // values is refused on the way in.
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            json.starts_with(r#"{"kind":"colors-1","values":["#),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<Signature>(&json).unwrap(), s);
        assert!(
            serde_json::from_str::<Signature>(r#"{"kind":"colors-1","values":[1.0]}"#).is_err()
        );
    }

    #[test]
    fn a_kind_from_a_later_build_loads_and_saves_back_unchanged() {
        let json = r#"{"kind":"face-location-1","values":[0.25,-3.5,1e-7]}"#;
        let s: Signature = serde_json::from_str(json).expect("an unknown kind loads");
        assert_eq!(s.kind(), &SignatureKind::Unknown("face-location-1".into()));
        assert_eq!(s.kind().count(), None);
        assert_eq!(s.values(), &[0.25, -3.5, 1e-7]);
        assert_eq!(s.bands(), None);
        let again = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Signature>(&again).unwrap(), s);
        assert!(again.contains(r#""kind":"face-location-1""#), "{again}");
        // Never compared, not even with itself: the match asks.
        assert_eq!(distance(&s, &s), None);
        let red = sig([45.0, 60.0, 40.0], 0.05);
        assert_eq!(distance(&red, &s), None);
        assert_eq!(
            choose(&s, [0.3, 0.2], &[(red.clone(), [0.3, 0.2])]),
            Choice::Ask { guess: None }
        );
        assert_eq!(
            choose(&red, [0.3, 0.2], &[(s, [0.3, 0.2])]),
            Choice::Ask { guess: None }
        );
    }

    #[test]
    fn nothing_that_is_not_finite_gets_into_a_signature_or_a_distance() {
        let mut bands = [Band {
            mean: [50.0, 0.0, 0.0],
            std: [1.0; 3],
            fill: 0.5,
        }; 3];
        assert!(Signature::colors(bands, f32::NAN).is_none());
        assert!(Signature::colors(bands, 0.05).is_some());
        bands[1].mean[0] = f32::INFINITY;
        assert!(Signature::colors(bands, 0.05).is_none());
        // Finite values whose distance is not: no distance.
        bands[1].mean[0] = 3e38;
        let far = Signature::colors(bands, 0.05).unwrap();
        bands[1].mean[0] = -3e38;
        let other = Signature::colors(bands, 0.05).unwrap();
        assert_eq!(distance(&far, &other), None);
        // An empty preview, or a box that is not finite: a signature
        // compared with nothing.
        let empty = Rgb8::new(0, 0, Vec::new());
        let s = signature(
            &empty,
            &box_mask([0.1, 0.1, 0.2, 0.2]),
            [0.1, 0.1, 0.2, 0.2],
        );
        assert!(s.values().iter().all(|v| v.is_finite()));
        let (img, body, _) = figure(400, 500, 100, 50);
        let s = signature(&img, &body, [f32::NAN, 0.1, 0.2, 0.2]);
        assert!(s.values().iter().all(|v| v.is_finite()));
        assert_eq!(distance(&s, &s), None);
    }

    fn sig(top: [f32; 3], size: f32) -> Signature {
        let band = |mean: [f32; 3]| Band {
            mean,
            std: [5.0, 3.0, 3.0],
            fill: 0.6,
        };
        Signature::colors(
            [
                band([60.0, 15.0, 20.0]),
                band(top),
                band([30.0, 5.0, -30.0]),
            ],
            size,
        )
        .unwrap()
    }

    /// `s` with only the bands `keep` says.
    fn only(s: &Signature, keep: [bool; 3]) -> Signature {
        let mut b = s.bands().unwrap();
        for (band, k) in b.iter_mut().zip(keep) {
            if !k {
                *band = Band::default();
            }
        }
        Signature::colors(b, s.size().unwrap()).unwrap()
    }

    #[test]
    fn the_match_decides_alone_only_where_it_can() {
        let red = sig([45.0, 60.0, 40.0], 0.05);
        let white = sig([90.0, 0.0, 2.0], 0.05);
        let green = sig([50.0, -40.0, 30.0], 0.05);
        // On the picture it was made on: the person picked, at nought,
        // however many people.
        assert_eq!(
            choose(
                &red,
                [0.3, 0.2],
                &[(white.clone(), [0.7, 0.2]), (red.clone(), [0.3, 0.2])]
            ),
            Choice::Person(1, 0.0)
        );
        // There, two the signature cannot tell apart: where the face
        // was decides.
        assert_eq!(
            choose(
                &red,
                [0.3, 0.2],
                &[(red.clone(), [0.7, 0.2]), (red.clone(), [0.31, 0.21])]
            ),
            Choice::Person(1, 0.0)
        );
        // Another frame of one person: them within the bound, else
        // nobody.
        let red_again = sig([48.0, 57.0, 38.0], 0.055);
        let d = distance(&red, &red_again).unwrap();
        assert!(!same_render(&red, &red_again) && d < 4.0, "{d}");
        assert_eq!(
            choose(&red, [0.3, 0.2], &[(red_again.clone(), [0.7, 0.2])]),
            Choice::Person(0, d)
        );
        assert!(distance(&red, &green).unwrap() > BOUND);
        assert_eq!(
            choose(&red, [0.3, 0.2], &[(green.clone(), [0.3, 0.2])]),
            Choice::Nobody
        );
        assert_eq!(choose(&red, [0.3, 0.2], &[]), Choice::Nobody);
        // Another frame of two: always asked, the clear nearest as the
        // guess.
        assert_eq!(
            choose(
                &red,
                [0.3, 0.2],
                &[(green.clone(), [0.3, 0.2]), (red_again.clone(), [0.7, 0.2])]
            ),
            Choice::Ask { guess: Some(1) }
        );
        // Two near alike: no guess.
        let red_too = sig([47.0, 58.0, 39.0], 0.055);
        assert_eq!(
            choose(
                &red,
                [0.3, 0.2],
                &[(red_again.clone(), [0.7, 0.2]), (red_too, [0.3, 0.2])]
            ),
            Choice::Ask { guess: None }
        );
        // Nobody near: no guess.
        assert_eq!(
            choose(
                &red,
                [0.3, 0.2],
                &[(green.clone(), [0.3, 0.2]), (white.clone(), [0.7, 0.2])]
            ),
            Choice::Ask { guess: None }
        );
        // Someone who cannot be compared is unknown, not far: no
        // guess, however near the other.
        let head_only = only(&green, [true, false, false]);
        assert_eq!(distance(&red, &head_only), None);
        assert_eq!(
            choose(
                &red,
                [0.3, 0.2],
                &[
                    (red_again.clone(), [0.7, 0.2]),
                    (head_only.clone(), [0.3, 0.2])
                ]
            ),
            Choice::Ask { guess: None }
        );
        // Alone on a picture and not comparable: asked, not "nobody".
        assert_eq!(
            choose(&red, [0.3, 0.2], &[(head_only, [0.3, 0.2])]),
            Choice::Ask { guess: None }
        );
    }

    #[test]
    fn signatures_compare_on_the_upper_band_or_two_bands() {
        let a = sig([45.0, 60.0, 40.0], 0.05);
        let b = sig([48.0, 57.0, 38.0], 0.055);
        assert!(distance(&a, &b).is_some());
        // The upper band alone is enough; head and lower are too.
        assert!(distance(&a, &only(&b, [false, true, false])).is_some());
        assert!(distance(&a, &only(&b, [true, false, true])).is_some());
        // The head alone, or the lower alone, is not.
        assert_eq!(distance(&a, &only(&b, [true, false, false])), None);
        assert_eq!(distance(&a, &only(&b, [false, false, true])), None);
        // Only what both show counts.
        assert_eq!(
            distance(
                &only(&a, [true, true, false]),
                &only(&b, [false, false, true])
            ),
            None
        );
    }

    #[test]
    fn the_size_counts_as_a_ratio() {
        let a = sig([45.0, 60.0, 40.0], 0.05);
        let b = sig([45.0, 60.0, 40.0], 0.10);
        let c = sig([45.0, 60.0, 40.0], 0.025);
        let (ab, ac) = (distance(&a, &b).unwrap(), distance(&a, &c).unwrap());
        assert!((ab - ac).abs() < 1e-4);
        assert!(ab > 0.0);
    }

    #[test]
    fn draw_places_each_piece_and_takes_the_most() {
        // A picture of 400 × 200 drawn into 100 × 50: a piece over
        // (40..80, 20..60) all ones, and one over the whole picture at
        // a quarter, its own resolution coarser.
        let pieces = [
            Piece {
                rect: Rect {
                    x0: 40,
                    y0: 20,
                    x1: 80,
                    y1: 60,
                },
                mask: Mask::new(160, 160, vec![1.0; 160 * 160]),
            },
            Piece {
                rect: Rect {
                    x0: 0,
                    y0: 0,
                    x1: 400,
                    y1: 200,
                },
                mask: Mask::new(20, 10, vec![0.25; 200]),
            },
        ];
        let r = draw(&pieces, (400, 200), (100, 50));
        assert_eq!(r.len(), 5000);
        assert_eq!(r[10 * 100 + 15], 1.0);
        assert_eq!(r[5 * 100 + 10], 1.0);
        assert!((r[14 * 100 + 19] - 1.0).abs() < 1e-6);
        assert_eq!(r[30 * 100 + 50], 0.25);
        assert_eq!(r[4 * 100 + 9], 0.25);
        // A piece's edge mid-pixel covers that pixel by half.
        let edge = [Piece {
            rect: Rect {
                x0: 42,
                y0: 0,
                x1: 400,
                y1: 200,
            },
            mask: Mask::new(358, 200, vec![1.0; 358 * 200]),
        }];
        let r = draw(&edge, (400, 200), (100, 50));
        assert!((r[10 * 100 + 10] - 0.5).abs() < 1e-6, "{}", r[1010]);
        assert_eq!(r[10 * 100 + 9], 0.0);
        assert_eq!(r[10 * 100 + 11], 1.0);
    }

    /// A `person` instance: its box, and a mask of `inside` within the
    /// rectangle `r` (fractions), nearly nought elsewhere.
    fn body(bbox: [f32; 4], r: [f32; 4], inside: f32) -> Instance {
        Instance {
            score: 0.9,
            bbox,
            mask: Mask::new(
                MASK,
                MASK,
                (0..MASK * MASK)
                    .map(|i| {
                        let (u, v) = (
                            (i % MASK) as f32 / MASK as f32,
                            (i / MASK) as f32 / MASK as f32,
                        );
                        if (r[0]..r[2]).contains(&u) && (r[1]..r[3]).contains(&v) {
                            inside
                        } else {
                            0.02
                        }
                    })
                    .collect(),
            ),
        }
    }

    #[test]
    fn faces_pair_with_the_body_whose_mask_holds_them() {
        // An adult holding a baby: the adult's mask covers the baby
        // too, as sure as the baby's own; the smaller box wins the tie.
        let adult = body([0.3, 0.1, 0.8, 1.0], [0.3, 0.1, 0.8, 1.0], 0.97);
        let baby = body([0.5, 0.5, 0.65, 0.7], [0.5, 0.5, 0.65, 0.7], 0.965);
        let adult_face = [0.42, 0.15, 0.5, 0.25];
        let baby_face = [0.55, 0.52, 0.6, 0.58];
        assert_eq!(
            pair(&[adult_face, baby_face], &[&adult, &baby]),
            vec![Some(0), Some(1)]
        );
        // In either order of the bodies' scores.
        assert_eq!(
            pair(&[baby_face, adult_face], &[&adult, &baby]),
            vec![Some(1), Some(0)]
        );
        // A clearly higher mask wins over a smaller box.
        let faint = body([0.5, 0.5, 0.65, 0.7], [0.5, 0.5, 0.65, 0.7], 0.6);
        assert_eq!(pair(&[baby_face], &[&adult, &faint]), vec![Some(0)]);
        // Two faces, one body: the better face has it, the other has
        // none rather than sharing it.
        let other_face = [0.65, 0.3, 0.72, 0.4];
        assert_eq!(
            pair(&[adult_face, other_face], &[&adult]),
            vec![Some(0), None]
        );
        // Two faces whose best body is the same: the second takes the
        // next one that holds it.
        let second = body([0.6, 0.2, 0.8, 0.6], [0.6, 0.2, 0.8, 0.6], 0.8);
        assert_eq!(
            pair(&[adult_face, other_face], &[&adult, &second]),
            vec![Some(0), Some(1)]
        );
        // A body whose box holds the face but whose mask does not.
        let beside = body([0.0, 0.0, 1.0, 1.0], [0.0, 0.0, 0.3, 1.0], 0.95);
        assert_eq!(pair(&[adult_face], &[&beside]), vec![None]);
    }

    #[test]
    fn the_encodings_keep_the_latest_crops_of_one_picture() {
        let mut e: Encodings<u32> = Encodings::new(2);
        let rect = |x: usize| Rect {
            x0: x,
            y0: 0,
            x1: x + 10,
            y1: 10,
        };
        let mut made = 0;
        let mut get = |e: &mut Encodings<u32>, key: u64, x: usize| {
            *e.crop_or(key, rect(x), || {
                made += 1;
                Ok(x as u32 + 100 * key as u32)
            })
            .unwrap()
        };
        assert_eq!(get(&mut e, 1, 1), 101);
        assert_eq!(get(&mut e, 1, 2), 102);
        assert_eq!(get(&mut e, 1, 1), 101);
        assert_eq!(get(&mut e, 1, 3), 103);
        // 2 was the least recent and went; 1 stayed.
        assert_eq!(get(&mut e, 1, 1), 101);
        assert_eq!(get(&mut e, 1, 2), 102);
        // Another picture: the same rectangle is not the same crop.
        assert_eq!(get(&mut e, 2, 2), 202);
        drop(get);
        assert_eq!(made, 5);
        assert_eq!(e.crops(), 1);
        assert_eq!(e.key(), Some(2));
        assert_eq!(*e.preview_or(2, || Ok(7)).unwrap(), 7);
        assert_eq!(*e.preview_or(2, || Ok(8)).unwrap(), 7);
        // The preview of another picture is made afresh, and the old
        // picture's crops go with it.
        assert_eq!(*e.preview_or(3, || Ok(9)).unwrap(), 9);
        assert_eq!(e.crops(), 0);
        e.clear();
        assert!(!e.has_preview() && e.crops() == 0 && e.key().is_none());
    }

    #[test]
    fn routes_are_named() {
        for r in [Route::Whole, Route::Eye, Route::Mouth] {
            assert_eq!(Route::from_name(r.name()), Some(r));
        }
        assert_eq!(Route::from_name("nose"), None);
    }

    /// A stand-in for the model that finds by color: its encoding is
    /// the picture itself, and each phrase is the pixels of its colors,
    /// as one instance a connected part, boxed.
    struct Colors {
        encoded: Vec<(usize, usize)>,
    }

    const SKIN: [u8; 3] = [220, 180, 150];
    const IRIS: [u8; 3] = [40, 90, 200];
    const LIPS: [u8; 3] = [190, 60, 70];

    impl Segment for Colors {
        type Encoding = Rgb8;
        fn encode(&mut self, image: &Rgb8) -> Result<Rgb8> {
            self.encoded.push((image.width, image.height));
            Ok(image.clone())
        }
        fn decode(&mut self, image: &Rgb8, phrase: &Phrase) -> Result<Found> {
            let colors: &[[u8; 3]] = match phrase.text.as_str() {
                "face" | "person" => &[SKIN, IRIS, LIPS],
                "eyes" | "iris of the eye" => &[IRIS],
                "mouth" | "lips" => &[LIPS],
                _ => &[],
            };
            let at = |x: usize, y: usize| {
                let sx = (x as f32 + 0.5) / MASK as f32 * image.width as f32;
                let sy = (y as f32 + 0.5) / MASK as f32 * image.height as f32;
                let i = (sy as usize * image.width + sx as usize) * 3;
                let p = [image.data[i], image.data[i + 1], image.data[i + 2]];
                if colors.contains(&p) { 0.95 } else { 0.02 }
            };
            let all = Mask::new(
                MASK,
                MASK,
                (0..MASK * MASK).map(|i| at(i % MASK, i / MASK)).collect(),
            );
            // "eyes" is one instance over both, as SAM 3 gives it.
            let parts = if phrase.text == "eyes" {
                components(&all, 0.5)
                    .into_iter()
                    .reduce(|a, b| {
                        [
                            a[0].min(b[0]),
                            a[1].min(b[1]),
                            a[2].max(b[2]),
                            a[3].max(b[3]),
                        ]
                    })
                    .into_iter()
                    .collect()
            } else {
                components(&all, 0.5)
            };
            let instances = parts
                .into_iter()
                .map(|b| {
                    let inside =
                        |x: usize, y: usize| (b[0]..b[2]).contains(&x) && (b[1]..b[3]).contains(&y);
                    Instance {
                        score: 0.9,
                        bbox: b.map(|v| v as f32 / MASK as f32),
                        mask: Mask::new(
                            MASK,
                            MASK,
                            (0..MASK * MASK)
                                .map(|i| {
                                    if inside(i % MASK, i / MASK) {
                                        all.data[i]
                                    } else {
                                        0.0
                                    }
                                })
                                .collect(),
                        ),
                    }
                })
                .collect();
            Ok(Found {
                presence: 0.9,
                instances,
            })
        }
    }

    fn table() -> Presets {
        Presets::new(
            [
                "face",
                "eyes",
                "mouth",
                "person",
                "iris of the eye",
                "lips",
                "hair",
                "upper body clothing",
            ]
            .iter()
            .map(|t| Phrase {
                text: t.to_string(),
                mask: vec![false; CONTEXT],
                features: vec![0.0; CONTEXT * WIDTH],
            })
            .collect(),
        )
    }

    /// A picture `scale` times 400 × 300 with a face of skin, two
    /// irises and a mouth.
    fn portrait(scale: usize) -> Rgb8 {
        let (w, h) = (400 * scale, 300 * scale);
        let mut data = vec![90u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x / scale, y / scale);
                let c = if (240..256).contains(&u) && (240..250).contains(&v) {
                    Some(LIPS)
                } else if ((224..236).contains(&u) || (260..272).contains(&u))
                    && (200..212).contains(&v)
                {
                    Some(IRIS)
                } else if (200..300).contains(&u) && (160..280).contains(&v) {
                    Some(SKIN)
                } else {
                    None
                };
                if let Some(c) = c {
                    data[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
                }
            }
        }
        Rgb8::new(w, h, data)
    }

    fn crop(image: &Rgb8, r: Rect) -> Rgb8 {
        let mut data = Vec::with_capacity(r.width() * r.height() * 3);
        for y in r.y0..r.y1 {
            data.extend(&image.data[(y * image.width + r.x0) * 3..(y * image.width + r.x1) * 3]);
        }
        Rgb8::new(r.width(), r.height(), data)
    }

    #[test]
    fn the_eye_route_finds_each_iris_in_the_full_resolution_picture() {
        // The full picture at four times the preview's pixels.
        let full = portrait(4);
        let preview = portrait(1);
        let luma = preview.luma();
        let mut regions = Vec::new();
        let mut region = |r: Rect| {
            regions.push(r);
            crop(&full, r)
        };
        let mut encodings = Encodings::new(8);
        let mut picture = Picture {
            preview: &preview,
            luma: &luma,
            size: (full.width, full.height),
            key: 1,
            region: &mut region,
            encodings: &mut encodings,
            matte: None,
        };
        let mut sam = Colors {
            encoded: Vec::new(),
        };
        let presets = table();
        let pieces = find(
            &mut sam,
            &presets,
            &mut picture,
            Route::Eye,
            "iris of the eye",
            None,
        )
        .expect("find");
        assert_eq!(pieces.len(), 2);
        // Each crop is 2.5 times the eye's 48 px, about its center.
        for (p, cx) in pieces.iter().zip([920.0, 1064.0]) {
            let r = p.rect;
            // Within the face crop's cells, 624 px over 288.
            assert!((r.width() as f32 - 120.0).abs() <= 12.0, "{r:?}");
            assert!((((r.x0 + r.x1) as f32 / 2.0) - cx).abs() <= 6.0, "{r:?}");
            assert!((((r.y0 + r.y1) as f32 / 2.0) - 824.0).abs() <= 6.0, "{r:?}");
            // At the crop's own pixels.
            assert_eq!((p.mask.width, p.mask.height), (r.width(), r.height()));
        }
        // Drawn into a raster at the preview's size: the irises, and
        // nothing else.
        let raster = draw(&pieces, (full.width, full.height), (400, 300));
        let at = |x: usize, y: usize| raster[y * 400 + x];
        assert!(at(230, 206) > 0.8 && at(266, 206) > 0.8);
        assert!(at(248, 206) < 0.2 && at(230, 230) < 0.2 && at(100, 100) == 0.0);
        // The preview once, a face crop, two eye crops; the eye crops
        // rendered for their luma.
        assert_eq!(sam.encoded.len(), 4);
        assert_eq!(sam.encoded[0], (400, 300));

        // Again, another phrase on the same picture: only decodes.
        let pieces = find(&mut sam, &presets, &mut picture, Route::Mouth, "lips", None).unwrap();
        assert_eq!(pieces.len(), 1);
        assert_eq!(
            sam.encoded.len(),
            5,
            "the face crop is kept, the mouth crop is new"
        );
        let raster = draw(&pieces, (full.width, full.height), (400, 300));
        assert!(raster[245 * 400 + 248] > 0.8);
        assert!(raster[206 * 400 + 230] < 0.2);
        // Rendered: the face crop once, each eye crop and the mouth
        // crop, all in the full picture's pixels.
        assert_eq!(regions.len(), 4);
        assert!(regions.iter().all(|r| r.x1 <= 1600 && r.y1 <= 1200));
    }

    #[test]
    fn the_whole_route_keeps_to_the_person_asked_for() {
        let preview = portrait(1);
        let luma = preview.luma();
        let mut region = |r: Rect| crop(&preview, r);
        let mut encodings = Encodings::new(4);
        let mut picture = Picture {
            preview: &preview,
            luma: &luma,
            size: (400, 300),
            key: 1,
            region: &mut region,
            encodings: &mut encodings,
            matte: None,
        };
        let mut sam = Colors {
            encoded: Vec::new(),
        };
        let presets = table();
        let people = people(&mut sam, &presets, &mut picture).unwrap();
        assert_eq!(people.len(), 1);
        let p = &people[0];
        assert!(
            (p.at[0] - 0.625).abs() < 0.01 && (p.at[1] - 0.55).abs() < 0.01,
            "{:?}",
            p.at
        );
        // The person's mask, a mouth elsewhere left out.
        let mut someone = p.clone();
        someone.body = Some(Mask::new(MASK, MASK, vec![0.0; MASK * MASK]));
        let lips = find(
            &mut sam,
            &presets,
            &mut picture,
            Route::Whole,
            "lips",
            Some(p),
        )
        .unwrap();
        let none = find(
            &mut sam,
            &presets,
            &mut picture,
            Route::Whole,
            "lips",
            Some(&someone),
        )
        .unwrap();
        assert_eq!(lips.len(), 1);
        assert_eq!((lips[0].mask.width, lips[0].mask.height), (400, 300));
        assert!(lips[0].mask.at(248, 245) > 0.8);
        assert!(none[0].mask.data.iter().all(|&v| v < 1e-3));
        // A phrase not in the table is refused, never asked.
        assert!(matches!(
            find(
                &mut sam,
                &presets,
                &mut picture,
                Route::Whole,
                "dress",
                None
            ),
            Err(Error::NotPreset(_))
        ));
        assert_eq!(sam.encoded.len(), 1, "one preview, encoded once");
    }

    /// A stand-in that answers each phrase with fixed instances,
    /// whatever the picture.
    struct Fixed(Vec<(&'static str, Vec<Instance>)>);

    impl Segment for Fixed {
        type Encoding = ();
        fn encode(&mut self, _: &Rgb8) -> Result<()> {
            Ok(())
        }
        fn decode(&mut self, _: &(), phrase: &Phrase) -> Result<Found> {
            let instances = self
                .0
                .iter()
                .find(|(t, _)| *t == phrase.text)
                .map(|(_, i)| i.clone())
                .unwrap_or_default();
            Ok(Found {
                presence: 0.9,
                instances,
            })
        }
    }

    /// Two people side by side on a 400 × 400 picture, in different
    /// clothes; their faces' boxes.
    fn two_people() -> (Rgb8, [[f32; 4]; 2]) {
        let (w, h) = (400usize, 400usize);
        let mut data = vec![128u8; w * h * 3];
        for (left, top_color) in [(60usize, [200u8, 30, 40]), (260, [40, 160, 60])] {
            for y in 40..380 {
                for x in left..left + 80 {
                    let c = if y < 100 { [220, 180, 150] } else { top_color };
                    data[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
                }
            }
        }
        let faces = [[0.2, 0.1, 0.3, 0.25], [0.7, 0.1, 0.8, 0.25]];
        (Rgb8::new(w, h, data), faces)
    }

    fn face(bbox: [f32; 4], score: f32) -> Instance {
        Instance {
            score,
            bbox,
            mask: box_mask(bbox),
        }
    }

    /// The people `sam` finds on the two-person picture, and what a
    /// shape made for the first of them on another frame asks of them.
    fn ask_of_two(bodies: Vec<Instance>) -> (Vec<Person>, Choice) {
        let (img, faces) = two_people();
        let luma = img.luma();
        let mut region = |r: Rect| crop(&img, r);
        let mut encodings = Encodings::new(2);
        let mut picture = Picture {
            preview: &img,
            luma: &luma,
            size: (400, 400),
            key: 1,
            region: &mut region,
            encodings: &mut encodings,
            matte: None,
        };
        let mut sam = Fixed(vec![
            ("face", vec![face(faces[0], 0.9), face(faces[1], 0.85)]),
            ("person", bodies),
        ]);
        let people = people(&mut sam, &table(), &mut picture).unwrap();
        // The first person on another frame: the same clothes, a
        // little lighter, so not this picture's own render.
        let mut v = people[0].signature.values().to_vec();
        v[7] += 2.0;
        let wanted = Signature::new(SignatureKind::Colors1, v).unwrap();
        let there: Vec<_> = people.iter().map(|p| (p.signature.clone(), p.at)).collect();
        let c = choose(&wanted, [0.9, 0.9], &there);
        (people, c)
    }

    #[test]
    fn two_faces_and_one_body_are_two_people_and_asked_about() {
        let one = body([0.15, 0.1, 0.35, 0.95], [0.15, 0.1, 0.35, 0.95], 0.95);
        let (people, c) = ask_of_two(vec![one]);
        assert_eq!(people.len(), 2);
        assert!(people[0].body.is_some() && people[1].body.is_none());
        // The face no body went with has a head band and no other, and
        // is compared with nothing.
        let b = people[1].signature.bands().unwrap();
        assert!(b[0].is_there() && !b[1].is_there() && !b[2].is_there());
        assert_eq!(distance(&people[0].signature, &people[1].signature), None);
        // Not a picture of one person: asked, and with someone not
        // comparable on it, no guess.
        assert_eq!(c, Choice::Ask { guess: None });
    }

    #[test]
    fn one_body_over_two_people_is_still_two_people_asked_about() {
        // SAM's `person` merged the two into one instance.
        let merged = body([0.1, 0.05, 0.9, 0.95], [0.1, 0.05, 0.9, 0.95], 0.95);
        let (people, c) = ask_of_two(vec![merged]);
        assert_eq!(people.len(), 2);
        assert!(people[0].body.is_some() && people[1].body.is_none());
        assert!(matches!(c, Choice::Ask { .. }), "{c:?}");
        // With a body each, still asked: two people on another frame.
        let a = body([0.15, 0.1, 0.35, 0.95], [0.15, 0.1, 0.35, 0.95], 0.95);
        let b = body([0.65, 0.1, 0.85, 0.95], [0.65, 0.1, 0.85, 0.95], 0.95);
        let (people, c) = ask_of_two(vec![a, b]);
        assert!(people.iter().all(|p| p.body.is_some()));
        assert_eq!(c, Choice::Ask { guess: Some(0) });
    }

    /// `run` on the two-person picture with `sam`.
    fn on_two<R>(sam: &mut Fixed, run: impl FnOnce(&mut Fixed, &mut Picture<()>) -> R) -> R {
        let (img, _) = two_people();
        let luma = img.luma();
        let mut region = |r: Rect| crop(&img, r);
        let mut encodings = Encodings::new(2);
        let mut picture = Picture {
            preview: &img,
            luma: &luma,
            size: (400, 400),
            key: 1,
            region: &mut region,
            encodings: &mut encodings,
            matte: None,
        };
        run(sam, &mut picture)
    }

    #[test]
    fn a_face_with_no_body_resolves_alone_on_its_own_picture() {
        let (_, faces) = two_people();
        let one = body([0.15, 0.1, 0.35, 0.95], [0.15, 0.1, 0.35, 0.95], 0.95);
        let mut sam = Fixed(vec![
            ("face", vec![face(faces[0], 0.9), face(faces[1], 0.85)]),
            ("person", vec![one]),
        ]);
        let people = on_two(&mut sam, |sam, pic| people(sam, &table(), pic).unwrap());
        assert!(people[1].body.is_none());
        let there: Vec<_> = people.iter().map(|p| (p.signature.clone(), p.at)).collect();
        // Picked, then rendered again: the same person, though no
        // distance can be had for them, themself included.
        assert_eq!(distance(&people[1].signature, &people[1].signature), None);
        assert_eq!(
            choose(&people[1].signature, people[1].at, &there),
            Choice::Person(1, 0.0)
        );
        // The same render drifted by what WebGPU moves a value.
        let drifted: Vec<f32> = people[1]
            .signature
            .values()
            .iter()
            .map(|v| v + 0.01)
            .collect();
        let drifted = Signature::new(SignatureKind::Colors1, drifted).unwrap();
        assert_eq!(
            choose(&drifted, people[1].at, &there),
            Choice::Person(1, 0.0)
        );
        // And the one with a body, too.
        assert!(matches!(
            choose(&people[0].signature, people[0].at, &there),
            Choice::Person(0, _)
        ));
    }

    #[test]
    fn the_whole_route_keeps_a_bodiless_persons_instances_whole() {
        let (_, faces) = two_people();
        // A body for the second person only; the first has none.
        let theirs = body([0.65, 0.1, 0.85, 0.95], [0.65, 0.1, 0.85, 0.95], 0.95);
        let inst = |b: [f32; 4]| body(b, b, 0.95);
        // A top under the first face, wider than the region the body
        // would fill; one on the second person; one straddling, more
        // on the second person than near the first.
        let top = inst([0.05, 0.3, 0.45, 0.8]);
        let other = inst([0.66, 0.3, 0.84, 0.8]);
        let straddling = inst([0.36, 0.5, 0.84, 0.6]);
        let mut sam = Fixed(vec![
            ("face", vec![face(faces[0], 0.9), face(faces[1], 0.85)]),
            ("person", vec![theirs]),
            ("upper body clothing", vec![top, other, straddling]),
        ]);
        let pieces = on_two(&mut sam, |sam, pic| {
            let people = people(sam, &table(), pic).unwrap();
            assert!(people[0].body.is_none());
            find(
                sam,
                &table(),
                pic,
                Route::Whole,
                "upper body clothing",
                Some(&people[0]),
            )
            .unwrap()
        });
        let m = &pieces[0].mask;
        assert_eq!((m.width, m.height), (400, 400));
        // Whole: past the region's sides (x under 0.1 and over 0.4) as
        // well as inside it, and down to its foot.
        for (x, y) in [(0.07, 0.55), (0.25, 0.55), (0.43, 0.55), (0.25, 0.78)] {
            let v = m.at((x * 400.0) as usize, (y * 400.0) as usize);
            assert!(v > 0.8, "({x}, {y}): {v}");
        }
        // Nothing of the other two.
        for (x, y) in [(0.75, 0.4), (0.6, 0.55), (0.75, 0.7)] {
            let v = m.at((x * 400.0) as usize, (y * 400.0) as usize);
            assert!(v < 0.1, "({x}, {y}): {v}");
        }
    }

    #[test]
    fn pairing_is_a_total_order_however_many_bodies() {
        // Thirty bodies a hair apart in value, so ties chain all the
        // way down: the smallest box among those within the tie of the
        // highest, and no panic from the sort.
        let n = 30;
        let bodies: Vec<Instance> = (0..n)
            .map(|k| {
                let v = 0.95 - 0.004 * k as f32;
                let side = 0.9 - 0.02 * ((k * 7) % n) as f32;
                body(
                    [0.05, 0.05, 0.05 + side, 0.05 + side],
                    [0.0, 0.0, 1.0, 1.0],
                    v,
                )
            })
            .collect();
        let refs: Vec<&Instance> = bodies.iter().collect();
        let faces = vec![[0.4, 0.4, 0.5, 0.5]; 3];
        let got = pair(&faces, &refs);
        // Within 0.01 of 0.95: bodies 0, 1 and 2; of those the
        // smallest box is body 2 (side 0.9 - 0.02 * 14).
        assert_eq!(got[0], Some(2));
        // Then the best untaken is body 0 (0.95), with only 1 in the
        // tie (3 is 0.012 below); the smaller of the two is 1. Last,
        // 0 alone.
        assert_eq!(got[1], Some(1));
        assert_eq!(got[2], Some(0));
        assert!(got.iter().all(|g| g.is_some()));
        let mut picked: Vec<_> = got.iter().flatten().collect();
        picked.dedup();
        assert_eq!(picked.len(), 3);
    }

    #[test]
    fn a_face_found_twice_is_one_person() {
        let a = face([0.2, 0.1, 0.3, 0.25], 0.9);
        let twice = face([0.205, 0.105, 0.305, 0.255], 0.7);
        let near = face([0.28, 0.1, 0.38, 0.25], 0.8);
        assert!(iou(a.bbox, twice.bbox) > 0.5);
        assert!(iou(a.bbox, near.bbox) < 0.5);
        let kept = distinct(vec![twice.clone(), a.clone(), near.clone()]);
        assert_eq!(kept, vec![a.clone(), near.clone()]);
        // Through `people`: one face found twice on a picture of one
        // person is one person, and decides alone.
        let one = body([0.15, 0.1, 0.35, 0.95], [0.15, 0.1, 0.35, 0.95], 0.95);
        let mut sam = Fixed(vec![("face", vec![a, twice]), ("person", vec![one])]);
        let people = on_two(&mut sam, |sam, pic| people(sam, &table(), pic).unwrap());
        assert_eq!(people.len(), 1);
        let mut v = people[0].signature.values().to_vec();
        v[7] += 2.0;
        let elsewhere = Signature::new(SignatureKind::Colors1, v).unwrap();
        let there: Vec<_> = people.iter().map(|p| (p.signature.clone(), p.at)).collect();
        assert!(matches!(
            choose(&elsewhere, [0.9, 0.9], &there),
            Choice::Person(0, _)
        ));
    }

    #[test]
    fn the_same_render_needs_a_known_kind_a_band_and_the_place() {
        // A later build's kind, its values all near each other: never
        // the same render, so a group is asked about.
        let later = |v: f32| {
            Signature::new(
                SignatureKind::Unknown("face-embedding-1".into()),
                vec![v; 512],
            )
            .unwrap()
        };
        assert!(!same_render(&later(0.05), &later(0.05)));
        assert_eq!(
            choose(
                &later(0.05),
                [0.3, 0.2],
                &[(later(0.04), [0.3, 0.2]), (later(0.06), [0.7, 0.2])]
            ),
            Choice::Ask { guess: None }
        );
        // Two signatures with no band: anyone's, never the same render.
        assert!(!same_render(&Signature::blank(), &Signature::blank()));
        assert_eq!(
            choose(
                &Signature::blank(),
                [0.3, 0.2],
                &[
                    (Signature::blank(), [0.3, 0.2]),
                    (Signature::blank(), [0.7, 0.2])
                ]
            ),
            Choice::Ask { guess: None }
        );
        // The same values far from where the face was: not the picture
        // the shape was made on, so a group is asked about, with them
        // as the guess.
        let red = sig([45.0, 60.0, 40.0], 0.05);
        let green = sig([50.0, -40.0, 30.0], 0.05);
        assert!(same_render(&red, &red));
        assert_eq!(
            choose(
                &red,
                [0.3, 0.2],
                &[(red.clone(), [0.7, 0.2]), (green.clone(), [0.3, 0.2])]
            ),
            Choice::Ask { guess: Some(0) }
        );
        // Within a face's width of it: decided.
        assert_eq!(
            choose(
                &red,
                [0.3, 0.2],
                &[(red.clone(), [0.33, 0.22]), (green, [0.7, 0.2])]
            ),
            Choice::Person(0, 0.0)
        );
    }

    #[test]
    fn the_whole_route_keeps_a_bodiless_persons_parts_under_a_merged_body() {
        let (_, faces) = two_people();
        // SAM merged both people into one body, which the first face
        // took; the second has none, and its top reaches past the
        // region a body under its face would fill.
        let merged = body([0.1, 0.05, 0.9, 0.95], [0.1, 0.05, 0.9, 0.95], 0.95);
        let top = body([0.55, 0.3, 0.95, 0.8], [0.55, 0.3, 0.95, 0.8], 0.95);
        let mut sam = Fixed(vec![
            ("face", vec![face(faces[0], 0.9), face(faces[1], 0.85)]),
            ("person", vec![merged]),
            ("upper body clothing", vec![top]),
        ]);
        let pieces = on_two(&mut sam, |sam, pic| {
            let people = people(sam, &table(), pic).unwrap();
            assert!(people[0].body.is_some() && people[1].body.is_none());
            find(
                sam,
                &table(),
                pic,
                Route::Whole,
                "upper body clothing",
                Some(&people[1]),
            )
            .unwrap()
        });
        let m = &pieces[0].mask;
        for (x, y) in [(0.57, 0.55), (0.75, 0.55), (0.93, 0.55)] {
            let v = m.at((x * 400.0) as usize, (y * 400.0) as usize);
            assert!(v > 0.8, "({x}, {y}): {v}");
        }
    }

    #[test]
    fn a_merged_body_does_not_give_one_person_the_others_top() {
        let (_, faces) = two_people();
        // SAM merged both people into one body, which the first face
        // took; each has a top of their own.
        let merged = body([0.1, 0.05, 0.9, 0.95], [0.1, 0.05, 0.9, 0.95], 0.95);
        let left = body([0.15, 0.3, 0.35, 0.8], [0.15, 0.3, 0.35, 0.8], 0.95);
        let right = body([0.65, 0.3, 0.85, 0.8], [0.65, 0.3, 0.85, 0.8], 0.95);
        let mut sam = Fixed(vec![
            ("face", vec![face(faces[0], 0.9), face(faces[1], 0.85)]),
            ("person", vec![merged]),
            ("upper body clothing", vec![left, right]),
        ]);
        let tops = on_two(&mut sam, |sam, pic| {
            let people = people(sam, &table(), pic).unwrap();
            assert!(people[0].body.is_some() && people[1].body.is_none());
            people
                .iter()
                .map(|p| {
                    find(
                        sam,
                        &table(),
                        pic,
                        Route::Whole,
                        "upper body clothing",
                        Some(p),
                    )
                    .unwrap()
                    .remove(0)
                    .mask
                })
                .collect::<Vec<_>>()
        });
        let at = |m: &Mask, x: f32, y: f32| m.at((x * 400.0) as usize, (y * 400.0) as usize);
        // Each keeps their own top, and not the other's.
        assert!(at(&tops[0], 0.25, 0.55) > 0.8);
        assert!(
            at(&tops[0], 0.75, 0.55) < 0.2,
            "{}",
            at(&tops[0], 0.75, 0.55)
        );
        assert!(at(&tops[1], 0.75, 0.55) > 0.8);
        assert!(at(&tops[1], 0.25, 0.55) < 0.2);
        // A body of one's own still keeps them to it.
        let own = body([0.15, 0.1, 0.35, 0.95], [0.15, 0.1, 0.35, 0.95], 0.95);
        assert_eq!(holds(&own.mask, faces.into_iter()), 1);
    }

    #[test]
    fn a_neighbors_top_inside_ones_body_stays_the_neighbors() {
        let (_, faces) = two_people();
        // Two bodies that overlap, one seated in front of the other;
        // the second's top reaches into the first's outline.
        let a = body([0.15, 0.1, 0.45, 0.95], [0.15, 0.1, 0.45, 0.95], 0.95);
        let b = body([0.35, 0.1, 0.85, 0.95], [0.35, 0.1, 0.85, 0.95], 0.95);
        let top_a = body([0.17, 0.3, 0.33, 0.6], [0.17, 0.3, 0.33, 0.6], 0.95);
        let top_b = body([0.37, 0.3, 0.83, 0.6], [0.37, 0.3, 0.83, 0.6], 0.95);
        let mut sam = Fixed(vec![
            ("face", vec![face(faces[0], 0.9), face(faces[1], 0.85)]),
            ("person", vec![a, b]),
            ("upper body clothing", vec![top_a, top_b]),
        ]);
        let tops = on_two(&mut sam, |sam, pic| {
            let people = people(sam, &table(), pic).unwrap();
            assert!(people.iter().all(|p| p.body.is_some()));
            people
                .iter()
                .map(|p| {
                    find(
                        sam,
                        &table(),
                        pic,
                        Route::Whole,
                        "upper body clothing",
                        Some(p),
                    )
                    .unwrap()
                    .remove(0)
                    .mask
                })
                .collect::<Vec<_>>()
        });
        let at = |m: &Mask, x: f32, y: f32| m.at((x * 400.0) as usize, (y * 400.0) as usize);
        assert!(at(&tops[0], 0.25, 0.45) > 0.8);
        assert!(
            at(&tops[0], 0.41, 0.45) < 0.2,
            "{}",
            at(&tops[0], 0.41, 0.45)
        );
        assert!(at(&tops[1], 0.6, 0.45) > 0.8);
        assert!(at(&tops[1], 0.25, 0.45) < 0.2);
    }

    #[test]
    fn a_near_copy_of_ones_own_body_does_not_take_ones_top() {
        let (_, faces) = two_people();
        // Her body found twice, the second a little larger and paired
        // with no face; her top is wider than the first.
        let a = body([0.15, 0.1, 0.35, 0.95], [0.15, 0.1, 0.35, 0.95], 0.95);
        let again = body([0.12, 0.08, 0.38, 0.95], [0.12, 0.08, 0.38, 0.95], 0.95);
        let b = body([0.65, 0.1, 0.85, 0.95], [0.65, 0.1, 0.85, 0.95], 0.95);
        let top = body([0.13, 0.3, 0.37, 0.6], [0.13, 0.3, 0.37, 0.6], 0.95);
        let mut sam = Fixed(vec![
            ("face", vec![face(faces[0], 0.9), face(faces[1], 0.85)]),
            ("person", vec![a, again, b]),
            ("upper body clothing", vec![top]),
        ]);
        let mask = on_two(&mut sam, |sam, pic| {
            let people = people(sam, &table(), pic).unwrap();
            // She took the smaller of the two.
            assert!(people[0].body.as_ref().unwrap().at(37, 150) < 0.5);
            find(
                sam,
                &table(),
                pic,
                Route::Whole,
                "upper body clothing",
                Some(&people[0]),
            )
            .unwrap()
            .remove(0)
            .mask
        });
        let v = mask.at(100, 180);
        assert!(v > 0.8, "her top kept: {v}");
    }

    /// Each person's Whole person mask on the two-person picture, and
    /// everyone's, with `bodies` as SAM's `person` and the faces given.
    fn whole_people(faces: Vec<Instance>, bodies: Vec<Instance>) -> (Vec<Mask>, Mask) {
        let mut sam = Fixed(vec![("face", faces), ("person", bodies)]);
        on_two(&mut sam, |sam, pic| {
            let people = people(sam, &table(), pic).unwrap();
            let mut ask = |who: Option<&Person>| {
                find(sam, &table(), pic, Route::Whole, PERSON, who)
                    .unwrap()
                    .remove(0)
                    .mask
            };
            let each = people.iter().map(|p| ask(Some(p))).collect();
            (each, ask(None))
        })
    }

    fn at(m: &Mask, x: f32, y: f32) -> f32 {
        m.at((x * 400.0) as usize, (y * 400.0) as usize)
    }

    #[test]
    fn a_whole_person_is_their_own_body() {
        let (_, faces) = two_people();
        let a = body([0.15, 0.05, 0.35, 0.95], [0.15, 0.05, 0.35, 0.95], 0.95);
        let b = body([0.65, 0.05, 0.85, 0.95], [0.65, 0.05, 0.85, 0.95], 0.95);
        // A body found twice, a little larger and no one's.
        let again = body([0.12, 0.03, 0.38, 0.95], [0.12, 0.03, 0.38, 0.95], 0.95);
        let (each, all) = whole_people(
            vec![face(faces[0], 0.9), face(faces[1], 0.85)],
            vec![a, b, again],
        );
        // Each is their own body, head to foot, and nothing of the
        // other's, nor of the copy's larger outline.
        for (m, (mine, theirs)) in each.iter().zip([(0.25, 0.75), (0.75, 0.25)]) {
            for y in [0.1, 0.5, 0.9] {
                assert!(at(m, mine, y) > 0.8, "{mine}, {y}: {}", at(m, mine, y));
                assert!(at(m, theirs, y) < 0.1);
            }
        }
        assert!(at(&each[0], 0.13, 0.5) < 0.2, "{}", at(&each[0], 0.13, 0.5));
        // All people: both.
        assert!(at(&all, 0.25, 0.5) > 0.8 && at(&all, 0.75, 0.5) > 0.8);
    }

    #[test]
    fn a_body_merged_over_two_people_is_shared_between_them() {
        let (_, faces) = two_people();
        let merged = body([0.1, 0.05, 0.9, 0.95], [0.1, 0.05, 0.9, 0.95], 0.95);
        let (each, all) = whole_people(
            vec![face(faces[0], 0.9), face(faces[1], 0.85)],
            vec![merged],
        );
        // Each their own side of it, the one with no body of their own
        // too, and no cell in both.
        assert!(at(&each[0], 0.25, 0.5) > 0.8 && at(&each[0], 0.75, 0.5) < 0.1);
        assert!(at(&each[1], 0.75, 0.5) > 0.8 && at(&each[1], 0.25, 0.5) < 0.1);
        assert!(at(&each[0], 0.25, 0.9) > 0.8 && at(&each[1], 0.75, 0.9) > 0.8);
        let both = each[0]
            .data
            .iter()
            .zip(&each[1].data)
            .filter(|&(&a, &b)| a > 0.5 && b > 0.5)
            .count();
        assert!(both < 400, "{both} pixels in both");
        assert!(at(&all, 0.5, 0.5) > 0.8);
    }

    #[test]
    fn a_whole_person_of_a_face_alone_is_their_face() {
        let (_, faces) = two_people();
        // A body for the first; the second's head is all SAM saw.
        let a = body([0.15, 0.05, 0.35, 0.95], [0.15, 0.05, 0.35, 0.95], 0.95);
        let (each, _) = whole_people(vec![face(faces[0], 0.9), face(faces[1], 0.85)], vec![a]);
        assert!(
            at(&each[1], 0.75, 0.17) > 0.8,
            "{}",
            at(&each[1], 0.75, 0.17)
        );
        assert!(at(&each[1], 0.75, 0.5) < 0.1 && at(&each[1], 0.25, 0.5) < 0.1);
    }

    /// With Subject's matte over both people, the one with a body takes
    /// its edge, and the head SAM saw alone keeps SAM's edge to the
    /// pixel: no collar of Subject's neck and shoulders under the chin.
    #[test]
    fn a_head_alone_keeps_sams_edge_beside_subjects_matte() {
        let (img, faces) = two_people();
        let a = body([0.15, 0.05, 0.35, 0.95], [0.15, 0.05, 0.35, 0.95], 0.95);
        let mut sam = Fixed(vec![
            ("face", vec![face(faces[0], 0.9), face(faces[1], 0.85)]),
            ("person", vec![a]),
        ]);
        let matte = mask_of(400, 400, |x, y| {
            (40..380).contains(&y) && ((60..140).contains(&x) || (260..340).contains(&x))
        });
        let luma = img.luma();
        let mut region = |r: Rect| crop(&img, r);
        let mut encodings = Encodings::new(2);
        let mut picture = Picture {
            preview: &img,
            luma: &luma,
            size: (400, 400),
            key: 1,
            region: &mut region,
            encodings: &mut encodings,
            matte: None,
        };
        let people = people(&mut sam, &table(), &mut picture).unwrap();
        assert!(people[0].body.is_some() && people[1].body.is_none());
        let mut each = |pic: &mut Picture<()>| -> Vec<Mask> {
            people
                .iter()
                .map(|p| {
                    find(&mut sam, &table(), pic, Route::Whole, PERSON, Some(p))
                        .unwrap()
                        .remove(0)
                        .mask
                })
                .collect()
        };
        let sams = each(&mut picture);
        picture.matte = Some(&matte);
        let subjects = each(&mut picture);
        assert_eq!(subjects[1], sams[1], "the head alone, SAM's edge");
        assert_ne!(subjects[0], sams[0], "the body, Subject's edge");
        let below = |m: &Mask| (105..125).map(|y| m.at(300, y)).fold(0f32, f32::max);
        assert!(
            below(&subjects[1]) < 0.1,
            "no collar: {}",
            below(&subjects[1])
        );
    }

    #[test]
    fn a_persons_mask_is_solid_with_its_gaps_kept() {
        // A body at 0.8, its glasses an unsure hole at 0.3 and a gap
        // between an arm and the body that reaches background.
        let m = mask_of(MASK, MASK, |x, y| {
            let body = (100..190).contains(&x) && (40..280).contains(&y);
            let glasses = (130..160).contains(&x) && (60..70).contains(&y);
            let gap = (110..118).contains(&x) && (150..220).contains(&y);
            body && !glasses && !gap
        });
        let mut m = m;
        for (i, v) in m.data.iter_mut().enumerate() {
            let (x, y) = (i % MASK, i / MASK);
            if *v > 0.5 {
                *v = 0.8;
            } else if (130..160).contains(&x) && (60..70).contains(&y) {
                *v = 0.3;
            } else if (110..118).contains(&x) && (150..220).contains(&y) {
                *v = if y == 185 { 0.02 } else { 0.3 };
            } else {
                *v = 0.0;
            }
        }
        let solid = harden(&fill_unsure(&m));
        let at = |x: usize, y: usize| solid.at(x, y);
        assert_eq!(at(150, 150), 1.0, "the body, solid");
        assert_eq!(at(145, 65), 1.0, "the glasses filled");
        assert!(at(114, 185) < 1e-6, "the gap kept");
        assert!(at(114, 160) < 1e-6, "all of it");
        assert_eq!(at(50, 50), 0.0, "the background");
    }

    /// Subject's edge on a whole person, on masks made by hand: a
    /// person at cells 100 to 190 across with an open gap by her arm,
    /// a second person touching her right side, and a matte that keeps
    /// her hair two cells past SAM's outline, finds a held thing six
    /// out, leaves a blotch in her body, closes her gap and runs on
    /// into her neighbor. At two pixels a cell.
    #[test]
    fn subject_gives_a_whole_person_its_edge_within_their_region() {
        const S: usize = 2;
        let (w, h) = (MASK * S, MASK * S);
        let cells = |f: &dyn Fn(usize, usize) -> f32| {
            Mask::new(
                MASK,
                MASK,
                (0..MASK * MASK).map(|i| f(i % MASK, i / MASK)).collect(),
            )
        };
        let her = |x: usize, y: usize| (100..190).contains(&x) && (40..280).contains(&y);
        let gap = |x: usize, y: usize| (110..118).contains(&x) && (150..220).contains(&y);
        let him = |x: usize, y: usize| (190..250).contains(&x) && (60..280).contains(&y);
        // An arm of hers off on its own, which Subject does not see.
        let stray = |x: usize, y: usize| (20..40).contains(&x) && (200..240).contains(&y);
        let sam = |mine: &dyn Fn(usize, usize) -> bool| {
            let m = cells(&|x, y| {
                if mine(x, y) && !gap(x, y) {
                    0.9
                } else if gap(x, y) && y != 185 {
                    0.3
                } else {
                    0.02
                }
            });
            let (filled, open) = fill_unsure_open(&m);
            (harden(&filled), open)
        };
        let (solid, open) = sam(&|x, y| her(x, y) || stray(x, y));
        assert!(open[185 * MASK + 114], "her gap is left open");
        let matte = cells(&|x, y| {
            if (98..100).contains(&x) && (40..60).contains(&y) {
                0.6 // hair
            } else if (90..94).contains(&x) && (100..120).contains(&y) {
                1.0 // a held thing
            } else if (140..150).contains(&x) && (200..210).contains(&y) {
                0.2 // a blotch
            } else if her(x, y) || him(x, y) {
                1.0
            } else {
                0.0
            }
        })
        .resampled(w, h);
        let edge = |solid: &Mask, keep: &[bool]| {
            let sams = solid.resampled(w, h);
            (subject_edge(solid, keep, &sams, &matte), sams)
        };
        let theirs: Vec<bool> = (0..MASK * MASK).map(|i| him(i % MASK, i / MASK)).collect();
        let keep: Vec<bool> = theirs.iter().zip(&open).map(|(&a, &b)| a || b).collect();
        let (out, sams) = edge(&solid, &keep);
        let at = |m: &Mask, x: usize, y: usize| m.at(x * S + 1, y * S + 1);
        assert!(at(&out, 98, 50) > 0.5, "her hair: {}", at(&out, 98, 50));
        assert!(at(&sams, 98, 50) < 0.1, "which SAM's edge loses");
        assert!(at(&out, 92, 110) < 1e-6, "the held thing, six out");
        assert!(at(&out, 145, 205) > 0.99, "the blotch filled");
        assert!(at(&out, 150, 150) > 0.99, "her body");
        assert!(at(&out, 114, 185) < 0.1, "the gap kept open");
        assert!(at(&out, 50, 50) < 1e-6, "the background");
        // Her stray arm, which Subject does not hold, keeps SAM's edge.
        for (x, y) in [(30, 220), (19, 220), (20, 220), (41, 230)] {
            let (a, b) = (at(&out, x, y), at(&sams, x, y));
            assert!((a - b).abs() < 1e-6, "({x}, {y}): {a} against {b}");
        }
        // Into him: SAM's split, to the pixel.
        for x in 189..200 {
            for y in [100, 200] {
                let (a, b) = (at(&out, x, y), at(&sams, x, y));
                assert!((a - b).abs() < 1e-6, "({x}, {y}): {a} against {b}");
            }
        }
        // And he, given her as someone else's, shares no pixel with her
        // that SAM's edges did not already share.
        let (his, _) = sam(&him);
        let mine: Vec<bool> = solid.data.iter().map(|&v| v > CUT).collect();
        let (out_him, sams_him) = edge(&his, &mine);
        let shared = |a: &Mask, b: &Mask| {
            a.data
                .iter()
                .zip(&b.data)
                .filter(|&(&p, &q)| p > 0.5 && q > 0.5)
                .count()
        };
        assert_eq!(shared(&out, &out_him), shared(&sams, &sams_him));
        // No matte held anywhere: SAM's edge, unchanged.
        let none = Mask::new(w, h, vec![0.0; w * h]);
        let sams = solid.resampled(w, h);
        assert_eq!(subject_edge(&solid, &keep, &sams, &none), sams);
    }

    #[test]
    fn cells_to_counts_cells_to_the_nearest_on() {
        let mut on = vec![false; 7 * 5];
        on[2 * 7 + 3] = true;
        let d = cells_to(&on, 7, 5);
        assert_eq!(d[2 * 7 + 3], 0.0);
        assert_eq!(d[2 * 7 + 5], 2.0);
        assert!((d[0] - (1.0 + 2.0 * std::f32::consts::SQRT_2)).abs() < 1e-6);
        assert!(cells_to(&[false; 4], 2, 2).iter().all(|d| d.is_infinite()));
    }

    #[test]
    fn an_encoding_can_go_to_the_worker() {
        fn send<T: Send>() {}
        send::<Encoding>();
        send::<Sam3>();
    }
}
