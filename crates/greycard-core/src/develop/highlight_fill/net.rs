//! The learned highlight fill's network: its weights, the context
//! encoder over the thumbnail, the sampling of the encoder's map at
//! each region's point, and the per-region MLP (notes §719 section 2;
//! `scripts/highlight-fill-oracle/SPEC.md` stages 8 and 9, after the
//! trial's `hl2.Context` and `hl6.RegionNet6`).
//!
//! Core names no model: the caller hands in [`FillWeights`] (built
//! into greycard-ai per version) and the version's [`FillLook`].
//!
//! Everything is f32 in plain loops, without fused multiply-adds, so
//! the result is the same bits on every machine and at any thread
//! count: each output is accumulated in one fixed order (stated at
//! [`FillWeights::encode`] and [`FillWeights::outputs`]), and threads
//! only split the outputs between them. torch sums in its own BLAS and
//! convolution orders, so we match it to SPEC's tolerances, not to the
//! bit.

use rayon::prelude::*;

/// Region features the MLP reads (SPEC stage 9, in that order).
pub const FEATURES: usize = 20;
/// The encoder map's channels, sampled at each region's point.
pub const CONTEXT: usize = 32;
/// The MLP's hidden width.
pub const HIDDEN: usize = 128;
/// The numbers a region gets (SPEC stage 10: r0 to r11).
pub const OUTPUTS: usize = 12;
/// The thumbnail's channels: log2 of the mean of x in three, then the
/// share of pixels with any channel clipped and the share fully clipped.
pub const THUMB_CHANNELS: usize = 5;
/// Every weight and bias of the network.
pub const PARAMETERS: usize = 57_964;

/// The tensors by their PyTorch state-dict names and shapes, in the
/// order a blob stores them. Convolutions are (out, in, 3, 3), linears
/// (out, in), applied as x·Wᵀ + b.
pub const TENSORS: [(&str, &[usize]); 16] = [
    ("ctx.net.0.weight", &[16, 5, 3, 3]),
    ("ctx.net.0.bias", &[16]),
    ("ctx.net.2.weight", &[32, 16, 3, 3]),
    ("ctx.net.2.bias", &[32]),
    ("ctx.net.4.weight", &[32, 32, 3, 3]),
    ("ctx.net.4.bias", &[32]),
    ("ctx.net.6.weight", &[32, 32, 3, 3]),
    ("ctx.net.6.bias", &[32]),
    ("ctx.net.8.weight", &[32, 32, 3, 3]),
    ("ctx.net.8.bias", &[32]),
    ("mlp.0.weight", &[HIDDEN, FEATURES + CONTEXT]),
    ("mlp.0.bias", &[HIDDEN]),
    ("mlp.2.weight", &[HIDDEN, HIDDEN]),
    ("mlp.2.bias", &[HIDDEN]),
    ("mlp.4.weight", &[OUTPUTS, HIDDEN]),
    ("mlp.4.bias", &[OUTPUTS]),
];

/// The encoder's five 3x3 convolutions: (stride, dilation). Each pads
/// by its dilation with zeros, so a stride of 1 keeps the size and a
/// stride of 2 gives ceil(n / 2).
const ENCODER: [(usize, usize); 5] = [(1, 1), (2, 1), (1, 2), (2, 1), (1, 2)];

/// The single-blob weights format's first eight bytes.
pub const MAGIC: [u8; 8] = *b"GCHLFILL";
/// The single-blob weights format's version, after [`MAGIC`].
pub const FORMAT_VERSION: u32 = 1;

/// One named tensor: f32 values in C (row-major) order.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub name: String,
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

/// Why weights were refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WeightsError {
    #[error("not highlight fill weights (wrong magic)")]
    Magic,
    #[error("highlight fill weights format {0}, this build reads {FORMAT_VERSION}")]
    Version(u32),
    #[error("the highlight fill weights end early")]
    Truncated,
    #[error("{0} bytes after the highlight fill weights' last tensor")]
    Trailing(usize),
    #[error("the highlight fill weights claim {0} tensors")]
    Count(u32),
    #[error("a highlight fill tensor's name is not UTF-8")]
    Name,
    #[error("unknown highlight fill tensor {0:?}")]
    Unknown(String),
    #[error("highlight fill tensor {0:?} given twice")]
    Duplicate(String),
    /// A blob holds the tensors in [`TENSORS`]' order, so that every
    /// blob read writes back the same bytes.
    #[error("highlight fill tensor {found:?} where {expected:?} belongs")]
    Order {
        expected: &'static str,
        found: String,
    },
    #[error("highlight fill tensor {0:?} missing")]
    Missing(&'static str),
    #[error("highlight fill tensor {name:?} has shape {found:?}, not {expected:?}")]
    Shape {
        name: String,
        expected: Vec<usize>,
        found: Vec<usize>,
    },
    #[error("highlight fill tensor {name:?} holds {found} values for {expected}")]
    Length {
        name: String,
        expected: usize,
        found: usize,
    },
    /// A NaN or an infinity would reach every region's outputs, so
    /// weights must be finite.
    #[error("highlight fill tensor {0:?} holds a value that is not finite")]
    NotFinite(String),
}

/// A weight and its bias.
#[derive(Debug, Clone, PartialEq)]
struct Layer {
    weight: Vec<f32>,
    bias: Vec<f32>,
}

/// The network's weights, every shape checked: the context encoder's
/// five convolutions and the MLP's three linears.
#[derive(Debug, Clone, PartialEq)]
pub struct FillWeights {
    encoder: [Layer; 5],
    mlp: [Layer; 3],
}

/// A version's look constants: the settings fixed at inference beside
/// the trained weights, which belong to the version as the weights do
/// (notes §719, "Untrained settings"; `oracle.py`'s constants block).
/// The values are as `oracle.py` writes them; the synthesis rounds
/// each where SPEC.md says (its "Precision with an arbitrary f").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillLook {
    /// The core's ab rolled toward white with its rise: SPEC stage 10,
    /// keep = 1 − whiten · mix · s2 · tie (`CORE_WHITEN`), the whitening
    /// rounded to f32 and multiplied left to right in f32 (SPEC,
    /// "Precision with an arbitrary f").
    pub core_whiten: f64,
    /// The core rise scaled past the tie: SPEC stage 10,
    /// gain_h = g + mix · H2 · s2 · (1 + (boost − 1) · tie)
    /// (`CORE_BOOST`), boost − 1 computed in f64 and then rounded to f32
    /// (f32 0.4 for 1.4; SPEC, "Precision with an arbitrary f").
    pub core_boost: f64,
    /// The harmonic continuation is known only on unclipped cells
    /// brighter than this: SPEC stage 6, fixed = (1 − fa) · (yh > it)
    /// (`HARM_BRIGHT`).
    pub harmonic_bright: f64,
    /// The core depth in full-size px over which boost and whitening
    /// ramp in, at f = 1; a reduced develop divides both by f (SPEC
    /// constants, `DEPTH_TIE`).
    pub depth_tie: [f64; 2],
    /// The gain's hand-over blur σ in half-size cells, at f = 1; a
    /// reduced develop takes max(σ / f, 1) (SPEC constants,
    /// `MIX_SIGMA`).
    pub gain_sigma: f64,
    /// Where the network's fields may reach, past what it was trained
    /// on: None (hl-1) everywhere the synthesis makes them.
    pub reach: Option<Reach>,
}

/// hl-2's two rules over hl-1's fields (notes §719 §7, the failures on
/// large regions): the network trained on 384 px crops never saw a
/// region deeper than about 192 px, nor learned the partly clipped band
/// of one larger than that. Every length is in full-size px at f = 1;
/// a reduced develop divides each by f, as the depth tie.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reach {
    /// The size gate: a region's contribution is whole up to depth
    /// `gate[0]` (its `depth0`, the network's depth feature) and none
    /// from `gate[1]`, a smoothstep between; a region at or past
    /// `gate[1]` keeps the classical fill to the bit.
    pub gate: [f64; 2],
    /// Core only: the fields are zero off the fully clipped cells and
    /// rise from zero at the core's edge, by smoothstep over this many
    /// px of Euclidean distance into the core, or over the region's own
    /// deepest core distance where that is less, so a small core keeps
    /// its whole fill at its center. None: the gate alone, the fields
    /// of a region under it hl-1's.
    pub ramp: Option<f64>,
    /// Neighbors share a gate: a region takes the smallest gate among
    /// itself and every region with a cell within this many px of one
    /// of its own (a square reach, chessboard), so the pieces of one
    /// blown sky broken by branches are gated as the sky is. None: each
    /// region its own gate.
    pub near: Option<f64>,
}

/// The encoder's output: [`CONTEXT`] planes, channels first, at about a
/// quarter of the thumbnail's size.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMap {
    height: usize,
    width: usize,
    /// (CONTEXT, height, width), C order.
    data: Vec<f32>,
}

/// One row of the per-region pass: a region, or the outside.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionRow {
    /// Row 0 when it is id 0: its outputs are zeroed (SPEC stage 9).
    pub outside: bool,
    pub features: [f32; FEATURES],
    /// Where to sample the map, in grid_sample's [-1, 1] coordinates
    /// ([`grid_point`]).
    pub point: [f32; 2],
}

/// Exact GELU, 0.5x(1 + erf(x/√2)), in torch's order:
/// (x · 0.5) · (1 + erf(x · 1/√2)), f32 throughout.
pub fn gelu(x: f32) -> f32 {
    x * 0.5 * (1.0 + libm::erff(x * std::f32::consts::FRAC_1_SQRT_2))
}

/// The sampling extent of a box in its frame (SPEC stage 9's `where`):
/// [box_x / fw, box_y / fh, 2·Wh / fw, 2·Hh / fh], each an f64 division
/// rounded to f32. `half` is the padded half-size grid (Wh, Hh), so an
/// odd size overstates the extent by a pixel, as trained.
pub fn frame_where(box_xy: [u32; 2], frame: [u32; 2], half: [usize; 2]) -> [f32; 4] {
    let (fw, fh) = (f64::from(frame[0]), f64::from(frame[1]));
    [
        (f64::from(box_xy[0]) / fw) as f32,
        (f64::from(box_xy[1]) / fh) as f32,
        (2.0 * half[0] as f64 / fw) as f32,
        (2.0 * half[1] as f64 / fh) as f32,
    ]
}

/// A region's sampling point from its centroid (cx, cy) in [0, 1] of
/// the half grid: g = (where₀ + where₂·cx)·2 − 1, and likewise for y,
/// in f32.
pub fn grid_point(where_: [f32; 4], centroid: [f32; 2]) -> [f32; 2] {
    [
        (where_[0] + where_[2] * centroid[0]) * 2.0 - 1.0,
        (where_[1] + where_[3] * centroid[1]) * 2.0 - 1.0,
    ]
}

impl FillWeights {
    /// Weights from named tensors, every one of [`TENSORS`] exactly
    /// once with its shape, every value finite.
    pub fn from_tensors(tensors: Vec<Tensor>) -> Result<Self, WeightsError> {
        let mut slots: [Option<Vec<f32>>; 16] = Default::default();
        for t in tensors {
            let i = index(&t.name)?;
            check(i, &t.shape)?;
            let expected = TENSORS[i].1.iter().product::<usize>();
            if t.data.len() != expected {
                return Err(WeightsError::Length {
                    name: t.name,
                    expected,
                    found: t.data.len(),
                });
            }
            if !t.data.iter().all(|v| v.is_finite()) {
                return Err(WeightsError::NotFinite(t.name));
            }
            if slots[i].is_some() {
                return Err(WeightsError::Duplicate(t.name));
            }
            slots[i] = Some(t.data);
        }
        let mut take = |i: usize| slots[i].take().ok_or(WeightsError::Missing(TENSORS[i].0));
        let mut layer = |l: usize| -> Result<Layer, WeightsError> {
            Ok(Layer {
                weight: take(2 * l)?,
                bias: take(2 * l + 1)?,
            })
        };
        Ok(Self {
            encoder: [layer(0)?, layer(1)?, layer(2)?, layer(3)?, layer(4)?],
            mlp: [layer(5)?, layer(6)?, layer(7)?],
        })
    }

    /// The tensors, by name, in [`TENSORS`]' order.
    pub fn tensors(&self) -> Vec<Tensor> {
        TENSORS
            .iter()
            .enumerate()
            .map(|(i, (name, shape))| Tensor {
                name: (*name).to_string(),
                shape: shape.to_vec(),
                data: self.data(i).to_vec(),
            })
            .collect()
    }

    fn data(&self, i: usize) -> &[f32] {
        let layer = match i / 2 {
            l @ 0..5 => &self.encoder[l],
            l => &self.mlp[l - 5],
        };
        if i.is_multiple_of(2) {
            &layer.weight
        } else {
            &layer.bias
        }
    }

    /// The single-blob format, all little-endian:
    ///
    /// - [`MAGIC`] (8 bytes), [`FORMAT_VERSION`] (u32), the tensor
    ///   count (u32);
    /// - per tensor: the name's length (u16) and its UTF-8 bytes, the
    ///   number of dimensions (u8), each dimension (u32), then the
    ///   values (f32, C order).
    ///
    /// `scripts/highlight-fill-oracle/export_weights.py` writes the same
    /// bytes from a checkpoint.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + 64 * TENSORS.len() + 4 * PARAMETERS);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(TENSORS.len() as u32).to_le_bytes());
        for (i, (name, shape)) in TENSORS.iter().enumerate() {
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.push(shape.len() as u8);
            for &d in shape.iter() {
                out.extend_from_slice(&(d as u32).to_le_bytes());
            }
            for v in self.data(i) {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    /// Reads [`FillWeights::to_bytes`]' format, the tensors in
    /// [`TENSORS`]' order, so `to_bytes` gives back the very bytes of
    /// every blob it accepts. Never panics, and
    /// allocates only for a tensor whose name and shape are known and
    /// whose values the buffer holds, so a hostile blob costs at most
    /// its own size.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, WeightsError> {
        let mut r = Reader(bytes);
        if r.take(8)? != MAGIC {
            return Err(WeightsError::Magic);
        }
        let version = r.u32()?;
        if version != FORMAT_VERSION {
            return Err(WeightsError::Version(version));
        }
        let count = r.u32()?;
        if count as usize > TENSORS.len() {
            return Err(WeightsError::Count(count));
        }
        let mut tensors = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let len = usize::from(r.u16()?);
            let name = std::str::from_utf8(r.take(len)?).map_err(|_| WeightsError::Name)?;
            let i = index(name)?;
            let expected = TENSORS[tensors.len()].0;
            if i != tensors.len() {
                return Err(WeightsError::Order {
                    expected,
                    found: name.to_string(),
                });
            }
            let ndim = usize::from(r.take(1)?[0]);
            let mut shape = Vec::with_capacity(ndim.min(4));
            for _ in 0..ndim {
                shape.push(r.u32()? as usize);
            }
            check(i, &shape)?;
            let n = TENSORS[i].1.iter().product::<usize>();
            let data = r
                .take(4 * n)?
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&b| f32::from_le_bytes(b))
                .collect();
            tensors.push(Tensor {
                name: name.to_string(),
                shape,
                data,
            });
        }
        if !r.0.is_empty() {
            return Err(WeightsError::Trailing(r.0.len()));
        }
        Self::from_tensors(tensors)
    }

    /// Every weight and bias: [`PARAMETERS`].
    pub fn parameters(&self) -> usize {
        (0..TENSORS.len()).map(|i| self.data(i).len()).sum()
    }

    /// The context encoder (SPEC stage 8) over a thumbnail of
    /// `height` × `width` × [`THUMB_CHANNELS`], channels last: the first
    /// three channels divided by 4, then five 3x3 convolutions with
    /// zero padding, each followed by exact GELU ([`gelu`]).
    ///
    /// Each output value is summed in f32 from +0 over the input
    /// channels, then the kernel's rows, then its columns, all
    /// ascending, skipping taps in the padding; the bias is added last,
    /// then GELU. Threads split the output channels.
    ///
    /// # Panics
    /// If the thumbnail is empty or its length is not
    /// `height · width · 5`.
    pub fn encode(&self, thumb: &[f32], height: usize, width: usize) -> ContextMap {
        assert!(height > 0 && width > 0, "an empty thumbnail");
        assert_eq!(thumb.len(), height * width * THUMB_CHANNELS);
        let plane = height * width;
        let mut x = vec![0.0f32; THUMB_CHANNELS * plane];
        for (p, px) in thumb.as_chunks::<THUMB_CHANNELS>().0.iter().enumerate() {
            for (c, &v) in px.iter().enumerate() {
                x[c * plane + p] = if c < 3 { v / 4.0 } else { v };
            }
        }
        let (mut h, mut w) = (height, width);
        for (layer, &(stride, dilation)) in self.encoder.iter().zip(&ENCODER) {
            (x, h, w) = conv3(layer, &x, h, w, stride, dilation);
        }
        ContextMap {
            height: h,
            width: w,
            data: x,
        }
    }

    /// The MLP (SPEC stage 9) on one region: [features ‖ context] →
    /// 128 → GELU → 128 → GELU → [`OUTPUTS`], before any zeroing.
    ///
    /// Each output is summed in f32 from +0 over its inputs in
    /// ascending order, then the bias is added.
    pub fn outputs(&self, features: &[f32; FEATURES], context: &[f32; CONTEXT]) -> [f32; OUTPUTS] {
        let mut input = [0.0f32; FEATURES + CONTEXT];
        input[..FEATURES].copy_from_slice(features);
        input[FEATURES..].copy_from_slice(context);
        let mut a = [0.0f32; HIDDEN];
        linear(&self.mlp[0], &input, &mut a);
        a.iter_mut().for_each(|v| *v = gelu(*v));
        let mut b = [0.0f32; HIDDEN];
        linear(&self.mlp[1], &a, &mut b);
        b.iter_mut().for_each(|v| *v = gelu(*v));
        let mut out = [0.0f32; OUTPUTS];
        linear(&self.mlp[2], &b, &mut out);
        out
    }

    /// Every row's 12 numbers: the map sampled at its point, the MLP,
    /// and the outside row's outputs zeroed (SPEC stage 9). Threads
    /// split the rows; the result is in the rows' order.
    pub fn region_outputs(&self, map: &ContextMap, rows: &[RegionRow]) -> Vec<[f32; OUTPUTS]> {
        rows.par_iter()
            .map(|row| {
                if row.outside {
                    [0.0; OUTPUTS]
                } else {
                    self.outputs(&row.features, &map.sample(row.point))
                }
            })
            .collect()
    }
}

impl ContextMap {
    /// A map from its planes, (CONTEXT, height, width) in C order: None
    /// when it is empty or the length is not CONTEXT · height · width.
    pub fn new(height: usize, width: usize, data: Vec<f32>) -> Option<Self> {
        let n = height.checked_mul(width)?.checked_mul(CONTEXT)?;
        (n > 0 && data.len() == n).then_some(Self {
            height,
            width,
            data,
        })
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn width(&self) -> usize {
        self.width
    }

    /// (CONTEXT, height, width), C order.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// The map at `point`, as torch's CPU `grid_sample` takes it
    /// (bilinear, border padding, align_corners false; SPEC stage 9):
    /// ix = (g + 1)·(w/2) − 0.5 clamped to [0, w − 1], weights from the
    /// floor's distances, and per channel nw·vnw + ne·vne + sw·vsw +
    /// se·vse in that order, a neighbor past the last row or column
    /// read as 0 (its weight is 0 there). A NaN point samples the
    /// corner (0, 0).
    pub fn sample(&self, point: [f32; 2]) -> [f32; CONTEXT] {
        let at = |g: f32, n: usize| {
            let v = (g + 1.0) * (n as f32 / 2.0) - 0.5;
            v.max(0.0).min((n - 1) as f32)
        };
        let (ix, iy) = (at(point[0], self.width), at(point[1], self.height));
        let (x0, y0) = (ix.floor(), iy.floor());
        let (w, n) = (ix - x0, iy - y0);
        let (e, s) = (1.0 - w, 1.0 - n);
        let (nw, ne, sw, se) = (s * e, s * w, n * e, n * w);
        let (x0, y0) = (x0 as usize, y0 as usize);
        let (x1, y1) = (x0 + 1, y0 + 1);
        let plane = self.height * self.width;
        let mut out = [0.0f32; CONTEXT];
        for (c, o) in out.iter_mut().enumerate() {
            let p = &self.data[c * plane..(c + 1) * plane];
            let v = |y: usize, x: usize| {
                if y < self.height && x < self.width {
                    p[y * self.width + x]
                } else {
                    0.0
                }
            };
            *o = v(y0, x0) * nw + v(y0, x1) * ne + v(y1, x0) * sw + v(y1, x1) * se;
        }
        out
    }
}

fn index(name: &str) -> Result<usize, WeightsError> {
    TENSORS
        .iter()
        .position(|(n, _)| *n == name)
        .ok_or_else(|| WeightsError::Unknown(name.to_string()))
}

fn check(i: usize, shape: &[usize]) -> Result<(), WeightsError> {
    if shape != TENSORS[i].1 {
        return Err(WeightsError::Shape {
            name: TENSORS[i].0.to_string(),
            expected: TENSORS[i].1.to_vec(),
            found: shape.to_vec(),
        });
    }
    Ok(())
}

/// A cursor over a blob that never reads past its end.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WeightsError> {
        if n > self.0.len() {
            return Err(WeightsError::Truncated);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u16(&mut self) -> Result<u16, WeightsError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, WeightsError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// x·Wᵀ + b for one input vector.
fn linear(layer: &Layer, input: &[f32], out: &mut [f32]) {
    for (o, (row, b)) in out
        .iter_mut()
        .zip(layer.weight.chunks_exact(input.len()).zip(&layer.bias))
    {
        let mut acc = 0.0f32;
        for (w, x) in row.iter().zip(input) {
            acc += w * x;
        }
        *o = acc + b;
    }
}

/// The outputs o in 0..n_out whose input o·stride + off lies in
/// 0..n_in, as a range.
fn valid(n_out: usize, n_in: usize, stride: usize, off: isize) -> std::ops::Range<usize> {
    let s = stride as isize;
    let lo = if off >= 0 { 0 } else { (-off + s - 1) / s };
    let room = n_in as isize - off;
    let hi = if room <= 0 { 0 } else { (room + s - 1) / s };
    (lo as usize).min(n_out)..(hi as usize).min(n_out)
}

/// A 3x3 convolution padded by its dilation with zeros, then GELU:
/// channels-first planes in and out.
fn conv3(
    layer: &Layer,
    x: &[f32],
    h: usize,
    w: usize,
    stride: usize,
    dilation: usize,
) -> (Vec<f32>, usize, usize) {
    let cin = x.len() / (h * w);
    let pad = dilation;
    let oh = (h + 2 * pad - 2 * dilation - 1) / stride + 1;
    let ow = (w + 2 * pad - 2 * dilation - 1) / stride + 1;
    let mut out = vec![0.0f32; layer.bias.len() * oh * ow];
    out.par_chunks_mut(oh * ow)
        .enumerate()
        .for_each(|(oc, plane)| {
            let kernel = &layer.weight[oc * cin * 9..(oc + 1) * cin * 9];
            for ic in 0..cin {
                let src = &x[ic * h * w..(ic + 1) * h * w];
                for ky in 0..3 {
                    let offy = (ky * dilation) as isize - pad as isize;
                    for kx in 0..3 {
                        let offx = (kx * dilation) as isize - pad as isize;
                        let k = kernel[ic * 9 + ky * 3 + kx];
                        let xs = valid(ow, w, stride, offx);
                        for oy in valid(oh, h, stride, offy) {
                            let iy = (oy * stride) as isize + offy;
                            let row = &src[iy as usize * w..(iy as usize + 1) * w];
                            let orow = &mut plane[oy * ow..(oy + 1) * ow];
                            for ox in xs.clone() {
                                let ix = ((ox * stride) as isize + offx) as usize;
                                orow[ox] += k * row[ix];
                            }
                        }
                    }
                }
            }
            let b = layer.bias[oc];
            for v in plane.iter_mut() {
                *v = gelu(*v + b);
            }
        });
    (out, oh, ow)
}

#[cfg(test)]
mod tests;
