//! Merges in camera space: the HDR merge of a tripod bracket here, the
//! panorama's steps in the submodules.
//!
//! Each frame comes in as demosaiced camera RGB in its own sensor
//! units (nothing clipped after the demosaic, no gains on it), so a
//! photosite the sensor saturated reads 1 in its own channel. The
//! merge puts every frame on the reference's scale by its exposure
//! relative to the reference, read from EXIF and then measured from
//! the pixels both frames hold well ([`refine_exposure`]); drops each
//! frame's clipped samples ([`clip_mask`]); and averages the rest by
//! the inverse of their variance on that scale ([`Accumulator`]), so a
//! brighter frame's cleaner shadows outweigh a darker frame's by the
//! square of the exposure between them. Where every frame clipped, the
//! darkest frame's own value stands, taken after its highlight
//! reconstruction ([`Accumulator::fill_clipped`]).
//!
//! The merge is a per-pixel sum, so it streams: the caller adds one
//! frame at a time and the accumulators are all that grows, a sum and
//! a weight a pixel whatever the count.
//!
//! The written file is stored divided by its brightest white-balanced
//! sample ([`headroom`]); [`noise_profile`] says its noise as a DNG
//! `NoiseProfile`, and [`Tag`] what it was merged from.
//!
//! A panorama's own steps are in the submodules: [`features`] for where
//! two frames meet, [`pano`] for where each lands on the canvas,
//! [`seam`] for where they hand over, and [`blend`] for the bands.
//!
//! Nothing here is ported from another project.

pub mod blend;
pub mod features;
pub mod ghost;
pub mod pano;
pub mod seam;
mod tag;

pub use ghost::{Ghosts, Judge};
pub use tag::{Geometry, Laid, Placed, Recipe, Registration, Tag};

use rayon::prelude::*;

use crate::develop::noise::NoiseModel;
use crate::image::CameraImage;
use crate::raw::Shot;

/// A sample at or over this fraction of its channel's clip, in any
/// channel, is clipped: the demosaic softens a saturated photosite's
/// neighbors a little under 1.
pub const CLIP: f32 = 0.97;
/// How far a clipped pixel's mask is grown: the demosaic's reach
/// around a saturated photosite.
pub const GROW: usize = 2;
/// The band the exposure is measured over: pixels both frames hold
/// above this fraction of clip (out of the read noise) ...
pub const LOW: f32 = 0.01;
/// ... and under this one (below the knee where a sensor's response
/// bends).
pub const KNEE: f32 = 0.7;
/// A frame of a tripod bracket that moved more than this many pixels
/// is resampled onto the reference; less is left as it is.
pub const MOVED: f64 = 0.1;
/// Every this many pixels is read for the exposure's median: a 45 MP
/// frame gives millions either way.
const STRIDE: usize = 7;

/// A frame's exposure from its EXIF: shutter times ISO over the
/// f-number squared, in arbitrary units only a ratio means anything
/// in. A missing ISO or f-number counts as the same for every frame;
/// none without a shutter.
pub fn exif_exposure(shot: &Shot) -> Option<f64> {
    let t = f64::from(shot.exposure_time?);
    if !(t.is_finite() && t > 0.0) {
        return None;
    }
    let iso = shot.iso.map_or(100.0, f64::from).max(1.0);
    let n = shot
        .f_number
        .map(f64::from)
        .filter(|n| n.is_finite() && *n > 0.0)
        .unwrap_or(1.0);
    Some(t * iso / (n * n))
}

/// The frame a bracket is merged onto: the middle exposure, by the
/// frames' EXIF exposures (a frame with none counts as the darkest);
/// of an even count, the darker of the middle two, which clips less.
pub fn middle(exposures: &[Option<f64>]) -> usize {
    let mut order: Vec<usize> = (0..exposures.len()).collect();
    order.sort_by(|&a, &b| {
        exposures[a]
            .unwrap_or(0.0)
            .total_cmp(&exposures[b].unwrap_or(0.0))
            .then(a.cmp(&b))
    });
    order[(order.len().max(1) - 1) / 2]
}

/// Camera luminance, the mosaic's weighting: green counted twice.
#[inline]
pub fn luma(px: &[f32]) -> f32 {
    0.25 * px[0] + 0.5 * px[1] + 0.25 * px[2]
}

/// The pixels of `rgb` (interleaved camera RGB in sensor units,
/// `width` by `height`) with any channel at or over [`CLIP`], or not a
/// number (outside a resampled frame), grown by [`GROW`].
pub fn clip_mask(rgb: &[f32], width: usize, height: usize) -> Vec<bool> {
    assert_eq!(rgb.len(), width * height * 3);
    let hit: Vec<bool> = rgb
        .par_chunks(3)
        .map(|p| p.iter().any(|v| !v.is_finite() || *v >= CLIP))
        .collect();
    grow(&hit, width, height, GROW)
}

/// `mask` grown by `r` pixels each way, a square: rows first, then
/// columns.
pub fn grow(mask: &[bool], width: usize, height: usize, r: usize) -> Vec<bool> {
    assert_eq!(mask.len(), width * height);
    if r == 0 || width == 0 {
        return mask.to_vec();
    }
    let mut rows = vec![false; width * height];
    rows.par_chunks_mut(width)
        .zip(mask.par_chunks(width))
        .for_each(|(out, src)| {
            // The nearest set pixel to the left, then to the right.
            let mut last: Option<usize> = None;
            for x in 0..width {
                if src[x] {
                    last = Some(x);
                }
                out[x] = last.is_some_and(|l| x - l <= r);
            }
            let mut next: Option<usize> = None;
            for x in (0..width).rev() {
                if src[x] {
                    next = Some(x);
                }
                out[x] |= next.is_some_and(|n| n - x <= r);
            }
        });
    let mut out = vec![false; width * height];
    out.par_chunks_mut(width).enumerate().for_each(|(y, o)| {
        for yy in y.saturating_sub(r)..=(y + r).min(height - 1) {
            for (a, b) in o.iter_mut().zip(&rows[yy * width..(yy + 1) * width]) {
                *a |= *b;
            }
        }
    });
    out
}

/// An exposure measured from the pixels: the frame's over the
/// reference's, and how many pixels said so.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measured {
    pub ratio: f64,
    pub samples: usize,
}

/// Fewer pixels than this in the band and the measure is not taken.
pub const MIN_SAMPLES: usize = 1000;

/// The exposure of `frame` relative to `reference`, both camera RGB
/// in their own sensor units with their clip masks: the median ratio
/// of their luminances over the pixels neither clipped and both hold
/// between [`LOW`] and [`KNEE`]. Shutters miss their marks by a few
/// percent, and a merge on the marks shows it as banding where the
/// weights change hands. None with fewer than [`MIN_SAMPLES`] pixels.
pub fn refine_exposure(
    reference: &[f32],
    reference_mask: &[bool],
    frame: &[f32],
    frame_mask: &[bool],
) -> Option<Measured> {
    assert_eq!(reference.len(), frame.len());
    let n = reference_mask.len();
    let mut ratios: Vec<f32> = (0..n)
        .into_par_iter()
        .step_by(STRIDE)
        .filter_map(|i| {
            if reference_mask[i] || frame_mask[i] {
                return None;
            }
            let a = luma(&reference[i * 3..i * 3 + 3]);
            let b = luma(&frame[i * 3..i * 3 + 3]);
            (a > LOW && b > LOW && a < KNEE && b < KNEE).then(|| b / a)
        })
        .collect();
    if ratios.len() < MIN_SAMPLES {
        return None;
    }
    let mid = ratios.len() / 2;
    ratios.select_nth_unstable_by(mid, f32::total_cmp);
    Some(Measured {
        ratio: f64::from(ratios[mid]),
        samples: ratios.len(),
    })
}

/// The two luminance planes a fit of `frame` onto `reference` reads:
/// the frame's divided by its exposure `e` onto the reference's scale,
/// and both held to the band both frames hold well, from [`LOW`] of
/// the darker frame's clip to [`KNEE`] of the brighter's. A clip's
/// edge then falls at the same light in both, and moves with the
/// picture rather than with the exposure, which a fit would read as
/// motion; and the darker frame's shadows, mostly noise, are flat in
/// both.
pub fn fit_planes(reference: &[f32], frame: &[f32], e: f64) -> (Vec<f32>, Vec<f32>) {
    let e = e as f32;
    let lo = LOW * (1.0 / e).max(1.0);
    let hi = KNEE * (1.0 / e).min(1.0);
    let held = |v: f32| if v.is_finite() { v.clamp(lo, hi) } else { hi };
    let a = reference.par_chunks(3).map(|p| held(luma(p))).collect();
    let b = frame.par_chunks(3).map(|p| held(luma(p) / e)).collect();
    (a, b)
}

/// Below this ratio of its structure tensor's eigenvalues, a plane's
/// structure runs one way (a horizon over sky and sea, one edge between
/// two flat patches) and a translation along it is not to be read from
/// it: a fit there finds motion in the noise.
pub const MIN_CONDITION: f64 = 0.05;
/// A move a fit finds is taken only when it leaves at most this share
/// of the identity's residual: a frame that really moved fits clearly
/// better moved back.
pub const BETTER: f64 = 0.9;
/// The reduction the structure is read at, so the noise averages out
/// and what is left is the picture's.
const STRUCTURE_STEP: usize = 8;

/// How well a translation is posed on `plane` (`width` by `height`,
/// linear and positive): the smaller over the larger eigenvalue of the
/// structure tensor of its log, read at an eighth of its size. Near 1
/// for structure every way, near 0 for structure one way.
pub fn condition(plane: &[f32], width: usize, height: usize) -> f64 {
    let s = STRUCTURE_STEP;
    let (w, h) = (width / s, height / s);
    if w < 3 || h < 3 {
        return 0.0;
    }
    let small: Vec<f64> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let mut sum = 0.0f64;
            for yy in y * s..(y + 1) * s {
                let row = &plane[yy * width + x * s..yy * width + (x + 1) * s];
                sum += row.iter().map(|v| f64::from(*v)).sum::<f64>();
            }
            (sum / (s * s) as f64).max(1e-6).log2()
        })
        .collect();
    let (mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let gx = (small[y * w + x + 1] - small[y * w + x - 1]) / 2.0;
            let gy = (small[(y + 1) * w + x] - small[(y - 1) * w + x]) / 2.0;
            xx += gx * gx;
            yy += gy * gy;
            xy += gx * gy;
        }
    }
    let (t, d) = (xx + yy, xx * yy - xy * xy);
    if t <= 0.0 {
        return 0.0;
    }
    let root = ((t * t / 4.0) - d).max(0.0).sqrt();
    let (big, small) = (t / 2.0 + root, t / 2.0 - root);
    (small / big).max(0.0)
}

/// The root-mean-square difference of the logs of `a` and `b`, their
/// mean difference taken out (an exposure is an offset in the log),
/// over the pixels both hold: what a fit's move is judged by against
/// the identity's.
pub fn residual(a: &[f32], b: &[f32]) -> f64 {
    let (sum, sq, n) = a
        .par_iter()
        .zip(b)
        .filter(|(x, y)| x.is_finite() && y.is_finite() && **x > 0.0 && **y > 0.0)
        .map(|(x, y)| {
            let d = f64::from(x.log2() - y.log2());
            (d, d * d, 1usize)
        })
        .reduce(|| (0.0, 0.0, 0), |p, q| (p.0 + q.0, p.1 + q.1, p.2 + q.2));
    if n == 0 {
        return f64::INFINITY;
    }
    let mean = sum / n as f64;
    (sq / n as f64 - mean * mean).max(0.0).sqrt()
}

/// What pairs of frames say of the sensor's noise: a frame divided by
/// its exposure less the reference cancels the scene and leaves their
/// two noises, so its variance at each level, unlike one frame's own
/// estimate, does not read an edge as noise. Each pair gives, by level
/// (the brighter frame's, on the reference's scale), a variance from
/// the median of the squared differences of the green channel, which a
/// stray edge or a moved branch does not pull; [`PairNoise::fit`] reads
/// one shot and read term, in the reference's units, from all of them.
#[derive(Debug, Clone, Default)]
pub struct PairNoise {
    /// Each level's point: the shot term's factor, the read term's
    /// factor, and the variance there.
    points: Vec<(f64, f64, f64)>,
}

/// Every this many pixels is read for the pairs' noise.
const PAIR_STRIDE: usize = 3;
/// Half-stop bins from 2^-14 to 1 of the reference's clip.
const PAIR_BINS: usize = 28;
/// Fewer differences than this in a bin and it says nothing.
const PAIR_MIN: usize = 200;
/// The median of a chi-square of one degree: a Gaussian's squared
/// median over its variance.
const CHI2_MEDIAN: f64 = 0.454_936;

impl PairNoise {
    /// The pair of `frame` (exposure `e` relative to the reference,
    /// shot at `r` times its ISO) and `reference`, both camera RGB in
    /// their own units with their clip masks.
    pub fn add(
        &mut self,
        reference: &[f32],
        reference_mask: &[bool],
        frame: &[f32],
        frame_mask: &[bool],
        e: f64,
        r: f64,
    ) {
        let n = reference_mask.len();
        let mut bins: Vec<Vec<f32>> = vec![Vec::new(); PAIR_BINS];
        let e32 = e as f32;
        for i in (0..n).step_by(PAIR_STRIDE) {
            if reference_mask[i] || frame_mask[i] {
                continue;
            }
            let (g, k) = (reference[i * 3 + 1], frame[i * 3 + 1] / e32);
            if !(g.is_finite() && k.is_finite()) {
                continue;
            }
            let level = if e > 1.0 { k } else { g };
            if level <= 0.0 {
                continue;
            }
            let at = ((level.log2() + 14.0) * 2.0).floor();
            if !(0.0..PAIR_BINS as f32).contains(&at) {
                continue;
            }
            let d = k - g;
            bins[at as usize].push(d * d);
        }
        for (b, mut d2) in bins.into_iter().enumerate() {
            if d2.len() < PAIR_MIN {
                continue;
            }
            let mid = d2.len() / 2;
            d2.select_nth_unstable_by(mid, f32::total_cmp);
            let var = f64::from(d2[mid]) / CHI2_MEDIAN;
            let level = ((b as f64 + 0.5) / 2.0 - 14.0).exp2();
            if var > 0.0 {
                self.points
                    .push((level * (r / e + 1.0), r * r / (e * e) + 1.0, var));
            }
        }
    }

    /// The shot and read terms, in the reference's own units, that fit
    /// every pair's variances best in relative terms; none with too
    /// few levels, or none that fit with a positive shot term.
    pub fn fit(&self) -> Option<Noise> {
        if self.points.len() < 3 {
            return None;
        }
        let (mut a11, mut a12, mut a22, mut b1, mut b2) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for &(u, v, var) in &self.points {
            let (r1, r2) = (u / var, v / var);
            a11 += r1 * r1;
            a12 += r1 * r2;
            a22 += r2 * r2;
            b1 += r1;
            b2 += r2;
        }
        let det = a11 * a22 - a12 * a12;
        let (mut shot, mut read) = if det.abs() > f64::MIN_POSITIVE {
            ((b1 * a22 - b2 * a12) / det, (a11 * b2 - a12 * b1) / det)
        } else {
            (b1 / a11, 0.0)
        };
        if read < 0.0 {
            // No read noise to see: the line through the origin.
            read = 0.0;
            shot = b1 / a11;
        }
        (shot.is_finite() && shot > 0.0).then(|| Noise {
            shot: shot as f32,
            read: (read as f32).max(Noise::READ_FLOOR),
        })
    }
}

/// A frame's noise as the weights read it, in its own sensor units:
/// variance `shot * level + read`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Noise {
    pub shot: f32,
    pub read: f32,
}

impl Noise {
    /// What a frame with no noise measured is taken to have: a sensor
    /// of about 50,000 electrons at clip and a read noise near 2^-12
    /// of it. Only the ratio between frames moves the weights, and a
    /// bracket's frames share their sensor.
    pub const ASSUMED: Noise = Noise {
        shot: 2e-5,
        read: 6e-8,
    };

    /// The least read term there is: one step of a 14-bit sensor's
    /// quantization, squared, in units of its clip. No sensor reads
    /// with less noise than a step, so a fit that lands at zero is the
    /// fit's failure, not the sensor's; and under a read term of
    /// nothing a black sample's weight is the inverse of the float's
    /// floor, some 1e37, which pins its pixel to black whatever the
    /// other frames hold.
    pub const READ_FLOOR: f32 = 1.0 / (16384.0 * 16384.0);

    /// The core's noise model (measured before the gains, in the
    /// frame's normalized levels) as one pair, the channels weighted
    /// as [`luma`] weights them; [`Noise::ASSUMED`] for a model that
    /// measured nothing.
    pub fn of_model(model: &NoiseModel) -> Noise {
        let w = [0.25f32, 0.5, 0.25];
        let shot: f32 = (0..3).map(|c| w[c] * model.a[c]).sum();
        let read: f32 = (0..3).map(|c| w[c] * model.b[c]).sum();
        if model.is_silent() || !(shot.is_finite() && read.is_finite()) || shot <= 0.0 {
            Noise::ASSUMED
        } else {
            Noise {
                shot,
                read: read.max(Noise::READ_FLOOR),
            }
        }
    }

    /// This noise for a frame shot at `r` times the ISO: the shot term
    /// grows with the gain, the read term, the same in electrons, with
    /// its square.
    pub fn at_iso(&self, r: f32) -> Noise {
        Noise {
            shot: self.shot * r,
            read: self.read * r * r,
        }
    }

    /// The variance at `level` in the frame's own units, the read
    /// term never under [`Noise::READ_FLOOR`], however the noise was
    /// made.
    pub fn variance(&self, level: f32) -> f32 {
        let read = if self.read.is_nan() {
            Noise::READ_FLOOR
        } else {
            self.read.max(Noise::READ_FLOOR)
        };
        (self.shot * level.max(0.0) + read).max(Noise::READ_FLOOR)
    }
}

/// One frame as the accumulator takes it: camera RGB in its own
/// sensor units, its clip mask, its exposure relative to the
/// reference, and its noise.
pub struct Frame<'a> {
    pub rgb: &'a [f32],
    pub mask: &'a [bool],
    pub exposure: f64,
    pub noise: Noise,
}

/// The merge's running sums: each channel's weighted sum on the
/// reference's scale and the weight, one a pixel. A weight below zero
/// marks a pixel every frame clipped, its value set whole.
pub struct Accumulator {
    pub width: usize,
    pub height: usize,
    sum: Vec<f32>,
    weight: Vec<f32>,
    /// Each frame added: its exposure and noise, for the profile.
    frames: Vec<(f64, Noise)>,
}

impl Accumulator {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            sum: vec![0.0; width * height * 3],
            weight: vec![0.0; width * height],
            frames: Vec::new(),
        }
    }

    /// The bytes the sums hold.
    pub fn bytes(&self) -> usize {
        (self.sum.len() + self.weight.len()) * std::mem::size_of::<f32>()
    }

    /// The frames added, each its exposure and noise.
    pub fn frames(&self) -> &[(f64, Noise)] {
        &self.frames
    }

    /// `frame` added: each sample it holds unclipped divided by its
    /// exposure onto the reference's scale, weighted by the inverse of
    /// its variance there, `e^2 / (shot * l + read)` at its own
    /// luminance `l`. One weight a pixel for all three channels, so a
    /// pixel's color comes from the same frames as its brightness.
    pub fn add(&mut self, frame: &Frame<'_>) {
        self.add_kept(frame, None);
    }

    /// [`Accumulator::add`] with each pixel's weight multiplied by
    /// `keep`'s, 0 to 1: what the deghosting leaves of a frame. A keep
    /// of one adds the pixel exactly as `add` does.
    fn add_kept(&mut self, frame: &Frame<'_>, keep: Option<&[f32]>) {
        let w = self.width;
        assert_eq!(frame.rgb.len(), w * self.height * 3);
        assert_eq!(frame.mask.len(), w * self.height);
        let e = frame.exposure;
        let noise = frame.noise;
        self.sum
            .par_chunks_mut(w * 3)
            .zip(self.weight.par_chunks_mut(w))
            .zip(frame.rgb.par_chunks(w * 3))
            .zip(frame.mask.par_chunks(w))
            .enumerate()
            .for_each(|(y, (((sum, weight), rgb), mask))| {
                let keep = keep.map(|k| &k[y * w..(y + 1) * w]);
                for x in 0..w {
                    if mask[x] || weight[x] < 0.0 {
                        continue;
                    }
                    let p = &rgb[x * 3..x * 3 + 3];
                    if !p.iter().all(|v| v.is_finite()) {
                        continue;
                    }
                    let mut wt = (e * e / f64::from(noise.variance(luma(p)))) as f32;
                    if let Some(keep) = keep {
                        if keep[x] <= 0.0 {
                            continue;
                        }
                        wt *= keep[x];
                    }
                    for c in 0..3 {
                        sum[x * 3 + c] += wt * (f64::from(p[c]) / e) as f32;
                    }
                    weight[x] += wt;
                }
            });
        self.frames.push((e, noise));
    }

    /// Where no frame added so far held the pixel, `darkest`'s own
    /// value on the reference's scale, set whole: called once every
    /// frame is in, with the darkest frame after its highlight
    /// reconstruction. How many pixels it set.
    pub fn fill_clipped(&mut self, darkest: &Frame<'_>) -> usize {
        let w = self.width;
        let e = darkest.exposure;
        self.sum
            .par_chunks_mut(w * 3)
            .zip(self.weight.par_chunks_mut(w))
            .zip(darkest.rgb.par_chunks(w * 3))
            .map(|((sum, weight), rgb)| {
                let mut set = 0;
                for x in 0..w {
                    if weight[x] != 0.0 {
                        continue;
                    }
                    for c in 0..3 {
                        let v = rgb[x * 3 + c];
                        sum[x * 3 + c] = if v.is_finite() {
                            (f64::from(v.max(0.0)) / e) as f32
                        } else {
                            0.0
                        };
                    }
                    weight[x] = -1.0;
                    set += 1;
                }
                set
            })
            .sum()
    }

    /// The merge: each pixel's weighted mean, on the reference's scale.
    pub fn finish(self) -> CameraImage {
        let Accumulator {
            width,
            height,
            mut sum,
            weight,
            ..
        } = self;
        sum.par_chunks_mut(3).zip(&weight).for_each(|(p, &wt)| {
            if wt > 0.0 {
                p.iter_mut().for_each(|v| *v /= wt);
            } else if wt == 0.0 {
                p.fill(0.0);
            }
        });
        CameraImage::from_data(width, height, sum).expect("the sums are the frame's size")
    }
}

/// `image`'s samples over `bound` held to it, and how many were: the
/// merge's last guard. A merged sample is a weighted mean of unclipped
/// samples, each under its frame's clip over its exposure, or the
/// darkest frame's own value where every frame clipped; so none can be
/// over the darkest frame's brightest over its exposure, and one that
/// is was made by the arithmetic, not the scene.
pub fn hold_to(image: &mut CameraImage, bound: f32) -> usize {
    image
        .data
        .par_iter_mut()
        .map(|v| {
            if *v > bound {
                *v = bound;
                1
            } else {
                0
            }
        })
        .sum()
}

/// The headroom a merge needs so neither the writer's white level nor
/// the develop's clip after the gains bites: its brightest balanced
/// sample, and never under 1.
pub fn headroom(image: &CameraImage, gains: [f32; 3]) -> f32 {
    image
        .data
        .par_chunks(3)
        .map(|p| {
            (0..3)
                .map(|c| {
                    if p[c].is_finite() {
                        p[c] * gains[c]
                    } else {
                        0.0
                    }
                })
                .fold(0.0f32, f32::max)
        })
        .reduce(|| 1.0, f32::max)
        .max(1.0)
}

/// The merge's variance at `level` on the reference's scale: the
/// inverse of the summed inverse variances of the frames that hold it
/// unclipped there, or the darkest frame's alone where none does.
pub fn variance_at(frames: &[(f64, Noise)], level: f64) -> f64 {
    let level = level.max(0.0);
    let inverse: f64 = frames
        .iter()
        .filter(|(e, _)| level * e < f64::from(CLIP))
        .map(|(e, n)| e * e / f64::from(n.variance((level * e) as f32)))
        .sum();
    if inverse > 0.0 {
        return 1.0 / inverse;
    }
    frames
        .iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(0.0, |(e, n)| {
            f64::from(n.variance((level * e) as f32)) / (e * e)
        })
}

/// A DNG `NoiseProfile` for the merge stored divided by `headroom`:
/// the pair `(S, O)` whose line `S * y + O` keeps the largest ratio to
/// the merge's variance smallest, either way, at each stored level `y`
/// from 2^-14 up to the reference's clip, a quarter stop apart.
///
/// The true curve is not a line: the variance steps up wherever a
/// frame drops out at its clip, fourfold where the frame two stops
/// brighter goes, so no line is within a factor of two of all of it,
/// and a least-squares line sits on the many levels under the step
/// and leaves those over it several times short. Over the band the
/// reference holds, the line is as far over under the step as under
/// it over the step. Above the reference's clip, where the darkest
/// frame stands alone, it says too little; the file's readers (the
/// grain's estimate, the fill's cap, the sharpen's floor) see that
/// range only in highlights pulled down.
pub fn noise_profile(frames: &[(f64, Noise)], headroom: f32) -> [f64; 2] {
    let h = f64::from(headroom.max(1.0));
    let top = (f64::from(CLIP) / h).min(1.0).log2();
    let points: Vec<(f64, f64)> = (0..)
        .map(|i| -14.0 + f64::from(i) / 4.0)
        .take_while(|stops| *stops <= top)
        .map(|stops| {
            let y = stops.exp2();
            (y, variance_at(frames, y * h) / (h * h))
        })
        .filter(|(_, v)| v.is_finite() && *v > 0.0)
        .collect();
    let Some(&(_, v0)) = points.first() else {
        return [0.0, 0.0];
    };
    // The worst ratio of a line to the curve, in stops of variance.
    let worst = |s: f64, o: f64| {
        points
            .iter()
            .map(|(y, v)| ((s * y + o) / v).log2().abs())
            .fold(0.0, f64::max)
    };
    // A search on a log grid: the slope between the least and the most
    // the curve's levels ask, the offset from none to the darkest
    // level's whole variance.
    let slopes: Vec<f64> = points.iter().map(|(y, v)| v / y).collect();
    let (lo, hi) = slopes
        .iter()
        .fold((f64::MAX, 0.0f64), |(lo, hi), s| (lo.min(*s), hi.max(*s)));
    let grid = |from: f64, to: f64, n: usize| -> Vec<f64> {
        (0..=n)
            .map(|i| from * (to / from).powf(i as f64 / n as f64))
            .collect()
    };
    let mut offsets = grid(v0 * 1e-3, v0 * 2.0, 160);
    offsets.push(0.0);
    let mut best = (f64::MAX, [0.0, 0.0]);
    for &s in &grid(lo.max(f64::MIN_POSITIVE), hi.max(lo * 1.0001), 160) {
        for &o in &offsets {
            let w = worst(s, o);
            if w < best.0 {
                best = (w, [s, o]);
            }
        }
    }
    best.1
}

/// The long edge our merged DNGs' embedded preview is reduced to, at
/// most. [`preview`] reduces by a whole step, so the edge lands between
/// half this and this. Not larger: rawler's `DngWriter::preview` fits
/// anything wider than 1024 into 1024 by 768 by nearest neighbor, so a
/// 2048 preview came out the same size and coarser. A larger one waits
/// on that writer taking a size.
pub const PREVIEW_SIDE: usize = 1024;

/// An 8-bit sRGB rendering of camera RGB on the reference's scale, for
/// a DNG's preview and thumbnail: the gains, the camera matrix, sRGB,
/// clipped, box-averaged down to `max_side` on the long edge.
pub fn preview(
    image: &CameraImage,
    balance: &rawcolor::WhiteBalance,
    max_side: usize,
) -> crate::Result<image::RgbImage> {
    use crate::color::{CAT, WORKING_SPACE};
    let step = image
        .width
        .max(image.height)
        .div_ceil(max_side.max(1))
        .max(1);
    let (w, h) = ((image.width / step).max(1), (image.height / step).max(1));
    let gains = balance.coefficients_f32();
    let cam = balance.matrix_f32();
    let to_srgb = WORKING_SPACE
        .to_space_matrix(&rawcolor::RgbSpace::SRGB, CAT)
        .ok_or_else(|| crate::Error::Unsupported("working space to sRGB".into()))?
        .rows
        .map(|r| r.map(|v| v as f32));
    let data: Vec<u8> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let mut row = Vec::with_capacity(w * 3);
            for x in 0..w {
                let mut s = [0.0f32; 3];
                let mut n = 0.0f32;
                for sy in y * step..((y + 1) * step).min(image.height) {
                    for sx in x * step..((x + 1) * step).min(image.width) {
                        let p = image.pixel(sx, sy);
                        for c in 0..3 {
                            if p[c].is_finite() {
                                s[c] += p[c];
                            }
                        }
                        n += 1.0;
                    }
                }
                let b = [0, 1, 2].map(|c| s[c] / n.max(1.0) * gains[c]);
                // Columns, as the core's `apply_matrix` reads them.
                let wk = [0, 1, 2]
                    .map(|r| (cam[0][r] * b[0] + cam[1][r] * b[1] + cam[2][r] * b[2]).min(1.0));
                for r in to_srgb {
                    let v = (r[0] * wk[0] + r[1] * wk[1] + r[2] * wk[2]).clamp(0.0, 1.0);
                    let e: f32 = if v <= 0.003_130_8 {
                        12.92 * v
                    } else {
                        1.055 * v.powf(1.0 / 2.4) - 0.055
                    };
                    row.push((e * 255.0).round() as u8);
                }
            }
            row
        })
        .collect();
    image::RgbImage::from_raw(w as u32, h as u32, data)
        .ok_or_else(|| crate::Error::Unsupported("preview size".into()))
}

/// Brackets made up for tests: a mosaic scene exposed at a gain,
/// clipped as a sensor clips, with shot and read noise, written as a
/// small mosaic DNG with EXIF a probe reads.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use std::path::Path;

    use ::rawler::RawImage;
    use ::rawler::cfa::{CFA, PlaneColor};
    use ::rawler::decoders::{Camera, RawMetadata};
    use ::rawler::dng::writer::DngWriter;
    use ::rawler::dng::{
        CompressionParams, CropMode, DNG_VERSION_V1_4, DngCompression, DngPhotometricConversion,
    };
    use ::rawler::exif::Exif;
    use ::rawler::formats::tiff::{Rational, SRational};
    use ::rawler::imgop::xyz::Illuminant;
    use ::rawler::pixarray::PixU16;
    use ::rawler::rawimage::{BlackLevel, CFAConfig, RawPhotometricInterpretation, WhiteLevel};
    use ::rawler::tags::{DngTag, ExifTag};

    /// The scene's light at a pixel and channel (R, G, B as the RGGB
    /// mosaic has them), in the reference's sensor units: a smooth
    /// ramp across the frame from deep shadow to four stops over clip,
    /// with texture so a registration has something to hold.
    pub fn scene(x: usize, y: usize, c: usize, width: usize, height: usize) -> f32 {
        let u = x as f32 / width.max(1) as f32;
        let v = y as f32 / height.max(1) as f32;
        // -12 stops at the left to +4 at the right.
        let base = (u * 16.0 - 12.0).exp2();
        // Fine texture, and patches of other reflectances at no one
        // spacing, so a fit has edges to hold that do not repeat.
        let fine = 0.3 * ((x as f32 * 0.7).sin() * (y as f32 * 0.45).cos());
        let cell = (x / 23).wrapping_mul(7919) ^ (y / 19).wrapping_mul(104_729);
        let patch = [0.7, 1.0, 1.3, 0.85, 1.15][cell % 5];
        let texture = (1.0 + fine) * patch;
        let tint = [0.55, 1.0, 0.7][c] * (1.0 + 0.2 * v);
        base * texture * tint
    }

    /// Black and white levels of the made-up sensor, 14 bits.
    pub const BLACK: u16 = 512;
    pub const WHITE: u16 = 16383;

    /// A frame of the bracket: `scene` times `gain`, noise of variance
    /// `noise` (the sensor's normalized units) added with `seed`,
    /// clipped at the white, as `width` by `height` RGGB samples.
    pub fn mosaic(
        width: usize,
        height: usize,
        gain: f32,
        noise: super::Noise,
        seed: u64,
    ) -> Vec<u16> {
        mosaic_shifted(width, height, gain, noise, seed, 0)
    }

    /// [`mosaic`] with the scene moved `dx` pixels to the left: a
    /// tripod knocked between frames.
    pub fn mosaic_shifted(
        width: usize,
        height: usize,
        gain: f32,
        noise: super::Noise,
        seed: u64,
        dx: usize,
    ) -> Vec<u16> {
        mosaic_of(width, height, gain, noise, seed, |x, y, c| {
            scene(x + dx, y, c, width, height)
        })
    }

    /// Two flat patches side by side, one edge between them: a scene
    /// whose structure runs one way.
    pub fn two_patches(x: usize, _y: usize, c: usize, width: usize) -> f32 {
        let tint = [0.55, 1.0, 0.7][c];
        if x < width / 2 {
            0.05 * tint
        } else {
            0.4 * tint
        }
    }

    /// A checker of 16-pixel squares with soft edges, as a lens draws
    /// one, between two levels, moved `dx` pixels: structure both ways.
    pub fn checker(x: usize, y: usize, c: usize, dx: usize) -> f32 {
        let tint = [0.55, 1.0, 0.7][c];
        let wave = |v: usize| (std::f32::consts::PI * v as f32 / 16.0).cos();
        let v = 0.5 + 0.5 * wave(x + dx) * wave(y);
        (0.05 + 0.35 * v) * tint
    }

    /// A frame of `light` (the reference's sensor units at a pixel and
    /// channel) times `gain`, noise of `noise` added with `seed`,
    /// clipped at the white.
    pub fn mosaic_of(
        width: usize,
        height: usize,
        gain: f32,
        noise: super::Noise,
        seed: u64,
        light: impl Fn(usize, usize, usize) -> f32,
    ) -> Vec<u16> {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64
        };
        let range = f32::from(WHITE - BLACK);
        let mut out = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let c = match (y % 2, x % 2) {
                    (0, 0) => 0,
                    (1, 1) => 2,
                    _ => 1,
                };
                let signal = light(x, y, c) * gain;
                let (u1, u2) = (rnd().max(1e-12), rnd());
                let g = ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32;
                let v = (signal + g * noise.variance(signal).sqrt()).min(1.0);
                out.push((f32::from(BLACK) + v * range).round().clamp(0.0, 65535.0) as u16);
            }
        }
        out
    }

    /// A camera matrix (XYZ to camera, D65) close to a real body's.
    pub const COLOR_MATRIX: [f32; 9] = [0.7, -0.15, -0.05, -0.4, 1.25, 0.15, -0.05, 0.2, 0.6];

    /// `samples` written as an RGGB mosaic DNG of `make` and `model`,
    /// its as-shot white `neutral`, its EXIF a shutter of `shutter`
    /// seconds (as a fraction) at f/4 and ISO 100.
    pub fn write_dng(
        path: &Path,
        samples: Vec<u16>,
        width: usize,
        height: usize,
        shutter: (u32, u32),
        model: &str,
    ) {
        write_dng_at(path, samples, width, height, shutter, 100, model);
    }

    /// A merge's file at `out`, made of the frame [`write_dng`] wrote
    /// at `frame`: a flat picture of its size, a stop of headroom, and
    /// our tag.
    pub fn write_merged(frame: &Path, out: &Path) {
        let raw = crate::decode::decode_path(frame).unwrap();
        let (w, h) = (raw.width, raw.height);
        let image = crate::CameraImage::from_data(w, h, vec![0.25; w * h * 3]).unwrap();
        let tag = super::Tag {
            kind: "hdr".into(),
            frames: vec!["frame".into()],
            reference: 0,
            denoised: false,
            headroom_stops: 1.0,
            lens_corrected: false,
            ..Default::default()
        };
        let file = std::io::BufWriter::new(std::fs::File::create(out).unwrap());
        crate::dng::write_merged_dng(
            file,
            &raw,
            &image,
            [2.0, 1.0, 1.5],
            &crate::dng::LinearDngOptions {
                headroom: Some(2.0),
                ..Default::default()
            },
            &crate::dng::Merged {
                noise_profile: [1e-5, 1e-8],
                tag: &tag,
            },
        )
        .unwrap();
    }

    /// [`write_dng`] at `iso`.
    pub fn write_dng_at(
        path: &Path,
        samples: Vec<u16>,
        width: usize,
        height: usize,
        shutter: (u32, u32),
        iso: u16,
        model: &str,
    ) {
        write_dng_shot(path, samples, (width, height), shutter, iso, model, None);
    }

    /// The light all round a camera at pixel `(x, y)` and channel `c`
    /// of a frame `width` by `height` turned `yaw` radians about its
    /// vertical, its focal length `focal` pixels: value noise on the
    /// sphere of directions, a cell about a hundredth of a radian, and
    /// a gentle gradient, nothing near the clip. Frames of it are a
    /// panorama a camera turned exactly about its lens.
    pub fn turned(
        x: usize,
        y: usize,
        c: usize,
        width: usize,
        height: usize,
        focal: f64,
        yaw: f64,
    ) -> f32 {
        let rx = (x as f64 + 0.5 - width as f64 / 2.0) / focal;
        let ry = (y as f64 + 0.5 - height as f64 / 2.0) / focal;
        let (s, co) = yaw.sin_cos();
        let (dx, dz) = (co * rx + s, -s * rx + co);
        let azimuth = dx.atan2(dz);
        let elevation = ry.atan2(dx.hypot(dz));
        let hash = |i: i64, j: i64, o: u64| {
            let mut h = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ (j as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
                ^ o.wrapping_mul(0x1656_67B1_9E37_79F9);
            h ^= h >> 29;
            h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h ^= h >> 32;
            (h >> 11) as f64 / (1u64 << 53) as f64
        };
        let noise = |u: f64, v: f64, o: u64| {
            let (i, j) = (u.floor(), v.floor());
            let ease = |t: f64| t * t * (3.0 - 2.0 * t);
            let (a, b) = (ease(u - i), ease(v - j));
            let (i, j) = (i as i64, j as i64);
            let top = hash(i, j, o) * (1.0 - a) + hash(i + 1, j, o) * a;
            let bottom = hash(i, j + 1, o) * (1.0 - a) + hash(i + 1, j + 1, o) * a;
            top * (1.0 - b) + bottom * b
        };
        let fine = noise(azimuth * 100.0, elevation * 100.0, 1);
        let coarse = noise(azimuth * 12.0, elevation * 12.0, 2);
        let tint = [0.55, 1.0, 0.7][c];
        ((0.05 + 0.25 * fine + 0.2 * coarse) * tint) as f32
    }

    /// [`write_dng_at`] with the focal length in millimeters the EXIF
    /// says, if any.
    pub fn write_dng_shot(
        path: &Path,
        samples: Vec<u16>,
        (width, height): (usize, usize),
        shutter: (u32, u32),
        iso: u16,
        model: &str,
        focal: Option<u32>,
    ) {
        let cfa = CFA::new("RGGB");
        let mut camera = Camera::new();
        camera.make = "Synthetic".into();
        camera.model = model.into();
        camera.clean_make = camera.make.clone();
        camera.clean_model = camera.model.clone();
        camera.cfa = cfa.clone();
        let raw = RawImage::new(
            camera,
            PixU16::new_with(samples, width, height),
            1,
            [2.0, 1.0, 1.5, f32::NAN],
            RawPhotometricInterpretation::Cfa(CFAConfig::new(&cfa, &PlaneColor::default())),
            Some(BlackLevel::new(&[BLACK], 1, 1, 1)),
            Some(WhiteLevel::new(vec![u32::from(WHITE)])),
            false,
        );
        let metadata = RawMetadata {
            exif: Exif {
                exposure_time: Some(Rational::new(shutter.0, shutter.1)),
                fnumber: Some(Rational::new(4, 1)),
                iso_speed_ratings: Some(iso),
                date_time_original: Some("2026:11:01 12:00:00".into()),
                lens_model: Some("Synthetic 35mm".into()),
                focal_length: focal.map(|f| Rational::new(f, 1)),
                ..Exif::default()
            },
            make: "Synthetic".into(),
            model: model.into(),
            lens: None,
            unique_image_id: None,
            rating: None,
        };
        let mut file = std::io::Cursor::new(Vec::new());
        let mut dng = DngWriter::new(&mut file, DNG_VERSION_V1_4).unwrap();
        let mut main = dng.subframe_on_root(0);
        main.raw_image(
            &raw,
            CropMode::None,
            DngCompression::Uncompressed,
            DngPhotometricConversion::Original,
            &CompressionParams::default(),
        )
        .unwrap();
        main.finalize().unwrap();
        dng.load_base_tags(&raw).unwrap();
        dng.load_metadata(&metadata).unwrap();
        let m: Vec<SRational> = COLOR_MATRIX
            .iter()
            .map(|v| SRational::new((v * 10_000.0).round() as i32, 10_000))
            .collect();
        let root = dng.root_ifd_mut();
        root.add_tag(DngTag::CalibrationIlluminant1, u16::from(Illuminant::D65));
        root.add_tag(DngTag::ColorMatrix1, m.as_slice());
        root.add_tag(
            DngTag::AsShotNeutral,
            [
                Rational::new(500_000, 1_000_000),
                Rational::new(1_000_000, 1_000_000),
                Rational::new(666_667, 1_000_000),
            ]
            .as_slice(),
        );
        root.add_tag(ExifTag::Orientation, 1u16);
        dng.close().unwrap();
        std::fs::write(path, file.into_inner()).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat frame of `v` in every channel.
    fn flat(n: usize, v: f32) -> Vec<f32> {
        vec![v; n * 3]
    }

    /// A black frame under a noise whose read term is nothing weighs
    /// what the floor of a read term gives it: finite, and no more
    /// than a sensor's quantization step allows.
    #[test]
    fn a_black_sample_under_no_read_noise_has_a_bounded_weight() {
        let rgb = flat(4, 0.0);
        let mask = vec![false; 4];
        let mut acc = Accumulator::new(2, 2);
        acc.add(&Frame {
            rgb: &rgb,
            mask: &mask,
            exposure: 0.5,
            noise: Noise {
                shot: 2e-5,
                read: 0.0,
            },
        });
        let most = 0.25 / Noise::READ_FLOOR;
        for &wt in &acc.weight {
            assert!(wt.is_finite() && wt > 0.0 && wt <= most * 1.0001, "{wt}");
        }
        let fitted = Noise {
            shot: 2e-5,
            read: -1.0,
        };
        assert_eq!(fitted.variance(0.0), Noise::READ_FLOOR);
    }

    /// Samples over the bound are held to it and counted; the rest,
    /// and a sample not a number, are left.
    #[test]
    fn samples_over_the_bound_are_held_to_it() {
        let mut image =
            CameraImage::from_data(2, 1, vec![0.5, 4.0, 4.5, 5180.0, f32::NAN, -1.0]).unwrap();
        assert_eq!(hold_to(&mut image, 4.0), 2);
        assert_eq!(image.data[..4], [0.5, 4.0, 4.0, 4.0]);
        assert!(image.data[4].is_nan());
        assert_eq!(image.data[5], -1.0);
        assert!(headroom(&image, [1.0, 1.0, 1.0]) <= 4.0);
    }

    #[test]
    fn the_exposure_is_shutter_and_iso_over_the_aperture_squared() {
        let shot = |t, iso, n| Shot {
            exposure_time: Some(t),
            iso: Some(iso),
            f_number: Some(n),
            ..Shot::default()
        };
        let a = exif_exposure(&shot(1.0 / 100.0, 100, 4.0)).unwrap();
        let b = exif_exposure(&shot(1.0 / 25.0, 100, 4.0)).unwrap();
        let c = exif_exposure(&shot(1.0 / 100.0, 400, 8.0)).unwrap();
        assert!((b / a - 4.0).abs() < 1e-6);
        assert!((c / a - 1.0).abs() < 1e-6);
        assert_eq!(exif_exposure(&Shot::default()), None);
        // The middle of five, the darker middle of four.
        let e = [Some(4.0), Some(1.0), Some(16.0), Some(0.25), Some(0.0625)];
        assert_eq!(middle(&e), 1);
        assert_eq!(middle(&e[..4]), 1);
        assert_eq!(middle(&[Some(2.0), None]), 1);
    }

    /// A known ratio between two frames is recovered to 0.01%, the
    /// clipped and the near-black pixels left out.
    #[test]
    fn the_exposure_refinement_recovers_a_known_ratio() {
        let (w, h) = (400, 300);
        let truth = 3.8137;
        let mut reference = Vec::with_capacity(w * h * 3);
        let mut frame = Vec::with_capacity(w * h * 3);
        for i in 0..w * h {
            // From black to past clip in the brighter frame.
            let l = (i as f32 / (w * h) as f32 * 12.0 - 10.0).exp2();
            for c in 0..3 {
                let v = l * [0.6, 1.0, 0.8][c];
                reference.push(v.min(1.0));
                frame.push((v * truth as f32).min(1.0));
            }
        }
        let rm = clip_mask(&reference, w, h);
        let fm = clip_mask(&frame, w, h);
        let m = refine_exposure(&reference, &rm, &frame, &fm).unwrap();
        assert!(
            ((m.ratio - truth) / truth).abs() < 1e-4,
            "{} against {truth}",
            m.ratio
        );
        // Every pixel clipped in one or the other: nothing to measure.
        let all = vec![true; w * h];
        assert_eq!(refine_exposure(&reference, &all, &frame, &fm), None);
    }

    /// One clipped channel masks its pixel and the two around it each
    /// way, and nothing past them; a NaN (outside a resampled frame)
    /// masks too.
    #[test]
    fn the_clip_mask_grows_by_the_demosaics_reach() {
        let (w, h) = (11, 9);
        let mut rgb = flat(w * h, 0.5);
        let at = (4 * w + 5) * 3;
        rgb[at + 2] = 0.98;
        let m = clip_mask(&rgb, w, h);
        for y in 0..h {
            for x in 0..w {
                let near = x.abs_diff(5) <= GROW && y.abs_diff(4) <= GROW;
                assert_eq!(m[y * w + x], near, "({x}, {y})");
            }
        }
        // Just under the clip is held.
        rgb[at + 2] = 0.969;
        assert!(clip_mask(&rgb, w, h).iter().all(|m| !m));
        rgb[0] = f32::NAN;
        let m = clip_mask(&rgb, w, h);
        assert!(m[0] && m[2 * w + 2] && !m[3 * w + 3]);
    }

    /// A frame four times brighter weighs sixteen times as much where
    /// read noise rules (deep shadow), so the merge sits on it.
    #[test]
    fn a_brighter_frame_outweighs_a_darker_one_by_the_exposure_squared() {
        let n = 4;
        let noise = Noise {
            shot: 0.0,
            read: 1e-6,
        };
        let none = vec![false; n];
        let mut acc = Accumulator::new(n, 1);
        // The truth on the reference's scale is 0.01: the reference
        // reads 0.011, the bright frame (x4) reads 0.04 exactly.
        acc.add(&Frame {
            rgb: &flat(n, 0.011),
            mask: &none,
            exposure: 1.0,
            noise,
        });
        acc.add(&Frame {
            rgb: &flat(n, 0.04),
            mask: &none,
            exposure: 4.0,
            noise,
        });
        let out = acc.finish();
        // Weights 1 and 16: (0.011 + 16 * 0.01) / 17.
        let want = (0.011 + 16.0 * 0.01) / 17.0;
        assert!(
            out.data.iter().all(|v| (v - want).abs() < 1e-7),
            "{:?}",
            out.data
        );
        // With shot noise the ratio of weights at one level is e^2 over
        // the variance there.
        let shot = Noise {
            shot: 2e-5,
            read: 6e-8,
        };
        let (l, e) = (0.2f32, 4.0f64);
        let bright = e * e / f64::from(shot.variance(l * e as f32));
        let dark = 1.0 / f64::from(shot.variance(l));
        assert!(bright > dark * 3.0);
    }

    /// A pixel the reference clipped is the darker frame's; one every
    /// frame clipped is the darkest frame's own value, set whole.
    #[test]
    fn the_darkest_frame_stands_where_every_frame_clipped() {
        let (w, h) = (3, 1);
        let mut acc = Accumulator::new(w, h);
        let noise = Noise::ASSUMED;
        // Pixel 0 held by both; 1 clipped in the reference only; 2 in
        // both.
        let reference = [0.1, 0.1, 0.1, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let rm = [false, true, true];
        let dark = [0.025, 0.025, 0.025, 0.2, 0.2, 0.2, 0.99, 0.99, 0.99];
        let dm = [false, false, true];
        acc.add(&Frame {
            rgb: &reference,
            mask: &rm,
            exposure: 1.0,
            noise,
        });
        let darkest = Frame {
            rgb: &dark,
            mask: &dm,
            exposure: 0.25,
            noise,
        };
        acc.add(&darkest);
        // The reconstructed darkest frame reads past its clip there.
        let reconstructed = [0.025, 0.025, 0.025, 0.2, 0.2, 0.2, 1.3, 1.1, 1.2];
        assert_eq!(
            acc.fill_clipped(&Frame {
                rgb: &reconstructed,
                ..darkest
            }),
            1
        );
        let out = acc.finish();
        assert!((out.data[3] - 0.8).abs() < 1e-6, "{:?}", out.data);
        assert!((out.data[6] - 5.2).abs() < 1e-5 && (out.data[8] - 4.8).abs() < 1e-5);
        assert!((out.data[0] - 0.1).abs() < 1e-6);
    }

    /// The accumulators are a sum and a weight a pixel, whatever the
    /// count of frames added.
    #[test]
    fn the_sums_do_not_grow_with_the_count() {
        let (w, h) = (64, 48);
        let mut acc = Accumulator::new(w, h);
        let bytes = acc.bytes();
        assert_eq!(bytes, w * h * 4 * 4);
        let rgb = flat(w * h, 0.2);
        let mask = vec![false; w * h];
        for k in 0..9 {
            acc.add(&Frame {
                rgb: &rgb,
                mask: &mask,
                exposure: f64::from(k + 1),
                noise: Noise::ASSUMED,
            });
            assert_eq!(acc.bytes(), bytes);
        }
        assert_eq!(acc.frames().len(), 9);
    }

    /// The profile's line is one frame's when there is one, and over a
    /// bracket's band it is no further from the merge's variance than
    /// half the step a frame's clip makes, either way.
    #[test]
    fn the_noise_profile_follows_the_merge() {
        let one = [(1.0, Noise::ASSUMED)];
        let [s, o] = noise_profile(&one, 1.0);
        assert!((s - 2e-5).abs() / 2e-5 < 0.05, "{s}");
        assert!((o - 6e-8).abs() / 6e-8 < 0.05, "{o}");
        // Three frames two stops apart, stored under two stops: the
        // variance steps fourfold where the bright frame clips.
        let three = [
            (1.0, Noise::ASSUMED),
            (4.0, Noise::ASSUMED),
            (0.25, Noise::ASSUMED),
        ];
        let h = 4.0f32;
        let [s, o] = noise_profile(&three, h);
        let mut worst = (0.0f64, 0.0f64);
        // Up to the reference's clip, the band the line is fitted to.
        for i in 0..=47 {
            let y = (f64::from(i) / 4.0 - 14.0).exp2();
            let want = variance_at(&three, y * f64::from(h)) / f64::from(h * h);
            let r = (s * y + o) / want;
            worst = (worst.0.max(r), worst.1.max(1.0 / r));
        }
        assert!(worst.0 < 2.2 && worst.1 < 2.2, "{worst:?}");
        assert!(s > 0.0);
    }

    /// A two-patch scene's structure runs one way; a checker's every
    /// way.
    #[test]
    fn the_condition_tells_one_way_structure_from_two() {
        let (w, h) = (256, 160);
        let plane = |f: &dyn Fn(usize, usize) -> f32| -> Vec<f32> {
            (0..w * h).map(|i| f(i % w, i / w)).collect()
        };
        let patches = plane(&|x, _| testing::two_patches(x, 0, 1, w));
        let checker = plane(&|x, y| testing::checker(x, y, 1, 0));
        assert!(condition(&patches, w, h) < MIN_CONDITION / 10.0);
        assert!(condition(&checker, w, h) > 0.5);
        // The residual: none against itself, some against a shift.
        assert_eq!(residual(&checker, &checker), 0.0);
        let moved = plane(&|x, y| testing::checker(x, y, 1, 3));
        assert!(residual(&checker, &moved) > 0.1);
    }

    /// Pairs of frames whose noise is known give it back: the scene
    /// cancels, whatever its edges.
    #[test]
    fn the_pairs_give_back_the_noise() {
        let (w, h) = (512, 384);
        let truth = Noise {
            shot: 4e-5,
            read: 2e-7,
        };
        let mut s = 7u64;
        let mut gauss = move || {
            let mut u = || {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 11) as f64 / (1u64 << 53) as f64
            };
            let (a, b) = (u().max(1e-12), u());
            ((-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()) as f32
        };
        // A scene of sharp patches over twelve stops.
        let light = |i: usize| {
            let (x, y) = (i % w, i / w);
            ((x * 24 / w) as f32 * 0.5 - 12.0).exp2() * if (y / 16) % 2 == 0 { 1.0 } else { 0.6 }
        };
        let frame = |e: f32, r: f32, gauss: &mut dyn FnMut() -> f32| -> Vec<f32> {
            let n = truth.at_iso(r);
            (0..w * h)
                .flat_map(|i| {
                    let v = light(i) * e;
                    [v, v, v]
                })
                .map(|v| (v + gauss() * n.variance(v).sqrt()).min(1.0))
                .collect()
        };
        let reference = frame(1.0, 1.0, &mut gauss);
        let bright = frame(4.0, 1.0, &mut gauss);
        let none = vec![false; w * h];
        let mut pairs = PairNoise::default();
        pairs.add(
            &reference,
            &none,
            &bright,
            &clip_mask(&bright, w, h),
            4.0,
            1.0,
        );
        let got = pairs.fit().unwrap();
        assert!((got.shot / truth.shot - 1.0).abs() < 0.1, "{got:?}");
        assert!((got.read / truth.read - 1.0).abs() < 0.3, "{got:?}");
        // A frame four times the ISO, at the same shutter: four times the
        // exposure and the noise of the gain.
        let mut pairs = PairNoise::default();
        let iso = frame(4.0, 4.0, &mut gauss);
        pairs.add(&reference, &none, &iso, &clip_mask(&iso, w, h), 4.0, 4.0);
        let got = pairs.fit().unwrap();
        assert!((got.shot / truth.shot - 1.0).abs() < 0.1, "{got:?}");
        assert_eq!(PairNoise::default().fit(), None);
    }

    #[test]
    fn rows_grow_without_reaching_past_the_edge() {
        let (w, h) = (5, 1);
        let mut m = vec![false; w];
        m[0] = true;
        assert_eq!(grow(&m, w, h, 2), [true, true, true, false, false]);
        assert_eq!(grow(&m, w, h, 0), m);
    }
}
