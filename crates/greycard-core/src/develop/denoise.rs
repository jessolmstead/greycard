//! Profiled noise reduction: wavelet shrinkage in a variance-stabilized
//! space, from a noise model.
//!
//! Ported from darktable's `src/iop/denoiseprofile.c` and
//! `src/common/eaw.c` (copyright 2012-2024 darktable developers, the
//! wavelet path by Johannes Hanika and the profiled transform work by
//! Rawfiner; GPL-3.0-or-later), the "wavelets auto" mode in its Y0U0V0
//! color mode. The transform, the à trous wavelet with its edge guard,
//! the per-band BayesShrink threshold with its empirical constants, and
//! the scale count are theirs. The buffer layout, the threading and the
//! local variance measurement are ours, and two things are simplified,
//! below.
//!
//! The idea: with the model `var = a x + b` per channel, the generalized
//! Anscombe transform `2 sqrt(x/a + 3/8 + b/a^2)` turns every channel into
//! one whose noise has standard deviation 1 everywhere. In that space the
//! image is rotated into a luma and two chroma channels, decomposed with
//! an undecimated 5-tap B3 spline wavelet into detail bands, each band
//! soft-thresholded by a BayesShrink rule from the band's measured
//! variance and the known noise variance at that scale, summed back, and
//! rotated and transformed back with a closed-form unbiased inverse
//! (Mäkitalo and Foi).
//!
//! **Departures.** darktable uses one profile, the green channel's, for
//! all three channels and adapts it with the white balance coefficients,
//! which is why its luma/chroma matrix is white-balance-adaptive and why
//! its thresholds carry empirical multipliers (a 2.5 on the assumed
//! noise, an 8 on the shrink, tuned against that matrix). greycard has a
//! model per channel, so the transform is applied per channel and the
//! rotation is the plain orthonormal Y0U0V0 of Lebrun, Colom and Morel:
//! noise stays white with unit variance in every channel, exactly, and
//! the threshold is the textbook BayesShrink, the band's noise variance
//! (computed from the kernel, not approximated) over the band's signal
//! deviation, times [`DenoiseOptions::strength`], set on the benchmark
//! and, by default, following the measured noise. darktable's newer transform with its "preserve
//! shadows" exponent and "bias correction" sliders is not ported either:
//! the classic transform with the unbiased inverse has no knobs and no
//! bias to correct.

use rayon::prelude::*;

use super::noise::NoiseModel;
use crate::error::{Error, Result};

/// How hard to denoise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DenoiseOptions {
    /// Wavelet shrinkage or non-local means.
    pub method: DenoiseMethod,
    /// Multiplier on the BayesShrink thresholds: 1.0 is the textbook
    /// rule, more smooths more. For non-local means it divides the
    /// patch dissimilarity instead, to the same effect.
    pub strength: Strength,
    /// Further multiplier on the two chrominance channels' thresholds.
    /// Color noise has no fine detail worth keeping, so it can be
    /// shrunk harder than luminance without softening the picture.
    pub chroma: f32,
    /// Settings for the non-local means method.
    pub nlm: super::nlm::NlmOptions,
    /// For the hybrid: the first wavelet scale shrunk after the means.
    pub hybrid_from: usize,
}

impl Default for DenoiseOptions {
    fn default() -> Self {
        Self {
            method: DenoiseMethod::Hybrid,
            strength: Strength::Auto,
            chroma: DEFAULT_CHROMA,
            nlm: super::nlm::NlmOptions::default(),
            hybrid_from: DEFAULT_HYBRID_FROM,
        }
    }
}

/// Which denoiser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DenoiseMethod {
    /// À trous wavelets with locally adaptive BayesShrink, this module.
    Wavelets,
    /// Non-local means, [`super::nlm`].
    Nlm,
    /// Non-local means, then the wavelets from
    /// [`DenoiseOptions::hybrid_from`] up: the means for the fine grain
    /// and the specks, the wavelets for the coarse mottling a small
    /// search window cannot see. The default: best on the benchmark
    /// from moderate noise up and by eye at every ISO (notes §13r).
    #[default]
    Hybrid,
}

impl DenoiseMethod {
    pub const ALL: [DenoiseMethod; 3] = [
        DenoiseMethod::Wavelets,
        DenoiseMethod::Nlm,
        DenoiseMethod::Hybrid,
    ];

    pub fn name(self) -> &'static str {
        match self {
            DenoiseMethod::Wavelets => "wavelets",
            DenoiseMethod::Nlm => "nlm",
            DenoiseMethod::Hybrid => "hybrid",
        }
    }
}

impl std::str::FromStr for DenoiseMethod {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|m| m.name() == s)
            .ok_or_else(|| format!("{s}: want wavelets, nlm or hybrid"))
    }
}

/// The threshold multiplier, chosen or left to the noise.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Strength {
    /// From the model's noise at mid grey: [`STRENGTH_LOW`] up to a
    /// sigma of [`SIGMA_LOW`], [`STRENGTH_HIGH`] from [`SIGMA_HIGH`],
    /// a straight line between. The bench put the optimum at 1.5 for
    /// noise 0.01 and at 2.5 for 0.05, with a tie at 0.03 (notes §13n).
    #[default]
    Auto,
    Fixed(f32),
}

impl Strength {
    /// The multiplier for a model.
    pub fn resolve(self, model: &NoiseModel) -> f32 {
        match self {
            Strength::Fixed(s) => s,
            Strength::Auto => {
                let sigma = model.sigma(1, super::noise::MID_GREY);
                let t = ((sigma - SIGMA_LOW) / (SIGMA_HIGH - SIGMA_LOW)).clamp(0.0, 1.0);
                STRENGTH_LOW + (STRENGTH_HIGH - STRENGTH_LOW) * t
            }
        }
    }
}

impl std::str::FromStr for Strength {
    type Err = String;

    /// `auto` or a number.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("auto") {
            return Ok(Strength::Auto);
        }
        s.parse::<f32>()
            .map(Strength::Fixed)
            .map_err(|_| format!("{s}: want auto or a number"))
    }
}

/// What the denoise did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DenoiseStats {
    pub method: DenoiseMethod,
    /// Wavelet scales processed; 0 when the model was silent, the
    /// image too small, or the method not wavelets.
    pub scales: usize,
    /// The wavelets' strength; the means' is in `nlm`.
    pub strength: f32,
    /// What non-local means did, when it ran.
    pub nlm: Option<super::nlm::NlmStats>,
}

/// The most wavelet scales darktable processes.
pub(crate) const MAX_BANDS: usize = 7;

/// The strength the benchmark chose for quiet frames (notes §13g, §13l).
pub const STRENGTH_LOW: f32 = 1.5;
/// The strength the benchmark chose for very noisy frames (notes §13n).
pub const STRENGTH_HIGH: f32 = 2.5;
/// Green-channel sigma at mid grey up to which [`STRENGTH_LOW`] applies.
pub const SIGMA_LOW: f32 = 0.03;
/// Green-channel sigma at mid grey from which [`STRENGTH_HIGH`] applies.
pub const SIGMA_HIGH: f32 = 0.05;
/// The chrominance multiplier the benchmark chose (notes §13k).
pub const DEFAULT_CHROMA: f32 = 2.0;
/// The hybrid's first shrunk wavelet scale (notes §13r).
pub const DEFAULT_HYBRID_FROM: usize = 2;

const FILTER: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// The variance of unit white noise in each detail band: the squared
/// norm of the difference between consecutive à trous smoothing
/// kernels, which are separable products of the dilated 5-tap filter.
pub(crate) fn band_variances() -> [f32; MAX_BANDS] {
    // 1D kernel after s levels, centered in a buffer wide enough for all.
    let span = 2 * (2usize << MAX_BANDS);
    let center = span / 2;
    let mut prev = vec![0.0f64; 2 * span + 1];
    prev[center] = 1.0;
    let mut out = [0.0f32; MAX_BANDS];
    for (s, v) in out.iter_mut().enumerate() {
        let mult = 1usize << s;
        let mut next = vec![0.0f64; prev.len()];
        for (i, &p) in prev.iter().enumerate() {
            if p == 0.0 {
                continue;
            }
            for (t, &f) in FILTER.iter().enumerate() {
                let j = i as isize + mult as isize * (t as isize - 2);
                if j >= 0 && (j as usize) < next.len() {
                    next[j as usize] += p * f as f64;
                }
            }
        }
        let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
        let (pp, pn, nn) = (dot(&prev, &prev), dot(&prev, &next), dot(&next, &next));
        *v = (pp * pp - 2.0 * pn * pn + nn * nn) as f32;
        prev = next;
    }
    out
}

/// Denoise interleaved RGB in place. `model` is the noise of `rgb` as it
/// is (after white balance, so see [`NoiseModel::after_gains`]).
///
/// Two passes. The first measures each band's local variance on a
/// decimated pyramid ([`variance_grids`]); the second runs the à trous
/// chain over three planes, luma and the two chromas, a band of rows at
/// a time, and collects the shrunk detail in the caller's buffer, which
/// the planes are the one other frame this allocates beside.
pub fn denoise_profiled(
    rgb: &mut [f32],
    width: usize,
    height: usize,
    model: &NoiseModel,
    options: &DenoiseOptions,
) -> Result<DenoiseStats> {
    if rgb.len() != width * height * 3 {
        return Err(Error::Unsupported(
            "denoise: RGB buffer size mismatch".into(),
        ));
    }
    // Each method has its own automatic ramp (§13n, §13r).
    let strength = match (options.method, options.strength) {
        (DenoiseMethod::Nlm | DenoiseMethod::Hybrid, Strength::Auto) => {
            super::nlm::auto_strength(model.sigma(1, super::noise::MID_GREY))
        }
        (_, strength) => strength.resolve(model),
    };
    let nothing = DenoiseStats {
        method: options.method,
        scales: 0,
        strength,
        nlm: None,
    };
    if strength <= 0.0 || strength.is_nan() || model.is_silent() {
        return Ok(nothing);
    }
    // A channel with no noise of its own still has to go through the
    // transform; give it a floor so the transform is finite.
    let mut model = *model;
    for c in 0..3 {
        if model.sigma(c, super::noise::MID_GREY) < super::noise::SILENT_SIGMA {
            model.b[c] = super::noise::SILENT_SIGMA * super::noise::SILENT_SIGMA;
        }
    }
    let scales = scale_count(width, height);
    let room = scales > 0 && width >= 2 << (scales - 1) && height >= 2 << (scales - 1);
    let vst: [Vst; 3] = std::array::from_fn(|c| Vst::new(model.a[c], model.b[c]));
    let clock = std::time::Instant::now();
    let mut mark = clock;
    let mut lap = |what: &str| {
        let now = std::time::Instant::now();
        log::debug!("denoise: {what} {:.0} ms", (now - mark).as_secs_f64() * 1e3);
        mark = now;
    };

    // Into the stabilized space once, for both the means and the
    // wavelets, and out once at the end: a second pass through the
    // unbiased inverse shifts every level by a fraction of the noise
    // (notes §13u).
    rgb.par_chunks_mut(3).for_each(|px| {
        for c in 0..3 {
            px[c] = vst[c].forward(px[c]);
        }
    });
    let unstabilize = |rgb: &mut [f32]| {
        rgb.par_chunks_mut(3).for_each(|px| {
            for c in 0..3 {
                px[c] = vst[c].inverse(px[c]);
            }
        });
    };

    lap("forward transform");
    let mut nlm_stats = None;
    let mut first_scale = 0;
    // What the means leave in each band, as a fraction of the white
    // noise the wavelets otherwise assume: all of it at the coarse
    // scales, next to nothing at the fine ones.
    let mut band_left = [1.0f32; MAX_BANDS];
    if options.method != DenoiseMethod::Wavelets {
        let stats = super::nlm::denoise_stabilized(
            rgb,
            width,
            height,
            model.sigma(1, super::noise::MID_GREY),
            strength,
            &options.nlm,
        );
        lap("non-local means");
        if options.method == DenoiseMethod::Nlm || !room {
            unstabilize(rgb);
            return Ok(DenoiseStats {
                nlm: Some(stats),
                ..nothing
            });
        }
        band_left = super::nlm::residual_band_ratios(&stats, options.nlm.search_radius);
        lap("residual band ratios");
        nlm_stats = Some(stats);
        first_scale = options.hybrid_from;
    } else if !room {
        unstabilize(rgb);
        return Ok(nothing);
    }
    // The hybrid's wavelet strength is the wavelets' own ramp; the
    // strength given applies to the means.
    let strength = match options.method {
        DenoiseMethod::Hybrid => Strength::Auto.resolve(&model),
        _ => strength,
    };
    let nothing = DenoiseStats {
        nlm: nlm_stats,
        strength,
        ..nothing
    };

    let band_var = band_variances();
    let n = width * height;

    // The chain runs on three planes rather than the interleaved
    // triples, so a tap of the blur is a loop over contiguous floats.
    // The rotation to luma and chroma reads the caller's buffer into
    // them, and from then on that buffer is the accumulator for the
    // shrunk detail; the answer goes back into it at the end.
    let mut planes = vec![0.0f32; 3 * n];
    to_planes(rgb, &mut planes);
    lap("rotation to luma and chroma");
    let grids = variance_grids(&planes, width, height, scales);
    lap("variance grids");
    let acc = rgb;
    acc.par_chunks_mut(CHUNK).for_each(|c| c.fill(0.0));
    let mut bands = Bands::new(width, height, scales);
    lap("accumulator and band buffers");

    let gain = [
        strength,
        strength * options.chroma.max(0.0),
        strength * options.chroma.max(0.0),
    ];
    for (scale, &sb2) in band_var.iter().enumerate().take(scales) {
        // Scales the means already cleaned pass through unshrunk, and
        // the others are shrunk against the noise the means left.
        let gain = if scale < first_scale { [0.0; 3] } else { gain };
        let sb2 = sb2 * band_left[scale];
        // BayesShrink per channel and per place: threshold = noise
        // variance over the signal's standard deviation, the latter from
        // the band's variance around here less the noise's. A tile of
        // pure noise measures the noise variance give or take sqrt(2/N)
        // of it; only what clears that margin counts as signal, so flat
        // areas get an infinite threshold rather than one drawn from the
        // estimate's own scatter.
        let shrink = Shrink {
            grid: &grids[scale],
            noise_floor: sb2 * (1.0 + 2.0 * (2.0 / LocalVariance::SAMPLES as f32).sqrt()),
            gain,
            sb2,
        };
        atrous_level(
            &mut planes,
            acc,
            width,
            height,
            scale,
            sb2,
            &shrink,
            &mut bands,
        );
        lap(&format!("scale {scale}"));
    }

    // Residue plus detail, then rotated back and unstabilized into the
    // caller's buffer. Two passes: the detail lives in that buffer as
    // planes, and its interleaved rows land on planar rows not yet read.
    planes
        .par_chunks_mut(CHUNK)
        .zip(acc.par_chunks(CHUNK))
        .for_each(|(p, a)| {
            for (p, a) in p.iter_mut().zip(a) {
                *p += *a;
            }
        });
    from_planes(&planes, acc, &vst);
    lap("sum, rotation back and inverse transform");
    log::debug!(
        "denoise: {:.0} ms in all",
        clock.elapsed().as_secs_f64() * 1e3
    );
    Ok(DenoiseStats { scales, ..nothing })
}

/// darktable's scale count: as many as fit a filter a fifth of the
/// image's larger side, at most [`MAX_BANDS`].
fn scale_count(width: usize, height: usize) -> usize {
    let largest = (2 * (2u32 << (MAX_BANDS - 1)) + 1) as f32;
    let supp0 = largest.min(width.max(height) as f32 * 0.2);
    let i0 = ((supp0 - 1.0) * 0.5).log2();
    (0..MAX_BANDS)
        .take_while(|&s| 1.0 - (s as f32 + 0.5) / i0 >= 0.0)
        .count()
}

/// One channel's transform, arranged so a model with `a` near or at zero
/// (read noise only) stays finite in single precision.
///
/// The generalized Anscombe transform is `2 sqrt(x/a + 3/8 + b/a^2)`;
/// shifted to be zero at `x = 0` and multiplied through by `a` it is
/// `2x / (sqrt(a x + s0^2) + s0)` with `s0 = sqrt(3a^2/8 + b)`, which is
/// `x / sqrt(b)` when `a` is zero. The shift changes nothing the
/// wavelets see.
///
/// Public because the learned denoiser (in `greycard-ai`) sees and
/// predicts values in this space, with the same closed forms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vst {
    a: f32,
    s0: f32,
}

impl Vst {
    pub fn new(a: f32, b: f32) -> Self {
        Self {
            a,
            s0: (3.0 / 8.0 * a * a + b).sqrt(),
        }
    }

    /// The transform of channel `c` of a model.
    pub fn of(model: &NoiseModel, c: usize) -> Self {
        Self::new(model.a[c], model.b[c])
    }

    #[inline]
    pub fn forward(&self, x: f32) -> f32 {
        2.0 * x / ((self.a * x + self.s0 * self.s0).max(0.0).sqrt() + self.s0)
    }

    /// The algebraic inverse, `a d^2 / 4 + d s0`: exact for a value that
    /// is the transform of a clean signal, which is what a denoiser
    /// trained against transformed clean targets estimates. For the
    /// transform of a noisy sample use [`Vst::inverse`], which takes
    /// the noise's bias out.
    #[inline]
    pub fn inverse_clean(&self, d: f32) -> f32 {
        self.a * d * d / 4.0 + d * self.s0
    }

    /// The closed-form unbiased inverse (Mäkitalo and Foi), in terms of
    /// the shifted value `d`: with `D = d + 2 s0 / a` the inverse is
    /// `a (D^2/4 + c1/D - c2/D^2 + c3/D^3 - 1/8 - b/a^2)`, and `a/D` is
    /// `a^2 / (a d + 2 s0)`, finite for every `a`.
    #[inline]
    pub fn inverse(&self, d: f32) -> f32 {
        let (a, s0) = (self.a, self.s0);
        let sqrt_3_2 = 1.5f32.sqrt();
        let r = a / (a * d + 2.0 * s0);
        a * d * d / 4.0
            + d * s0
            + a / 4.0
            + a * (0.25 * sqrt_3_2 * r - 11.0 / 8.0 * r * r + 5.0 / 8.0 * sqrt_3_2 * r * r * r)
    }
}

/// Orthonormal luma and two chroma axes: `(r+g+b)/sqrt3`, `(r-b)/sqrt2`,
/// `(r-2g+b)/sqrt6`.
#[inline]
fn to_yuv([r, g, b]: [f32; 3]) -> [f32; 3] {
    [
        (r + g + b) / 3f32.sqrt(),
        (r - b) / 2f32.sqrt(),
        (r - 2.0 * g + b) / 6f32.sqrt(),
    ]
}

#[inline]
fn from_yuv([y, u, v]: [f32; 3]) -> [f32; 3] {
    let (s3, s2, s6) = (3f32.sqrt(), 2f32.sqrt(), 6f32.sqrt());
    [
        y / s3 + u / s2 + v / s6,
        y / s3 - 2.0 * v / s6,
        y / s3 - u / s2 + v / s6,
    ]
}

/// Elements per task of the flat passes over the frame.
const CHUNK: usize = 1 << 16;

/// The rotation into luma and chroma, interleaved `rgb` into the three
/// planes of `planes`.
fn to_planes(rgb: &[f32], planes: &mut [f32]) {
    let n = rgb.len() / 3;
    let (p0, rest) = planes.split_at_mut(n);
    let (p1, p2) = rest.split_at_mut(n);
    p0.par_chunks_mut(CHUNK)
        .zip(p1.par_chunks_mut(CHUNK))
        .zip(p2.par_chunks_mut(CHUNK))
        .zip(rgb.par_chunks(3 * CHUNK))
        .for_each(|(((p0, p1), p2), rgb)| {
            for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
                let [y, u, v] = to_yuv(*px);
                p0[i] = y;
                p1[i] = u;
                p2[i] = v;
            }
        });
}

/// The three planes rotated back and taken out of the stabilized space
/// into interleaved `rgb`.
fn from_planes(planes: &[f32], rgb: &mut [f32], vst: &[Vst; 3]) {
    let n = rgb.len() / 3;
    rgb.par_chunks_mut(3 * CHUNK)
        .zip(planes[..n].par_chunks(CHUNK))
        .zip(planes[n..2 * n].par_chunks(CHUNK))
        .zip(planes[2 * n..].par_chunks(CHUNK))
        .for_each(|(((rgb, p0), p1), p2)| {
            for (i, px) in rgb.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let v = from_yuv([p0[i], p1[i], p2[i]]);
                *px = std::array::from_fn(|c| vst[c].inverse(v[c]));
            }
        });
}

/// The soft threshold of one band, from its local variance grid.
struct Shrink<'a> {
    grid: &'a LocalVariance,
    noise_floor: f32,
    gain: [f32; 3],
    sb2: f32,
}

impl Shrink<'_> {
    /// Add the shrunk detail `input - coarse` to `acc`, over a run of
    /// row `y` starting at `x0`: `input`, `coarse` and `acc` are that
    /// run in each of the three planes.
    ///
    /// The grid's vertical interpolation is the row's and is taken
    /// once; its four corner values are a tile column's and are taken
    /// once per column, so the loop over a column's pixels is plain
    /// floats and vectorizes. The interpolation itself is the same
    /// arithmetic per pixel as before, and gives the same bits.
    fn row(
        &self,
        y: usize,
        x0: usize,
        input: [&[f32]; 3],
        coarse: [&[f32]; 3],
        acc: [&mut [f32]; 3],
    ) {
        let g = self.grid;
        let n = coarse[0].len();
        let tile = g.tile as f32;
        let fy = ((y as f32 + 0.5) / tile - 0.5).max(0.0);
        let ty = (fy as usize).min(g.tiles_y - 1);
        let ty1 = (ty + 1).min(g.tiles_y - 1);
        let ay = (fy - ty as f32).min(1.0);
        let row0 = &g.values[ty * g.tiles_x..(ty + 1) * g.tiles_x];
        let row1 = &g.values[ty1 * g.tiles_x..(ty1 + 1) * g.tiles_x];
        // Position in tile units, measured from the first tile's
        // center; the tile column it falls in.
        let column = |x: usize| {
            let fx = ((x as f32 + 0.5) / tile - 0.5).max(0.0);
            (fx as usize).min(g.tiles_x - 1)
        };
        let [a0, a1, a2] = acc;
        let mut i = 0;
        while i < n {
            let tx = column(x0 + i);
            let tx1 = (tx + 1).min(g.tiles_x - 1);
            // The run of the block in this column. Tile widths are even
            // and `x + 0.5` exact, so the column changes exactly where
            // the integer form says, and the last column runs on.
            let end = if tx + 1 == g.tiles_x {
                n
            } else {
                ((tx + 1) * g.tile + g.tile / 2 - x0).min(n)
            };
            debug_assert!(end > i && column(x0 + end - 1) == tx);
            debug_assert!(end == n || column(x0 + end) == tx + 1);
            let (v00, v10, v01, v11) = (row0[tx], row0[tx1], row1[tx], row1[tx1]);
            let txf = tx as f32;
            for (c, a) in [&mut *a0, &mut *a1, &mut *a2].into_iter().enumerate() {
                let (input, coarse, a) = (&input[c][i..end], &coarse[c][i..end], &mut a[i..end]);
                let (v00, v10, v01, v11) = (v00[c], v10[c], v01[c], v11[c]);
                let (gain, sb2, floor) = (self.gain[c], self.sb2, self.noise_floor);
                for (j, a) in a.iter_mut().enumerate() {
                    let fx = ((x0 + i + j) as f32 + 0.5) / tile - 0.5;
                    let ax = (fx.max(0.0) - txf).min(1.0);
                    let top = v00 + (v10 - v00) * ax;
                    let bottom = v01 + (v11 - v01) * ax;
                    let var = top + (bottom - top) * ay;
                    let std_x = (var - floor).max(1e-12).sqrt();
                    let t = gain * sb2 / std_x;
                    let d = input[j] - coarse[j];
                    *a += (d - t).max(0.0) + (d + t).min(0.0);
                }
            }
            i = end;
        }
    }
}

/// Rows per band of the in-place chain. The coarse rows of a band are
/// held back until no later band reads the rows they replace, so this
/// must be at least the blur's reach, `2^(scale+1)` rows.
const BAND_ROWS: usize = 256;

/// Pixels per block of a row in the blur. A block's sums and weights
/// and the five tap rows' stretch of it are what the tap loops stream,
/// and at this size they stay in the first-level cache.
const BLOCK: usize = 512;

/// The coarse rows of the band being blurred and of the band before it,
/// planar, `rows` rows a plane: one pair for every level of the chain.
struct Bands {
    this: Vec<f32>,
    prev: Vec<f32>,
    rows: usize,
}

impl Bands {
    fn new(width: usize, height: usize, scales: usize) -> Self {
        let rows = BAND_ROWS.max(1 << scales).min(height);
        Self {
            this: vec![0.0; 3 * rows * width],
            prev: vec![0.0; 3 * rows * width],
            rows,
        }
    }
}

/// Per-worker scratch for the blur: a block's stretch of the five tap
/// rows in each plane, its weighted sums, one per plane, and its
/// summed weights.
#[derive(Default)]
struct Scratch {
    /// The tap rows' stretch of the block with the blur's reach either
    /// side, the row's ends repeated past them: `[tap][plane][pixel]`.
    rows: Vec<f32>,
    sum: [Vec<f32>; 3],
    wgt: Vec<f32>,
}

/// One à trous level in place: `cur` holds the level's input in three
/// planes and, on return, its coarse (the 5x5 B3 spline blur at spacing
/// `2^scale`, edge-guarded so that pixels many sigmas apart in color do
/// not mix); the detail `input - coarse`, shrunk, is added to the three
/// planes of `acc`.
///
/// The blur of a band of rows reads `reach` rows beyond it, so a row of
/// `cur` may only be replaced once the band after the one it belongs to
/// has been blurred. Each band's coarse goes to a band buffer, and its
/// detail is shrunk into `acc` in the same sweep; after the next band
/// is blurred, the rows nothing later reads are copied into `cur`, from
/// the previous buffer's tail and this one's head.
#[allow(clippy::too_many_arguments)]
fn atrous_level(
    cur: &mut [f32],
    acc: &mut [f32],
    width: usize,
    height: usize,
    scale: usize,
    band_variance: f32,
    shrink: &Shrink,
    bands: &mut Bands,
) {
    let n = width * height;
    let mult = 1usize << scale;
    let reach = 2 * mult;
    let band = BAND_ROWS.max(reach).min(height);
    debug_assert!(band <= bands.rows, "the band buffers fit every level");
    let stride = bands.rows * width;
    // darktable's guard: exp2(-max(0, distance^2 * 0.02 / sigma_band^2 - 9)).
    let inv_sigma2 = 0.02 / band_variance;

    let mut prev_y0 = 0;
    let mut committed = 0;
    let mut y0 = 0;
    let mut blur_time = std::time::Duration::ZERO;
    let mut commit_time = std::time::Duration::ZERO;
    while y0 < height {
        let y1 = (y0 + band).min(height);
        let rows = y1 - y0;
        let t = std::time::Instant::now();
        {
            let cur = &*cur;
            let input: [&[f32]; 3] = [&cur[..n], &cur[n..2 * n], &cur[2 * n..]];
            let (a0, rest) = acc.split_at_mut(n);
            let (a1, a2) = rest.split_at_mut(n);
            let (t0, rest) = bands.this.split_at_mut(stride);
            let (t1, t2) = rest.split_at_mut(stride);
            let span = y0 * width..y1 * width;
            t0[..rows * width]
                .par_chunks_mut(width)
                .zip(t1[..rows * width].par_chunks_mut(width))
                .zip(t2[..rows * width].par_chunks_mut(width))
                .zip(a0[span.clone()].par_chunks_mut(width))
                .zip(a1[span.clone()].par_chunks_mut(width))
                .zip(a2[span].par_chunks_mut(width))
                .enumerate()
                .for_each_init(
                    Scratch::default,
                    |scratch, (i, (((((t0, t1), t2), a0), a1), a2))| {
                        blur_row(
                            input,
                            width,
                            height,
                            y0 + i,
                            mult,
                            inv_sigma2,
                            shrink,
                            scratch,
                            [t0, t1, t2],
                            [a0, a1, a2],
                        );
                    },
                );
        }
        blur_time += t.elapsed();
        let t = std::time::Instant::now();
        let until = if y1 == height { height } else { y1 - reach };
        let coarse_row = |p: usize, r: usize| -> &[f32] {
            let (buf, base) = if r < y0 {
                (&bands.prev, prev_y0)
            } else {
                (&bands.this, y0)
            };
            &buf[(p * bands.rows + r - base) * width..][..width]
        };
        for (p, plane) in cur.chunks_exact_mut(n).enumerate() {
            plane[committed * width..until * width]
                .par_chunks_mut(width)
                .enumerate()
                .for_each(|(i, row)| row.copy_from_slice(coarse_row(p, committed + i)));
        }
        commit_time += t.elapsed();
        committed = until;
        std::mem::swap(&mut bands.this, &mut bands.prev);
        prev_y0 = y0;
        y0 = y1;
    }
    log::debug!(
        "denoise: scale {scale}: blur and shrink {:.0} ms, commit {:.0} ms",
        blur_time.as_secs_f64() * 1e3,
        commit_time.as_secs_f64() * 1e3
    );
}

/// Row `y` of the guarded blur of `input` at spacing `mult` into `out`,
/// and the detail it leaves, shrunk, into `acc`; `out` and `acc` are
/// the row in each plane.
///
/// The row goes in blocks of [`BLOCK`] pixels. A block's stretch of each
/// of the five tap rows, with the blur's reach either side and the
/// row's ends repeated past them, is copied into the scratch first: one
/// plain copy per stretch, which the memory system streams, where the
/// taps reading the frame directly stalled on fifteen short runs a
/// block from a spacing of four up. Then each of the 25 taps is a loop
/// over the block's contiguous run of every plane, floats in and floats
/// out with no index arithmetic or clamp per pixel, so the vectorizer
/// takes it. The taps come in the order the per-pixel form took them,
/// so the sums are the same to the bit.
#[allow(clippy::too_many_arguments)]
fn blur_row(
    input: [&[f32]; 3],
    width: usize,
    height: usize,
    y: usize,
    mult: usize,
    inv_sigma2: f32,
    shrink: &Shrink,
    scratch: &mut Scratch,
    mut out: [&mut [f32]; 3],
    acc: [&mut [f32]; 3],
) {
    let m = mult as isize;
    let reach = 2 * mult;
    let tap_rows: [usize; 5] = std::array::from_fn(|jj| {
        (y as isize + m * (jj as isize - 2)).clamp(0, height as isize - 1) as usize
    });
    // A stretch is the block and the reach either side.
    let ew = BLOCK + 2 * reach;
    scratch.rows.resize(15 * ew, 0.0);
    for s in &mut scratch.sum {
        s.resize(BLOCK, 0.0);
    }
    scratch.wgt.resize(BLOCK, 0.0);
    let [a0, a1, a2] = acc;
    let mut x0 = 0;
    while x0 < width {
        let x1 = (x0 + BLOCK).min(width);
        let n = x1 - x0;
        let len = n + 2 * reach;
        for (jj, &yy) in tap_rows.iter().enumerate() {
            for (c, plane) in input.iter().enumerate() {
                let row = &plane[yy * width..(yy + 1) * width];
                let dst = &mut scratch.rows[(jj * 3 + c) * ew..][..len];
                // Pixels `x0 - reach .. x1 + reach`, clamped to the row.
                let start = x0 as isize - reach as isize;
                let left = (-start).max(0) as usize;
                let right = (start + len as isize - width as isize).max(0) as usize;
                let middle = len - left - right;
                dst[..left].fill(row[0]);
                let from = (start + left as isize) as usize;
                dst[left..left + middle].copy_from_slice(&row[from..from + middle]);
                dst[left + middle..].fill(row[width - 1]);
            }
        }
        for s in &mut scratch.sum {
            s[..n].fill(0.0);
        }
        scratch.wgt[..n].fill(0.0);
        let stretch = |jj: usize, c: usize, off: isize| -> &[f32] {
            let at = (reach as isize + off) as usize;
            &scratch.rows[(jj * 3 + c) * ew + at..][..n]
        };
        let p: [&[f32]; 3] = std::array::from_fn(|c| stretch(2, c, 0));
        for (jj, fy) in FILTER.iter().enumerate() {
            for (ii, fx) in FILTER.iter().enumerate() {
                let off = m * (ii as isize - 2);
                let q: [&[f32]; 3] = std::array::from_fn(|c| stretch(jj, c, off));
                tap(
                    p,
                    q,
                    fy * fx,
                    inv_sigma2,
                    &mut scratch.sum,
                    &mut scratch.wgt,
                );
            }
        }
        for (c, o) in out.iter_mut().enumerate() {
            let o = &mut o[x0..x1];
            let (s, w) = (&scratch.sum[c][..n], &scratch.wgt[..n]);
            for i in 0..n {
                o[i] = s[i] / w[i];
            }
        }
        let coarse: [&[f32]; 3] = std::array::from_fn(|c| &out[c][x0..x1]);
        shrink.row(
            y,
            x0,
            p,
            coarse,
            [&mut a0[x0..x1], &mut a1[x0..x1], &mut a2[x0..x1]],
        );
        x0 = x1;
    }
}

/// The guard's floor: the exponent past which a tap weighs no less.
///
/// The means' weight bottoms out at `2^-126`, which is right for them,
/// since their weights are never scaled down. Here a tap's weight is
/// the guard times a filter weight as small as `1/256`, and that times
/// the pixel: at the coarse scales, where the band's noise variance is
/// small and the guard is engaged nearly everywhere and far past its
/// floor, those products are subnormal, and subnormal arithmetic is
/// microcode at a hundred cycles an operation. That, and not the
/// memory, was the difference between scale 0 and scale 6: a flat
/// frame, whose guard never engages, ran every scale at the speed of
/// scale 0, and a frame of pure noise ran every scale at twice that.
/// Nor does a select help, since both its sides are computed. So the
/// guard is floored at `2^-60` before the filter weight: against the
/// center tap's own `36/256` a weight that small could change no sum
/// by as much as a last bit, and every value the loop makes is normal.
const FLOOR: f32 = 60.0;

/// One tap of the blur over a block: `p` is the block's run of the
/// center row in each plane, `q` the tap's pixels for it, and `f` the
/// tap's filter weight before the guard.
///
/// Out of line so the loop holds no `&mut` into the frame: the sums go
/// to the scratch and the frame is only read (notes §128).
#[inline(never)]
fn tap(
    p: [&[f32]; 3],
    q: [&[f32]; 3],
    f: f32,
    inv_sigma2: f32,
    sum: &mut [Vec<f32>; 3],
    wgt: &mut [f32],
) {
    let k = p[0].len();
    let (p0, p1, p2) = (&p[0][..k], &p[1][..k], &p[2][..k]);
    let (q0, q1, q2) = (&q[0][..k], &q[1][..k], &q[2][..k]);
    let [s0, s1, s2] = sum;
    let (s0, s1, s2) = (&mut s0[..k], &mut s1[..k], &mut s2[..k]);
    let wg = &mut wgt[..k];
    for i in 0..k {
        let dist = sq(p0[i] - q0[i]) + sq(p1[i] - q1[i]) + sq(p2[i] - q2[i]);
        let e = (dist * inv_sigma2 - 9.0).min(FLOOR);
        let w = f * super::nlm::weight(e);
        wg[i] += w;
        s0[i] += w * q0[i];
        s1[i] += w * q1[i];
        s2[i] += w * q2[i];
    }
}

#[inline]
fn sq(x: f32) -> f32 {
    x * x
}

/// A detail band's variance around each pixel: the mean square over
/// tiles of [`Self::TILE`] times `2^scale` (the coefficients are
/// correlated over the filter's spacing, so the window grows with it),
/// interpolated bilinearly between tile centers so thresholds do not
/// step at tile edges.
///
/// Measured on the critically sampled coefficients: the band at
/// spacing `2^scale` evaluated every `2^scale` pixels, which is the
/// level of a plain decimated pyramid, so every tile at every scale is
/// [`Self::SAMPLES`] coefficients. The guard is left out of the
/// measurement; it only differs from the plain blur at strong edges,
/// where the variance is large and the threshold small either way.
pub(crate) struct LocalVariance {
    tiles_x: usize,
    tiles_y: usize,
    tile: usize,
    pub(crate) values: Vec<[f32; 3]>,
}

impl LocalVariance {
    /// Tile side at scale 0, in pixels; at every scale, in coefficients.
    const TILE: usize = 8;
    /// Coefficients averaged per full tile.
    const SAMPLES: usize = Self::TILE * Self::TILE;
}

/// The local variance of every band, from a decimated pyramid of the
/// three planes of `planes`: each level is the previous blurred with
/// the 5-tap filter and taken every other pixel, which is the unguarded
/// à trous chain on the lattice of its scale. Costs a third of one plain
/// blur of the image over all levels, and a quarter of the image in
/// memory.
pub(crate) fn variance_grids(
    planes: &[f32],
    width: usize,
    height: usize,
    scales: usize,
) -> Vec<LocalVariance> {
    let mut grids = Vec::with_capacity(scales);
    let mut level: Vec<f32> = Vec::new();
    let (mut w, mut h) = (width, height);
    for scale in 0..scales {
        let src = if scale == 0 { planes } else { &level };
        let (grid, next) = pyramid_level(src, w, h, scale);
        grids.push(grid);
        level = next;
        w = w.div_ceil(2);
        h = h.div_ceil(2);
    }
    grids
}

/// One pyramid level: the tile variances of `src` minus its blur, and
/// the blur decimated by two for the next level, plane by plane.
fn pyramid_level(src: &[f32], w: usize, h: usize, scale: usize) -> (LocalVariance, Vec<f32>) {
    let tile = LocalVariance::TILE;
    let tiles_x = w.div_ceil(tile);
    let tiles_y = h.div_ceil(tile);
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    let mut next = vec![0.0f32; 3 * nw * nh];
    let mut values = vec![[0.0f32; 3]; tiles_x * tiles_y];
    for (c, (plane, next)) in src
        .chunks_exact(w * h)
        .zip(next.chunks_exact_mut(nw * nh))
        .enumerate()
    {
        let sums = pyramid_plane(plane, w, h, next);
        debug_assert_eq!(sums.len(), values.len());
        for (v, s) in values.iter_mut().zip(sums) {
            v[c] = s;
        }
    }
    (
        LocalVariance {
            tiles_x,
            tiles_y,
            tile: tile << scale,
            values,
        },
        next,
    )
}

/// One plane of a pyramid level: the tile variances of `src` minus its
/// blur, and the blur decimated by two into `next`.
fn pyramid_plane(src: &[f32], w: usize, h: usize, next: &mut [f32]) -> Vec<f32> {
    // Rows per task: a multiple of two and of the tile.
    const CHUNK: usize = 8 * LocalVariance::TILE;
    let tile = LocalVariance::TILE;
    let tiles_x = w.div_ceil(tile);
    let nw = w.div_ceil(2);
    next.par_chunks_mut(CHUNK / 2 * nw)
        .enumerate()
        .flat_map_iter(|(k, nrows)| {
            let y0 = k * CHUNK;
            let y1 = (y0 + CHUNK).min(h);
            // The rows blurred along x: the chunk's and two beyond it.
            let ha = y0.saturating_sub(2);
            let hb = (y1 + 2).min(h);
            let mut hrows = vec![0.0f32; (hb - ha) * w];
            for (i, hrow) in hrows.chunks_exact_mut(w).enumerate() {
                blur_row_h(&src[(ha + i) * w..(ha + i + 1) * w], hrow);
            }
            let hrow = |y: isize| (y.clamp(0, h as isize - 1) as usize - ha) * w;

            let tile_rows = (y1 - y0).div_ceil(tile);
            let mut sums = vec![0.0f64; tile_rows * tiles_x];
            let mut counts = vec![0u32; tile_rows * tiles_x];
            let mut blur = vec![0.0f32; w];
            for y in y0..y1 {
                let taps: [&[f32]; 5] = std::array::from_fn(|j| {
                    let at = hrow(y as isize + j as isize - 2);
                    &hrows[at..at + w]
                });
                for (x, b) in blur.iter_mut().enumerate() {
                    let mut s = 0.0f32;
                    for (f, tap) in FILTER.iter().zip(taps) {
                        s += f * tap[x];
                    }
                    *b = s;
                }
                let srow = &src[y * w..(y + 1) * w];
                let ty = (y - y0) / tile;
                for tx in 0..tiles_x {
                    let xs = tx * tile..((tx + 1) * tile).min(w);
                    let t = ty * tiles_x + tx;
                    counts[t] += xs.len() as u32;
                    let mut s = sums[t];
                    for (&v, &b) in srow[xs.clone()].iter().zip(&blur[xs]) {
                        let d = v - b;
                        s += (d * d) as f64;
                    }
                    sums[t] = s;
                }
                if y % 2 == 0 {
                    let nrow = &mut nrows[(y - y0) / 2 * nw..][..nw];
                    for (nb, b) in nrow.iter_mut().zip(blur.iter().step_by(2)) {
                        *nb = *b;
                    }
                }
            }
            sums.iter()
                .zip(counts)
                .map(|(s, n)| (s / n as f64) as f32)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// One row blurred along x with the 5-tap filter, edges clamped.
fn blur_row_h(src: &[f32], out: &mut [f32]) {
    let w = src.len();
    let inner = 2..w.saturating_sub(2);
    for (x, o) in out.iter_mut().enumerate() {
        if inner.contains(&x) {
            continue;
        }
        let mut s = 0.0f32;
        for (i, f) in FILTER.iter().enumerate() {
            let xx = (x as isize + i as isize - 2).clamp(0, w as isize - 1) as usize;
            s += f * src[xx];
        }
        *o = s;
    }
    if inner.is_empty() {
        return;
    }
    let n = inner.len();
    let out = &mut out[inner];
    let (s0, s1, s2, s3, s4) = (
        &src[..n],
        &src[1..1 + n],
        &src[2..2 + n],
        &src[3..3 + n],
        &src[4..4 + n],
    );
    for i in 0..n {
        let mut s = 0.0f32;
        s += FILTER[0] * s0[i];
        s += FILTER[1] * s1[i];
        s += FILTER[2] * s2[i];
        s += FILTER[3] * s3[i];
        s += FILTER[4] * s4[i];
        out[i] = s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::noise::tests::Gauss;

    #[test]
    fn clean_inverse_undoes_the_forward_exactly() {
        for (a, b) in [(3e-4, 5e-8), (1e-2, 1e-4), (0.0, 1e-4), (0.05, 0.0)] {
            let vst = Vst::new(a, b);
            for i in 0..=120 {
                let x = i as f32 / 100.0;
                let back = vst.inverse_clean(vst.forward(x));
                assert!((back - x).abs() < 2e-6, "a {a} b {b}: {x} -> {back}");
            }
        }
    }

    #[test]
    fn transform_round_trips_and_rotation_is_orthonormal() {
        for (a, b) in [(2e-4f32, 3e-6f32), (2e-4, 0.0), (0.0, 3e-6), (1e-9, 1e-6)] {
            let vst = Vst::new(a, b);
            for x in [0.001, 0.02, 0.18, 0.7, 1.5] {
                let back = vst.inverse(vst.forward(x));
                // The unbiased inverse of a noiseless value sits a quarter
                // of a Poisson count high, which is a quarter of `a`.
                assert!(
                    (back - x).abs() < 0.3 * a + 1e-6,
                    "{x} -> {back} with a {a} b {b}"
                );
                // Unit noise: the slope is one over the model's sigma.
                let h = 1e-4;
                let slope = (vst.forward(x + h) - vst.forward(x - h)) / (2.0 * h);
                let want = 1.0 / (a * x + 3.0 / 8.0 * a * a + b).sqrt();
                assert!(
                    (slope / want - 1.0).abs() < 0.02,
                    "slope {slope} vs {want} at {x} with a {a} b {b}"
                );
            }
        }
        let v = [0.3, -0.7, 1.1];
        let back = from_yuv(to_yuv(v));
        for c in 0..3 {
            assert!((back[c] - v[c]).abs() < 1e-6);
        }
        let y = to_yuv(v);
        let norm = |p: [f32; 3]| p.iter().map(|q| q * q).sum::<f32>();
        assert!((norm(y) - norm(v)).abs() < 1e-5, "length preserved");
    }

    /// The chain over `scales` levels with nothing shrunk, on planes.
    fn keep_everything(cur: &mut [f32], acc: &mut [f32], w: usize, h: usize, scales: usize) {
        let want = band_variances();
        let keep = LocalVariance {
            tiles_x: 1,
            tiles_y: 1,
            tile: w,
            values: vec![[1.0; 3]],
        };
        let mut bands = Bands::new(w, h, scales);
        for (scale, &sb2) in want.iter().enumerate().take(scales) {
            let shrink = Shrink {
                grid: &keep,
                noise_floor: 0.0,
                gain: [0.0; 3],
                sb2,
            };
            atrous_level(cur, acc, w, h, scale, sb2, &shrink, &mut bands);
        }
    }

    #[test]
    fn band_variances_are_measured_right() {
        // Unit white noise: each band's variance on the pyramid must
        // match the kernel norm, and the in-place chain with nothing
        // shrunk must give back the input as bands plus residue.
        let (w, h) = (512, 512);
        let mut g = Gauss(99);
        let input: Vec<f32> = (0..w * h * 3).map(|_| g.next()).collect();
        let want = band_variances();
        assert!((want[0] - 0.79).abs() < 0.02, "{want:?}");
        let grids = variance_grids(&input, w, h, 4);
        for (scale, (grid, &want_var)) in grids.iter().zip(&want).enumerate() {
            let per: [f32; 3] = std::array::from_fn(|c| {
                grid.values.iter().map(|v| v[c]).sum::<f32>() / grid.values.len() as f32
            });
            let got = (per[0] + per[1] + per[2]) / 3.0;
            assert!(
                (got / want_var - 1.0).abs() < 0.06,
                "scale {scale}: {got} ({per:?}) vs {want_var}"
            );
            assert_eq!(grid.tile, 8 << scale);
            assert_eq!(grid.tiles_x, w.div_ceil(8 << scale));
        }

        let mut cur = input.clone();
        let mut acc = vec![0.0f32; w * h * 3];
        keep_everything(&mut cur, &mut acc, w, h, 4);
        for ((a, r), i) in acc.iter().zip(&cur).zip(&input) {
            assert!((a + r - i).abs() < 1e-4);
        }
    }

    /// The guarded blur in its plain per-pixel form, on planes: the
    /// reference the blocked, vectorized one is checked against.
    fn plain_blur(input: &[f32], w: usize, h: usize, mult: usize, inv_sigma2: f32) -> Vec<f32> {
        let n = w * h;
        let mut out = vec![0.0f32; 3 * n];
        for y in 0..h {
            for x in 0..w {
                let px: [f32; 3] = std::array::from_fn(|c| input[c * n + y * w + x]);
                let mut sum = [0.0f32; 3];
                let mut wgt = 0.0f32;
                for (jj, fy) in FILTER.iter().enumerate() {
                    let yy = (y as isize + mult as isize * (jj as isize - 2))
                        .clamp(0, h as isize - 1) as usize;
                    for (ii, fx) in FILTER.iter().enumerate() {
                        let xx = (x as isize + mult as isize * (ii as isize - 2))
                            .clamp(0, w as isize - 1) as usize;
                        let q: [f32; 3] = std::array::from_fn(|c| input[c * n + yy * w + xx]);
                        let dist = sq(px[0] - q[0]) + sq(px[1] - q[1]) + sq(px[2] - q[2]);
                        let guard = super::super::nlm::weight(dist * inv_sigma2 - 9.0);
                        let w = fy * fx * guard;
                        wgt += w;
                        for c in 0..3 {
                            sum[c] += w * q[c];
                        }
                    }
                }
                for c in 0..3 {
                    out[c * n + y * w + x] = sum[c] / wgt;
                }
            }
        }
        out
    }

    fn chain_matches_the_plain_one(w: usize, h: usize, scales: usize) {
        let mut g = Gauss(7);
        // Rows of a slow wave, noise, and a bright bar to bring the
        // guard in.
        let input: Vec<f32> = (0..w * h * 3)
            .map(|i| {
                let (x, y) = (i % w, i / w % h);
                let bar = if y % 40 == 7 && x > 3 { 20.0 } else { 0.0 };
                (y as f32 * 0.01).sin() + 0.3 * g.next() + bar
            })
            .collect();
        let want = band_variances();
        let keep = LocalVariance {
            tiles_x: 1,
            tiles_y: 1,
            tile: w,
            values: vec![[1.0; 3]],
        };
        let mut cur = input.clone();
        let mut acc = vec![0.0f32; w * h * 3];
        let mut plain = input.clone();
        let mut bands = Bands::new(w, h, scales);
        for (scale, &sb2) in want.iter().enumerate().take(scales) {
            let shrink = Shrink {
                grid: &keep,
                noise_floor: 0.0,
                gain: [0.0; 3],
                sb2,
            };
            atrous_level(&mut cur, &mut acc, w, h, scale, sb2, &shrink, &mut bands);
            plain = plain_blur(&plain, w, h, 1 << scale, 0.02 / sb2);
            assert_eq!(cur, plain, "coarse after scale {scale} at {w}x{h}");
        }
        for ((a, r), i) in acc.iter().zip(&cur).zip(&input) {
            assert!((a + r - i).abs() < 1e-4);
        }
    }

    #[test]
    fn in_place_chain_matches_a_plain_one() {
        // A tall, thin image so the chain runs over several bands at
        // every scale, and a two-band scale whose reach is a whole
        // band's worth of rows; then a wide, short one so a row goes in
        // several blocks and the coarse taps clamp at both ends of it.
        chain_matches_the_plain_one(24, 3 * BAND_ROWS + 37, 4);
        chain_matches_the_plain_one(2 * BLOCK + 37, 40, 4);
    }

    #[test]
    fn scale_count_matches_the_reference() {
        assert_eq!(scale_count(6000, 4000), 7);
        assert_eq!(scale_count(768, 512), 6);
        assert_eq!(scale_count(64, 64), 3);
    }

    /// Left half flat, right half a mosaic of 12-pixel tiles at random
    /// levels, so the wavelet bands carry signal the way a photograph's
    /// do; a gentle gradient over everything.
    fn noisy_scene(w: usize, h: usize, model: &NoiseModel, seed: u64) -> (Vec<f32>, Vec<f32>) {
        let mut tiles = Gauss(seed ^ 0xABCD);
        let levels: Vec<f32> = (0..(w / 12 + 1) * (h / 12 + 1))
            .map(|_| 0.1 + 0.4 * (tiles.uniform_public() as f32))
            .collect();
        let truth: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                let base = if x < w / 2 {
                    0.12
                } else {
                    levels[(y / 12) * (w / 12 + 1) + x / 12]
                };
                let v = base + 0.05 * y as f32 / h as f32;
                [v, v * 0.9, v * 0.8]
            })
            .collect();
        let mut g = Gauss(seed);
        let noisy: Vec<f32> = truth
            .iter()
            .enumerate()
            .map(|(i, &v)| (v + g.next() * model.sigma(i % 3, v)).max(0.0))
            .collect();
        (truth, noisy)
    }

    fn mse(a: &[f32], b: &[f32], w: usize, h: usize, x0: usize, x1: usize) -> f32 {
        let mut e = 0.0;
        let mut n = 0;
        for y in 8..h - 8 {
            for x in x0..x1 {
                for c in 0..3 {
                    e += sq(a[(y * w + x) * 3 + c] - b[(y * w + x) * 3 + c]);
                    n += 1;
                }
            }
        }
        e / n as f32
    }

    #[test]
    fn removes_the_noise_from_flat_areas_and_improves_the_rest() {
        let (w, h) = (192, 128);
        let model = NoiseModel {
            a: [6e-4; 3],
            b: [1e-5; 3],
        };
        let (truth, noisy) = noisy_scene(w, h, &model, 3);
        let mut out = noisy.clone();
        let stats = denoise_profiled(&mut out, w, h, &model, &DenoiseOptions::default()).unwrap();
        // The default is the hybrid: the means, then the wavelets.
        assert!(stats.scales >= 4, "{stats:?}");
        assert_eq!(stats.method, DenoiseMethod::Hybrid);
        assert!(stats.nlm.is_some_and(|n| n.patch_radius == 1), "{stats:?}");
        let flat_before = mse(&noisy, &truth, w, h, 8, w / 2 - 16);
        let flat_after = mse(&out, &truth, w, h, 8, w / 2 - 16);
        assert!(
            flat_after < 0.1 * flat_before,
            "flat {flat_before} -> {flat_after}"
        );
        let tiles_before = mse(&noisy, &truth, w, h, w / 2 + 8, w - 8);
        let tiles_after = mse(&out, &truth, w, h, w / 2 + 8, w - 8);
        assert!(
            tiles_after < tiles_before,
            "tiles {tiles_before} -> {tiles_after}"
        );
        // Flat areas keep their mean.
        let mean = |img: &[f32], x0: usize, x1: usize| {
            let mut s = 0.0;
            let mut n = 0;
            for y in 8..h - 8 {
                for x in x0..x1 {
                    s += img[(y * w + x) * 3];
                    n += 1;
                }
            }
            s / n as f32
        };
        let (mt, mo) = (mean(&truth, 8, w / 2 - 8), mean(&out, 8, w / 2 - 8));
        assert!((mo / mt - 1.0).abs() < 0.02, "mean {mt} -> {mo}");
        // Tile edges survive: across the boundary between two tiles that
        // differ a lot, most of the true difference is still there.
        let y = h / 2 + 6;
        let mut checked = 0;
        for tx in (w / 2 + 12..w - 12).step_by(12) {
            let want = truth[(y * w + tx) * 3] - truth[(y * w + tx - 1) * 3];
            if want.abs() < 0.15 {
                continue;
            }
            let got = out[(y * w + tx + 1) * 3] - out[(y * w + tx - 2) * 3];
            assert!(got / want > 0.6, "edge at {tx}: {got} of {want}");
            checked += 1;
        }
        assert!(checked >= 2, "the scene must have big edges to check");
    }

    /// The synthetic frame the golden test runs: the noisy scene, with
    /// an edge strong enough to bring the blur's guard into play.
    fn golden_frame() -> (usize, usize, NoiseModel, Vec<f32>) {
        let (w, h) = (203, 131);
        let model = NoiseModel {
            a: [2e-3, 1.5e-3, 2.5e-3],
            b: [1e-5, 2e-5, 1.5e-5],
        };
        let (_, mut noisy) = noisy_scene(w, h, &model, 11);
        // A bright bar across the middle: the guard's case.
        for y in h / 2 - 5..h / 2 + 5 {
            for x in 20..w - 20 {
                for c in 0..3 {
                    noisy[(y * w + x) * 3 + c] += 0.6;
                }
            }
        }
        (w, h, model, noisy)
    }

    /// The values the wavelets and the hybrid gave on the golden frame
    /// before the speed work on the chain, when the blur's guard was
    /// `f32::exp2` and the chain ran per pixel over interleaved RGB.
    /// The tolerance is 1e-5, under a 16-bit sample's own step; the
    /// polynomial guard moves a 45 MP frame's worst sample by 4.2e-7.
    #[test]
    fn the_chain_holds_the_values_it_had_before_the_speed_work() {
        const WAVELETS: [f32; 24] = [
            1.17342725e-1,
            9.625273e-2,
            1.1002353e-1,
            1.2120184e-1,
            9.925471e-2,
            1.0319633e-1,
            4.7045967e-1,
            2.4791166e-1,
            1.378571e-1,
            3.8701472e-1,
            1.8551075e-1,
            1.1701198e-1,
            1.2624949e-1,
            1.0397324e-1,
            1.17351e-1,
            1.315554e-1,
            3.188896e-1,
            2.514307e-1,
            2.649147e-1,
            2.9115e-1,
            1.7451216e-1,
            1.3788058e-1,
            1.1207844e-1,
            1.20419584e-1,
        ];
        const WAVELETS_MEAN: f64 = 0.249_760_404;
        const HYBRID: [f32; 24] = [
            1.2006721e-1,
            9.753793e-2,
            1.1058618e-1,
            1.12879835e-1,
            1.00722715e-1,
            1.1340004e-1,
            4.9413782e-1,
            2.550038e-1,
            1.449193e-1,
            3.9810595e-1,
            2.0375985e-1,
            1.1476959e-1,
            1.3018292e-1,
            1.0514216e-1,
            1.19627886e-1,
            1.309233e-1,
            3.1801692e-1,
            2.4102746e-1,
            2.4581566e-1,
            2.892357e-1,
            1.7671259e-1,
            1.3548045e-1,
            1.1304262e-1,
            1.2111085e-1,
        ];
        const HYBRID_MEAN: f64 = 0.250_059_453;
        for (method, golden, golden_mean) in [
            (DenoiseMethod::Wavelets, WAVELETS, WAVELETS_MEAN),
            (DenoiseMethod::Hybrid, HYBRID, HYBRID_MEAN),
        ] {
            let (w, h, model, mut rgb) = golden_frame();
            let stats = denoise_profiled(
                &mut rgb,
                w,
                h,
                &model,
                &DenoiseOptions {
                    method,
                    ..DenoiseOptions::default()
                },
            )
            .unwrap();
            assert_eq!(stats.scales, 4, "{stats:?}");
            for (k, &want) in golden.iter().enumerate() {
                let got = rgb[(k * 1277) % (w * h * 3)];
                assert!(
                    (got - want).abs() < 1e-5,
                    "{method:?} sample {k}: {got} vs {want}"
                );
            }
            let mean = rgb.iter().map(|&v| v as f64).sum::<f64>() / rgb.len() as f64;
            assert!((mean - golden_mean).abs() < 1e-7, "{method:?} mean {mean}");
        }
    }

    #[test]
    fn chroma_multiplier_shrinks_color_noise_harder() {
        let (w, h) = (192, 128);
        let model = NoiseModel {
            a: [6e-4; 3],
            b: [1e-5; 3],
        };
        // Tiles colored independently per channel on the right, so the
        // chrominance bands carry real signal there and get finite
        // thresholds; a flat neutral left half, which is recognized as
        // noise and cleared whatever the multiplier.
        let mut tiles = Gauss(0x5EED);
        let levels: Vec<[f32; 3]> = (0..(w / 12 + 1) * (h / 12 + 1))
            .map(|_| std::array::from_fn(|_| 0.1 + 0.4 * tiles.uniform_public() as f32))
            .collect();
        let truth: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                if x < w / 2 {
                    [0.12, 0.11, 0.1]
                } else {
                    levels[(y / 12) * (w / 12 + 1) + x / 12]
                }
            })
            .collect();
        let mut g = Gauss(7);
        let noisy: Vec<f32> = truth
            .iter()
            .enumerate()
            .map(|(i, &v)| (v + g.next() * model.sigma(i % 3, v)).max(0.0))
            .collect();
        // Chrominance taken out of the tiles, relative to the input.
        let chroma_removed = |img: &[f32]| {
            let mut s = 0.0f64;
            let mut n = 0;
            for y in 8..h - 8 {
                for x in w / 2 + 8..w - 8 {
                    let i = (y * w + x) * 3;
                    let d: Vec<f64> = (0..3).map(|c| (img[i + c] - noisy[i + c]) as f64).collect();
                    s += (d[0] - d[2]).powi(2) + (d[0] - 2.0 * d[1] + d[2]).powi(2);
                    n += 1;
                }
            }
            s / n as f64
        };
        let run = |chroma: f32| {
            let mut out = noisy.clone();
            denoise_profiled(
                &mut out,
                w,
                h,
                &model,
                &DenoiseOptions {
                    method: DenoiseMethod::Wavelets,
                    strength: Strength::Fixed(STRENGTH_LOW),
                    chroma,
                    ..DenoiseOptions::default()
                },
            )
            .unwrap();
            (chroma_removed(&out), mse(&out, &truth, w, h, 8, w / 2 - 8))
        };
        let (removed_1, flat_1) = run(1.0);
        let (removed_3, flat_3) = run(3.0);
        assert!(
            removed_3 > 1.2 * removed_1,
            "removed {removed_1} -> {removed_3}"
        );
        // The flat half was already clear.
        assert!(flat_3 <= flat_1 * 1.01, "flat {flat_1} -> {flat_3}");
    }

    #[test]
    fn auto_strength_follows_the_noise() {
        let model = |sigma: f32| NoiseModel {
            a: [0.0; 3],
            b: [sigma * sigma; 3],
        };
        assert_eq!(Strength::Auto.resolve(&model(0.005)), STRENGTH_LOW);
        assert_eq!(Strength::Auto.resolve(&model(SIGMA_LOW)), STRENGTH_LOW);
        let mid = Strength::Auto.resolve(&model((SIGMA_LOW + SIGMA_HIGH) / 2.0));
        assert!(
            (mid - (STRENGTH_LOW + STRENGTH_HIGH) / 2.0).abs() < 1e-5,
            "{mid}"
        );
        assert_eq!(Strength::Auto.resolve(&model(0.1)), STRENGTH_HIGH);
        assert_eq!(Strength::Fixed(0.7).resolve(&model(0.1)), 0.7);
        assert_eq!("auto".parse::<Strength>().unwrap(), Strength::Auto);
        assert_eq!("2.5".parse::<Strength>().unwrap(), Strength::Fixed(2.5));
        assert!("loud".parse::<Strength>().is_err());
    }

    #[test]
    fn silent_model_and_zero_strength_leave_the_image_alone() {
        let (w, h) = (64, 48);
        let model = NoiseModel {
            a: [6e-4; 3],
            b: [1e-5; 3],
        };
        let (_, noisy) = noisy_scene(w, h, &model, 5);
        let mut out = noisy.clone();
        let stats = denoise_profiled(
            &mut out,
            w,
            h,
            &NoiseModel::default(),
            &DenoiseOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.scales, 0);
        assert_eq!(out, noisy);
        let stats = denoise_profiled(
            &mut out,
            w,
            h,
            &model,
            &DenoiseOptions {
                method: DenoiseMethod::Wavelets,
                strength: Strength::Fixed(0.0),
                chroma: 1.0,
                ..DenoiseOptions::default()
            },
        )
        .unwrap();
        assert_eq!(stats.scales, 0);
        assert!(denoise_profiled(&mut out, w, h + 1, &model, &DenoiseOptions::default()).is_err());
    }
}
