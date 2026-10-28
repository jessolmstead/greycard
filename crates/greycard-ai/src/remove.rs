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
//! A window past [`MAX`], up to [`FACTOR`] times it, is taken down to
//! [`MAX`], run, and its fill brought back up ([`scaled`],
//! [`Remove::remove_window`]).
//!
//! The four-step remove is the same UNet with our LCM LoRA beside its
//! weights, distilled from it at 1024 on the same empty prompt
//! ([`crate::registry::REMOVE_FAST`], in the same directory), run by
//! LCM's sampler ([`Lcm`]) over 3 UNet calls. The caller picks the
//! sampler a removal runs ([`Remove::set_steps`], [`Steps`]); one
//! UNet is held at a time. The 4-step sampler takes fresh noise for
//! each call but the last, drawn after the first latent's from the
//! same seed.
//!
//! The answer is the window under the hole alone, weighted by the
//! hole's own values ([`composite`]), brought first to the picture's
//! level in a ring round the hole ([`level`]) and then across the
//! hole's edge ([`Gains`]): the retouch's blend feathers it with the
//! coverage's edge, as it does LaMa's. The decoder gives back
//! the whole window, and how far it strays from the picture outside
//! the hole is the check on the WebGPU provider, which answers a
//! device out of memory with finite garbage rather than an error.
//!
//! Three sessions, each 5.1 GB (UNet) or up to 6 GB at its peak
//! (decoder at 1024): all open at once is 12.2 GB of the card, one at
//! a time about 7 GB for a few seconds more ([`Memory`]). Neither ONNX
//! Runtime's WebGPU provider nor wgpu says how much memory the adapter
//! has, but a GPU on unified memory has the machine's, so the choice
//! is made from that ([`policy`]): one at a time on a discrete card
//! or a small machine, more kept open on a large unified one, and
//! `GREYCARD_REMOVE_RESIDENT` (`1`, `unet`, or `0`) over either.
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
use crate::registry::{File, REMOVE, REMOVE_FAST};
use crate::runtime::{self, Error, Provider, Result};
use crate::store::Store;

mod mottle;
mod scale;
mod seam;

pub use scale::{FACTOR, factor_for, scaled, scaled_size};
pub use seam::Gains;

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

/// The 4-step schedule's length (the UNet with the LCM LoRA), entered at the same [`STRENGTH`]: its first
/// step is skipped too, so 3 UNet calls.
pub const LCM_STEPS: usize = 4;

/// The training schedule's length, from which both samplers pick.
const TRAIN: usize = 1000;

/// The LCM LoRA's distillation schedule (`original_inference_steps`),
/// whose timesteps the 4 are picked from: every 20th.
const LCM_ORIGINAL: usize = 50;

/// How LCM's boundary scalings stretch a timestep (`timestep_scaling`),
/// and the data's standard deviation they assume.
const LCM_TIME_SCALE: f64 = 10.0;
const LCM_SIGMA_DATA: f64 = 0.5;

/// Which sampler and UNet a removal runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Steps {
    /// Euler discrete over [`STEPS`], the RORem UNet: [`crate::REMOVE`].
    Euler25,
    /// LCM over [`LCM_STEPS`], the UNet with the LCM LoRA:
    /// [`crate::registry::REMOVE_FAST`] beside [`crate::REMOVE`].
    Lcm4,
}

impl Steps {
    /// What `GREYCARD_REMOVE_STEPS` asks a bench or a test for: `4`
    /// or `25`, else nothing.
    pub fn from_env(var: Option<&str>) -> Option<Self> {
        match var {
            Some("4") => Some(Steps::Lcm4),
            Some("25") => Some(Steps::Euler25),
            _ => None,
        }
    }

    /// The sampler in a log line's words.
    pub fn words(self) -> &'static str {
        match self {
            Steps::Euler25 => "25 Euler steps",
            Steps::Lcm4 => "4 LCM steps",
        }
    }

    /// How many latents of noise a removal takes: Euler's one to
    /// start from; LCM's that and one for each call but the last,
    /// which re-noises to the next timestep.
    pub fn noises(self) -> usize {
        match self {
            Steps::Euler25 => 1,
            Steps::Lcm4 => Lcm::new(LCM_STEPS, STRENGTH).timesteps.len(),
        }
    }
}

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
    /// The UNet kept open for the size last used, the VAE's two
    /// opened for their part and dropped after: its 5.1 GB held, and
    /// its load, all but a tenth of a second of the loads, paid once.
    /// With the decoder whole the peak at 1024 is about 11 GB, so a
    /// machine under 24 GB keeps it in tiles.
    UnetResident,
    /// Each opened for its part and dropped after: about 7 GB at
    /// 1024, about 13 s a removal.
    OneAtATime,
}

impl Memory {
    /// What `GREYCARD_REMOVE_RESIDENT` asks for: all three at `1`, the
    /// UNet at `unet`, one at a time at `0`, and `None` otherwise, for
    /// the machine to decide ([`policy`]).
    pub fn from_env(var: Option<&str>) -> Option<Self> {
        match var {
            Some("1") => Some(Memory::Resident),
            Some("unet") => Some(Memory::UnetResident),
            Some("0") => Some(Memory::OneAtATime),
            _ => None,
        }
    }

    /// Whether `part`'s session stays open after its run.
    fn keeps(self, part: Part) -> bool {
        match self {
            Memory::Resident => true,
            Memory::UnetResident => part == Part::Unet,
            Memory::OneAtATime => false,
        }
    }
}

/// What the remove knows of the machine it runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Machine {
    /// The bytes of physical memory, or `None` when the system will
    /// not say.
    pub memory: Option<u64>,
    /// Whether the GPU shares that memory ([`runtime::webgpu_unified`]),
    /// or `None` when no GPU adapter answered.
    pub unified: Option<bool>,
}

/// How the remove uses the card, and why, for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub memory: Memory,
    pub vae: Vae,
    pub why: String,
}

/// Where a unified machine keeps the UNet open, where it runs the
/// decoder whole as well, and where it keeps all three open, in GiB as
/// the system reports them, to the nearest: a 16, 24, or 32 GB machine
/// can report a little under, the kernel's and the firmware's share
/// taken out, so the bounds sit a GiB or two below the sizes they
/// stand for.
const UNET_FROM: u64 = 15;
const WHOLE_FROM: u64 = 23;
const ALL_FROM: u64 = 30;

/// How the remove uses the card on `machine`, with what the
/// environment's `GREYCARD_REMOVE_RESIDENT` and `GREYCARD_REMOVE_TILED`
/// ask for over it. On unified memory the card's memory is the
/// machine's: from 16 GB the UNet stays open, from 24 GB the decoder
/// runs whole as well (a 24 GB Mac decoded whole with all three open),
/// and from 32 GB all three stay open. A discrete card's memory is
/// not known, nor is a machine's that will not say, and they keep to
/// one at a time with the decoder in tiles, about 7 GB at 1024.
pub fn policy(machine: Option<Machine>, resident: Option<&str>, tiled: Option<&str>) -> Policy {
    // In GiB to the nearest, which is how a machine's memory is sold.
    let gib = machine
        .and_then(|m| m.memory)
        .map(|b| (b + (1 << 29)) >> 30);
    let (memory, vae, mut why) = match (machine.map(|m| m.unified), gib) {
        (Some(Some(true)), Some(g)) => {
            let (memory, vae) = if g >= ALL_FROM {
                (Memory::Resident, Vae::Whole)
            } else if g >= WHOLE_FROM {
                (Memory::UnetResident, Vae::Whole)
            } else if g >= UNET_FROM {
                (Memory::UnetResident, Vae::Tiled)
            } else {
                (Memory::OneAtATime, Vae::DEFAULT)
            };
            (memory, vae, format!("{g} GiB unified"))
        }
        (Some(Some(true)), None) => (
            Memory::OneAtATime,
            Vae::DEFAULT,
            "unified, its memory not known".to_string(),
        ),
        (Some(Some(false)), _) => (
            Memory::OneAtATime,
            Vae::DEFAULT,
            "a discrete card, its memory not known".to_string(),
        ),
        (Some(None), _) => (
            Memory::OneAtATime,
            Vae::DEFAULT,
            "no GPU adapter named, its memory not known".to_string(),
        ),
        (None, _) => (
            Memory::OneAtATime,
            Vae::DEFAULT,
            "the machine not asked".to_string(),
        ),
    };
    let memory = match Memory::from_env(resident) {
        Some(m) => {
            why.push_str(&format!(
                ", GREYCARD_REMOVE_RESIDENT={}",
                resident.unwrap_or("")
            ));
            m
        }
        None => memory,
    };
    let vae = match Vae::from_env(tiled) {
        Some(v) => {
            why.push_str(&format!(", GREYCARD_REMOVE_TILED={}", tiled.unwrap_or("")));
            v
        }
        None => vae,
    };
    Policy { memory, vae, why }
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
    /// `0`, and `None` otherwise, for the machine to decide
    /// ([`policy`]).
    pub fn from_env(var: Option<&str>) -> Option<Self> {
        match var {
            Some("1") => Some(Vae::Tiled),
            Some("0") => Some(Vae::Whole),
            _ => None,
        }
    }

    /// Tiled where the card's memory is not known: the same answer to
    /// 45 dB for a gigabyte less at the peak, measured in the editor.
    pub const DEFAULT: Vae = Vae::Tiled;
}

/// One removal's parts, in seconds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Times {
    /// The square the window was run at.
    pub size: usize,
    /// How far the window was taken down to be run ([`factor_for`]):
    /// 1 at its own pixels.
    pub factor: f32,
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
    /// What the answer was matched to the picture's level by
    /// ([`level`]).
    pub level: Level,
    /// The gains the fill was brought to the picture's level by across
    /// the hole's edge, `None` where it was not ([`Gains`]).
    pub under: Option<Gains>,
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
    let (acp, train) = (alphas_cumprod(), TRAIN);
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

/// The training schedule's cumulative products of the alphas: scaled
/// linear betas from 0.00085 to 0.012 over [`TRAIN`] steps.
fn alphas_cumprod() -> Vec<f64> {
    let (b0, b1) = (0.00085f64.sqrt(), 0.012f64.sqrt());
    let mut acp = Vec::with_capacity(TRAIN);
    let mut prod = 1.0f64;
    for i in 0..TRAIN {
        let b = b0 + (b1 - b0) * i as f64 / (TRAIN - 1) as f64;
        prod *= 1.0 - b * b;
        acp.push(prod);
    }
    acp
}

/// diffusers' `LCMScheduler` (Luo et al., Latent Consistency Models,
/// 2023) over the same training schedule, as the inpainting pipeline
/// runs it: `steps` timesteps picked evenly from every 20th of the
/// training steps, from 999 down ([`LCM_ORIGINAL`]), then cut at
/// `strength` as [`schedule`] is. At 4 and 0.99 that is 999, 759, 499,
/// and 259, the first skipped.
pub struct Lcm {
    /// The timesteps run.
    pub timesteps: Vec<usize>,
    acp: Vec<f64>,
}

impl Lcm {
    pub fn new(steps: usize, strength: f64) -> Self {
        let k = TRAIN / LCM_ORIGINAL;
        // 19, 39, ..., 999, from the top.
        let origin: Vec<usize> = (1..=LCM_ORIGINAL).rev().map(|i| i * k - 1).collect();
        let all: Vec<usize> = (0..steps)
            .map(|i| origin[i * origin.len() / steps])
            .collect();
        let start = steps - ((steps as f64 * strength) as usize).min(steps);
        Self {
            timesteps: all[start..].to_vec(),
            acp: alphas_cumprod(),
        }
    }

    /// The cumulative alpha at the `i`th timestep run.
    pub fn alpha(&self, i: usize) -> f64 {
        self.acp[self.timesteps[i]]
    }

    /// The image's latents noised to the first timestep, as
    /// `add_noise` does: √ᾱ z + √(1 − ᾱ) ε.
    pub fn start(&self, latents: &[f32], noise: &[f32]) -> Vec<f32> {
        let a = self.alpha(0);
        let (s, n) = (a.sqrt() as f32, (1.0 - a).sqrt() as f32);
        latents
            .iter()
            .zip(noise)
            .map(|(z, e)| s * z + n * e)
            .collect()
    }

    /// The `i`th step: the clean latents predicted from the UNet's
    /// noise `eps`, through the consistency boundary's scalings, then
    /// noised again to the next timestep with `noise`; the last step
    /// gives the prediction alone and takes no noise.
    pub fn step(&self, i: usize, x: &mut [f32], eps: &[f32], noise: Option<&[f32]>) {
        let a = self.alpha(i);
        let t = self.timesteps[i] as f64 * LCM_TIME_SCALE;
        let sd2 = LCM_SIGMA_DATA * LCM_SIGMA_DATA;
        let c_skip = (sd2 / (t * t + sd2)) as f32;
        let c_out = (t / (t * t + sd2).sqrt()) as f32;
        let (sa, sb) = (a.sqrt() as f32, (1.0 - a).sqrt() as f32);
        let next = (i + 1 < self.timesteps.len()).then(|| self.alpha(i + 1));
        for (k, (v, e)) in x.iter_mut().zip(eps).enumerate() {
            let x0 = (*v - sb * e) / sa;
            let denoised = c_out * x0 + c_skip * *v;
            *v = match (next, noise) {
                (Some(an), Some(n)) => {
                    an.sqrt() as f32 * denoised + (1.0 - an).sqrt() as f32 * n[k]
                }
                _ => denoised,
            };
        }
    }
}

/// The sampler's loop, from the image's latents (scaled) and `noise`
/// ([`Steps::noises`] latents of it, one after another) to the clean
/// latents, with `unet` answering the noise for the model's input and
/// a timestep.
pub fn sample(
    steps: Steps,
    latents: &[f32],
    noise: &[f32],
    mut unet: impl FnMut(&[f32], f32) -> Result<Vec<f32>>,
) -> Result<Vec<f32>> {
    let n = latents.len();
    if noise.len() != steps.noises() * n {
        return Err(Error::Shape(format!(
            "{} noise values for {} latents and {steps:?}",
            noise.len(),
            n
        )));
    }
    let check = |eps: &[f32]| {
        if eps.len() == n {
            Ok(())
        } else {
            Err(Error::Shape(format!("{} values from the UNet", eps.len())))
        }
    };
    match steps {
        Steps::Euler25 => {
            let (ts, sigmas) = schedule(STEPS, STRENGTH);
            let mut x: Vec<f32> = latents
                .iter()
                .zip(noise)
                .map(|(z, e)| z + e * sigmas[0])
                .collect();
            for i in 0..ts.len() {
                let scale = 1.0 / (sigmas[i] * sigmas[i] + 1.0).sqrt();
                let input: Vec<f32> = x.iter().map(|&v| v * scale).collect();
                let eps = unet(&input, ts[i])?;
                check(&eps)?;
                let dt = sigmas[i + 1] - sigmas[i];
                for (v, e) in x.iter_mut().zip(&eps) {
                    *v += dt * e;
                }
            }
            Ok(x)
        }
        Steps::Lcm4 => {
            let lcm = Lcm::new(LCM_STEPS, STRENGTH);
            let mut x = lcm.start(latents, &noise[..n]);
            let calls = lcm.timesteps.len();
            for i in 0..calls {
                let eps = unet(&x, lcm.timesteps[i] as f32)?;
                check(&eps)?;
                let fresh = (i + 1 < calls).then(|| &noise[(i + 1) * n..(i + 2) * n]);
                lcm.step(i, &mut x, &eps, fresh);
            }
            Ok(x)
        }
    }
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
/// The decoder's answer is first brought to the picture's level round
/// the hole ([`level`]), so the fill does not sit darker than its
/// surround where the blend feathers it in, and then its hole to the
/// picture's level across the hole's edge, so a level the UNet set
/// for the whole hole does not show either ([`Gains`]); last, its
/// chroma is smoothed where it is mottled past the picture's round it
/// ([`mottle`]).
/// [`Error::Implausible`] when the decoder's answer outside the hole
/// strays from the picture by more than [`bound`] allows for its
/// detail ([`drift`]), correlates with it under [`CORRELATION`], or is
/// not finite: what a card out of memory gives.
pub fn composite(image: &Rgbf, hole: &[f32], decoded: &Rgbf) -> Result<Rgbf> {
    leveled(image, hole, decoded).map(|(out, _, _)| out)
}

/// [`composite`], the [`Level`] it matched the answer by, and the
/// gains it put under the hole ([`seam::matched`]).
fn leveled(image: &Rgbf, hole: &[f32], decoded: &Rgbf) -> Result<(Rgbf, Level, Option<Gains>)> {
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
    let matched = level(image, hole, decoded);
    let (decoded, under) = seam::matched(image, hole, &matched.apply(decoded));
    let (decoded, smoothed) = mottle::smoothed(image, hole, &decoded);
    if let Some(s) = smoothed {
        log::info!(
            "remove: the fill's chroma over the picture's round it at the latents' scale, smoothed by {:.2} and {:.2} on the two chroma axes",
            s.amount[0],
            s.amount[1]
        );
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
    Ok((out, matched, under))
}

/// How wide the ring round the hole is that the decoder's answer is
/// matched to the picture in ([`level`]), in the run's pixels.
pub const RING: usize = 24;

/// The radius of the box blur both are low-passed with before the
/// match, so the grain and the model's own texture stay out of it.
const LOWPASS: usize = 8;

/// The least spread (standard deviation) of the answer's low-passed
/// values in the ring, per channel, for a gain to be fitted; under it
/// only an offset is. The fp16 decoder's error is an offset, and on a
/// ring of little tonal range the slope is the model's texture: on
/// three real windows a gain fitted on each half of the ring agreed
/// within 0.01 where the half's spread was 0.11 or more and drifted
/// 0.03 apart at 0.07. Where the ring spans a range the red gain came
/// out 1.03 to 1.04 on all three, the decoder flattening red a little.
const SPREAD: f64 = 0.1;

/// The least ring, in boxes of the low-pass ([`LOWPASS`], 17 a side,
/// about one independent sample each), for a gain to be fitted: a
/// slope from a dozen samples is noise.
const SAMPLES: usize = 16;

/// The gains a fit may give; one past them is taken for a bad fit and
/// the channel matched by its offset alone. The fitted gains on real
/// windows were 0.99 to 1.06.
const GAINS: (f64, f64) = (0.9, 1.1);

/// A per-channel gain and offset that takes the decoder's answer to
/// the picture's level: `picture ≈ gain × answer + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub gain: [f32; 3],
    pub offset: [f32; 3],
}

impl Default for Level {
    fn default() -> Self {
        Self::NONE
    }
}

impl Level {
    /// No change.
    pub const NONE: Level = Level {
        gain: [1.0; 3],
        offset: [0.0; 3],
    };

    /// `image` taken through the gain and offset, clamped to 0 to 1.
    pub fn apply(&self, image: &Rgbf) -> Rgbf {
        if *self == Self::NONE {
            return image.clone();
        }
        let data = image
            .data
            .iter()
            .enumerate()
            .map(|(i, &v)| (v * self.gain[i % 3] + self.offset[i % 3]).clamp(0.0, 1.0))
            .collect();
        Rgbf::new(image.width, image.height, data)
    }
}

/// The pixels outside `hole` (nonzero is in it) within `r` of it, a
/// square's reach, in a `width` × `height` window.
pub fn ring(hole: &[f32], width: usize, height: usize, r: usize) -> Vec<bool> {
    // Dilate by a square: along rows, then along columns, each by the
    // nearest hole pixel's distance in a forward and a backward sweep.
    let reach = |len: usize, at: &dyn Fn(usize) -> bool| -> Vec<bool> {
        let mut near = vec![usize::MAX; len];
        let mut last = None;
        for (i, n) in near.iter_mut().enumerate() {
            if at(i) {
                last = Some(i);
            }
            if let Some(l) = last {
                *n = i - l;
            }
        }
        last = None;
        for i in (0..len).rev() {
            if at(i) {
                last = Some(i);
            }
            if let Some(l) = last {
                near[i] = near[i].min(l - i);
            }
        }
        near.iter().map(|&d| d <= r).collect()
    };
    let mut rows = vec![false; width * height];
    for y in 0..height {
        let row = reach(width, &|x| hole[y * width + x] > 0.0);
        rows[y * width..(y + 1) * width].copy_from_slice(&row);
    }
    let mut out = vec![false; width * height];
    for x in 0..width {
        let col = reach(height, &|y| rows[y * width + x]);
        for (y, &c) in col.iter().enumerate() {
            out[y * width + x] = c && hole[y * width + x] <= 0.0;
        }
    }
    out
}

/// `image`'s channels box-blurred by `radius` over the pixels `known`
/// marks, each the mean of the known pixels round it (0 where none is).
fn known_blur(image: &Rgbf, known: &[bool], radius: usize) -> Vec<[f64; 3]> {
    let (w, h) = (image.width, image.height);
    // A summed-area table of the values and of the count, one wider and
    // taller than the window.
    let stride = w + 1;
    let mut table = vec![[0.0f64; 4]; stride * (h + 1)];
    for y in 0..h {
        let mut row = [0.0f64; 4];
        for x in 0..w {
            let i = y * w + x;
            if known[i] {
                for (r, v) in row.iter_mut().zip(&image.data[i * 3..i * 3 + 3]) {
                    *r += f64::from(v.clamp(0.0, 1.0));
                }
                row[3] += 1.0;
            }
            let above = table[y * stride + x + 1];
            table[(y + 1) * stride + x + 1] = std::array::from_fn(|k| above[k] + row[k]);
        }
    }
    let mut out = vec![[0.0f64; 3]; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(radius), (y + radius + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(radius), (x + radius + 1).min(w));
            let s: [f64; 4] = std::array::from_fn(|k| {
                table[y1 * stride + x1][k] - table[y0 * stride + x1][k] - table[y1 * stride + x0][k]
                    + table[y0 * stride + x0][k]
            });
            if s[3] > 0.0 {
                out[y * w + x] = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
            }
        }
    }
    out
}

/// The gain and offset that take `decoded` to `image`'s level, fitted
/// by least squares per channel over the [`RING`] round `hole`, both
/// low-passed over the known pixels alone ([`LOWPASS`]): the fp16 VAE
/// gives the window back a little darker than it went in, and the
/// blend would show that as a dark rim round the fill and the fill
/// itself low. An offset alone by default, the mean difference over the
/// ring; a gain and offset by least squares only where the ring spans
/// a wide range ([`SPREAD`]) over enough samples ([`SAMPLES`]), and
/// where that gain falls outside [`GAINS`], logged, the offset alone
/// again. [`Level::NONE`] with no ring (no hole, or no known pixel
/// round it).
pub fn level(image: &Rgbf, hole: &[f32], decoded: &Rgbf) -> Level {
    let (w, h) = (image.width, image.height);
    let ring = ring(hole, w, h, RING);
    let n = ring.iter().filter(|&&r| r).count();
    if n == 0 || decoded.width != w || decoded.height != h {
        return Level::NONE;
    }
    let known: Vec<bool> = hole.iter().map(|&m| m <= 0.0).collect();
    let (a, b) = (
        known_blur(decoded, &known, LOWPASS),
        known_blur(image, &known, LOWPASS),
    );
    let mut level = Level::NONE;
    for c in 0..3 {
        let (mut sx, mut sy, mut sxx, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for (i, _) in ring.iter().enumerate().filter(|(_, r)| **r) {
            let (x, y) = (a[i][c], b[i][c]);
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
        }
        let k = n as f64;
        let (mx, my) = (sx / k, sy / k);
        let var = sxx / k - mx * mx;
        let wide = var >= SPREAD * SPREAD && n >= SAMPLES * (2 * LOWPASS + 1).pow(2);
        let fitted = if wide { (sxy / k - mx * my) / var } else { 1.0 };
        let gain = if (GAINS.0..=GAINS.1).contains(&fitted) {
            fitted
        } else {
            log::info!(
                "remove: a gain of {fitted:.3} on channel {c} round the hole is past {:?}; matched by the offset alone",
                GAINS
            );
            1.0
        };
        level.gain[c] = gain as f32;
        level.offset[c] = (my - gain * mx) as f32;
    }
    level
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
    steps: Steps,
    /// Whether the store has the 4-step UNet's files.
    fast: bool,
    /// The empty prompt's embeddings, which both UNets take.
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
    /// removal, and in resident mode they stay open after it. The
    /// machine is not asked: one at a time unless the environment
    /// says otherwise ([`Remove::load_on`]).
    pub fn load(store: &Store, providers: &[Provider]) -> Result<Self> {
        Self::load_on(store, providers, None)
    }

    /// [`Remove::load`], with the [`Memory`] mode and the decoder
    /// chosen for `machine` ([`policy`]).
    pub fn load_on(
        store: &Store,
        providers: &[Provider],
        machine: Option<Machine>,
    ) -> Result<Self> {
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
        let fast = store.have(&REMOVE_FAST);
        let resident = std::env::var("GREYCARD_REMOVE_RESIDENT").ok();
        let tiled = std::env::var("GREYCARD_REMOVE_TILED").ok();
        let Policy { memory, vae, why } = policy(machine, resident.as_deref(), tiled.as_deref());
        log::info!(
            "remove: {why}: {}, the decoder {} (GREYCARD_REMOVE_RESIDENT=1, unet, or 0 and GREYCARD_REMOVE_TILED=1 or 0 choose)",
            match memory {
                Memory::Resident => "all three sessions kept open, about 12 GB held at 1024",
                Memory::UnetResident => "the UNet kept open, 5.1 GB held",
                Memory::OneAtATime => "one session at a time, nothing held",
            },
            match vae {
                Vae::Tiled => "in 512 tiles",
                Vae::Whole => "whole",
            }
        );
        Ok(Self {
            store: store.clone(),
            memory,
            vae,
            steps: Steps::Euler25,
            fast,
            prompt,
            pooled,
            time_ids,
            resident: None,
            times: Times::default(),
        })
    }

    /// Which sampler the next removal runs: 25 Euler steps until
    /// [`Remove::set_steps`] says otherwise.
    pub fn steps(&self) -> Steps {
        self.steps
    }

    /// Whether the store had the 4-step UNet's files at the load.
    pub fn has_fast(&self) -> bool {
        self.fast
    }

    /// Run `steps` from the next removal on: [`Error::Missing`] for
    /// the 4-step sampler when the store does not have its UNet. A UNet
    /// held open for the other sampler is let go first, with a log
    /// line, so two are never held at once; the VAE's stay.
    pub fn set_steps(&mut self, steps: Steps) -> Result<()> {
        if steps == Steps::Lcm4 && !self.fast {
            return Err(Error::Missing(REMOVE_FAST.name));
        }
        if steps != self.steps {
            let held = self.resident.as_mut().and_then(|r| r.unet.take()).is_some();
            if held {
                log::info!(
                    "remove: {} from the next removal: the {} UNet let go, 5.1 GB",
                    steps.words(),
                    match self.steps {
                        Steps::Euler25 => "25-step",
                        Steps::Lcm4 => "4-step",
                    }
                );
            }
            self.steps = steps;
        }
        Ok(())
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
        let noise = normals(self.steps.noises() * 4 * l * l, seed);
        self.run(image, hole, &noise, Pool::Any)
    }

    /// [`Remove::remove`] for a window of a side up to [`FACTOR`] times
    /// [`MAX`]: one past [`MAX`] is taken down until its longer side is
    /// [`MAX`], run, and its fill brought back up under the hole alone
    /// ([`scaled`]); the drift and correlation checks are the run's, at
    /// the size it ran at, and [`Times::factor`] says how far.
    pub fn remove_window(&mut self, image: &Rgbf, hole: &[f32], seed: u64) -> Result<Rgbf> {
        let (w, h) = (image.width, image.height);
        match factor_for(w, h) {
            Some(f) if f > 1.0 => {
                let total = Instant::now();
                let out = scaled(image, hole, MAX, |small, small_hole| {
                    self.remove(small, small_hole, seed)
                });
                self.times.factor = f;
                self.times.total = total.elapsed().as_secs_f64();
                out
            }
            Some(_) => self.remove(image, hole, seed),
            None => Err(Error::Shape(format!(
                "remove wants a side of 1 to {}, got {w}x{h}",
                FACTOR * MAX
            ))),
        }
    }

    /// [`Remove::remove`] on given noise, [`Steps::noises`] latents of
    /// [1, 4, size / 8, size / 8] one after another, with the hole
    /// taken to the latents as diffusers takes it
    /// ([`Pool::TopLeft`]): for matching the reference pipeline.
    #[doc(hidden)]
    pub fn remove_with_noise(&mut self, image: &Rgbf, hole: &[f32], noise: &[f32]) -> Result<Rgbf> {
        let size = self.check(image, hole)?;
        if noise.len() != self.steps.noises() * 4 * (size / 8) * (size / 8) {
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
        if noise.len() != self.steps.noises() * 4 * (size / 8) * (size / 8) {
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
        let path = if part == Part::Unet && self.steps == Steps::Lcm4 {
            self.store
                .dir(&REMOVE_FAST)
                .join(format!("unet-lcm-{size}.onnx"))
        } else {
            self.store
                .path(&REMOVE, Self::graph(part, self.graph_size(part, size)))
        };
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
    /// one kept open when the [`Memory`] mode keeps `part` (the kept
    /// ones dropped when the size changes), else one opened for `f`
    /// and dropped after.
    fn with<T>(
        &mut self,
        part: Part,
        size: usize,
        f: impl FnOnce(&mut Session) -> Result<T>,
    ) -> Result<(T, f64)> {
        let keep = self.memory.keeps(part);
        let kept = if keep {
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
        } else {
            None
        };
        let t = Instant::now();
        let mut session = match kept {
            Some(s) => s,
            None => self.open(part, size)?,
        };
        let load = t.elapsed().as_secs_f64();
        let out = f(&mut session);
        if let Some(r) = self.resident.as_mut().filter(|r| keep && r.size == size) {
            *r.slot(part) = Some(session);
        }
        Ok((out?, load))
    }

    fn run(&mut self, image: &Rgbf, hole: &[f32], noise: &[f32], pool: Pool) -> Result<Rgbf> {
        let (w, h) = (image.width, image.height);
        let total = Instant::now();
        let (decoded, mut times) = self.generate(image, hole, noise, pool)?;
        times.drift = drift(image, hole, &decoded);
        let out = leveled(image, hole, &decoded).map(|(out, level, under)| {
            times.level = level;
            times.under = under;
            out
        });
        times.total = total.elapsed().as_secs_f64();
        self.times = times;
        log::info!(
            "remove {w}x{h} at {}: loads {:.2}s, encode {:.2}s, {} UNet calls {:.2}s, decode {:.2}s, {:.2}s in all ({:?}, decoder {}); outside the hole off by {:.4} where its detail is {:.4}, correlation {}; matched to the picture round the hole by gains {:.4?} and offsets {:.4?}",
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
                .map_or("not asked (flat)".to_string(), |r| format!("{r:.3}")),
            times.level.gain,
            times.level.offset
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
            factor: 1.0,
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
        let latents: Vec<f32> = image_lat.iter().map(|z| z * SCALE).collect();
        let steps = self.steps;
        let mut calls = 0;
        let t = Instant::now();
        let (x, load) = self.with(Part::Unet, size, |s| {
            sample(steps, &latents, noise, |input, timestep| {
                calls += 1;
                let mut sample: Vec<f16> = Vec::with_capacity(9 * l * l);
                sample.extend(input.iter().map(|&v| f16::from_f32(v)));
                sample.extend_from_slice(&mask16);
                sample.extend_from_slice(&masked16);
                let out = s.run(ort::inputs![
                    "sample" => Tensor::from_array(([1usize, 9, l, l], sample))?,
                    "timestep" => Tensor::from_array(([1usize], vec![timestep]))?,
                    "encoder_hidden_states" => Tensor::from_array(([1usize, 77, 2048], prompt.clone()))?,
                    "text_embeds" => Tensor::from_array(([1usize, POOLED], pooled.clone()))?,
                    "time_ids" => Tensor::from_array(([1usize, 6], ids.clone()))?,
                ])?;
                let (_, eps) = out["out_sample"].try_extract_tensor::<f16>()?;
                Ok(eps.iter().map(|e| e.to_f32()).collect())
            })
        })?;
        times.loads += load;
        times.steps = t.elapsed().as_secs_f64() - load;
        times.unet_calls = calls;
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

    /// diffusers' `LCMScheduler` from RORem's Euler config,
    /// `set_timesteps(4)` and the cut at strength 0.99 (diffusers
    /// 0.41.0, float32): the timesteps and their cumulative alphas.
    const LCM_ALL: [usize; 4] = [999, 759, 499, 259];
    const LCM_ALPHAS: [f64; 4] = [
        4.660_095_088e-3,
        5.221_289_024e-2,
        2.776_694_298e-1,
        6.589_752_436e-1,
    ];

    #[test]
    fn the_lcm_schedule_is_diffusers_own() {
        let lcm = Lcm::new(LCM_STEPS, STRENGTH);
        assert_eq!(lcm.timesteps, LCM_ALL[1..]);
        assert_eq!(Lcm::new(LCM_STEPS, 1.0).timesteps, LCM_ALL);
        let all = Lcm::new(LCM_STEPS, 1.0);
        for (i, want) in LCM_ALPHAS.iter().enumerate() {
            let got = all.alpha(i);
            assert!(
                (got - want).abs() <= 1e-6 * want,
                "{i}: {got} against {want}"
            );
        }
        assert_eq!(Steps::Lcm4.noises(), 3);
        assert_eq!(Steps::Euler25.noises(), 1);
    }

    /// One LCM step on a known input, sin(0.7 i) for the latents and
    /// cos(0.3 i) for the UNet's noise, i from 0 to 15, against
    /// diffusers' `LCMScheduler.step`: at 759 with the noise its
    /// generator drew (torch's seed 0), and at 259, the last, with none.
    #[test]
    fn an_lcm_step_is_diffusers_own() {
        let sample: Vec<f32> = (0..16).map(|i| (i as f32 * 0.7).sin()).collect();
        let eps: Vec<f32> = (0..16).map(|i| (i as f32 * 0.3).cos()).collect();
        let noise = [
            -1.125_839_8,
            -1.152_360_2,
            -0.250_578_58,
            -0.433_878_8,
            0.848_710_36,
            0.692_009_15,
            -0.316_012_77,
            -2.115_219_4,
            0.322_274_92,
            -1.263_334_8,
            0.349_983_2,
            0.308_133_93,
            0.119_841_51,
            1.237_657_9,
            1.116_777_2,
            -0.247_278_15,
        ];
        let next = [
            -3.201_925,
            -1.638_571_9,
            0.206_624_18,
            0.226_319_94,
            0.680_310_6,
            -0.379_607_08,
            -1.768_420_6,
            -2.929_927_3,
            0.473_650_7,
            0.994_773_5,
            4.035_123_3,
            4.757_647,
            4.085_919_4,
            3.417_526_2,
            1.204_688_2,
            -1.765_562_3,
        ];
        let last = [
            -0.719_38,
            0.106_343_45,
            0.620_217_56,
            0.616_189_4,
            0.151_989_56,
            -0.483_006_83,
            -0.910_224_6,
            -0.847_079_16,
            -0.247_173_19,
            0.671_083_7,
            1.521_503_9,
            1.927_669_5,
            1.697_866_4,
            0.915_309_97,
            -0.098_771_8,
            -0.932_03,
        ];
        let lcm = Lcm::new(LCM_STEPS, STRENGTH);
        for (i, noise, want) in [(0, Some(&noise[..]), next), (2, None, last)] {
            let mut x = sample.clone();
            lcm.step(i, &mut x, &eps, noise);
            for (k, (a, b)) in x.iter().zip(want).enumerate() {
                assert!(
                    (a - b).abs() <= 1e-5 * b.abs().max(1.0),
                    "step {i}, {k}: {a} against {b}"
                );
            }
        }
    }

    /// Four steps on a stand-in UNet that answers its input as the
    /// noise: the same seed makes the same latents, another seed others,
    /// and every call sees one of the schedule's timesteps.
    #[test]
    fn four_steps_are_the_seeds() {
        let n = 4 * 8 * 8;
        let latents: Vec<f32> = (0..n).map(|i| (i as f32 * 0.37).sin()).collect();
        let run = |seed| {
            let mut seen = Vec::new();
            let noise = normals(Steps::Lcm4.noises() * n, seed);
            let x = sample(Steps::Lcm4, &latents, &noise, |input, t| {
                seen.push(t);
                Ok(input.to_vec())
            })
            .unwrap();
            (x, seen)
        };
        let (a, seen) = run(7);
        assert_eq!(seen, [759.0, 499.0, 259.0]);
        assert!(a.iter().all(|v| v.is_finite()));
        assert_eq!(a, run(7).0);
        assert_ne!(a, run(8).0);
        // Too little noise is refused.
        assert!(matches!(
            sample(Steps::Lcm4, &latents, &normals(n, 7), |i, _| Ok(i.to_vec())),
            Err(Error::Shape(_))
        ));
    }

    /// A load runs 25 steps whatever the store holds; four are taken
    /// only with the four-step files beside the remove's, and refused
    /// as missing otherwise. Nothing is opened: the files are blanks
    /// of their published sizes.
    #[test]
    fn four_steps_are_asked_for_and_want_their_files() {
        let root = std::env::temp_dir().join(format!(
            "greycard-remove-steps-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::at(&root);
        let lay = |m: &crate::Model| {
            for f in m.files {
                let path = store.path(m, f);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::File::create(&path)
                    .unwrap()
                    .set_len(f.bytes)
                    .unwrap();
            }
        };
        lay(&REMOVE);
        let mut remove = Remove::load(&store, &[Provider::WebGpu]).expect("load");
        assert_eq!(remove.steps(), Steps::Euler25);
        assert!(!remove.has_fast());
        assert!(matches!(
            remove.set_steps(Steps::Lcm4),
            Err(Error::Missing(_))
        ));
        assert_eq!(remove.steps(), Steps::Euler25);
        lay(&REMOVE_FAST);
        let mut remove = Remove::load(&store, &[Provider::WebGpu]).expect("load");
        assert_eq!(remove.steps(), Steps::Euler25, "not chosen by the store");
        assert!(remove.has_fast());
        remove.set_steps(Steps::Lcm4).expect("four steps");
        assert_eq!(remove.steps(), Steps::Lcm4);
        remove.set_steps(Steps::Euler25).expect("25 steps");
        assert_eq!(remove.steps(), Steps::Euler25);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_steps_are_read_from_the_environment() {
        assert_eq!(Steps::from_env(None), None);
        assert_eq!(Steps::from_env(Some("4")), Some(Steps::Lcm4));
        assert_eq!(Steps::from_env(Some("25")), Some(Steps::Euler25));
        assert_eq!(Steps::from_env(Some("8")), None);
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
        // The decoder the picture outside, 0.9 inside (one off by a
        // level is matched first: the test above).
        let mut decoded = image.clone();
        for c in 0..3 {
            decoded.data[(5 * w + 5) * 3 + c] = 0.9;
            decoded.data[(5 * w + 6) * 3 + c] = 0.9;
        }
        let out = composite(&image, &hole, &decoded).unwrap();
        assert_eq!(out.at(0, 0), image.at(0, 0));
        for v in out.at(5, 5) {
            assert!((v - 0.9).abs() < 1e-5, "{v}");
        }
        // Half way where the hole is half.
        let p = image.at(6, 5);
        for (got, p) in out.at(6, 5).iter().zip(p) {
            assert!((got - (p + 0.9) / 2.0).abs() < 1e-5);
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

    /// A `w` × `h` picture of `f` at each pixel's place, the same in
    /// every channel, a hole of radius 40 in its middle, and the
    /// decoder's answer the picture times `gain` plus `offset`.
    fn leveled_case(
        f: impl Fn(f32, f32, usize) -> f32,
        gain: f32,
        offset: f32,
    ) -> (Rgbf, Vec<f32>, Rgbf) {
        let (w, h) = (256, 192);
        let image = Rgbf::new(
            w,
            h,
            (0..w * h)
                .flat_map(|i| [f((i % w) as f32, (i / w) as f32, i); 3])
                .collect(),
        );
        let hole = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 - 120.0, (i / w) as f32 - 90.0);
                f32::from(u8::from(x * x + y * y < 40.0 * 40.0))
            })
            .collect();
        let decoded = Rgbf::new(
            w,
            h,
            image.data.iter().map(|&v| v * gain + offset).collect(),
        );
        (image, hole, decoded)
    }

    /// The mean of `d` less `image` over the ring round `hole`.
    fn ring_off(image: &Rgbf, hole: &[f32], d: &Rgbf) -> [f64; 3] {
        let ring = ring(hole, image.width, image.height, RING);
        let n = ring.iter().filter(|&&r| r).count() as f64;
        let mut s = [0.0f64; 3];
        for (i, _) in ring.iter().enumerate().filter(|(_, r)| **r) {
            for (c, v) in s.iter_mut().enumerate() {
                *v += f64::from(d.data[i * 3 + c] - image.data[i * 3 + c]) / n;
            }
        }
        s
    }

    /// The composite of `decoded` is the picture to the bit outside
    /// the hole and within `inside` of it under the hole after the
    /// ring's match alone, within `under` after the match across the
    /// hole's edge too (which reads the texture at the edge, so is
    /// looser on a picture of strong patches).
    fn composite_within(image: &Rgbf, hole: &[f32], decoded: &Rgbf, inside: f32, under: f32) {
        let ringed = level(image, hole, decoded).apply(decoded);
        let out = composite(image, hole, decoded).expect("an honest answer");
        for (i, &m) in hole.iter().enumerate() {
            for c in 0..3 {
                let (a, b) = (out.data[i * 3 + c], image.data[i * 3 + c]);
                if m <= 0.0 {
                    assert_eq!(a.to_bits(), b.to_bits());
                } else {
                    let r = ringed.data[i * 3 + c];
                    assert!((r - b).abs() < inside, "{r} against {b}");
                    assert!((a - b).abs() < under, "{a} against {b}");
                }
            }
        }
    }

    /// A decoder's answer a little dark and flat, the picture times
    /// 0.99 less 0.003, on a ring of grain round a flat grey: too
    /// even for a gain, so the offset alone, which brings the ring's
    /// mean back and leaves under the hole a hundredth of the grain.
    #[test]
    fn an_even_ring_is_matched_by_its_offset() {
        let grain = |i: usize| ((i * 7919) % 101) as f32 / 101.0 * 0.04;
        let (image, hole, decoded) = leveled_case(|_, _, i| 0.4 + grain(i), 0.99, -0.003);
        assert!(
            ring_off(&image, &hole, &decoded)
                .iter()
                .all(|v| *v < -0.006)
        );
        let l = level(&image, &hole, &decoded);
        assert_eq!(l.gain, [1.0; 3]);
        for o in l.offset {
            assert!((o - (0.01 * 0.42 + 0.003)).abs() < 2e-4, "{o}");
        }
        for v in ring_off(&image, &hole, &l.apply(&decoded)) {
            assert!(v.abs() < 1e-4, "{v}");
        }
        composite_within(&image, &hole, &decoded, 5e-4, 2e-3);
        // No hole, or nothing round it: nothing to match by.
        let (w, h) = (image.width, image.height);
        assert_eq!(level(&image, &vec![0.0; w * h], &decoded), Level::NONE);
        assert_eq!(level(&image, &vec![1.0; w * h], &decoded), Level::NONE);
    }

    /// The same answer on a ring of wide range, light and dark
    /// patches: the gain is fitted, 1 / 0.99, and the ring and the hole
    /// come back to the picture.
    #[test]
    fn a_wide_ring_is_matched_by_a_gain() {
        let patches = |x: f32, y: f32, _| 0.5 + 0.4 * (x / 10.0).sin() * (y / 12.0).sin();
        let (image, hole, decoded) = leveled_case(patches, 0.99, -0.003);
        let l = level(&image, &hole, &decoded);
        for (g, o) in l.gain.iter().zip(l.offset) {
            assert!((g - 1.0 / 0.99).abs() < 1e-3, "{g}");
            assert!((o - 0.003 / 0.99).abs() < 1e-3, "{o}");
        }
        for v in ring_off(&image, &hole, &l.apply(&decoded)) {
            assert!(v.abs() < 1e-4, "{v}");
        }
        composite_within(&image, &hole, &decoded, 1e-4, 5e-3);
        // A gain past what a decoder gives is a bad fit: the offset
        // alone.
        let (image, hole, decoded) = leveled_case(patches, 0.8, 0.0);
        let l = level(&image, &hole, &decoded);
        assert_eq!(l.gain, [1.0; 3]);
        for v in ring_off(&image, &hole, &l.apply(&decoded)) {
            assert!(v.abs() < 1e-3, "{v}");
        }
    }

    #[test]
    fn the_ring_is_the_band_round_the_hole() {
        let (w, h) = (20, 10);
        let hole: Vec<f32> = (0..w * h)
            .map(|i| f32::from(u8::from(i % w == 5 && i / w == 5)))
            .collect();
        let r = ring(&hole, w, h, 2);
        assert_eq!(r.iter().filter(|&&v| v).count(), 24);
        assert!(!r[5 * w + 5] && r[3 * w + 3] && r[7 * w + 7] && !r[2 * w + 5]);
    }

    /// A machine of `gib` GiB as the system reports it: a Mac's
    /// `hw.memsize` is the GiB it is sold as, a Linux box's a little
    /// under.
    fn machine(gib: f64, unified: bool) -> Option<Machine> {
        Some(Machine {
            memory: Some((gib * (1u64 << 30) as f64) as u64),
            unified: Some(unified),
        })
    }

    fn chosen(machine: Option<Machine>) -> (Memory, Vae) {
        let p = policy(machine, None, None);
        (p.memory, p.vae)
    }

    #[test]
    fn unified_memory_keeps_more_open_as_it_grows() {
        let one = (Memory::OneAtATime, Vae::Tiled);
        let unet = (Memory::UnetResident, Vae::Tiled);
        let whole = (Memory::UnetResident, Vae::Whole);
        let all = (Memory::Resident, Vae::Whole);
        assert_eq!(chosen(machine(8.0, true)), one);
        assert_eq!(chosen(machine(14.0, true)), one);
        assert_eq!(chosen(machine(15.4, true)), unet);
        assert_eq!(chosen(machine(16.0, true)), unet);
        assert_eq!(chosen(machine(18.0, true)), unet);
        assert_eq!(chosen(machine(22.4, true)), unet);
        assert_eq!(chosen(machine(22.6, true)), whole);
        assert_eq!(chosen(machine(24.0, true)), whole);
        assert_eq!(chosen(machine(31.2, true)), all);
        assert_eq!(chosen(machine(36.0, true)), all);
        assert_eq!(chosen(machine(128.0, true)), all);
    }

    #[test]
    fn a_discrete_card_or_an_unknown_machine_keeps_to_one_at_a_time() {
        let one = (Memory::OneAtATime, Vae::DEFAULT);
        assert_eq!(chosen(machine(64.0, false)), one);
        assert_eq!(chosen(None), one);
        let silent = Some(Machine {
            memory: None,
            unified: Some(true),
        });
        assert_eq!(chosen(silent), one);
        let no_adapter = Some(Machine {
            memory: Some(64 << 30),
            unified: None,
        });
        assert_eq!(chosen(no_adapter), one);
    }

    #[test]
    fn the_environment_wins_over_the_machine() {
        let big = machine(64.0, true);
        let small = machine(8.0, false);
        let p = policy(big, Some("0"), None);
        assert_eq!((p.memory, p.vae), (Memory::OneAtATime, Vae::Whole));
        assert_eq!(policy(big, Some("unet"), None).memory, Memory::UnetResident);
        assert_eq!(policy(big, None, Some("1")).vae, Vae::Tiled);
        let p = policy(small, Some("1"), Some("0"));
        assert_eq!((p.memory, p.vae), (Memory::Resident, Vae::Whole));
        assert_eq!(
            policy(small, Some("unet"), None).memory,
            Memory::UnetResident
        );
        // Anything else leaves it to the machine.
        assert_eq!(policy(big, Some("yes"), Some("")).memory, Memory::Resident);
        assert_eq!(policy(big, Some("yes"), Some("")).vae, Vae::Whole);
    }

    #[test]
    fn the_log_says_what_chose() {
        assert_eq!(
            policy(machine(24.0, true), None, None).why,
            "24 GiB unified"
        );
        assert_eq!(
            policy(machine(24.0, true), Some("1"), None).why,
            "24 GiB unified, GREYCARD_REMOVE_RESIDENT=1"
        );
        assert_eq!(
            policy(machine(16.0, false), None, Some("0")).why,
            "a discrete card, its memory not known, GREYCARD_REMOVE_TILED=0"
        );
        let no_adapter = Some(Machine {
            memory: Some(64 << 30),
            unified: None,
        });
        assert_eq!(
            policy(no_adapter, None, None).why,
            "no GPU adapter named, its memory not known"
        );
    }

    #[test]
    fn each_mode_keeps_its_parts() {
        let parts = [Part::Encoder, Part::Unet, Part::Decoder];
        let kept = |m: Memory| parts.map(|p| m.keeps(p));
        assert_eq!(kept(Memory::Resident), [true; 3]);
        assert_eq!(kept(Memory::UnetResident), [false, true, false]);
        assert_eq!(kept(Memory::OneAtATime), [false; 3]);
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
