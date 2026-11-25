//! The stack's coarse word on which frames are sharp where.
//!
//! The blend's weights come from the Laplacian energy of each frame's
//! log luminance at full resolution, over a nine-pixel window. Where
//! the picture has strong edges that energy is the picture's and the
//! weights choose well. Where it has only a faint, fine texture (the
//! skin of a fruit, a spot inside something translucent) it is the
//! noise's: the Laplacian weighs a frequency by its square, so the
//! sensor's white noise at the top of the band outweighs a texture
//! some octaves below it many times over. On the bed (`bed.rs`, base
//! ISO, a texture of 1.5% of its level) the frame focused there holds
//! 1.06 times the median frame's energy against 1.35 without noise,
//! wins 12% of its pixels where chance is 9%, and takes a share of
//! the weight of 0.095 among eleven frames: the merge averages the
//! stack there, and keeps a quarter of the texture.
//!
//! The same texture is plain a level or two down the pyramid, where
//! the noise has been averaged away and the texture has not: in an
//! 8 px block at the quarter-size level the focused frame's energy is
//! 20 to 80 times the least frame's, where at full resolution the
//! frames' energies wander between one and three times the least with
//! no peak at the focused one. So each frame's energy is taken on its
//! log luminance reduced once and twice and averaged over [`SIDE`] px
//! blocks (8 by 8 samples at half size, 4 by 4 at quarter size).
//!
//! **Judged against the frame's own finest band.** Noise in log
//! luminance is not the same in every frame, nor across one: its
//! variance goes as one over the light where shot noise rules and one
//! over its square where read noise does, so a frame a stop darker, or
//! a deep shadow in one, carries more noise energy and would stand out
//! of every flat block as the sharpest frame. No exposure scale cancels
//! both. So what the trust is judged on is a ratio: each block's
//! energy at the reduced level over the same frame's full-resolution
//! energy in the same block. On noise alone that ratio is the noise's
//! spectral shape, whatever its level, local or global, shot or read.
//! Where a frame is focused on a faint texture the mid band rises and
//! the finest, still the noise's, does not. A block holding a sample at
//! the log plane's floor (no light at all under read noise: spikes of
//! many stops) says nothing.
//!
//! **The evidence.** In the placing pass each frame's blocks are put
//! onto the reference (each reference block takes the frame's block
//! its center lands in, and none where the block's corners do not all
//! land inside the frame) and counted per block across the stack: how
//! many frames reached it, the largest ratio, a histogram of the
//! ratios in half stops (from which the median), and the largest raw
//! mid-band energy. What it costs does not grow with the count. The
//! blending pass takes each frame's blocks the same way, from the
//! frame as read and not the warped one: a warp softens the finest
//! band and would tilt the ratio.
//!
//! **Whom to believe.** A block's median ratio is its noise's, as long
//! as fewer than half the frames are sharp there, which in a stack of
//! [`MIN_FRAMES`] or more is what a stack is. Its sharpest frame is
//! believed by how many times the median it stands: never under
//! [`ratio_from`] the level and the count, wholly from [`RATIO_SPAN`]
//! times that, smoothstepped between in the log. The threshold grows
//! with the count because the largest of more noisy draws is larger;
//! it is set over the null (see [`ratio_from`]).
//!
//! **The blend.** A frame's share of a believed block is its raw
//! mid-band energy over the block's largest, squared: not the ratio,
//! since a frame in focus on a strong texture raises its finest band
//! too and the ratio would hand the block to a neighbor a step out of
//! focus (measured: 0.744 of the strong edges kept fell to 0.704). It
//! multiplies the frame's weights in the blend's [`BANDS`] finest
//! bands, read bilinearly between block centers; the two levels'
//! multipliers are multiplied, as independent evidence.
//!
//! **A frame brighter than the rest.** Where every frame holds the same
//! faint texture and one is brighter, its lower noise lifts its ratio,
//! and from about two stops over the rest the block is believed; the
//! brighter frame's raw energy is the texture's and less noise, so it
//! is weighed up, by about its inverse variance (four times at two
//! stops) and no further. Up to a stop over, nothing is believed or
//! chosen (bed: a stop over, 0.8% of half-size blocks believed, the
//! frames' multipliers within 4% of each other). The exposure scale
//! the ratio replaced had the same case at 1.3 and 2 stops; the ratio
//! does not remove it, and what it chooses there is the cleaner frame.
//!
//! Not the coarser bands, nor the top of the pyramid. The evidence
//! speaks for the bands it was taken in; below them the frames are
//! alike in focus and are averaged as the pyramid always averaged
//! them, which is what keeps a seam soft and the halo of §125 where it
//! was. Weighing the whole pyramid measured the same on the bed, and
//! on a real stack whose frames were shot at different shutter speeds
//! it moved the sky's brightness wherever the prior chose a frame.
//!
//! The multiplier never goes under [`FLOOR`], so it breaks ties and
//! does not overrule: where a frame's own full-resolution energy is a
//! hundred times another's (a hard edge, in focus), it still wins.
//! It is per frame and read from that frame's own blocks, so a block
//! with two depths in it (a near edge over a far ground) keeps both
//! sharp frames, and nothing in it assumes the frames are in focus
//! order. A stack of fewer than [`MIN_FRAMES`] has no median to speak
//! of and is merged as it was; so is any merge at a selectivity of 0,
//! which is the plain weighted average that setting promises.

use rayon::prelude::*;

use crate::register::Transform;

/// A block's side, in full-resolution pixels.
pub(super) const SIDE: usize = 16;

/// The pyramid levels the evidence is taken at: half and quarter size.
pub(super) const LEVELS: std::ops::RangeInclusive<usize> = 1..=2;

/// The blend's levels the prior weighs, from the finest: the bands its
/// evidence speaks for. Coarser bands, and the top of the pyramid
/// (the picture's brightness), keep the frames' own weights.
pub(super) const BANDS: usize = 3;

/// The fewest frames, in the stack and at a block, for a median.
pub(super) const MIN_FRAMES: usize = 8;

/// The histogram's bins, in half stops of block energy from
/// [`LOWEST`]: 2^-40 to 2^8 covers every energy a log plane makes.
const BINS: usize = 96;
const LOWEST: f32 = -40.0;
const BIN: f32 = 0.5;

/// The ratio of a block's sharpest frame to its median under which it
/// is not believed at all, at pyramid level `level` (1 or 2) for
/// `count` frames. Set over the null: flat fields of base-ISO noise,
/// 102,000 blocks a count (`bed_null`, as many as a 24 MP picture
/// has), the largest ratio seen against the threshold:
///
/// | frames | half size: largest, threshold, margin | quarter size |
/// |---|---|---|
/// | 8 | 2.92, 3.30, 1.13 | 5.91, 6.61, 1.12 |
/// | 11 | 2.77, 3.32, 1.20 | 5.19, 6.64, 1.28 |
/// | 31 | 2.77, 3.37, 1.22 | 5.36, 6.74, 1.26 |
/// | 100 | 2.83, 3.43, 1.21 | 5.24, 6.86, 1.31 |
/// | 250 | 2.96, 3.48, 1.18 | 5.91, 6.95, 1.18 |
///
/// The largest barely grows with the count (a block's median ratio
/// tightens as the largest of more draws rises), so the threshold
/// grows by a little in the log of it, for room.
pub(super) fn ratio_from(level: usize, count: usize) -> f32 {
    let ln = (count as f32).ln();
    if level <= 1 {
        3.2 + 0.05 * ln
    } else {
        6.4 + 0.1 * ln
    }
}

/// From this many times [`ratio_from`], believed wholly.
const RATIO_SPAN: f32 = 1.25;

/// A frame's block whose finest energy (log2 units squared, a mean of
/// the four-neighbor Laplacian's square) is over this is no evidence:
/// its log luminance wanders by more than about 0.07 stops a sample,
/// a signal to noise of about 20 or worse, and under that the log of a
/// noisy sample is no longer near Gaussian and the ratio has long
/// tails (bed: a stop-under frame in a read-noise shadow reached 14
/// times its block's median). A frame sharp on a strong edge is over
/// it too; there its own full-resolution weight decides, as it did
/// before the prior. Faint texture at a usable exposure is far under:
/// about 0.002 on the bed.
const NOISY: f32 = 0.1;

/// The least a multiplier can be.
const FLOOR: f32 = 0.02;

/// A frame's Laplacian energy averaged over square blocks: `mean` the
/// statistic the trust is judged on, `raw` the energy a frame's share
/// is read from.
pub(super) struct Blocks {
    /// The full-resolution frame's size.
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
    mean: Vec<f32>,
    raw: Vec<f32>,
}

impl Blocks {
    /// `energy` is a plane `2^level` times smaller than `full` each
    /// way, as [`super::reduce`] makes it: its pixel `j` sits on the
    /// full resolution's `2^level j`, so block `b` covers full-resolution
    /// pixels from about `SIDE b - 2^level / 2` on.
    pub fn of(energy: &[f32], full: (usize, usize), level: usize) -> Self {
        let width = full.0.div_ceil(1 << level);
        let height = full.1.div_ceil(1 << level);
        let side = (SIDE >> level).max(1);
        let columns = width.div_ceil(side);
        let rows = height.div_ceil(side);
        let mut mean = vec![0.0f32; columns * rows];
        mean.par_chunks_mut(columns)
            .enumerate()
            .for_each(|(by, out)| {
                let y0 = by * side;
                let y1 = (y0 + side).min(height);
                let mut sums = vec![0.0f64; columns];
                for y in y0..y1 {
                    for (x, &e) in energy[y * width..(y + 1) * width].iter().enumerate() {
                        sums[x / side] += e as f64;
                    }
                }
                for (bx, (o, s)) in out.iter_mut().zip(&sums).enumerate() {
                    let x0 = bx * side;
                    let x1 = (x0 + side).min(width);
                    *o = (s / ((x1 - x0) * (y1 - y0)) as f64) as f32;
                }
            });
        Self {
            width: full.0,
            height: full.1,
            columns,
            rows,
            raw: mean.clone(),
            mean,
        }
    }

    /// The blocks as they are, for a frame already in the reference's
    /// geometry.
    #[cfg(test)]
    pub fn as_placed(&self) -> &[f32] {
        &self.mean
    }

    /// Both measures, onto the reference by `onto`.
    fn both(&self, onto: &Transform) -> (Vec<f32>, Vec<f32>) {
        if onto == &Transform::IDENTITY {
            (self.mean.clone(), self.raw.clone())
        } else {
            (self.place(&self.mean, onto), self.place(&self.raw, onto))
        }
    }

    /// Onto the reference's blocks through `onto` (a reference pixel to
    /// this frame's, as the warps take it): each block the frame's
    /// block its center lands in, not a number where any corner of it
    /// lands outside the frame. The nearest and not an interpolation,
    /// so a placed frame's blocks are as noisy as the reference's and
    /// as the blending pass measures them.
    fn place(&self, values: &[f32], onto: &Transform) -> Vec<f32> {
        let side = SIDE as f64;
        let (w, h) = (self.width as f64, self.height as f64);
        let (cols, rows) = (self.columns, self.rows);
        let inside = |x: f64, y: f64| {
            let (u, v) = onto.apply(x, y);
            u >= 0.0 && v >= 0.0 && u <= w - 1.0 && v <= h - 1.0
        };
        (0..cols * rows)
            .into_par_iter()
            .map(|b| {
                let (bx, by) = (b % cols, b / cols);
                let (x0, y0) = (bx as f64 * side, by as f64 * side);
                let x1 = ((bx + 1) as f64 * side - 1.0).min(w - 1.0);
                let y1 = ((by + 1) as f64 * side - 1.0).min(h - 1.0);
                if !(inside(x0, y0) && inside(x1, y0) && inside(x0, y1) && inside(x1, y1)) {
                    return f32::NAN;
                }
                let (u, v) = onto.apply((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                let ix = ((u / side) as usize).min(cols - 1);
                let iy = ((v / side) as usize).min(rows - 1);
                values[iy * cols + ix]
            })
            .collect()
    }
}

/// One level's evidence, counted a frame at a time: per block how many
/// frames reached it, the largest, and a histogram of all of them.
struct Evidence {
    count: Vec<u32>,
    peak: Vec<f32>,
    histogram: Vec<u16>,
    /// The largest raw energy, which a frame's share is read against.
    raw_peak: Vec<f32>,
}

impl Evidence {
    fn new(blocks: usize) -> Self {
        Self {
            count: vec![0; blocks],
            peak: vec![f32::NEG_INFINITY; blocks],
            histogram: vec![0; blocks * BINS],
            raw_peak: vec![0.0; blocks],
        }
    }

    fn add(&mut self, (placed, raw): (Vec<f32>, Vec<f32>)) {
        self.count
            .par_iter_mut()
            .zip(self.peak.par_iter_mut())
            .zip(self.histogram.par_chunks_mut(BINS))
            .zip(self.raw_peak.par_iter_mut())
            .zip(placed.par_iter().zip(raw.par_iter()))
            .for_each(|((((n, p), h), rp), (&v, &e))| {
                if v.is_finite() && v > 0.0 && e.is_finite() {
                    *n += 1;
                    *p = p.max(v);
                    *rp = rp.max(e);
                    let bin = ((v.log2() - LOWEST) / BIN).clamp(0.0, (BINS - 1) as f32) as usize;
                    h[bin] = h[bin].saturating_add(1);
                }
            });
    }

    /// Per block the median (from the histogram, read linearly within
    /// its bin), the sharpest frame's level, and the trust.
    fn finish(self, level: usize) -> Level {
        let n = self.count.len();
        let mut median = vec![0.0f32; n];
        let mut trust = vec![0.0f32; n];
        for b in 0..n {
            let c = self.count[b] as usize;
            if c < MIN_FRAMES {
                continue;
            }
            let h = &self.histogram[b * BINS..(b + 1) * BINS];
            let half = c as f32 / 2.0;
            let mut below = 0.0f32;
            let mut m = LOWEST;
            for (i, &k) in h.iter().enumerate() {
                let k = k as f32;
                if below + k >= half {
                    m = LOWEST + BIN * (i as f32 + (half - below) / k.max(1.0));
                    break;
                }
                below += k;
            }
            let m = m.exp2();
            let r = self.peak[b] / m;
            let from = ratio_from(level, c).ln();
            let t = ((r.ln() - from) / RATIO_SPAN.ln()).clamp(0.0, 1.0);
            median[b] = m;
            trust[b] = t * t * (3.0 - 2.0 * t);
        }
        #[cfg(not(test))]
        let _ = median;
        Level {
            #[cfg(test)]
            median,
            #[cfg(test)]
            peak: self.peak,
            trust,
            raw_peak: self.raw_peak,
        }
    }
}

struct Level {
    /// The median and the largest ratio per block: for the bed.
    #[cfg(test)]
    median: Vec<f32>,
    #[cfg(test)]
    peak: Vec<f32>,
    trust: Vec<f32>,
    raw_peak: Vec<f32>,
}

impl Level {
    /// One frame's multiplier per block, from its blocks placed on the
    /// reference's, multiplied into `into`: its raw energy's share of
    /// the block's largest, squared. Not the trust's statistic: a frame
    /// in focus on a strong texture has its finest band raised too, and
    /// the ratio would hand the block to a neighbor a step out of focus.
    fn multiplier(&self, (placed, raw): (Vec<f32>, Vec<f32>), into: &mut [f32]) {
        into.par_iter_mut()
            .zip(placed.par_iter().zip(raw.par_iter()))
            .enumerate()
            .for_each(|(b, (m, (&v, &e)))| {
                let a = self.trust[b];
                if a <= 0.0 || !v.is_finite() || !e.is_finite() {
                    return;
                }
                let peak = self.raw_peak[b];
                let r = if peak > 0.0 {
                    (e / peak).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                *m *= FLOOR + (1.0 - FLOOR) * (1.0 - a + a * r * r);
            });
    }
}

/// The evidence of every level in [`LEVELS`], gathered a frame at a
/// time in the placing pass.
pub(super) struct Gathered {
    levels: Vec<Evidence>,
    columns: usize,
    rows: usize,
}

impl Gathered {
    pub fn new(width: usize, height: usize) -> Self {
        let (columns, rows) = (width.div_ceil(SIDE), height.div_ceil(SIDE));
        Self {
            levels: LEVELS.map(|_| Evidence::new(columns * rows)).collect(),
            columns,
            rows,
        }
    }

    /// One frame's blocks, one a level, onto the reference by `onto`.
    pub fn add(&mut self, frame: &[Blocks], onto: &Transform) {
        for (e, b) in self.levels.iter_mut().zip(frame) {
            e.add(b.both(onto));
        }
    }

    pub fn finish(self) -> Prior {
        Prior {
            levels: self
                .levels
                .into_iter()
                .zip(LEVELS)
                .map(|(e, level)| e.finish(level))
                .collect(),
            columns: self.columns,
            rows: self.rows,
        }
    }
}

/// What the stack said, for the blending pass.
pub(super) struct Prior {
    levels: Vec<Level>,
    columns: usize,
    rows: usize,
}

impl Prior {
    /// The share of blocks the prior has any say in, at the level that
    /// has most: for the log.
    pub fn trusted(&self) -> f32 {
        self.levels
            .iter()
            .map(|l| l.trust.iter().filter(|&&t| t > 0.0).count())
            .max()
            .unwrap_or(0) as f32
            / (self.columns * self.rows).max(1) as f32
    }

    /// The most a block's multipliers differ across `frames` (already
    /// in the reference's geometry): the largest over the least, the
    /// largest such ratio over the picture.
    #[cfg(test)]
    pub fn spread(&self, frames: &[Vec<Blocks>]) -> f32 {
        let all: Vec<Vec<f32>> = frames
            .iter()
            .map(|f| self.multiplier(f, &Transform::IDENTITY))
            .collect();
        (0..self.columns * self.rows)
            .map(|b| {
                let (lo, hi) = all.iter().fold((f32::MAX, 0.0f32), |(lo, hi), m| {
                    (lo.min(m[b]), hi.max(m[b]))
                });
                hi / lo
            })
            .fold(1.0f32, f32::max)
    }

    /// Per level, every block's sharpest frame over its median, and
    /// whether it has any trust: none where too few frames reached it.
    #[cfg(test)]
    pub fn ratios(&self) -> Vec<Vec<(f32, bool)>> {
        self.levels
            .iter()
            .map(|l| {
                l.median
                    .iter()
                    .zip(&l.peak)
                    .zip(&l.trust)
                    .filter(|((m, _), _)| **m > 0.0)
                    .map(|((m, p), t)| (p / m, *t > 0.0))
                    .collect()
            })
            .collect()
    }

    /// Per level, the most trust any block whose center lies within
    /// `radius` px of `(cx, cy)` has, and how many such blocks there
    /// are: for a measurement of what the prior says about one place.
    #[cfg(test)]
    pub fn trust_near(&self, (cx, cy): (f64, f64), radius: f64) -> Vec<(f32, usize)> {
        let side = SIDE as f64;
        let near: Vec<usize> = (0..self.columns * self.rows)
            .filter(|b| {
                let (bx, by) = ((b % self.columns) as f64, (b / self.columns) as f64);
                let (x, y) = ((bx + 0.5) * side, (by + 0.5) * side);
                ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() <= radius
            })
            .collect();
        self.levels
            .iter()
            .map(|l| {
                let most = near.iter().map(|&b| l.trust[b]).fold(0.0f32, f32::max);
                (most, near.len())
            })
            .collect()
    }

    /// A frame's multiplier per block, from that frame's blocks in its
    /// own geometry, placed by `onto` as the placing pass placed them.
    pub fn multiplier(&self, frame: &[Blocks], onto: &Transform) -> Vec<f32> {
        let mut multiplier = vec![1.0f32; self.columns * self.rows];
        for (level, blocks) in self.levels.iter().zip(frame) {
            level.multiplier(blocks.both(onto), &mut multiplier);
        }
        multiplier
    }

    /// `weight`, a plane of pyramid level `level` (`width` wide), times
    /// the multiplier read bilinearly between block centers.
    pub fn apply(&self, multiplier: &[f32], weight: &mut [f32], width: usize, level: usize) {
        let (cols, rows) = (self.columns, self.rows);
        // A level's pixel j sits on the full resolution's 2^level j.
        let step = (1usize << level) as f32;
        let side = SIDE as f32;
        weight
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, line)| {
                let gy = ((y as f32 * step + 0.5) / side - 0.5).clamp(0.0, (rows - 1) as f32);
                let iy = gy as usize;
                let jy = (iy + 1).min(rows - 1);
                let fy = gy - iy as f32;
                let at = |bx: usize, by: usize| multiplier[by * cols + bx];
                for (x, w) in line.iter_mut().enumerate() {
                    let gx = ((x as f32 * step + 0.5) / side - 0.5).clamp(0.0, (cols - 1) as f32);
                    let ix = gx as usize;
                    let jx = (ix + 1).min(cols - 1);
                    let fx = gx - ix as f32;
                    let top = at(ix, iy) + fx * (at(jx, iy) - at(ix, iy));
                    let bottom = at(ix, jy) + fx * (at(jx, jy) - at(ix, jy));
                    *w *= top + fy * (bottom - top);
                }
            });
    }
}

/// A frame's blocks at every level in [`LEVELS`], from its log
/// luminance (full resolution, `width` by `height`) and that plane's
/// own Laplacian energy (`finest`): each the level's energy over the
/// finest energy in the same block. Not a number where the finest is
/// nothing (a block flat to the last bit, or floored).
pub(super) fn blocks(log: &[f32], finest: &[f32], width: usize, height: usize) -> Vec<Blocks> {
    let below = Blocks::of(finest, (width, height), 0).mean;
    // A block holding a sample at the log plane's floor (light at or
    // under nothing, where read noise outweighs the signal) has spikes
    // of many stops in it, which no ratio survives: it says nothing.
    let lowest = log.iter().copied().fold(f32::INFINITY, f32::min);
    let floored: Vec<f32> = log
        .par_iter()
        .map(|&v| if v <= lowest { 1.0 } else { 0.0 })
        .collect();
    let floored = Blocks::of(&floored, (width, height), 0);
    let mut out = Vec::new();
    let mut plane: Option<Vec<f32>> = None;
    let (mut w, mut h) = (width, height);
    for level in 1..=*LEVELS.end() {
        let reduced = super::reduce(plane.as_deref().unwrap_or(log), w, h, 1);
        w = w.div_ceil(2);
        h = h.div_ceil(2);
        if LEVELS.contains(&level) {
            let energy = crate::sharpness::laplacian_energy(&reduced, w, h);
            let mut blocks = Blocks::of(&energy, (width, height), level);
            blocks
                .mean
                .par_iter_mut()
                .zip(below.par_iter())
                .zip(floored.mean.par_iter())
                .for_each(|((v, &f), &dark)| {
                    *v = if f > f32::MIN_POSITIVE && f <= NOISY && dark == 0.0 {
                        *v / f
                    } else {
                        f32::NAN
                    }
                });
            out.push(blocks);
        }
        plane = Some(reduced);
    }
    out
}
