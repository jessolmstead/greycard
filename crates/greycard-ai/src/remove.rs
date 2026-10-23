//! Remove: a hole made up by RORem (Li et al., CVPR 2025), SDXL
//! inpainting fine-tuned on removal pairs, where the eraser's LaMa
//! ([`crate::fill`]) would smear. On WebGPU alone: on the CPU a
//! removal takes minutes, so the CPU keeps LaMa.
//!
//! The arithmetic is diffusers' SDXL inpainting pipeline at guidance
//! 1.0 and the empty prompt, which RORem was trained on, so there is
//! one UNet call a step and no text encoder: the prompt's embeddings
//! are constants in the store ([`crate::registry::REMOVE`]). A window
//! of the picture, no side over [`MAX`], is mirrored out to a square
//! of 512 or 1024 ([`size_for`]), the sizes the VAE graphs are
//! exported at. The VAE encoder makes latents of the window and of
//! the window with the hole blanked (the latent mean, times
//! [`SCALE`]); the hole is taken to the latents' grid ([`latent_mask`]);
//! the UNet sees those two and the noisy latents, nine channels; Euler
//! discrete steps over the model's own schedule ([`schedule`]: scaled
//! linear betas, leading spacing, offset 1) from [`STRENGTH`] 0.99,
//! which is 24 UNet calls at [`STEPS`] 25; the decoder brings the
//! latents back. The noise comes from the seed through SplitMix64 and
//! Box-Muller ([`normals`]), so a seed makes the same fill each time
//! and three seeds make three variations.
//!
//! The answer is the window under the hole alone, weighted by the
//! hole's own values ([`composite`]): the retouch's blend feathers it
//! with the coverage's edge, as it does LaMa's. The decoder gives back
//! the whole window, and how far it strays from the picture outside
//! the hole is the check on the WebGPU provider, which answers a
//! device out of memory with finite garbage rather than an error.
//!
//! Three sessions, each 5.1 GB (UNet) or up to 6 GB at its peak
//! (decoder at 1024): all open at once is 12.2 GB of the card, one at
//! a time about 7 GB for a few seconds more ([`Memory`]). Neither ONNX
//! Runtime's WebGPU provider nor wgpu says how much memory the adapter
//! has, so one at a time is the default and `GREYCARD_REMOVE_RESIDENT=1`
//! keeps them open.
//!
//! The sessions keep the provider's default buffer cache, as every
//! session of the process does (`runtime`'s provider dispatch says
//! why): in the editor, after the denoiser, a 1024 removal one at a
//! time with the decoder whole peaks 7.3 to 7.6 GB over what was
//! there, 9.2 to 9.5 GB over the desktop. The decoder sets that peak,
//! so at 1024 it runs as the 512 graph over nine overlapping tiles
//! ([`Vae`], [`blend`]), and the UNet then sets it, 0.9 to 1.2 GB
//! lower; `GREYCARD_REMOVE_TILED=0` runs it whole. Alone in a process
//! the UNet sets the peak either way, 6.8 GB.

use std::time::Instant;

use half::f16;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{Tensor, TensorElementType};

use crate::fill::{implausible, mirror};
use crate::image::Rgbf;
use crate::registry::{File, REMOVE};
use crate::runtime::{self, Error, Provider, Result};
use crate::store::Store;

/// The longest side of a window, the larger VAE export's.
pub const MAX: usize = 1024;

/// The square sizes the graphs are exported at.
pub const SIZES: [usize; 2] = [512, 1024];

/// The side of the decoder's tiles at 1024, the smaller export's:
/// three tiles a side, 256 pixels apart.
pub const TILE: usize = 512;
/// How far neighboring tiles overlap, where their weights cross.
pub const OVERLAP: usize = 256;

/// SDXL's VAE latent scaling factor.
pub const SCALE: f32 = 0.13025;

/// The schedule's length, and how far into it the image is noised:
/// at 0.99 the first step is skipped and 24 UNet calls are made.
pub const STEPS: usize = 25;
pub const STRENGTH: f64 = 0.99;

/// How far, on average, the decoder's answer outside the hole may
/// stray from the picture before it is taken for garbage, for a window
/// whose [`Drift::detail`] is `detail`. The VAE round trip and the
/// regenerated outside lose fine detail, so how far they stray goes
/// with how much there is: measured end to end, a hole of a quarter of
/// the window in the middle, from 0.014 off on a shore at 1024 (detail
/// 0.013) to 0.068 on the busiest 512 crop of a set of 29 previews
/// (detail 0.064), off over detail between 0.93 and 1.11 on all six. A
/// fixed bound would sit either under a busy window's honest answer or
/// far over a smooth one's garbage; twice the detail and a floor of
/// 0.02 for a flat sky keeps an honest answer under half of it, and an
/// unrelated picture, which a card out of memory gives, off by 0.2 or
/// more, over it.
fn bound(detail: f32) -> f32 {
    0.02 + 2.0 * detail
}

/// The prompt's embeddings' shapes, as the files hold them.
const PROMPT: usize = 77 * 2048;
const POOLED: usize = 1280;

/// Which of the model's graphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Encoder,
    Unet,
    Decoder,
}

/// How the three sessions share the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Memory {
    /// All three kept open for the size last used: 12.2 GB at 1024,
    /// about 11 s a removal.
    Resident,
    /// Each opened for its part and dropped after: about 7 GB at
    /// 1024, about 13 s a removal.
    OneAtATime,
}

impl Memory {
    /// What `GREYCARD_REMOVE_RESIDENT` asks for: resident at `1`, one
    /// at a time otherwise, since the adapter's memory is not known.
    pub fn from_env(var: Option<&str>) -> Self {
        if var == Some("1") {
            Memory::Resident
        } else {
            Memory::OneAtATime
        }
    }
}

/// How the VAE's decoder runs on a square larger than [`TILE`]. The
/// encoder always runs whole: its peak at 1024 is under the UNet's,
/// so tiles would save nothing, and they cost the hole 3.8 dB against
/// the reference, since each tile's group norms see only the tile and
/// the UNet carries the small change in the latents into other detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vae {
    /// The decoder exported at the square's size, whole: at 1024,
    /// after the denoiser, it sets the removal's peak.
    Whole,
    /// The [`TILE`] decoder over overlapping tiles, blended across the
    /// overlap by a linear ramp ([`starts`], [`blend`]): at 1024 the
    /// UNet sets the peak, about 1 GB lower, for 0.6 s more, and the
    /// answer is the whole decoder's to 45 dB.
    Tiled,
}

impl Vae {
    /// What `GREYCARD_REMOVE_TILED` asks for: tiled at `1`, whole at
    /// `0`, and [`Vae::DEFAULT`] otherwise.
    pub fn from_env(var: Option<&str>) -> Self {
        match var {
            Some("1") => Vae::Tiled,
            Some("0") => Vae::Whole,
            _ => Vae::DEFAULT,
        }
    }

    /// Tiled: the same answer to 45 dB for a gigabyte less at the
    /// peak, measured in the editor.
    pub const DEFAULT: Vae = Vae::Tiled;
}

/// One removal's parts, in seconds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Times {
    /// The square the window was run at.
    pub size: usize,
    /// Whether the decoder ran over [`TILE`] tiles ([`Vae::Tiled`]).
    pub tiled: bool,
    /// Opening sessions during the removal.
    pub loads: f64,
    /// The encoder's two runs, the image and the image with the hole
    /// blanked.
    pub encode: f64,
    /// The UNet calls, and how many there were.
    pub steps: f64,
    pub unet_calls: usize,
    /// The decoder's run.
    pub decode: f64,
    /// The whole removal, loads included.
    pub total: f64,
    /// How far the decoder's answer outside the hole was from the
    /// picture, and the picture's own detail there ([`Drift`]).
    pub drift: Drift,
}

/// The decoder's answer outside the hole against the picture: the
/// mean absolute difference, and the window's mean local gradient
/// there, the mean absolute difference of each pixel from its right
/// and lower neighbors (both in 0 to 1, over the three channels); and
/// the normalized correlation of the two on the latents' grid, the
/// means of 8 × 8 blocks mostly outside the hole, each channel about
/// its own mean, or `None` where the picture there is too flat for one
/// to mean anything ([`FLAT`]). A block's mean leaves the grain out,
/// which the VAE does not give back and which correlates with nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Drift {
    pub off: f32,
    pub detail: f32,
    pub correlation: Option<f32>,
}

/// The least correlation of the decoder's answer outside the hole with
/// the picture that is taken for the picture. The drift bound alone
/// lets through a flat answer on a busy low-contrast window and the
/// picture shifted by a few pixels, both off by little on average; an
/// honest answer follows the picture, and either of those does not.
pub const CORRELATION: f32 = 0.5;

/// The standard deviation of the picture's 8 × 8 block means outside
/// the hole, over its channels, under which no correlation is asked:
/// a flat sky has no structure for an answer to follow, and the drift
/// bound is the check there.
pub const FLAT: f32 = 0.005;

/// The side of the blocks the correlation is taken over: a latent's.
const BLOCK: usize = 8;

/// [`Drift`] of `decoded` from `image` where `hole` is zero; zeros
/// with nothing outside the hole.
pub fn drift(image: &Rgbf, hole: &[f32], decoded: &Rgbf) -> Drift {
    let (w, h) = (image.width, image.height);
    let p = |i: usize, c: usize| image.data[i * 3 + c].clamp(0.0, 1.0);
    let (mut off, mut detail, mut n, mut pairs) = (0.0f64, 0.0f64, 0usize, 0usize);
    // Each block's sums outside the hole, the picture's and the
    // answer's by channel, and how many pixels.
    let bw = w.div_ceil(BLOCK);
    let mut blocks = vec![([0.0f64; 3], [0.0f64; 3], 0usize); bw * h.div_ceil(BLOCK)];
    for (i, _) in hole.iter().enumerate().filter(|(_, m)| **m <= 0.0) {
        let (x, y) = (i % w, i / w);
        let b = &mut blocks[(y / BLOCK) * bw + x / BLOCK];
        b.2 += 1;
        for c in 0..3 {
            b.0[c] += f64::from(p(i, c));
            b.1[c] += f64::from(decoded.data[i * 3 + c]);
            off += f64::from((p(i, c) - decoded.data[i * 3 + c]).abs());
            if x + 1 < w {
                detail += f64::from((p(i, c) - p(i + 1, c)).abs());
                pairs += 1;
            }
            if y + 1 < h {
                detail += f64::from((p(i, c) - p(i + w, c)).abs());
                pairs += 1;
            }
        }
        n += 3;
    }
    // The blocks at least half outside the hole, by their means: the
    // sums of the picture's, the answer's, their squares and their
    // product, by channel.
    let mut sums = [[0.0f64; 5]; 3];
    let mut k = 0.0f64;
    for (j, (a, b, count)) in blocks.iter().enumerate() {
        let (bx, by) = (j % bw * BLOCK, j / bw * BLOCK);
        let area = (w - bx).min(BLOCK) * (h - by).min(BLOCK);
        if *count == 0 || 2 * count < area {
            continue;
        }
        k += 1.0;
        for (c, s) in sums.iter_mut().enumerate() {
            let (a, b) = (a[c] / *count as f64, b[c] / *count as f64);
            s[0] += a;
            s[1] += b;
            s[2] += a * a;
            s[3] += b * b;
            s[4] += a * b;
        }
    }
    let (mut cov, mut va, mut vb) = (0.0f64, 0.0f64, 0.0f64);
    if k > 0.0 {
        for s in &sums {
            va += s[2] - s[0] * s[0] / k;
            vb += s[3] - s[1] * s[1] / k;
            cov += s[4] - s[0] * s[1] / k;
        }
    }
    let flat = k < 2.0 || (va.max(0.0) / (3.0 * k)).sqrt() < f64::from(FLAT);
    Drift {
        off: if n > 0 { (off / n as f64) as f32 } else { 0.0 },
        detail: if pairs > 0 {
            (detail / pairs as f64) as f32
        } else {
            0.0
        },
        // A flat answer on a picture that is not has no variance, and
        // correlates with it not at all.
        correlation: (!flat).then(|| {
            if vb <= 1e-12 {
                0.0
            } else {
                (cov / (va * vb).sqrt()) as f32
            }
        }),
    }
}

/// How a hole is taken to the latents' grid, one latent for each 8 × 8
/// block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    /// A latent is in the hole when any pixel of its block is, so a
    /// stroke thinner than 8 pixels is never missed.
    Any,
    /// The block's top left pixel, as diffusers' nearest resize does:
    /// for matching its reference.
    TopLeft,
}

/// The square a window of `width` × `height` is run at: the smaller
/// export that holds it, or `None` past [`MAX`] or for nothing.
pub fn size_for(width: usize, height: usize) -> Option<usize> {
    let side = width.max(height);
    if width == 0 || height == 0 {
        return None;
    }
    SIZES.into_iter().find(|&s| side <= s)
}

/// The Euler discrete schedule RORem ships with (scaled linear betas
/// from 0.00085 to 0.012 over 1000 training steps, leading spacing,
/// offset 1), `steps` long and entered at `strength`: the timesteps
/// run, and their sigmas with a final zero.
pub fn schedule(steps: usize, strength: f64) -> (Vec<f32>, Vec<f32>) {
    let (b0, b1, train) = (0.00085f64.sqrt(), 0.012f64.sqrt(), 1000usize);
    let mut acp = Vec::with_capacity(train);
    let mut prod = 1.0f64;
    for i in 0..train {
        let b = b0 + (b1 - b0) * i as f64 / (train - 1) as f64;
        prod *= 1.0 - b * b;
        acp.push(prod);
    }
    let ratio = train / steps;
    let all: Vec<usize> = (0..steps).rev().map(|i| i * ratio + 1).collect();
    let start = steps - ((steps as f64 * strength) as usize).min(steps);
    let ts = &all[start..];
    let mut sigmas: Vec<f32> = ts
        .iter()
        .map(|&t| ((1.0 - acp[t]) / acp[t]).sqrt() as f32)
        .collect();
    sigmas.push(0.0);
    (ts.iter().map(|&t| t as f32).collect(), sigmas)
}

/// `n` standard normals from `seed`: SplitMix64 (Steele, Lea, and
/// Flood, 2014) for the uniforms, two a normal through Box-Muller's
/// cosine, as the develop's grain makes its own.
pub fn normals(n: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    let mut next = move || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        ((z >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    (0..n)
        .map(|_| {
            let (u, v) = (next(), next());
            ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
        })
        .collect()
}

/// The hole of a `size` square taken to the latents' grid, `size / 8`
/// on a side, by `pool`.
pub fn latent_mask(hole: &[bool], size: usize, pool: Pool) -> Vec<f32> {
    let l = size / 8;
    (0..l * l)
        .map(|i| {
            let (x, y) = (i % l * 8, i / l * 8);
            let inside = match pool {
                Pool::TopLeft => hole[y * size + x],
                Pool::Any => {
                    (y..y + 8).any(|yy| hole[yy * size + x..yy * size + x + 8].contains(&true))
                }
            };
            if inside { 1.0 } else { 0.0 }
        })
        .collect()
}

/// Where tiles of `tile` start along a side of `len` so that
/// neighbors overlap by at least `overlap`: evenly spaced from 0 to
/// `len - tile`, on the latents' grid. One tile when it holds the side.
pub fn starts(len: usize, tile: usize, overlap: usize) -> Vec<usize> {
    debug_assert!(overlap < tile);
    if len <= tile {
        return vec![0];
    }
    let n = (len - overlap).div_ceil(tile - overlap);
    let span = len - tile;
    (0..n).map(|i| span * i / (n - 1) / 8 * 8).collect()
}

/// The weight of the `i`th sample of the `k`th of tiles of `tile`
/// starting at `starts` along a side: rising linearly across its
/// overlap with the tile before, falling across its overlap with the
/// one after, 1 elsewhere. Where two tiles overlap their weights sum
/// to 1, as long as a tile overlaps only its neighbors (`overlap` at
/// most half of `tile`); [`blend`] divides by the total either way.
fn ramp(starts: &[usize], k: usize, tile: usize, i: usize) -> f32 {
    let mut w = 1.0f32;
    if k > 0 {
        let up = (starts[k - 1] + tile).saturating_sub(starts[k]);
        if up > 0 {
            w = w.min((i as f32 + 0.5) / up as f32);
        }
    }
    if let Some(&next) = starts.get(k + 1) {
        let down = (starts[k] + tile).saturating_sub(next);
        if down > 0 {
            w = w.min(((tile - i) as f32 - 0.5) / down as f32);
        }
    }
    w
}

/// A `tile` square at (`x0`, `y0`) of planar `channels` × `side`
/// squares.
fn crop(
    planes: &[f32],
    channels: usize,
    side: usize,
    x0: usize,
    y0: usize,
    tile: usize,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(channels * tile * tile);
    for c in 0..channels {
        for y in y0..y0 + tile {
            let row = c * side * side + y * side + x0;
            out.extend_from_slice(&planes[row..row + tile]);
        }
    }
    out
}

/// A planar `channels` × `side` square made of `run`'s answers for
/// the `tile` squares at `xs` × `ys` (in the answer's units), each
/// `channels` × `tile` × `tile`, weighted by the product of their
/// [`ramp`]s and divided by the weights' sum: the tiled VAE's blend,
/// diffusers' (`AutoencoderKL`'s tiled decode) with each overlap
/// ramped from both sides. Zero where no tile reaches.
pub fn blend(
    side: usize,
    channels: usize,
    tile: usize,
    xs: &[usize],
    ys: &[usize],
    mut run: impl FnMut(usize, usize) -> Result<Vec<f32>>,
) -> Result<Vec<f32>> {
    let n = side * side;
    let mut sum = vec![0.0f32; channels * n];
    let mut weight = vec![0.0f32; n];
    let ramps =
        |starts: &[usize], k| -> Vec<f32> { (0..tile).map(|i| ramp(starts, k, tile, i)).collect() };
    for (ky, &y0) in ys.iter().enumerate() {
        let wy = ramps(ys, ky);
        for (kx, &x0) in xs.iter().enumerate() {
            let wx = ramps(xs, kx);
            let t = run(x0, y0)?;
            if t.len() != channels * tile * tile {
                return Err(Error::Shape(format!(
                    "{} values for a tile of {tile}",
                    t.len()
                )));
            }
            for y in 0..tile {
                for x in 0..tile {
                    let w = wy[y] * wx[x];
                    let d = (y0 + y) * side + x0 + x;
                    weight[d] += w;
                    for c in 0..channels {
                        sum[c * n + d] += w * t[c * tile * tile + y * tile + x];
                    }
                }
            }
        }
    }
    for (d, &w) in weight.iter().enumerate().filter(|(_, w)| **w > 0.0) {
        for c in 0..channels {
            sum[c * n + d] /= w;
        }
    }
    Ok(sum)
}

/// The window mirrored out to a `size` square, as the model takes it:
/// the picture in -1 to 1, the same with the hole blanked to 0, both
/// planar, and the hole (any nonzero value), the hole mirrored too so
/// nothing under it shows as known in the margin.
fn squared(image: &Rgbf, hole: &[f32], size: usize) -> (Vec<f32>, Vec<f32>, Vec<bool>) {
    let (w, h) = (image.width, image.height);
    let n = size * size;
    let mut picture = vec![0.0f32; 3 * n];
    let mut masked = vec![0.0f32; 3 * n];
    let mut mask = vec![false; n];
    for y in 0..size {
        let sy = mirror(y, h);
        for x in 0..size {
            let s = sy * w + mirror(x, w);
            let d = y * size + x;
            let m = hole[s] > 0.0;
            mask[d] = m;
            for c in 0..3 {
                let v = image.data[s * 3 + c].clamp(0.0, 1.0) * 2.0 - 1.0;
                picture[c * n + d] = v;
                masked[c * n + d] = if m { 0.0 } else { v };
            }
        }
    }
    (picture, masked, mask)
}

/// The decoder's planar square in -1 to 1, cut back to `width` ×
/// `height` in 0 to 1.
fn cut(planes: &[f32], size: usize, width: usize, height: usize) -> Result<Rgbf> {
    let n = size * size;
    if planes.len() != 3 * n {
        return Err(Error::Shape(format!(
            "the decoder gave {} values for {size}",
            planes.len()
        )));
    }
    let mut data = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            for c in 0..3 {
                let v = planes[c * n + y * size + x];
                data.push(((v + 1.0) * 0.5).clamp(0.0, 1.0));
            }
        }
    }
    Ok(Rgbf::new(width, height, data))
}

/// `image` with `decoded` put in under `hole`, weighted by its values
/// (in 0 to 1; the retouch's hole is 0 or 1 and its blend feathers),
/// the rest the picture clamped to 0 to 1 as [`implausible`] reads it.
/// [`Error::Implausible`] when the decoder's answer outside the hole
/// strays from the picture by more than [`bound`] allows for its
/// detail ([`drift`]), correlates with it under [`CORRELATION`], or is
/// not finite: what a card out of memory gives.
pub fn composite(image: &Rgbf, hole: &[f32], decoded: &Rgbf) -> Result<Rgbf> {
    if decoded.data.iter().any(|v| !v.is_finite()) {
        return Err(Error::Implausible("the removal is not finite".into()));
    }
    let d = drift(image, hole, decoded);
    if d.off > bound(d.detail) {
        return Err(Error::Implausible(format!(
            "the picture outside the hole came back as something else (off by {:.3} on average, where its detail is {:.3})",
            d.off, d.detail
        )));
    }
    if let Some(r) = d.correlation.filter(|&r| r.is_nan() || r < CORRELATION) {
        return Err(Error::Implausible(format!(
            "the picture outside the hole came back as something else (a correlation of {r:.3} with it)"
        )));
    }
    let mut data = Vec::with_capacity(image.data.len());
    for (i, &m) in hole.iter().enumerate() {
        let a = m.clamp(0.0, 1.0);
        for c in 0..3 {
            let (p, d) = (
                image.data[i * 3 + c].clamp(0.0, 1.0),
                decoded.data[i * 3 + c],
            );
            data.push(if m <= 0.0 { p } else { p + (d - p) * a });
        }
    }
    let out = Rgbf::new(image.width, image.height, data);
    if let Some(why) = implausible(image, hole, &out) {
        return Err(Error::Implausible(why));
    }
    Ok(out)
}

fn read_f16(path: &std::path::Path, len: usize) -> Result<Vec<f16>> {
    let bytes =
        std::fs::read(path).map_err(|e| Error::Shape(format!("{}: {e}", path.display())))?;
    if bytes.len() != 2 * len {
        return Err(Error::Shape(format!(
            "{} holds {} bytes, not {len} fp16 values",
            path.display(),
            bytes.len()
        )));
    }
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f16::from_le_bytes(*b))
        .collect())
}

/// A one-input, one-output graph run on f32 data, taken to and from
/// f16 where the graph wants it.
fn run1(session: &mut Session, shape: [usize; 4], data: &[f32]) -> Result<Vec<f32>> {
    let f16_of = |t: Option<TensorElementType>| t == Some(TensorElementType::Float16);
    let input = session.inputs()[0].name().to_string();
    let output = session.outputs()[0].name().to_string();
    let half_in = f16_of(session.inputs()[0].dtype().tensor_type());
    let half_out = f16_of(session.outputs()[0].dtype().tensor_type());
    let out = if half_in {
        let v: Vec<f16> = data.iter().map(|&x| f16::from_f32(x)).collect();
        session.run(ort::inputs![input => Tensor::from_array((shape, v))?])?
    } else {
        session.run(ort::inputs![input => Tensor::from_array((shape, data.to_vec()))?])?
    };
    let v: Vec<f32> = if half_out {
        let (_, v) = out[output.as_str()].try_extract_tensor::<f16>()?;
        v.iter().map(|x| x.to_f32()).collect()
    } else {
        out[output.as_str()].try_extract_tensor::<f32>()?.1.to_vec()
    };
    Ok(v)
}

/// The three sessions kept open, for one size.
struct Resident {
    size: usize,
    encoder: Option<Session>,
    unet: Option<Session>,
    decoder: Option<Session>,
}

impl Resident {
    fn slot(&mut self, part: Part) -> &mut Option<Session> {
        match part {
            Part::Encoder => &mut self.encoder,
            Part::Unet => &mut self.unet,
            Part::Decoder => &mut self.decoder,
        }
    }
}

/// The generative remove, loaded from a store: the prompt's constants
/// read, the [`Memory`] mode chosen, and in resident mode the sessions
/// for the size last run kept open.
pub struct Remove {
    store: Store,
    memory: Memory,
    vae: Vae,
    prompt: Vec<f16>,
    pooled: Vec<f16>,
    /// The size conditioning at 1024, as `time_ids.f16` holds it.
    time_ids: [f32; 6],
    resident: Option<Resident>,
    times: Times,
}

impl Remove {
    /// The model from `store`, on WebGPU: [`Error::Unsupported`] when
    /// `providers` does not offer it, since on the CPU the eraser's
    /// LaMa is the tool. Reads the prompt's constants and decides the
    /// [`Memory`] mode, logged; no session is opened until the first
    /// removal, and in resident mode they stay open after it.
    pub fn load(store: &Store, providers: &[Provider]) -> Result<Self> {
        if !providers.contains(&Provider::WebGpu) {
            return Err(Error::Unsupported(
                "the generative remove runs on WebGPU only; the CPU keeps the eraser",
            ));
        }
        if !store.have(&REMOVE) {
            return Err(Error::Missing(REMOVE.name));
        }
        let path = |name: &str| store.path(&REMOVE, file(name));
        let prompt = read_f16(&path("prompt_embeds.f16"), PROMPT)?;
        let pooled = read_f16(&path("text_embeds.f16"), POOLED)?;
        let ids = read_f16(&path("time_ids.f16"), 6)?;
        let time_ids = std::array::from_fn(|i| ids[i].to_f32());
        let asked = std::env::var("GREYCARD_REMOVE_RESIDENT").ok();
        let memory = Memory::from_env(asked.as_deref());
        match memory {
            Memory::Resident => log::info!(
                "remove: all three sessions kept open (GREYCARD_REMOVE_RESIDENT=1), about 12 GB of the card at 1024"
            ),
            Memory::OneAtATime => log::info!(
                "remove: one session at a time, about 7 GB of the card at 1024 (the adapter's memory is not known; GREYCARD_REMOVE_RESIDENT=1 keeps all three open)"
            ),
        }
        let vae = Vae::from_env(std::env::var("GREYCARD_REMOVE_TILED").ok().as_deref());
        log::info!(
            "remove: the decoder {} at 1024 (GREYCARD_REMOVE_TILED=0 or 1 chooses)",
            match vae {
                Vae::Tiled => "in 512 tiles",
                Vae::Whole => "whole",
            }
        );
        Ok(Self {
            store: store.clone(),
            memory,
            vae,
            prompt,
            pooled,
            time_ids,
            resident: None,
            times: Times::default(),
        })
    }

    /// Where it runs: WebGPU, always.
    pub fn provider(&self) -> Provider {
        Provider::WebGpu
    }

    /// How the sessions share the card, as `load` chose.
    pub fn memory(&self) -> Memory {
        self.memory
    }

    /// How the decoder runs at 1024, as `load` chose.
    pub fn vae(&self) -> Vae {
        self.vae
    }

    /// Run the decoder `vae`'s way from the next removal on.
    pub fn set_vae(&mut self, vae: Vae) {
        if vae != self.vae {
            self.vae = vae;
            self.resident = None;
        }
    }

    /// The last removal's parts.
    pub fn times(&self) -> Times {
        self.times
    }

    /// `image`, in 0 to 1, no side over [`MAX`], with the pixels where
    /// `hole` is nonzero made up from `seed`: run at the square
    /// [`size_for`] names, mirrored out to it, and put back under the
    /// hole alone ([`composite`]). The answer is in 0 to 1, and
    /// [`Error::Implausible`] when it cannot be right.
    pub fn remove(&mut self, image: &Rgbf, hole: &[f32], seed: u64) -> Result<Rgbf> {
        let size = self.check(image, hole)?;
        let l = size / 8;
        let noise = normals(4 * l * l, seed);
        self.run(image, hole, &noise, Pool::Any)
    }

    /// [`Remove::remove`] on given noise, [1, 4, size / 8, size / 8],
    /// with the hole taken to the latents as diffusers takes it
    /// ([`Pool::TopLeft`]): for matching the reference pipeline.
    #[doc(hidden)]
    pub fn remove_with_noise(&mut self, image: &Rgbf, hole: &[f32], noise: &[f32]) -> Result<Rgbf> {
        let size = self.check(image, hole)?;
        if noise.len() != 4 * (size / 8) * (size / 8) {
            return Err(Error::Shape(format!(
                "{} noise values for {size}",
                noise.len()
            )));
        }
        self.run(image, hole, noise, Pool::TopLeft)
    }

    /// The decoder's whole answer for [`Remove::remove_with_noise`],
    /// the window before it is put under the hole: for comparing two
    /// ways of running the decoder outside the hole too.
    #[doc(hidden)]
    pub fn decoded_with_noise(
        &mut self,
        image: &Rgbf,
        hole: &[f32],
        noise: &[f32],
    ) -> Result<Rgbf> {
        let size = self.check(image, hole)?;
        if noise.len() != 4 * (size / 8) * (size / 8) {
            return Err(Error::Shape(format!(
                "{} noise values for {size}",
                noise.len()
            )));
        }
        let total = Instant::now();
        let (decoded, mut times) = self.generate(image, hole, noise, Pool::TopLeft)?;
        times.drift = drift(image, hole, &decoded);
        times.total = total.elapsed().as_secs_f64();
        self.times = times;
        Ok(decoded)
    }

    /// Latents ([1, 4, size / 8, size / 8], scaled) decoded to the
    /// `size` square, in 0 to 1: for comparing with a reference's.
    #[doc(hidden)]
    pub fn decode(&mut self, latents: &[f32], size: usize) -> Result<Rgbf> {
        let unscaled: Vec<f32> = latents.iter().map(|v| v / SCALE).collect();
        let (planes, _) = self.decode_planes(&unscaled, size, size, size)?;
        cut(&planes, size, size, size)
    }

    /// Whether the decoder runs in tiles at `size`.
    fn tiles(&self, size: usize) -> bool {
        self.vae == Vae::Tiled && size > TILE
    }

    /// The size of the graph `part` runs on at `size`.
    fn graph_size(&self, part: Part, size: usize) -> usize {
        if part == Part::Decoder && self.tiles(size) {
            TILE
        } else {
            size
        }
    }

    /// The decoder's planar `size` square of the `unscaled` latents,
    /// and the seconds spent opening it: in tiles only as far as the
    /// `width` × `height` window the answer is cut back to.
    fn decode_planes(
        &mut self,
        unscaled: &[f32],
        size: usize,
        width: usize,
        height: usize,
    ) -> Result<(Vec<f32>, f64)> {
        let l = size / 8;
        if !self.tiles(size) {
            return self.with(Part::Decoder, size, |s| run1(s, [1, 4, l, l], unscaled));
        }
        let lt = TILE / 8;
        let at = starts(size, TILE, OVERLAP);
        let within =
            |side: usize| -> Vec<usize> { at.iter().copied().filter(|&s| s < side).collect() };
        let (xs, ys) = (within(width), within(height));
        self.with(Part::Decoder, size, |s| {
            blend(size, 3, TILE, &xs, &ys, |x, y| {
                let tile = crop(unscaled, 4, l, x / 8, y / 8, lt);
                run1(s, [1, 4, lt, lt], &tile)
            })
        })
    }

    fn check(&self, image: &Rgbf, hole: &[f32]) -> Result<usize> {
        let (w, h) = (image.width, image.height);
        match size_for(w, h) {
            Some(size) if hole.len() == w * h => Ok(size),
            _ => Err(Error::Shape(format!(
                "remove wants a side of 1 to {MAX}, got {w}x{h} and a hole of {}",
                hole.len()
            ))),
        }
    }

    fn graph(part: Part, size: usize) -> &'static File {
        file(&match part {
            Part::Encoder => format!("vae-encoder-{size}.onnx"),
            Part::Unet => format!("unet-{size}.onnx"),
            Part::Decoder => format!("vae-decoder-{size}.onnx"),
        })
    }

    fn open(&self, part: Part, size: usize) -> Result<Session> {
        let path = self
            .store
            .path(&REMOVE, Self::graph(part, self.graph_size(part, size)));
        // No warm-up: with WebGPU the only provider there is nothing
        // to fall through to, and a warm-up of the UNet is a step's
        // time on every open.
        let loaded = runtime::open(
            &path,
            GraphOptimizationLevel::Level3,
            &[Provider::WebGpu],
            None,
            |_| Ok(()),
        )?;
        Ok(loaded.session)
    }

    /// `f` on the session for `part` at `size` (the [`TILE`] graph
    /// for the decoder in tiles), and the seconds spent opening it: the
    /// one kept open in resident mode (the others
    /// dropped when the size changes), else one opened for `f` and
    /// dropped after.
    fn with<T>(
        &mut self,
        part: Part,
        size: usize,
        f: impl FnOnce(&mut Session) -> Result<T>,
    ) -> Result<(T, f64)> {
        let kept = match self.memory {
            Memory::Resident => {
                if self.resident.as_ref().is_some_and(|r| r.size != size) {
                    self.resident = None;
                }
                let r = self.resident.get_or_insert(Resident {
                    size,
                    encoder: None,
                    unet: None,
                    decoder: None,
                });
                r.slot(part).take()
            }
            Memory::OneAtATime => None,
        };
        let t = Instant::now();
        let mut session = match kept {
            Some(s) => s,
            None => self.open(part, size)?,
        };
        let load = t.elapsed().as_secs_f64();
        let out = f(&mut session);
        if let Some(r) = self.resident.as_mut().filter(|r| r.size == size) {
            *r.slot(part) = Some(session);
        }
        Ok((out?, load))
    }

    fn run(&mut self, image: &Rgbf, hole: &[f32], noise: &[f32], pool: Pool) -> Result<Rgbf> {
        let (w, h) = (image.width, image.height);
        let total = Instant::now();
        let (decoded, mut times) = self.generate(image, hole, noise, pool)?;
        times.drift = drift(image, hole, &decoded);
        let out = composite(image, hole, &decoded);
        times.total = total.elapsed().as_secs_f64();
        self.times = times;
        log::info!(
            "remove {w}x{h} at {}: loads {:.2}s, encode {:.2}s, {} UNet calls {:.2}s, decode {:.2}s, {:.2}s in all ({:?}, decoder {}); outside the hole off by {:.4} where its detail is {:.4}, correlation {}",
            times.size,
            times.loads,
            times.encode,
            times.unet_calls,
            times.steps,
            times.decode,
            times.total,
            self.memory,
            if times.tiled { "in tiles" } else { "whole" },
            times.drift.off,
            times.drift.detail,
            times
                .drift
                .correlation
                .map_or("not asked (flat)".to_string(), |r| format!("{r:.3}"))
        );
        out
    }

    /// The decoder's answer for the window, and the parts' times but
    /// the drift and the total.
    fn generate(
        &mut self,
        image: &Rgbf,
        hole: &[f32],
        noise: &[f32],
        pool: Pool,
    ) -> Result<(Rgbf, Times)> {
        let (w, h) = (image.width, image.height);
        let size = self.check(image, hole)?;
        let l = size / 8;
        let mut times = Times {
            size,
            tiled: self.tiles(size),
            ..Times::default()
        };
        let (picture, masked, square) = squared(image, hole, size);
        let mask = latent_mask(&square, size, pool);

        let t = Instant::now();
        let ((image_lat, masked_lat), load) = self.with(Part::Encoder, size, |s| {
            let shape = [1, 3, size, size];
            Ok((run1(s, shape, &picture)?, run1(s, shape, &masked)?))
        })?;
        times.loads += load;
        times.encode = t.elapsed().as_secs_f64() - load;
        if image_lat.len() != 4 * l * l || masked_lat.len() != 4 * l * l {
            return Err(Error::Shape(format!(
                "the encoder gave {} latents for {size}",
                image_lat.len()
            )));
        }

        let (ts, sigmas) = schedule(STEPS, STRENGTH);
        let mut x: Vec<f32> = image_lat
            .iter()
            .zip(noise)
            .map(|(z, e)| z * SCALE + e * sigmas[0])
            .collect();
        let mask16: Vec<f16> = mask.iter().map(|&v| f16::from_f32(v)).collect();
        let masked16: Vec<f16> = masked_lat
            .iter()
            .map(|&v| f16::from_f32(v * SCALE))
            .collect();
        let px = size as f32 / 1024.0;
        let ids: Vec<f16> = self
            .time_ids
            .iter()
            .map(|&v| f16::from_f32(v * px))
            .collect();
        let (prompt, pooled) = (self.prompt.clone(), self.pooled.clone());
        let t = Instant::now();
        let (_, load) = self.with(Part::Unet, size, |s| {
            for i in 0..ts.len() {
                let scale = 1.0 / (sigmas[i] * sigmas[i] + 1.0).sqrt();
                let mut sample: Vec<f16> = Vec::with_capacity(9 * l * l);
                sample.extend(x.iter().map(|&v| f16::from_f32(v * scale)));
                sample.extend_from_slice(&mask16);
                sample.extend_from_slice(&masked16);
                let out = s.run(ort::inputs![
                    "sample" => Tensor::from_array(([1usize, 9, l, l], sample))?,
                    "timestep" => Tensor::from_array(([1usize], vec![ts[i]]))?,
                    "encoder_hidden_states" => Tensor::from_array(([1usize, 77, 2048], prompt.clone()))?,
                    "text_embeds" => Tensor::from_array(([1usize, POOLED], pooled.clone()))?,
                    "time_ids" => Tensor::from_array(([1usize, 6], ids.clone()))?,
                ])?;
                let (_, eps) = out["out_sample"].try_extract_tensor::<f16>()?;
                if eps.len() != x.len() {
                    return Err(Error::Shape(format!("{} values from the UNet", eps.len())));
                }
                let dt = sigmas[i + 1] - sigmas[i];
                for (v, e) in x.iter_mut().zip(eps) {
                    *v += dt * e.to_f32();
                }
            }
            Ok(())
        })?;
        times.loads += load;
        times.steps = t.elapsed().as_secs_f64() - load;
        times.unet_calls = ts.len();
        if x.iter().any(|v| !v.is_finite()) {
            return Err(Error::Implausible("the latents are not finite".into()));
        }

        let t = Instant::now();
        let unscaled: Vec<f32> = x.iter().map(|v| v / SCALE).collect();
        let (planes, load) = self.decode_planes(&unscaled, size, w, h)?;
        times.loads += load;
        times.decode = t.elapsed().as_secs_f64() - load;
        Ok((cut(&planes, size, w, h)?, times))
    }
}

/// The registry's file of that name.
fn file(name: &str) -> &'static File {
    REMOVE
        .files
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("{name} is not one of {}'s files", REMOVE.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// diffusers' `EulerDiscreteScheduler` from RORem's own config,
    /// `set_timesteps(25)` and the inpainting pipeline's cut at
    /// strength 0.99, dumped in float32 by diffusers 0.41.0 (the config
    /// was written by 0.30.0).
    const TIMESTEPS: [f32; 24] = [
        921.0, 881.0, 841.0, 801.0, 761.0, 721.0, 681.0, 641.0, 601.0, 561.0, 521.0, 481.0, 441.0,
        401.0, 361.0, 321.0, 281.0, 241.0, 201.0, 161.0, 121.0, 81.0, 41.0, 1.0,
    ];
    const SIGMAS: [f32; 25] = [
        9.338_079,
        7.562_391,
        6.197_038,
        5.134_433,
        4.297_55,
        3.630_654_3,
        3.093_010_2,
        2.654_545,
        2.292_854_8,
        1.991_091_8,
        1.736_474_4,
        1.519_217_6,
        1.331_763_6,
        1.168_216_5,
        1.023_925_8,
        0.895_172_36,
        0.778_923_8,
        0.672_633_2,
        0.574_050_8,
        0.481_003_28,
        0.391_038_36,
        0.300_580_95,
        0.201_793_57,
        0.041_314_48,
        0.0,
    ];

    #[test]
    fn the_schedule_is_diffusers_own() {
        let (ts, sigmas) = schedule(STEPS, STRENGTH);
        assert_eq!(ts, TIMESTEPS);
        assert_eq!(sigmas.len(), SIGMAS.len());
        for (i, (a, b)) in sigmas.iter().zip(SIGMAS).enumerate() {
            assert!(
                (a - b).abs() <= 1e-6 * b.max(1.0),
                "sigma {i}: {a} against {b}"
            );
        }
        // At full strength the whole schedule runs.
        assert_eq!(schedule(STEPS, 1.0).0.len(), STEPS);
    }

    #[test]
    fn the_window_is_run_at_the_smaller_square_that_holds_it() {
        assert_eq!(size_for(1, 1), Some(512));
        assert_eq!(size_for(512, 300), Some(512));
        assert_eq!(size_for(513, 8), Some(1024));
        assert_eq!(size_for(300, 900), Some(1024));
        assert_eq!(size_for(1024, 1024), Some(1024));
        assert_eq!(size_for(1025, 10), None);
        assert_eq!(size_for(0, 10), None);
    }

    #[test]
    fn the_noise_is_the_seeds_and_standard() {
        let a = normals(65_536, 7);
        assert_eq!(a, normals(65_536, 7));
        assert_ne!(a, normals(65_536, 8));
        let mean = a.iter().map(|&v| f64::from(v)).sum::<f64>() / a.len() as f64;
        let var = a
            .iter()
            .map(|&v| (f64::from(v) - mean).powi(2))
            .sum::<f64>()
            / a.len() as f64;
        assert!(mean.abs() < 0.02, "{mean}");
        assert!((var - 1.0).abs() < 0.03, "{var}");
    }

    #[test]
    fn a_thin_stroke_reaches_the_latents() {
        // A stroke 3 pixels wide at rows 10 to 12: no block's top left
        // pixel is in it, but every block it crosses is in the hole.
        let size = 512;
        let hole: Vec<bool> = (0..size * size)
            .map(|i| (10..13).contains(&(i / size)))
            .collect();
        let any = latent_mask(&hole, size, Pool::Any);
        let top = latent_mask(&hole, size, Pool::TopLeft);
        assert_eq!(any.len(), 64 * 64);
        assert!(top.iter().all(|&v| v == 0.0));
        assert!(any[64..128].iter().all(|&v| v == 1.0));
        assert_eq!(any.iter().sum::<f32>(), 64.0);
        // A block wholly in the hole is in it either way.
        let hole: Vec<bool> = (0..size * size)
            .map(|i| (16..24).contains(&(i / size)))
            .collect();
        assert_eq!(
            latent_mask(&hole, size, Pool::Any),
            latent_mask(&hole, size, Pool::TopLeft)
        );
    }

    #[test]
    fn a_small_window_is_mirrored_out_with_its_hole() {
        let (w, h) = (5, 3);
        let image = Rgbf::new(
            w,
            h,
            (0..w * h * 3).map(|i| (i / 3) as f32 / 20.0).collect(),
        );
        let mut hole = vec![0.0; w * h];
        hole[w + 4] = 1.0;
        let (picture, masked, mask) = squared(&image, &hole, 512);
        let n = 512 * 512;
        // Pixel (4, 1) is the hole, and its mirror at (4, 3) and (4,
        // 5), and (4 + 8 = 12, 1) on the next fold.
        for (x, y) in [(4, 1), (4, 3), (12, 1)] {
            assert!(mask[y * 512 + x], "({x}, {y})");
            assert_eq!(masked[y * 512 + x], 0.0);
        }
        assert!(!mask[0]);
        // Mirrored: column 5 is column 3, row 3 is row 1.
        assert_eq!(picture[512 + 5], picture[512 + 3]);
        assert_eq!(picture[3 * 512], picture[512]);
        // In -1 to 1.
        assert_eq!(picture[0], -1.0);
        assert!((picture[2 * n + 2 * 512 + 3] - ((13.0 / 20.0) * 2.0 - 1.0)).abs() < 1e-6);
    }

    fn flat(w: usize, h: usize, v: f32) -> Rgbf {
        Rgbf::new(w, h, vec![v; w * h * 3])
    }

    #[test]
    fn the_answer_is_the_picture_outside_and_the_removal_under_the_hole() {
        let (w, h) = (16, 16);
        let image = Rgbf::new(w, h, (0..w * h * 3).map(|i| (i % 7) as f32 / 6.0).collect());
        let mut hole = vec![0.0; w * h];
        hole[5 * w + 5] = 1.0;
        hole[5 * w + 6] = 0.5;
        // The decoder near the picture outside, 0.9 inside.
        let mut decoded = image.clone();
        for v in &mut decoded.data {
            *v = (*v + 0.01).min(1.0);
        }
        for c in 0..3 {
            decoded.data[(5 * w + 5) * 3 + c] = 0.9;
            decoded.data[(5 * w + 6) * 3 + c] = 0.9;
        }
        let out = composite(&image, &hole, &decoded).unwrap();
        assert_eq!(out.at(0, 0), image.at(0, 0));
        assert_eq!(out.at(5, 5), [0.9; 3]);
        // Half way where the hole is half.
        let p = image.at(6, 5);
        for (got, p) in out.at(6, 5).iter().zip(p) {
            assert!((got - (p + 0.9) / 2.0).abs() < 1e-6);
        }
        // Past 1 outside: clamped, as the fill gives it.
        let bright = flat(w, h, 1.5);
        let out = composite(&bright, &hole, &flat(w, h, 1.0)).unwrap();
        assert_eq!(out.at(0, 0), [1.0; 3]);
    }

    #[test]
    fn a_decoder_answer_far_from_the_picture_is_refused() {
        let (w, h) = (16, 16);
        let image = flat(w, h, 0.5);
        let hole: Vec<f32> = (0..w * h).map(|i| if i < 20 { 1.0 } else { 0.0 }).collect();
        assert!(matches!(
            composite(&image, &hole, &flat(w, h, 0.9)),
            Err(Error::Implausible(_))
        ));
        let mut nan = flat(w, h, 0.5);
        nan.data[0] = f32::NAN;
        assert!(matches!(
            composite(&image, &hole, &nan),
            Err(Error::Implausible(_))
        ));
        // Near the picture is a removal.
        assert!(composite(&image, &hole, &flat(w, h, 0.51)).is_ok());
        // A busy window may stray further: stripes a pixel wide, 0.2
        // apart, have a detail of 0.1 and allow 0.22.
        let stripes = Rgbf::new(
            w,
            h,
            (0..w * h * 3)
                .map(|i| if (i / 3) % 2 == 0 { 0.4 } else { 0.6 })
                .collect(),
        );
        let d = drift(&stripes, &hole, &flat(w, h, 0.5));
        assert!((d.off - 0.1).abs() < 1e-5, "{d:?}");
        assert!((d.detail - 0.1).abs() < 0.01, "{d:?}");
        // Stripes a pixel wide are grain to the latents' grid: their
        // blocks are flat, so a smooth answer is not asked to follow.
        assert_eq!(d.correlation, None);
        assert!(composite(&stripes, &hole, &flat(w, h, 0.5)).is_ok());
        assert!(composite(&image, &hole, &flat(w, h, 0.6)).is_err());
        assert_eq!(drift(&image, &hole, &flat(w, h, 0.51)).correlation, None);
    }

    /// Stripes 8 pixels wide under a fine checker: the drift bound
    /// passes a flat answer on them and the stripes shifted a stripe
    /// over, both off by about 0.1 where the checker allows 0.4; the
    /// correlation of the blocks refuses both, and passes the stripes
    /// smoothed of their checker.
    #[test]
    fn an_answer_that_does_not_follow_the_picture_is_refused() {
        let (w, h) = (32, 32);
        let hole: Vec<f32> = (0..w * h).map(|i| if i < 20 { 1.0 } else { 0.0 }).collect();
        let picture = |shift: usize, checker: f32| {
            Rgbf::new(
                w,
                h,
                (0..w * h * 3)
                    .map(|i| {
                        let (x, y) = ((i / 3) % w, (i / 3) / w);
                        let stripe = if ((x + shift) / 8).is_multiple_of(2) {
                            0.4
                        } else {
                            0.6
                        };
                        stripe + if (x + y) % 2 == 0 { checker } else { -checker }
                    })
                    .collect(),
            )
        };
        let stripes = picture(0, 0.15);
        let d = drift(&stripes, &hole, &flat(w, h, 0.5));
        assert!(d.off < 0.02 + 2.0 * d.detail, "inside the bound: {d:?}");
        assert_eq!(d.correlation, Some(0.0), "{d:?}");
        assert!(matches!(
            composite(&stripes, &hole, &flat(w, h, 0.5)),
            Err(Error::Implausible(_))
        ));
        let shifted = picture(8, 0.15);
        let d = drift(&stripes, &hole, &shifted);
        assert!(d.off < 0.02 + 2.0 * d.detail, "inside the bound: {d:?}");
        assert!(d.correlation.is_some_and(|r| r < -0.9), "{d:?}");
        assert!(composite(&stripes, &hole, &shifted).is_err());
        let smooth = picture(0, 0.0);
        assert!(
            drift(&stripes, &hole, &smooth)
                .correlation
                .is_some_and(|r| r > 0.99)
        );
        assert!(composite(&stripes, &hole, &smooth).is_ok());
    }

    /// A flat sky with grain: structure of 0.002 under grain of 0.005,
    /// and the VAE's smooth answer. Taken pixel by pixel the two
    /// correlate at about 0.37, grain against none; on the blocks the
    /// picture is flat and no correlation is asked. A gentle gradient
    /// under the same grain is asked, and the smooth answer follows it.
    #[test]
    fn a_smooth_answer_on_a_grainy_sky_is_taken() {
        let (w, h) = (256, 256);
        let mut state = 0x5ca1_ab1e_u64;
        let mut gauss = move || {
            let mut u = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 11) as f32 + 0.5) / (1u64 << 53) as f32
            };
            let (u1, u2) = (u(), u());
            (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
        };
        let hole: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                f32::from(u8::from((96..160).contains(&x) && (96..160).contains(&y)))
            })
            .collect();
        for (slope, asked) in [(0.002f32 * 3.46, false), (0.05, true)] {
            // A ramp from top to bottom whose standard deviation is
            // about `slope` / 3.46.
            let smooth = |y: usize| 0.5 + slope * (y as f32 / h as f32 - 0.5);
            let mut grainy = Vec::with_capacity(w * h * 3);
            let mut answer = Vec::with_capacity(w * h * 3);
            for i in 0..w * h {
                let v = smooth(i / w);
                let g = 0.005 * gauss();
                grainy.extend([v + g; 3]);
                answer.extend([v; 3]);
            }
            let (grainy, answer) = (Rgbf::new(w, h, grainy), Rgbf::new(w, h, answer));
            let d = drift(&grainy, &hole, &answer);
            assert_eq!(d.correlation.is_some(), asked, "{slope}: {d:?}");
            assert!(d.correlation.is_none_or(|r| r > 0.9), "{slope}: {d:?}");
            assert!(composite(&grainy, &hole, &answer).is_ok(), "{slope}: {d:?}");
        }
    }

    #[test]
    fn it_is_not_offered_without_webgpu() {
        let store = Store::at("/nonexistent/greycard-models");
        assert!(matches!(
            Remove::load(&store, &[Provider::Cpu]),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            Remove::load(&store, &[Provider::WebGpu, Provider::Cpu]),
            Err(Error::Missing(_))
        ));
    }

    #[test]
    fn the_tiles_cover_the_square_and_overlap() {
        assert_eq!(starts(1024, TILE, OVERLAP), [0, 256, 512]);
        assert_eq!(starts(1024, TILE, 0), [0, 512]);
        assert_eq!(starts(1024, TILE, 384), [0, 128, 256, 384, 512]);
        assert_eq!(starts(512, TILE, OVERLAP), [0]);
        // On the latents' grid even where the spacing is not.
        let at = starts(1000, 512, 100);
        assert_eq!((at[0], *at.last().unwrap()), (0, 488));
        assert!(at.iter().all(|s| s % 8 == 0));
        assert!(at.windows(2).all(|p| p[0] + 512 - p[1] >= 100));
    }

    #[test]
    fn the_ramps_sum_to_one_across_an_overlap() {
        let at = starts(1024, TILE, OVERLAP);
        for p in 0..1024 {
            let sum: f32 = at
                .iter()
                .enumerate()
                .filter(|(_, s)| (**s..**s + TILE).contains(&p))
                .map(|(k, s)| ramp(&at, k, TILE, p - s))
                .sum();
            assert!((sum - 1.0).abs() < 1e-6, "{p}: {sum}");
        }
        // Outside any overlap a tile is itself.
        assert_eq!(ramp(&at, 0, TILE, 100), 1.0);
        assert_eq!(ramp(&at, 2, TILE, 500), 1.0);
    }

    /// Tiles cut from a square and blended back give the square, to
    /// rounding; tiles that each answer a constant of their own blend
    /// with no step between them.
    #[test]
    fn the_blend_puts_tiles_back_together() {
        let (side, tile, channels) = (64, 32, 2);
        let square: Vec<f32> = (0..channels * side * side)
            .map(|i| ((i * 37) % 101) as f32 / 100.0)
            .collect();
        let at = starts(side, tile, 16);
        assert_eq!(at, [0, 16, 32]);
        let back = blend(side, channels, tile, &at, &at, |x, y| {
            Ok(crop(&square, channels, side, x, y, tile))
        })
        .unwrap();
        for (a, b) in back.iter().zip(&square) {
            assert!((a - b).abs() < 1e-5, "{a} against {b}");
        }
        let steps = blend(side, 1, tile, &at, &at, |x, y| {
            Ok(vec![(x + y) as f32; tile * tile])
        })
        .unwrap();
        for row in steps.chunks(side) {
            assert!(row.windows(2).all(|p| (p[1] - p[0]).abs() <= 1.0 + 1e-4));
        }
        // Tiles kept only as far as a narrower window leave the rest
        // empty and the window whole.
        let back = blend(side, channels, tile, &at[..2], &at, |x, y| {
            Ok(crop(&square, channels, side, x, y, tile))
        })
        .unwrap();
        for y in 0..side {
            for x in 0..side {
                let i = y * side + x;
                if x < 48 {
                    assert!((back[i] - square[i]).abs() < 1e-5);
                } else {
                    assert_eq!(back[i], 0.0);
                }
            }
        }
    }

    #[test]
    fn tiled_unless_asked_otherwise() {
        assert_eq!(Vae::from_env(None), Vae::DEFAULT);
        assert_eq!(Vae::from_env(Some("1")), Vae::Tiled);
        assert_eq!(Vae::from_env(Some("0")), Vae::Whole);
    }

    #[test]
    fn one_at_a_time_unless_asked() {
        assert_eq!(Memory::from_env(None), Memory::OneAtATime);
        assert_eq!(Memory::from_env(Some("0")), Memory::OneAtATime);
        assert_eq!(Memory::from_env(Some("1")), Memory::Resident);
    }

    #[test]
    fn every_graph_is_a_registered_file() {
        for size in SIZES {
            for part in [Part::Encoder, Part::Unet, Part::Decoder] {
                assert!(Remove::graph(part, size).name.ends_with(".onnx"));
            }
        }
        for name in [
            "prompt_embeds.f16",
            "text_embeds.f16",
            "time_ids.f16",
            "unet.onnx_data",
        ] {
            file(name);
        }
        assert_eq!(file("prompt_embeds.f16").bytes, 2 * PROMPT as u64);
        assert_eq!(file("text_embeds.f16").bytes, 2 * POOLED as u64);
        assert_eq!(file("time_ids.f16").bytes, 12);
    }
}
