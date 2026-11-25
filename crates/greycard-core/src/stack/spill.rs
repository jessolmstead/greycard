//! What a defocused frame spills from a bright edge, kept out of the
//! weights.
//!
//! **The fault.** A frame focused away from a bright object holds that
//! object's blur disc: light spread over the ground beside it, with a
//! rim where the disc ends. The rim is a real edge in that frame, and
//! a hard one when the disc is blown, since a clip draws a sharp line
//! whatever the focus. The blend's weights, the full-resolution energy
//! of the log luminance and each band's contrast, read it as detail,
//! and where the frame focused on the object has only the ground's
//! texture the rim wins. Measured on a blown specular (8 px at six
//! times the clip, nine frames, `halo_bed`): the merge showed the
//! defocused frames' rims as contours at 9, 11 and 14 px where the
//! focused frame has one edge at 8, its edge fell from 1.82 to 0.83
//! stops a pixel, and a ring outside it went to 0.61 of the ground,
//! where the bands of one frame's rim met the levels of another's.
//! Five frames, too short for the prior, were as bad (0.53). The
//! reverse is as real: inside a dark object the defocused frames hold
//! the bright ground leaking in, and the merge put a black hole in
//! the middle of it. It is not the prior's doing (§740) and the log
//! does not answer it: a rim is as steep in stops as any texture.
//!
//! **What tells spill from detail.** Light is conserved: a blur moves
//! it, and does not make it. Where a frame is much brighter than the
//! darkest frame of the stack at a place, that light came from
//! somewhere, and if the darkest frame holds within reach a source
//! clearly brighter, it may have come from there. May: a bright speck
//! in focus is brighter than every frame that blurs it, and if a sky
//! brighter still is in reach the test so far takes the speck's own
//! frame for spill (in review: specks two stops over their ground, 32
//! to 60 px from a sky, lost 0.6 of their texture). So the excess must
//! also be joined to its source: each connected stretch of a frame's
//! excess is kept only where, somewhere along its edge, a place just
//! outside it is brighter in some frame than the frame's light just
//! inside. A glow is joined to the object it spills from, a leak into
//! a dark object to the ground around it; a speck is a stretch of its
//! own with only its own blur about it, and is let be. On the bright
//! side of an extended edge a defocused frame is darker by at most a
//! stop (a blur keeps half the light at the edge), and the darker
//! frame's source is no brighter than the sharp frame's value there,
//! so the sharp frame is not taken for spill either. A clip is the one
//! case where the source can be no brighter than its spill; at the clip
//! the source need only match.
//!
//! **How.** Each frame's log luminance is taken at quarter size (where
//! texture and noise are averaged and the spill is not), put on the
//! reference, and set level with the reference by the median of its
//! difference, so a frame shot a stop under is not the darkest
//! everywhere. In the placing pass the stack keeps, per sample, the
//! darkest frame's level, the brightest that frame holds within
//! [`reach`], whether it is at the clip there, and its grain; the
//! brightest any frame is; and the reference's level: six quarter-size
//! planes, whatever the count. The bar over the darkest is [`OVER`] and the noise's share:
//! in a deep shadow the darkest of many frames is a draw from the
//! noise's low tail, and without it every frame was held somewhere and
//! the few left won by a millionfold (in review: noise up by half, a
//! blotch of 0.29 stops).
//!
//! At the clip means within [`AT_CLIP`] of the brightest: in the blend
//! the stack's, in the placing pass the frame's own, before the stack's
//! is known; the margin takes in a clipped disc's rim, just under it.
//! A frame is held only if it has a clip: a plateau at its brightest,
//! within [`PLATEAU`] of it, over half of some quarter-size sample.
//! The margin alone does not tell a clip: a small bright disc in focus
//! with little noise was half within it of its frame's brightest, its
//! noise's highest draws.
//!
//! In the blend a frame's weights are held down at every level of its
//! pyramid where it spills, widened by [`WIDEN`] and feathered over
//! [`FEATHER`]: every level, not the finest, since a held frame must not
//! come back by its coarse bands, or the merge takes one frame's rim at
//! one level and another's at the next, which is the dark ring.
//!
//! **Only at the clip.** A frame is held only where its spill is
//! itself at the clip and the darkest frame's is not (a blown
//! highlight's disc): to [`FLOOR`], wholly through the widening and
//! down in the log over the feather, since a clipped rim is an edge as
//! hard as any, its band contrast squared by the selectivity, and a
//! hold that let a thousandth of it through left the ring. A stack
//! with no clip merges as it did with nothing held, to the bit.
//!
//! The prior (`super::prior`) is told the same, a frame at a time as the
//! placing pass reads them: a frame's block is no evidence of what the
//! block's sharpest holds where it spills onto a darker frame's ground.
//!
//! **Measured** (`halo_bed`, before and after; `scene_bed` for the
//! scenes it must leave alone). The blown specular at nine frames: one
//! edge, at 8 px, 1.75 stops a pixel (0.83 before; the focused frame's
//! 1.82), the darkest ring 0.94 of the ground (0.61; 0.96); at five,
//! 1.82 and 0.96 (1.14 and 0.53). Through the engine's real walk (nine
//! raw frames, reconstructed) the merge is the focused frame's profile
//! to the hundredth. Scenes with no clip merge as before, to the bit,
//! the bed's (§740) among them; specks and seeds by a blown sky come
//! out as with nothing held or a little better.
//!
//! [`OVER`] starts at a third of a stop because the log of a blurred
//! strong texture sits a little over the sharp one's (the log of a mean
//! is over the mean of the logs): below it the strong ground beside a
//! depth edge was held down in the frames sharp on it, and the halo
//! rose to 6%.
//!
//! **Left.** The glow of a bright object under the clip (0.16 stops
//! outside a disc at nine frames); the middle of a dark object before
//! a bright ground, which goes black; and dark hairs over a ground near
//! the clip, which keep a little less of it (0.835 to 0.819 of its
//! texture). A clip in one or two channels is no plateau in the
//! luminance and is not caught: on 83 raws through the placing read,
//! [`clips`] found a clip in 37, and 24 of the 30 with blown areas a
//! reviewer could see; where it fires, the plateau of all three
//! channels is on average 22% of the area clipped in any. X-Trans
//! frames are not caught at all (none of four with blown areas: their
//! luminance tops out at 0.9989 of the clip, no sample having all three
//! channels at it). Taking [`PLATEAU`] out to the hold's own fiftieth
//! of a stop caught 14 more of the 83, some of them X-Trans, and also a
//! frame with no channel within a quarter of the clip and a small bright
//! disc in focus with little noise, whose stacks it read again for
//! nothing; so it stays at a thousandth.
//!
//! The spill under the clip was held too, in review, and taken out: it
//! mended the glow (0.16 stops to 0.07) and the dark middle (black to
//! 0.23 stops under), and harmed thin bright structure. At a quarter of
//! the size light is not conserved in it: a thin bright hair averages
//! down with its ground, so where bright hairs at two depths cross over
//! a dark ground, the darkest frame's crossing reads as a source for
//! the other set's sharp frame, which is held (hairs' rms 0.067 to
//! 0.088, the worst 0.57 stops under). Keeping only frames no sharper
//! than the darkest, since a blur only lowers fine energy, left most of
//! it (0.077). A next attempt needs a test of conservation that thin
//! structure cannot fool: at full size, or by what a blur of the
//! darkest frame could put there.
//!
//! **Only where there is a clip.** A stack with no clip pays a pass over
//! each frame's log for [`clips`], and nothing else: no plane is made or
//! gathered, and the blend measures nothing. The planes are gathered
//! from the first frame at its clip, the frames placed before it read
//! again for them in their order; what is gathered is what gathering
//! from the first would gather (`super::Gathering`).
//!
//! **Made again.** What is gathered depends on the order the frames are
//! read in (the prior's skips), and a stack made again reads them in the
//! chain's order as the placing pass did, so the merge is the same to
//! the bit. It reads every frame for that, a stack without a clip too:
//! only the placing pass's read tells a clip. The placing pass reads a
//! frame clipped and the blend may read it reconstructed; the blend
//! holds its log under the brightest the placing pass saw, so the two
//! measure the same light.

use rayon::prelude::*;

use super::prior::{Role, SIDE};
use crate::register::{self, Edge, Kernel, Transform};

/// The pyramid level the frames are compared at: quarter size.
const LEVEL: usize = 2;

/// How far a frame's darker neighbors are searched for a source, at
/// full resolution: a thirty-second of the short side, never under
/// 64 px. A spill further out than that is not caught.
fn reach(width: usize, height: usize) -> usize {
    (width.min(height) / 32).max(64)
}

/// The stops over the darkest frame from which a frame is held down
/// not at all, and wholly, before the noise's share.
const OVER: (f32, f32) = (0.35, 0.85);

/// How far, in stops, a source must stand over the light it explains,
/// and the width of the ramp to that.
const CLEAR: f32 = 0.5;
const RAMP: f32 = 0.25;

/// How many deviations of a frame's own noise (at quarter size) the
/// bar over the darkest is raised by, besides the darkest's own draw
/// from the noise's low tail (about the root of twice the log of the
/// count, deviations): in a deep shadow the darkest is that draw, not a
/// level.
const NOISE: f32 = 2.0;

/// The noise's share of the bar over the darkest of `count` frames, in
/// deviations.
fn tail(count: usize) -> f32 {
    NOISE + (2.0 * (count.max(2) as f32).ln()).sqrt()
}

/// The hold is widened by this many quarter-size samples (8 px), then
/// feathered over as many again.
const WIDEN: usize = 2;
const FEATHER: usize = 2;

/// How far under the ceiling, in stops, a sample is at the clip.
const AT_CLIP: f32 = 0.02;

/// How far under a frame's brightest, in stops, a sample is on its
/// plateau: what tells a clip from the brightest of the noise.
const PLATEAU: f32 = 1e-3;

/// What a held frame's weight is multiplied by where it is held
/// wholly: low enough that a clipped rim's band contrast, squared by
/// the selectivity, cannot win it back.
const FLOOR: f32 = 1e-6;

/// The size of a plane `LEVEL` levels down.
fn reduced(width: usize, height: usize) -> (usize, usize) {
    let (mut w, mut h) = (width, height);
    for _ in 0..LEVEL {
        w = w.div_ceil(2);
        h = h.div_ceil(2);
    }
    (w, h)
}

/// A frame's log luminance at [`LEVEL`], in its own geometry, and the
/// deviation of the noise in it.
pub(super) struct Coarse {
    width: usize,
    height: usize,
    plane: Vec<f32>,
    noise: Vec<f32>,
    /// The share of each sample's light at the clip.
    clipped: Vec<f32>,
    /// The brightest sample of the full-resolution log.
    top: f32,
    /// The frame's grain: the lower quartile of `noise`, where the
    /// frame is flat and its fine energy is the noise's alone.
    grain: f32,
    /// Whether the frame has a clip ([`clips`]): it is held nowhere
    /// without one.
    clip: bool,
}

/// The brightest number in `plane`.
fn brightest(plane: &[f32]) -> f32 {
    plane
        .par_iter()
        .copied()
        .reduce(|| f32::NEG_INFINITY, f32::max)
}

/// Each full-resolution sample's weights in the samples [`LEVEL`]
/// binomial reductions of a line `n` long make of it: per sample, the
/// reduced samples it reaches and by how much, the line's ends held as
/// [`super::reduce`] holds them.
fn weights(n: usize) -> Vec<Vec<(usize, f32)>> {
    const TAPS: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    // What each sample of the level reached so far is made of.
    let mut of: Vec<Vec<(usize, f32)>> = (0..n).map(|p| vec![(p, 1.0)]).collect();
    let mut len = n;
    for _ in 0..LEVEL {
        let next = len.div_ceil(2);
        of = (0..next)
            .map(|x| {
                let mut made: Vec<(usize, f32)> = Vec::new();
                for (k, tap) in TAPS.iter().enumerate() {
                    let i = (2 * x + k).saturating_sub(2).min(len - 1);
                    for &(p, w) in &of[i] {
                        match made.iter_mut().find(|(q, _)| *q == p) {
                            Some((_, v)) => *v += tap * w,
                            None => made.push((p, tap * w)),
                        }
                    }
                }
                made
            })
            .collect();
        len = next;
    }
    let mut reach = vec![Vec::new(); n];
    for (x, made) in of.iter().enumerate() {
        for &(p, w) in made {
            reach[p].push((x, w));
        }
    }
    reach
}

/// Whether a frame's `log` (full resolution, `width` by `height`) has a
/// clip: some quarter-size sample over half on the plateau at its
/// brightest ([`PLATEAU`]), by [`LEVEL`] binomial reductions. Read from
/// the samples on it alone, with the reductions' own weights, so a
/// frame without a clip costs a pass over its plane and nothing held.
/// A single sample brighter than a blown plateau (a hot pixel) sets the
/// brightest over it and hides it; a read clamped at the clip, as the
/// engine's placing read is, has none, but a caller that hands the
/// stack frames over 1 may.
pub(super) fn clips(log: &[f32], width: usize, height: usize) -> bool {
    let clip = brightest(log) - PLATEAU;
    let at: Vec<usize> = log
        .par_iter()
        .enumerate()
        .filter_map(|(i, &v)| (v >= clip).then_some(i))
        .collect();
    // No sample takes more than a quarter of any one: under three is
    // under a half anywhere.
    if at.len() < 3 {
        return false;
    }
    let (w, h) = reduced(width, height);
    // A wide clip is a clip; past a sample in sixteen, reading it
    // sample by sample costs what reducing it would.
    if at.len() > w * h {
        return true;
    }
    let (across, down) = (weights(width), weights(height));
    let mut share = vec![0.0f32; w * h];
    for &i in &at {
        for &(y, wy) in &down[i / width] {
            for &(x, wx) in &across[i % width] {
                share[y * w + x] += wy * wx;
            }
        }
    }
    // Summed in another order than the reductions sum: a hair under the
    // half, so as never to call a clip none.
    share.iter().any(|&s| s > 0.5 - 1e-3)
}

/// `log` (a full-resolution log2 luminance plane) reduced to [`LEVEL`],
/// held under `ceiling` (stops) when given; `finest` is the plane's own
/// Laplacian energy, which on noise is twenty times its variance; `clip`
/// what [`clips`] says of it.
pub(super) fn coarse(
    log: &[f32],
    finest: &[f32],
    width: usize,
    height: usize,
    ceiling: Option<f32>,
    clip: bool,
) -> Coarse {
    let top = brightest(log);
    // At the clip: under the ceiling in the blend, the frame's own
    // brightest in the placing pass (the module's note).
    let at = ceiling.unwrap_or(top) - AT_CLIP;
    let down = |plane: &[f32]| {
        let (mut w, mut h) = (width, height);
        let mut out = super::reduce(plane, w, h, 1);
        for _ in 1..LEVEL {
            w = w.div_ceil(2);
            h = h.div_ceil(2);
            out = super::reduce(&out, w, h, 1);
        }
        out
    };
    let plane = match ceiling {
        Some(c) => down(&log.par_iter().map(|&v| v.min(c)).collect::<Vec<_>>()),
        None => down(log),
    };
    // White noise's Laplacian energy is twenty times its variance, and
    // two binomial reductions average about fifty samples of it.
    let mut noise = down(finest);
    noise
        .par_iter_mut()
        .for_each(|e| *e = (e.max(0.0) / 20.0 / 50.0).sqrt());
    let (w, h) = reduced(width, height);
    let clipped = if top >= at {
        down(
            &log.par_iter()
                .map(|&v| if v >= at { 1.0 } else { 0.0 })
                .collect::<Vec<_>>(),
        )
    } else {
        vec![0.0; w * h]
    };
    let mut sorted: Vec<f32> = noise.iter().copied().filter(|e| e.is_finite()).collect();
    let grain = if sorted.is_empty() {
        0.0
    } else {
        let q = sorted.len() / 4;
        *sorted.select_nth_unstable_by(q, f32::total_cmp).1
    };
    Coarse {
        width: w,
        height: h,
        plane,
        noise,
        clipped,
        top,
        grain,
        clip,
    }
}

impl Coarse {
    /// A plane of this frame's onto the reference's grid; not a number
    /// where the frame does not reach.
    fn placed(&self, plane: &[f32], onto: &Transform) -> Vec<f32> {
        if onto == &Transform::IDENTITY {
            return plane.to_vec();
        }
        register::warp_plane(
            plane,
            self.width,
            self.height,
            self.width,
            self.height,
            &onto.at_scale(1.0 / (1 << LEVEL) as f64),
            Edge::Fill(f32::NAN),
            Kernel::Bilinear,
        )
    }
}

/// The median of `a - b` where both are numbers: how far one frame's
/// light sits from the other's.
fn offset(a: &[f32], b: &[f32]) -> f32 {
    let mut d: Vec<f32> = a
        .iter()
        .zip(b)
        .map(|(x, y)| x - y)
        .filter(|v| v.is_finite())
        .collect();
    if d.is_empty() {
        return 0.0;
    }
    let mid = d.len() / 2;
    *d.select_nth_unstable_by(mid, f32::total_cmp).1
}

/// The largest of `line` within `radius` each way, into `out`, numbers
/// only, by van Herk and Gil and Werman: a running maximum forward and
/// back within blocks of the window's width, two comparisons a sample
/// whatever the radius.
fn max_line(line: &[f32], radius: usize, out: &mut [f32]) {
    let n = line.len();
    let k = 2 * radius + 1;
    let at = |i: usize| -> f32 {
        // `line` padded with nothing by `radius` each side.
        if i < radius || i - radius >= n {
            f32::NEG_INFINITY
        } else {
            let v = line[i - radius];
            if v.is_nan() { f32::NEG_INFINITY } else { v }
        }
    };
    let len = n + 2 * radius;
    let mut forward = vec![f32::NEG_INFINITY; len];
    let mut back = vec![f32::NEG_INFINITY; len];
    for i in 0..len {
        let v = at(i);
        forward[i] = if i % k == 0 { v } else { forward[i - 1].max(v) };
    }
    for i in (0..len).rev() {
        let v = at(i);
        back[i] = if i % k == k - 1 || i == len - 1 {
            v
        } else {
            back[i + 1].max(v)
        };
    }
    // The window about sample j is padded samples j to j + 2 radius.
    for (j, o) in out.iter_mut().enumerate() {
        *o = back[j].max(forward[j + 2 * radius]);
    }
}

/// The largest of `plane` within `radius` samples each way, numbers
/// only.
fn local_max(plane: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let mut rows = vec![0.0f32; plane.len()];
    rows.par_chunks_mut(width)
        .zip(plane.par_chunks(width))
        .for_each(|(out, row)| max_line(row, radius, out));
    // The columns, by way of a transpose.
    let mut columns = vec![0.0f32; plane.len()];
    columns
        .par_chunks_mut(height)
        .enumerate()
        .for_each(|(x, col)| {
            for (y, c) in col.iter_mut().enumerate() {
                *c = rows[y * width + x];
            }
        });
    let mut done = vec![0.0f32; plane.len()];
    done.par_chunks_mut(height)
        .zip(columns.par_chunks(height))
        .for_each(|(out, col)| max_line(col, radius, out));
    let mut out = vec![0.0f32; plane.len()];
    out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            *o = done[x * height + y];
        }
    });
    out
}

/// How much a frame at `v` stops spills, against the darkest frame's
/// `low` and the brightest that frame holds within reach, `source`,
/// with the frame's noise `noise` and the clip at `top`: 0 none, 1
/// wholly.
fn spill(v: f32, noise: f32, low: f32, source: f32, top: f32, tail: f32) -> f32 {
    if !v.is_finite() || !low.is_finite() {
        return 0.0;
    }
    let bar = OVER.0 + tail * noise;
    let over = smooth((v - low - bar) / (OVER.1 - OVER.0));
    let clear = CLEAR.min((top - v).max(0.0));
    over * smooth((source - v - clear + RAMP) / RAMP)
}

/// Whether light at `v` stands in the shadow of a brighter neighbor
/// `high`: the source the spill came from, as at a clip it need only
/// match.
fn lit_by(v: f32, high: f32, top: f32) -> bool {
    let clear = CLEAR.min((top - v).max(0.0));
    high - v > clear - RAMP / 2.0
}

/// The spill `hold` (one frame's, per sample) kept only where it is
/// joined to its source: each connected stretch of it is kept when
/// somewhere along its edge a sample outside it is brighter, in some
/// frame (`high`), than the frame's light (`v`) just inside. A sharp
/// bright speck is a stretch of its own with nothing brighter about
/// it, and is not spill however bright the things in reach.
fn joined(hold: &mut [f32], v: &[f32], high: &[f32], top: f32, width: usize, height: usize) {
    let n = hold.len();
    let mut label = vec![u32::MAX; n];
    let mut stack = Vec::new();
    let mut members = Vec::new();
    for start in 0..n {
        if hold[start] <= 0.0 || label[start] != u32::MAX {
            continue;
        }
        let id = start as u32;
        label[start] = id;
        stack.push(start);
        members.clear();
        let mut fed = false;
        while let Some(i) = stack.pop() {
            members.push(i);
            let (x, y) = (i % width, i / width);
            let mut look = |j: usize| {
                if hold[j] > 0.0 {
                    if label[j] == u32::MAX {
                        label[j] = id;
                        stack.push(j);
                    }
                } else if high[j].is_finite() && lit_by(v[i], high[j], top) {
                    fed = true;
                }
            };
            if x > 0 {
                look(i - 1);
            }
            if x + 1 < width {
                look(i + 1);
            }
            if y > 0 {
                look(i - width);
            }
            if y + 1 < height {
                look(i + width);
            }
        }
        if !fed {
            for &i in &members {
                hold[i] = 0.0;
            }
        }
    }
}

/// What the stack gathered per sample, as a hold is read against it.
struct Planes<'a> {
    width: usize,
    height: usize,
    count: usize,
    top: f32,
    low: &'a [f32],
    low_grain: &'a [f32],
    low_clipped: &'a [f32],
    source: &'a [f32],
    high: &'a [f32],
}

impl Planes<'_> {
    /// A frame's hold (`v`, level with the reference, its `noise`, its
    /// `grain` and its share at the clip): where it spills, joined to a
    /// source, and at the clip where the darkest is not.
    ///
    /// The bar is the noise's as the noisier of the frame and the
    /// darkest has it: one frame noisier than the rest is the darkest by
    /// its low draws, and a bar by the others' noise held them all (in
    /// review: a deep shadow 8.6 stops under where one frame of eleven
    /// had eight times the noise). The darkest's own fine energy is no
    /// measure of that, since it holds the darkest's detail too, and
    /// raised the bar over a blown disc's ring; the ratio of the frames'
    /// grains is.
    fn hold(&self, v: &[f32], noise: &[f32], grain: f32, clipped: &[f32]) -> Vec<f32> {
        let tail = tail(self.count);
        let mut hold: Vec<f32> = (0..v.len())
            .into_par_iter()
            .map(|i| {
                let noisier = (self.low_grain[i] / grain).max(1.0);
                let noise = if noisier.is_finite() {
                    noise[i] * noisier
                } else {
                    noise[i]
                };
                spill(v[i], noise, self.low[i], self.source[i], self.top, tail)
            })
            .collect();
        joined(&mut hold, v, self.high, self.top, self.width, self.height);
        hold.par_iter_mut().enumerate().for_each(|(i, h)| {
            if !(clipped[i] > 0.5 && self.low_clipped[i] < 0.5) {
                *h = 0.0;
            }
        });
        hold
    }
}

/// Per sample, the darkest frame's level and the brightest that frame
/// holds within reach, and the brightest any frame is, gathered a frame
/// at a time in the placing pass.
pub(super) struct Gathered {
    width: usize,
    height: usize,
    radius: usize,
    /// The prior's blocks: how many, and how many a row.
    blocks: usize,
    columns: usize,
    /// How many frames are gathered.
    count: usize,
    top: f32,
    reference: Option<Vec<f32>>,
    low: Vec<f32>,
    source: Vec<f32>,
    high: Vec<f32>,
    /// The darkest frame's share at the clip, and its grain.
    low_clipped: Vec<f32>,
    low_grain: Vec<f32>,
}

impl Gathered {
    pub fn new(width: usize, height: usize) -> Self {
        let (w, h) = reduced(width, height);
        let (columns, rows) = (width.div_ceil(SIDE), height.div_ceil(SIDE));
        Self {
            width: w,
            height: h,
            radius: reach(width, height) >> LEVEL,
            blocks: columns * rows,
            columns,
            count: 0,
            top: f32::NEG_INFINITY,
            reference: None,
            low: vec![f32::INFINITY; w * h],
            source: vec![f32::NEG_INFINITY; w * h],
            high: vec![f32::NEG_INFINITY; w * h],
            low_clipped: vec![0.0; w * h],
            low_grain: vec![0.0; w * h],
        }
    }

    fn planes(&self) -> Planes<'_> {
        Planes {
            width: self.width,
            height: self.height,
            count: self.count,
            top: self.top,
            low: &self.low,
            low_grain: &self.low_grain,
            low_clipped: &self.low_clipped,
            source: &self.source,
            high: &self.high,
        }
    }

    /// One frame, onto the reference by `onto`; the reference first, as
    /// the chain reads it. What the prior is to do with each of its
    /// blocks, as the frames read so far say: skip it where it spills
    /// onto a darker frame's ground.
    pub fn add(&mut self, frame: &Coarse, onto: &Transform) -> Vec<Role> {
        self.top = self.top.max(frame.top);
        self.count += 1;
        let placed = frame.placed(&frame.plane, onto);
        let noise = frame.placed(&frame.noise, onto);
        let shift = match &self.reference {
            None => {
                self.reference = Some(placed.clone());
                0.0
            }
            Some(r) => offset(&placed, r),
        };
        let v: Vec<f32> = placed.par_iter().map(|p| p - shift).collect();
        let most = local_max(&v, self.width, self.height, self.radius);
        let clipped = frame.placed(&frame.clipped, onto);
        // A frame with no clip is held nowhere: its roles are all kept,
        // unread.
        let roles = if frame.clip {
            self.roles(&v, &noise, frame.grain, &clipped)
        } else {
            vec![Role::Keep; self.blocks]
        };
        self.low
            .par_iter_mut()
            .zip(self.source.par_iter_mut())
            .zip(self.high.par_iter_mut())
            .zip(
                self.low_clipped
                    .par_iter_mut()
                    .zip(self.low_grain.par_iter_mut()),
            )
            .zip(v.par_iter().zip(most.par_iter()).zip(clipped.par_iter()))
            .for_each(|((((low, source), high), (lc, lg)), ((&v, &m), &c))| {
                if v.is_finite() {
                    if v < *low {
                        *low = v;
                        *source = m;
                        *lc = c;
                        *lg = frame.grain;
                    }
                    *high = high.max(v);
                }
            });
        roles
    }

    /// Each block's [`Role`] for a frame (`v`, level with the reference,
    /// and its `noise`) against the frames before it, read over the
    /// block and [`WIDEN`] about it as the hold is: skipped where any
    /// sample spills.
    fn roles(&self, v: &[f32], noise: &[f32], grain: f32, clipped: &[f32]) -> Vec<Role> {
        let (w, h) = (self.width, self.height);
        let hold = self.planes().hold(v, noise, grain, clipped);
        let side = SIDE >> LEVEL;
        let columns = self.columns;
        (0..self.blocks)
            .into_par_iter()
            .map(|b| {
                let (bx, by) = ((b % columns) * side, (b / columns) * side);
                for y in by.saturating_sub(WIDEN)..(by + side + WIDEN).min(h) {
                    for x in bx.saturating_sub(WIDEN)..(bx + side + WIDEN).min(w) {
                        if hold[y * w + x] > 0.5 {
                            return Role::Skip;
                        }
                    }
                }
                Role::Keep
            })
            .collect()
    }

    pub fn finish(self) -> Spill {
        Spill {
            width: self.width,
            height: self.height,
            count: self.count,
            top: self.top,
            reference: self.reference.unwrap_or_default(),
            low: self.low,
            source: self.source,
            high: self.high,
            low_clipped: self.low_clipped,
            low_grain: self.low_grain,
        }
    }
}

/// What the placing pass found, for the blend.
#[cfg_attr(test, derive(Debug, PartialEq))]
pub(super) struct Spill {
    width: usize,
    height: usize,
    count: usize,
    top: f32,
    reference: Vec<f32>,
    low: Vec<f32>,
    source: Vec<f32>,
    high: Vec<f32>,
    low_clipped: Vec<f32>,
    low_grain: Vec<f32>,
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Spill {
    /// What a frame's log is held under in the blend: the brightest the
    /// placing pass saw.
    pub fn ceiling(&self) -> f32 {
        self.top
    }

    /// The linear luminance from which a frame can be held: a frame with
    /// none at it is held nowhere.
    pub fn held_from(&self) -> f32 {
        (self.top - AT_CLIP).exp2()
    }

    /// A frame's hold per quarter-size sample, from its coarse plane in
    /// its own geometry placed by `onto`: 1 where it is not spilling,
    /// [`FLOOR`] where it is, through the widening, and down in the log
    /// over the feather; none where it is nowhere held.
    pub fn multiplier(&self, frame: &Coarse, onto: &Transform) -> Option<Vec<f32>> {
        let placed = frame.placed(&frame.plane, onto);
        let noise = frame.placed(&frame.noise, onto);
        let shift = offset(&placed, &self.reference);
        let (w, h) = (self.width, self.height);
        let v: Vec<f32> = placed.par_iter().map(|p| p - shift).collect();
        let clipped = frame.placed(&frame.clipped, onto);
        let planes = Planes {
            width: w,
            height: h,
            count: self.count,
            top: self.top,
            low: &self.low,
            low_grain: &self.low_grain,
            low_clipped: &self.low_clipped,
            source: &self.source,
            high: &self.high,
        };
        let hold = planes.hold(&v, &noise, frame.grain, &clipped);
        if !hold.par_iter().any(|&h| h > 0.0) {
            return None;
        }
        let held =
            crate::sharpness::box_blur(&local_max(&hold, w, h, WIDEN + FEATHER), w, h, FEATHER);
        let ln = FLOOR.ln();
        Some(
            held.into_par_iter()
                .map(|h| (h.clamp(0.0, 1.0).powi(2) * ln).exp())
                .collect(),
        )
    }

    /// `weight`, a plane of the blend's pyramid level `level` (`width`
    /// wide), times `multiplier` read bilinearly.
    pub fn apply(&self, multiplier: &[f32], weight: &mut [f32], width: usize, level: usize) {
        let (mw, mh) = (self.width, self.height);
        // A level's sample j sits on the full resolution's 2^level j.
        let step = (1usize << LEVEL) as f32 / (1usize << level) as f32;
        weight
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, line)| {
                let gy = (y as f32 / step).clamp(0.0, (mh - 1) as f32);
                let iy = gy as usize;
                let jy = (iy + 1).min(mh - 1);
                let fy = gy - iy as f32;
                let at = |x: usize, y: usize| multiplier[y * mw + x];
                for (x, w) in line.iter_mut().enumerate() {
                    let gx = (x as f32 / step).clamp(0.0, (mw - 1) as f32);
                    let ix = gx as usize;
                    let jx = (ix + 1).min(mw - 1);
                    let fx = gx - ix as f32;
                    let top = at(ix, iy) + fx * (at(jx, iy) - at(ix, iy));
                    let bottom = at(ix, jy) + fx * (at(jx, jy) - at(ix, jy));
                    *w *= top + fy * (bottom - top);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What [`clips`] says is what the placing pass's planes say, or
    /// a clip where they have none, never none where they have one:
    /// clusters of one to twelve samples at the clip in the corners, on
    /// the edges and inside, and a wide one.
    #[test]
    fn a_clip_is_found_wherever_it_is() {
        let (w, h) = (37usize, 29usize);
        let mut found = 0;
        for (cx, cy) in [(0, 0), (34, 26), (0, 13), (17, 0), (17, 13), (9, 21)] {
            for count in 1..=12usize {
                let mut log = vec![-3.0f32; w * h];
                for j in 0..count {
                    let (x, y) = ((cx + j % 3).min(w - 1), (cy + j / 3).min(h - 1));
                    log[y * w + x] = 0.0;
                }
                let truth = coarse(&log, &vec![0.0; w * h], w, h, None, true)
                    .clipped
                    .iter()
                    .any(|&c| c > 0.5);
                let said = clips(&log, w, h);
                assert!(said || !truth, "{count} at ({cx}, {cy})");
                assert!(said == truth || count >= 3, "{count} at ({cx}, {cy})");
                found += truth as usize;
            }
        }
        assert!(found > 0);
        let wide: Vec<f32> = (0..w * h)
            .map(|i| if i % w < 20 { 0.0 } else { -1.0 })
            .collect();
        assert!(clips(&wide, w, h));
    }

    /// The running maximum is the plain one, at every radius and with
    /// holes in the plane.
    #[test]
    fn the_running_maximum_is_the_plain_one() {
        let (w, h) = (37usize, 23usize);
        let plane: Vec<f32> = (0..w * h)
            .map(|i| {
                if i % 11 == 3 {
                    f32::NAN
                } else {
                    ((i * 7919) % 101) as f32
                }
            })
            .collect();
        for radius in [0usize, 1, 2, 5, 40] {
            let got = local_max(&plane, w, h, radius);
            for y in 0..h {
                for x in 0..w {
                    let mut want = f32::NEG_INFINITY;
                    for yy in y.saturating_sub(radius)..=(y + radius).min(h - 1) {
                        for xx in x.saturating_sub(radius)..=(x + radius).min(w - 1) {
                            let v = plane[yy * w + xx];
                            if v > want {
                                want = v;
                            }
                        }
                    }
                    assert_eq!(got[y * w + x], want, "radius {radius} at {x},{y}");
                }
            }
        }
    }

    /// Not a check, a timing: the running maximum at the reach a 100
    /// megapixel frame asks for, against the plain one it replaced.
    /// `cargo test --release -p greycard-core -- --ignored --nocapture
    /// timing_of_the_running_maximum`.
    #[test]
    #[ignore = "a timing, not a test"]
    fn timing_of_the_running_maximum() {
        let (fw, fh) = (12_000usize, 8_400usize);
        let (w, h) = reduced(fw, fh);
        let radius = reach(fw, fh) >> LEVEL;
        let plane: Vec<f32> = (0..w * h).map(|i| ((i * 7919) % 1013) as f32).collect();
        let start = std::time::Instant::now();
        let fast = local_max(&plane, w, h, radius);
        let took = start.elapsed().as_secs_f64();
        let start = std::time::Instant::now();
        let mut rows = vec![0.0f32; w * h];
        rows.par_chunks_mut(w)
            .zip(plane.par_chunks(w))
            .for_each(|(out, row)| {
                for (x, o) in out.iter_mut().enumerate() {
                    let (a, b) = (x.saturating_sub(radius), (x + radius).min(w - 1));
                    *o = row[a..=b].iter().copied().fold(f32::NEG_INFINITY, f32::max);
                }
            });
        let mut plain = vec![f32::NEG_INFINITY; w * h];
        plain.par_chunks_mut(w).enumerate().for_each(|(y, line)| {
            let (a, b) = (y.saturating_sub(radius), (y + radius).min(h - 1));
            for yy in a..=b {
                for (o, &v) in line.iter_mut().zip(&rows[yy * w..(yy + 1) * w]) {
                    *o = o.max(v);
                }
            }
        });
        let slow = start.elapsed().as_secs_f64();
        assert_eq!(fast, plain);
        println!(
            "{fw}x{fh} at quarter size, radius {radius}: running {took:.3} s, plain {slow:.3} s"
        );
    }
}
