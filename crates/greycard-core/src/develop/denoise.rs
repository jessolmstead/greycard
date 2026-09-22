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
/// chain in the caller's buffer, a band of rows at a time, and collects
/// the shrunk detail in the one other frame this allocates.
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
        if options.method == DenoiseMethod::Nlm || !room {
            unstabilize(rgb);
            return Ok(DenoiseStats {
                nlm: Some(stats),
                ..nothing
            });
        }
        band_left = super::nlm::residual_band_ratios(&stats, options.nlm.search_radius);
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

    // Rotate to luma and chroma, in place.
    rgb.par_chunks_mut(3).for_each(|px| {
        px.copy_from_slice(&to_yuv([px[0], px[1], px[2]]));
    });

    let grids = variance_grids(rgb, width, height, scales);

    let mut detail = vec![0.0f32; width * height * 3];
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
        atrous_in_place(rgb, &mut detail, width, height, scale, sb2, &shrink);
    }

    // Residue plus detail, rotate back, unstabilize.
    rgb.par_chunks_mut(3)
        .zip(detail.par_chunks(3))
        .for_each(|(px, d)| {
            let v = from_yuv([px[0] + d[0], px[1] + d[1], px[2] + d[2]]);
            for c in 0..3 {
                px[c] = vst[c].inverse(v[c]);
            }
        });
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

/// The soft threshold of one band, from its local variance grid.
struct Shrink<'a> {
    grid: &'a LocalVariance,
    noise_floor: f32,
    gain: [f32; 3],
    sb2: f32,
}

impl Shrink<'_> {
    /// Add the shrunk detail `d` at `(x, y)` to `acc`.
    #[inline]
    fn apply(&self, x: usize, y: usize, d: [f32; 3], acc: &mut [f32]) {
        let var = self.grid.at(x, y);
        for c in 0..3 {
            let std_x = (var[c] - self.noise_floor).max(1e-12).sqrt();
            let t = self.gain[c] * self.sb2 / std_x;
            acc[c] += (d[c] - t).max(0.0) + (d[c] + t).min(0.0);
        }
    }
}

/// Rows per band of the in-place chain. The coarse rows of a band are
/// held back until no later band reads the rows they replace, so this
/// must be at least the blur's reach, `2^(scale+1)` rows.
const BAND_ROWS: usize = 256;

/// One à trous level in place: `cur` holds the level's input and, on
/// return, its coarse (the 5x5 B3 spline blur at spacing `2^scale`,
/// edge-guarded so that pixels many sigmas apart in color do not mix);
/// the detail `input - coarse`, shrunk, is added to `acc`.
///
/// The blur of a band of rows reads `reach` rows beyond it, so a row of
/// `cur` may only be replaced once the band after the one it belongs to
/// has been blurred. Each band's coarse goes to a buffer; after the next
/// band is blurred, the rows nothing later reads are shrunk and
/// committed, from the previous buffer's tail and this one's head.
fn atrous_in_place(
    cur: &mut [f32],
    acc: &mut [f32],
    width: usize,
    height: usize,
    scale: usize,
    band_variance: f32,
    shrink: &Shrink,
) {
    let mult = 1usize << scale;
    let reach = 2 * mult;
    let band = BAND_ROWS.max(reach).min(height);
    let row = width * 3;
    // darktable's guard: exp2(-max(0, distance^2 * 0.02 / sigma_band^2 - 9)).
    let inv_sigma2 = 0.02 / band_variance;

    let mut this = vec![0.0f32; band * row];
    let mut prev = vec![0.0f32; band * row];
    let mut prev_y0 = 0;
    let mut committed = 0;
    let mut y0 = 0;
    while y0 < height {
        let y1 = (y0 + band).min(height);
        {
            let cur = &*cur;
            this[..(y1 - y0) * row]
                .par_chunks_mut(row)
                .enumerate()
                .for_each(|(i, crow)| {
                    blur_row(cur, width, height, y0 + i, mult, inv_sigma2, crow);
                });
        }
        let until = if y1 == height { height } else { y1 - reach };
        let coarse_row = |r: usize| -> &[f32] {
            let (buf, base) = if r < y0 {
                (&prev, prev_y0)
            } else {
                (&this, y0)
            };
            &buf[(r - base) * row..(r - base + 1) * row]
        };
        cur[committed * row..until * row]
            .par_chunks_mut(row)
            .zip(acc[committed * row..until * row].par_chunks_mut(row))
            .enumerate()
            .for_each(|(i, (irow, arow))| {
                let r = committed + i;
                let crow = coarse_row(r);
                for x in 0..width {
                    let d = std::array::from_fn(|c| irow[x * 3 + c] - crow[x * 3 + c]);
                    shrink.apply(x, r, d, &mut arow[x * 3..x * 3 + 3]);
                }
                irow.copy_from_slice(crow);
            });
        committed = until;
        std::mem::swap(&mut this, &mut prev);
        prev_y0 = y0;
        y0 = y1;
    }
}

/// Row `y` of the guarded blur of `input` at spacing `mult`.
fn blur_row(
    input: &[f32],
    width: usize,
    height: usize,
    y: usize,
    mult: usize,
    inv_sigma2: f32,
    crow: &mut [f32],
) {
    for x in 0..width {
        let px = &input[(y * width + x) * 3..(y * width + x) * 3 + 3];
        let mut sum = [0.0f32; 3];
        let mut wgt = 0.0f32;
        for (jj, fy) in FILTER.iter().enumerate() {
            let yy = (y as isize + mult as isize * (jj as isize - 2)).clamp(0, height as isize - 1)
                as usize;
            for (ii, fx) in FILTER.iter().enumerate() {
                let xx = (x as isize + mult as isize * (ii as isize - 2))
                    .clamp(0, width as isize - 1) as usize;
                let q = &input[(yy * width + xx) * 3..(yy * width + xx) * 3 + 3];
                let dist = sq(px[0] - q[0]) + sq(px[1] - q[1]) + sq(px[2] - q[2]);
                let guard = (-(dist * inv_sigma2 - 9.0).max(0.0)).exp2();
                let w = fy * fx * guard;
                wgt += w;
                for c in 0..3 {
                    sum[c] += w * q[c];
                }
            }
        }
        for c in 0..3 {
            crow[x * 3 + c] = sum[c] / wgt;
        }
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

    #[inline]
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        // Position in tile units, measured from the first tile's center.
        let fx = ((x as f32 + 0.5) / self.tile as f32 - 0.5).max(0.0);
        let fy = ((y as f32 + 0.5) / self.tile as f32 - 0.5).max(0.0);
        let tx = (fx as usize).min(self.tiles_x - 1);
        let ty = (fy as usize).min(self.tiles_y - 1);
        let tx1 = (tx + 1).min(self.tiles_x - 1);
        let ty1 = (ty + 1).min(self.tiles_y - 1);
        let ax = (fx - tx as f32).min(1.0);
        let ay = (fy - ty as f32).min(1.0);
        let v00 = self.values[ty * self.tiles_x + tx];
        let v10 = self.values[ty * self.tiles_x + tx1];
        let v01 = self.values[ty1 * self.tiles_x + tx];
        let v11 = self.values[ty1 * self.tiles_x + tx1];
        std::array::from_fn(|c| {
            let top = v00[c] + (v10[c] - v00[c]) * ax;
            let bottom = v01[c] + (v11[c] - v01[c]) * ax;
            top + (bottom - top) * ay
        })
    }
}

/// The local variance of every band, from a decimated pyramid of
/// `image`: each level is the previous blurred with the 5-tap filter
/// and taken every other pixel, which is the unguarded à trous chain on
/// the lattice of its scale. Costs a third of one plain blur of the
/// image over all levels, and a quarter of the image in memory.
pub(crate) fn variance_grids(
    image: &[f32],
    width: usize,
    height: usize,
    scales: usize,
) -> Vec<LocalVariance> {
    let mut grids = Vec::with_capacity(scales);
    let mut level: Vec<f32> = Vec::new();
    let (mut w, mut h) = (width, height);
    for scale in 0..scales {
        let src = if scale == 0 { image } else { &level };
        let (grid, next) = pyramid_level(src, w, h, scale);
        grids.push(grid);
        level = next;
        w = w.div_ceil(2);
        h = h.div_ceil(2);
    }
    grids
}

/// One pyramid level: the tile variances of `src` minus its blur, and
/// the blur decimated by two for the next level.
fn pyramid_level(src: &[f32], w: usize, h: usize, scale: usize) -> (LocalVariance, Vec<f32>) {
    // Rows per task: a multiple of two and of the tile.
    const CHUNK: usize = 8 * LocalVariance::TILE;
    let tile = LocalVariance::TILE;
    let tiles_x = w.div_ceil(tile);
    let tiles_y = h.div_ceil(tile);
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    let row = w * 3;
    let mut next = vec![0.0f32; nw * nh * 3];
    let values: Vec<[f32; 3]> = next
        .par_chunks_mut(CHUNK / 2 * nw * 3)
        .enumerate()
        .flat_map_iter(|(k, nrows)| {
            let y0 = k * CHUNK;
            let y1 = (y0 + CHUNK).min(h);
            // The rows blurred along x: the chunk's and two beyond it.
            let ha = y0.saturating_sub(2);
            let hb = (y1 + 2).min(h);
            let mut hrows = vec![0.0f32; (hb - ha) * row];
            for (i, hrow) in hrows.chunks_exact_mut(row).enumerate() {
                blur_row_h(&src[(ha + i) * row..(ha + i + 1) * row], w, hrow);
            }
            let hrow = |y: isize| (y.clamp(0, h as isize - 1) as usize - ha) * row;

            let tile_rows = (y1 - y0).div_ceil(tile);
            let mut sums = vec![[0.0f64; 3]; tile_rows * tiles_x];
            let mut counts = vec![0u32; tile_rows * tiles_x];
            for y in y0..y1 {
                let taps: [usize; 5] = std::array::from_fn(|j| hrow(y as isize + j as isize - 2));
                let ty = (y - y0) / tile;
                for x in 0..w {
                    let t = ty * tiles_x + x / tile;
                    counts[t] += 1;
                    for c in 0..3 {
                        let i = x * 3 + c;
                        let b: f32 = FILTER
                            .iter()
                            .zip(taps)
                            .map(|(f, off)| f * hrows[off + i])
                            .sum();
                        let d = src[y * row + i] - b;
                        sums[t][c] += (d * d) as f64;
                        if y % 2 == 0 && x % 2 == 0 {
                            nrows[((y - y0) / 2 * nw + x / 2) * 3 + c] = b;
                        }
                    }
                }
            }
            sums.iter()
                .zip(counts)
                .map(|(s, n)| std::array::from_fn(|c| (s[c] / n as f64) as f32))
                .collect::<Vec<_>>()
        })
        .collect();
    debug_assert_eq!(values.len(), tiles_x * tiles_y);
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

/// One row blurred along x with the 5-tap filter, edges clamped.
fn blur_row_h(src: &[f32], w: usize, out: &mut [f32]) {
    for x in 0..w {
        for c in 0..3 {
            out[x * 3 + c] = FILTER
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let xx = (x as isize + i as isize - 2).clamp(0, w as isize - 1) as usize;
                    f * src[xx * 3 + c]
                })
                .sum();
        }
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
            let got = grid.values.iter().map(|v| v[0]).sum::<f32>() / grid.values.len() as f32;
            assert!(
                (got / want_var - 1.0).abs() < 0.06,
                "scale {scale}: {got} vs {want_var}"
            );
            assert_eq!(grid.tile, 8 << scale);
            assert_eq!(grid.tiles_x, w.div_ceil(8 << scale));
        }

        let mut cur = input.clone();
        let mut acc = vec![0.0f32; w * h * 3];
        let keep = LocalVariance {
            tiles_x: 1,
            tiles_y: 1,
            tile: w,
            values: vec![[1.0; 3]],
        };
        for (scale, &sb2) in want.iter().enumerate().take(4) {
            let shrink = Shrink {
                grid: &keep,
                noise_floor: 0.0,
                gain: [0.0; 3],
                sb2,
            };
            atrous_in_place(&mut cur, &mut acc, w, h, scale, sb2, &shrink);
        }
        for ((a, r), i) in acc.iter().zip(&cur).zip(&input) {
            assert!((a + r - i).abs() < 1e-4);
        }
    }

    #[test]
    fn in_place_chain_matches_a_plain_one() {
        // A tall, thin image so the chain runs over several bands at
        // every scale, and a two-band scale whose reach is a whole
        // band's worth of rows.
        let (w, h) = (24, 3 * BAND_ROWS + 37);
        let mut g = Gauss(7);
        let input: Vec<f32> = (0..w * h * 3)
            .map(|i| ((i / 3 / w) as f32 * 0.01).sin() + 0.3 * g.next())
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
        for (scale, &sb2) in want.iter().enumerate().take(4) {
            let shrink = Shrink {
                grid: &keep,
                noise_floor: 0.0,
                gain: [0.0; 3],
                sb2,
            };
            atrous_in_place(&mut cur, &mut acc, w, h, scale, sb2, &shrink);
            let mut coarse = vec![0.0f32; w * h * 3];
            for (y, crow) in coarse.chunks_exact_mut(w * 3).enumerate() {
                blur_row(&plain, w, h, y, 1 << scale, 0.02 / sb2, crow);
            }
            plain = coarse;
            assert_eq!(cur, plain, "coarse after scale {scale}");
        }
        for ((a, r), i) in acc.iter().zip(&cur).zip(&input) {
            assert!((a + r - i).abs() < 1e-4);
        }
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
