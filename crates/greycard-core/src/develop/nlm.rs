//! Non-local means in the variance-stabilized space.
//!
//! Ported from darktable's `src/common/nlmeans_core.c` (copyright
//! 2011-2024 darktable developers, the sliding-window patch comparison by
//! Ralph Giles and the patch scattering by Rawfiner; GPL-3.0-or-later),
//! as `src/iop/denoiseprofile.c` drives it in its "non-local means" mode:
//! the patch offsets and their scattering, the patch dissimilarity with
//! its norm and floor, and the weight are theirs. The transform is the
//! one [`super::denoise`] uses, the tiling and threading are ours, and
//! the automatic patch size and scattering follow the reference's shape
//! on this engine's noise model rather than darktable's profile units.
//!
//! Each pixel becomes the weighted mean of the pixels at every offset in
//! the search window, weighted by how alike the patches around the two
//! are: `exp2(-max(0, d * norm - 2))` with `d` the sum over the patch of
//! squared differences, all channels. In the stabilized space, where the
//! noise has unit variance, two patches of the same content differ by
//! about six per pixel, under the floor, so like patches get full
//! weight. The scale of the dissimilarity is where this port departs
//! from the reference: see [`NORM`]. The reference's central pixel
//! weight is here too: the center pair's own difference, counted as if
//! it were the whole patch and mixed in by that weight, so a patch that
//! matches everywhere but at its center matches less.

use rayon::prelude::*;

use super::denoise::{MAX_BANDS, Vst, band_variances, variance_grids};
use super::noise::{MID_GREY, NoiseModel};

/// Non-local means settings; `None` leaves a value to the noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NlmOptions {
    /// Patch radius: patches are `2r+1` square. Larger for noisier
    /// frames, as the reference does.
    pub patch_radius: Option<usize>,
    /// Search radius: `(2r+1)^2` offsets are compared.
    pub search_radius: usize,
    /// Spread of the search offsets, 0 for a dense window, up to 1 for
    /// the same number of offsets scattered over a far wider one.
    pub scattering: Option<f32>,
    /// Weight of the center pair's own difference against the patch's:
    /// 0 compares patches alone, 1 counts the center as much as the
    /// whole patch. The reference's slider.
    pub center_weight: f32,
}

impl Default for NlmOptions {
    fn default() -> Self {
        Self {
            patch_radius: None,
            search_radius: DEFAULT_SEARCH,
            scattering: None,
            center_weight: DEFAULT_CENTER_WEIGHT,
        }
    }
}

/// The reference's default search radius.
pub const DEFAULT_SEARCH: usize = 7;
/// The largest patch radius, as the reference caps it.
pub const MAX_PATCH: usize = 8;
/// The central pixel weight: the reference's default. It is a small,
/// steady gain on the benchmark at every noise level (a third of a dB
/// on Kodak at a sigma of 0.05, a tenth at 0.03 and 0.01), flat from
/// here to 0.3, and a loss from 1 up, where each pixel keeps its own
/// noise by preferring partners that share it. It does not straighten
/// the edges at ISO 32000 it was tried for (notes §13t).
pub const DEFAULT_CENTER_WEIGHT: f32 = 0.1;

/// What the non-local means did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NlmStats {
    pub strength: f32,
    pub patch_radius: usize,
    pub scattering: f32,
    pub center_weight: f32,
    /// Offsets compared per pixel.
    pub patches: usize,
    /// The farthest offset, in pixels.
    pub max_shift: usize,
}

/// The dissimilarity scale at strength 1: the reference's 0.045 twenty
/// times over. The reference's own scale, at which any two patches
/// within a few sigmas of each other average fully, is the benchmark's
/// worst case at every noise level and a wash by eye; the optimum sits
/// here, where the weight halves at a mean squared difference of about
/// three sigmas per channel, flat from a sigma of 0.03 up (notes §13r).
pub const NORM: f32 = 0.9;

/// Patch radius for a green sigma at mid grey: 3x3 patches at every
/// noise. The reference grows patches to 17x17 by ISO 32000, and the
/// first port here went to 5x5 above a sigma of 0.04 on gamma-domain
/// tables; in the linear domain, with the hybrid's level shift fixed
/// (§13u), 3x3 wins at every noise level, by 0.2 dB and a tenth of a
/// ΔE at 0.05 (§13v). A patch shifted a pixel across an edge still
/// matches on most of its area, and the larger the patch the more so.
pub fn auto_patch_radius(_sigma: f32) -> usize {
    1
}

/// The automatic strength: 1.5 at every noise. The first port ramped
/// it from 1 to 2 over the noise; in the linear domain, with the
/// hybrid's level shift fixed, 1.5 beats both ends of that ramp at
/// every noise level on every measure, and 1 and 2 lose to it by a
/// tenth to a half of a dB (§13v). The reference reads its strength
/// through its own transform and has no ramp either.
pub const AUTO_STRENGTH: f32 = 1.5;

/// The automatic strength for a green sigma at mid grey.
pub fn auto_strength(_sigma: f32) -> f32 {
    AUTO_STRENGTH
}

/// Scattering left to the noise: none. The reference spreads the
/// search for speed at high ISO; the dense window scores better at
/// every noise level and costs the same.
pub fn auto_scattering(_sigma: f32) -> f32 {
    0.0
}

/// Denoise interleaved RGB in place. `model` is the noise of `rgb` as it
/// is; `strength` above 1 smooths harder (it divides the dissimilarity,
/// which is what the reference's strength does through its transform).
pub fn denoise_nlm(
    rgb: &mut [f32],
    width: usize,
    height: usize,
    model: &NoiseModel,
    strength: f32,
    options: &NlmOptions,
) -> NlmStats {
    let vst: [Vst; 3] = std::array::from_fn(|c| Vst::new(model.a[c], model.b[c]));
    rgb.par_chunks_mut(3).for_each(|px| {
        for c in 0..3 {
            px[c] = vst[c].forward(px[c]);
        }
    });
    let stats = denoise_stabilized(
        rgb,
        width,
        height,
        model.sigma(1, MID_GREY),
        strength,
        options,
    );
    rgb.par_chunks_mut(3).for_each(|px| {
        for c in 0..3 {
            px[c] = vst[c].inverse(px[c]);
        }
    });
    stats
}

/// The means on samples already in the stabilized space, where the
/// noise has unit variance in every channel; `sigma` is the green
/// noise at mid grey the automatic settings follow. A caller that
/// goes on to work in that space (the hybrid) transforms once each
/// way around the whole; a second pass through the unbiased inverse
/// shifts every level by a fraction of the noise, which the coarse
/// error measures (notes §13u).
pub(crate) fn denoise_stabilized(
    rgb: &mut [f32],
    width: usize,
    height: usize,
    sigma: f32,
    strength: f32,
    options: &NlmOptions,
) -> NlmStats {
    let patch_radius = options
        .patch_radius
        .unwrap_or_else(|| auto_patch_radius(sigma))
        .min(MAX_PATCH);
    let scattering = options
        .scattering
        .unwrap_or_else(|| auto_scattering(sigma))
        .clamp(0.0, 1.0);
    let offsets = offsets(options.search_radius.max(1), scattering);
    let max_shift = offsets
        .iter()
        .map(|&(r, c)| r.unsigned_abs().max(c.unsigned_abs()))
        .max()
        .unwrap_or(0);
    let center_weight = options.center_weight.max(0.0);
    let stats = NlmStats {
        strength,
        patch_radius,
        scattering,
        center_weight,
        patches: offsets.len(),
        max_shift,
    };

    let patch_pixels = ((2 * patch_radius + 1) * (2 * patch_radius + 1)) as f32;
    let params = Params {
        width,
        height,
        radius: patch_radius,
        // Over the patch's pixel count, so the dissimilarity is a mean
        // per pixel; strength divides it, so more strength smooths more;
        // and over one plus the center weight, which is how much has
        // been added to the sum.
        norm: NORM / patch_pixels / strength.max(1e-3) / (1.0 + center_weight),
        // The center pair's difference scaled up to the whole patch, as
        // the reference does, so the weight means the same at any size.
        center: center_weight * patch_pixels,
        offsets: &offsets,
    };
    let mut out = vec![0.0f32; width * height * 3];
    let row = width * 3;
    let tiles_x = width.div_ceil(TILE);
    out.par_chunks_mut(TILE * row)
        .enumerate()
        .for_each(|(ty, band)| {
            let y0 = ty * TILE;
            let y1 = (y0 + TILE).min(height);
            let th = y1 - y0;
            // The band's rows cut at the tile boundaries and gathered
            // tile by tile, so each tile writes its answer straight into
            // the frame: no buffer of its own to allocate and no copy
            // back. `slots` is filled row by row because that is the
            // order the splitting goes in, and read tile by tile.
            let mut slots: Vec<Option<&mut [f32]>> = (0..tiles_x * th).map(|_| None).collect();
            for (r, line) in band.chunks_mut(row).enumerate() {
                let mut rest = line;
                for (tx, slot) in slots.iter_mut().skip(r).step_by(th).enumerate() {
                    let wide = ((tx + 1) * TILE).min(width) - tx * TILE;
                    let (head, tail) = rest.split_at_mut(wide * 3);
                    *slot = Some(head);
                    rest = tail;
                }
            }
            let mut rows: Vec<&mut [f32]> = slots.into_iter().flatten().collect();
            debug_assert_eq!(rows.len(), tiles_x * th);
            rows.par_chunks_mut(th).enumerate().for_each_init(
                Scratch::default,
                |scratch, (tx, rows)| {
                    let x0 = tx * TILE;
                    let x1 = (x0 + TILE).min(width);
                    denoise_tile(rows, rgb, &params, scratch, (x0, x1), (y0, y1));
                },
            );
        });

    rgb.copy_from_slice(&out);
    stats
}

/// The noise the means leave in each à trous band, as a fraction of
/// the white unit noise the wavelets otherwise assume: measured, by
/// running the means as they ran (`stats`) over a field of pure unit
/// noise and reading the band variances of what comes out. Flat noise
/// is where the means average most, so this is the least they leave;
/// in texture they leave more, which errs toward keeping it.
pub(crate) fn residual_band_ratios(stats: &NlmStats, search_radius: usize) -> [f32; MAX_BANDS] {
    const SIDE: usize = 512;
    let mut rgb = unit_noise(SIDE * SIDE * 3, 0x9E37_79B9_7F4A_7C15);
    let options = NlmOptions {
        patch_radius: Some(stats.patch_radius),
        search_radius,
        scattering: Some(stats.scattering),
        center_weight: stats.center_weight,
    };
    denoise_stabilized(&mut rgb, SIDE, SIDE, 1.0, stats.strength, &options);
    let white = band_variances();
    let grids = variance_grids(&rgb, SIDE, SIDE, MAX_BANDS);
    std::array::from_fn(|scale| {
        let grid = &grids[scale];
        let n = (grid.values.len() * 3) as f32;
        let mean: f32 = grid.values.iter().map(|v| v[0] + v[1] + v[2]).sum::<f32>() / n;
        (mean / white[scale]).min(1.0)
    })
}

/// Standard normal deviates from a fixed seed (xorshift and Box-Muller).
fn unit_noise(n: usize, mut state: u64) -> Vec<f32> {
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut out = Vec::with_capacity(n + 1);
    while out.len() < n {
        let (u1, u2) = (next().max(1e-12), next());
        let r = (-2.0 * u1.ln()).sqrt();
        let (s, c) = (std::f64::consts::TAU * u2).sin_cos();
        out.push((r * c) as f32);
        out.push((r * s) as f32);
    }
    out.truncate(n);
    out
}

/// Tile side; the work per tile and per offset is the tile plus a
/// patch's margin, so this trades margin overhead against cache.
const TILE: usize = 96;

struct Params<'a> {
    width: usize,
    height: usize,
    radius: usize,
    norm: f32,
    center: f32,
    offsets: &'a [(isize, isize)],
}

/// The reference's search offsets: every `(row, col)` within the search
/// radius, each coordinate spread by `scatter` so that a scattering of
/// 1 covers a far wider window with the same number of offsets and no
/// duplicates.
fn offsets(search: usize, scattering: f32) -> Vec<(isize, isize)> {
    let s = search as isize;
    let mut out = Vec::with_capacity(((2 * s + 1) * (2 * s + 1)) as usize);
    for r in -s..=s {
        for c in -s..=s {
            out.push((scatter(scattering, r, c), scatter(scattering, c, r)));
        }
    }
    out
}

/// The reference's spread of one offset coordinate given the other:
/// identity at zero scattering, cubic growth at one, the second
/// coordinate mixed in to keep the offsets off a grid.
fn scatter(scattering: f32, i: isize, j: isize) -> isize {
    let (ai, aj) = (i.unsigned_abs() as f32, j.unsigned_abs() as f32);
    ((ai * ai * ai + 7.0 * ai * aj.sqrt()) * i.signum() as f32 * scattering / 6.0 + i as f32)
        as isize
}

/// Scratch a tile needs, kept across the tiles a thread takes so the
/// allocator is not asked for a hundred and fifty kilobytes per tile.
#[derive(Default)]
struct Scratch {
    /// Sums of weighted pixels and of weights, one per tile pixel.
    acc: Vec<[f32; 4]>,
    /// A ring of rows of squared differences. The vertical box sum needs
    /// the row entering its window and the one leaving it at the same
    /// moment, so the ring holds one row more than the window is tall.
    ring: Vec<f32>,
    /// The running vertical box sums, one row.
    vsum: Vec<f32>,
    /// The horizontal box sums across a row of the tile.
    hsum: Vec<f32>,
    /// The weights those sums turn into, kept apart from the
    /// accumulation so the transcendental has a loop of plain floats to
    /// itself and vectorizes.
    wrow: Vec<f32>,
}

impl Scratch {
    fn ready(&mut self, tw: usize, th: usize, ew: usize, rows: usize) {
        self.acc.clear();
        self.acc.resize(tw * th, [0.0; 4]);
        self.ring.resize(rows * ew, 0.0);
        self.vsum.resize(ew, 0.0);
        self.hsum.resize(tw, 0.0);
        self.wrow.resize(tw, 0.0);
    }
}

/// One tile of the output, `x0..x1` by `y0..y1`, interleaved RGB, into
/// `out`: the tile's own part of each of its rows, `y1 - y0` of them,
/// `(x1 - x0) * 3` long.
///
/// The rows of squared differences are made one at a time and kept in a
/// ring only as tall as the patch, so the vertical box sums run down
/// whole rows rather than down a column at a stride, and the tile's two
/// largest scratch planes go away. The arithmetic is the same and in the
/// same order, so the output is the same to the bit.
fn denoise_tile(
    out: &mut [&mut [f32]],
    input: &[f32],
    p: &Params,
    s: &mut Scratch,
    xs: (usize, usize),
    ys: (usize, usize),
) {
    let tw = xs.1 - xs.0;
    debug_assert_eq!(
        out.len(),
        ys.1 - ys.0,
        "a tile writes every one of its rows"
    );
    debug_assert!(out.iter().all(|line| line.len() == tw * 3));
    accumulate(input, p, s, xs, ys);
    for (line, arow) in out.iter_mut().zip(s.acc.chunks(tw)) {
        for (o, a) in line.as_chunks_mut::<3>().0.iter_mut().zip(arow) {
            let inv = if a[3] > 0.0 { 1.0 / a[3] } else { 0.0 };
            *o = [a[0] * inv, a[1] * inv, a[2] * inv];
        }
    }
}

/// The weighted sums for one tile, left in `s.acc`.
///
/// Deliberately not inlined into [`denoise_tile`], which is the one
/// thing here that looks like a stylistic choice and is not. Its caller
/// holds the frame's own rows as `&mut [f32]` read out of a slice, and a
/// reference loaded from memory carries no promise that it is distinct
/// from anything else; fold the writing out in beside this loop and the
/// compiler has to assume those rows may be the input or the scratch.
/// That costs a quarter of the means' speed, 0.79 s to 1.04 s on the
/// one-core benchmark.
#[inline(never)]
fn accumulate(input: &[f32], p: &Params, s: &mut Scratch, xs: (usize, usize), ys: (usize, usize)) {
    let ((x0, x1), (y0, y1)) = (xs, ys);
    let (w, h, r) = (p.width, p.height, p.radius);
    let (tw, th) = (x1 - x0, y1 - y0);
    let span = 2 * r + 1;
    let (ew, eh) = (tw + 2 * r, th + 2 * r);
    let rows = span + 1;
    s.ready(tw, th, ew, rows);
    let Scratch {
        acc,
        ring,
        vsum,
        hsum,
        wrow,
    } = s;

    for &(dr, dc) in p.offsets {
        // Center pixels whose partner is inside the image.
        let cx0 = (x0 as isize).max(-dc) as usize;
        let cx1 = (x1 as isize).min(w as isize - dc) as usize;
        let cy0 = (y0 as isize).max(-dr) as usize;
        let cy1 = (y1 as isize).min(h as isize - dr) as usize;
        if cx0 >= cx1 || cy0 >= cy1 {
            continue;
        }
        // Squared differences where both pixels of a pair exist; zero
        // elsewhere, as the reference's sums treat the outside.
        let ex0 = x0 as isize - r as isize;
        let ey0 = y0 as isize - r as isize;
        let px0 = (cx0 as isize - r as isize).max(0).max(-dc) as usize;
        let px1 = (cx1 as isize + r as isize)
            .min(w as isize)
            .min(w as isize - dc) as usize;
        let py0 = (cy0 as isize - r as isize).max(0).max(-dr) as usize;
        let py1 = (cy1 as isize + r as isize)
            .min(h as isize)
            .min(h as isize - dr) as usize;
        // Row `j` of the ring's coordinates, `0..eh`, is image row
        // `ey0 + j`; only the covered part carries differences.
        let (dlo, dhi) = ((px0 as isize - ex0) as usize, (px1 as isize - ex0) as usize);
        let row = |j: usize, dst: &mut [f32]| {
            let y = ey0 + j as isize;
            if y < py0 as isize || y >= py1 as isize {
                dst.fill(0.0);
                return;
            }
            let y = y as usize;
            dst[..dlo].fill(0.0);
            dst[dhi..].fill(0.0);
            let n = px1 - px0;
            let a = &input[(y * w + px0) * 3..][..n * 3];
            let yy = (y as isize + dr) as usize;
            let b = &input[(yy * w + (px0 as isize + dc) as usize) * 3..][..n * 3];
            let (a, _) = a.as_chunks::<3>();
            let (b, _) = b.as_chunks::<3>();
            for ((o, a), b) in dst[dlo..dhi].iter_mut().zip(a).zip(b) {
                let (e0, e1, e2) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
                *o = e0 * e0 + e1 * e1 + e2 * e2;
            }
        };

        // The window over rows `0..span`, summed in that order.
        vsum.fill(0.0);
        for k in 0..span.min(eh) {
            let (lo, hi) = (k * ew, k * ew + ew);
            row(k, &mut ring[lo..hi]);
            for (v, d) in vsum.iter_mut().zip(&ring[lo..hi]) {
                *v += *d;
            }
        }
        // The tile's rows, the window sliding down as they go. At row
        // `ty` the ring holds image rows `ty..ty + span`, so the patch's
        // own row, `ty + r`, is in it and the slot the row entering the
        // window wants is the one the row leaving it does not.
        // `old` is the slot of row `ty`; the slots are walked rather than
        // taken modulo, since a modulo by a value only known at run time
        // is an integer division in the middle of the loop.
        let mut old = 0usize;
        for ty in 0..th {
            let y = y0 + ty;
            let wrap = |i: usize| if i >= rows { i - rows } else { i };
            if y >= cy0 && y < cy1 {
                let c = wrap(old + r) * ew;
                let drow = &ring[c..c + ew];
                // `hsum[tx]` is the patch sum for tile column `tx`, over
                // `tx..tx + span` in extended coordinates, running.
                let mut run: f32 = vsum[..span.min(ew)].iter().sum();
                for (tx, hs) in hsum.iter_mut().enumerate() {
                    *hs = run;
                    if tx + span < ew {
                        run += vsum[tx + span] - vsum[tx];
                    }
                }
                let (tlo, thi) = (cx0 - x0, cx1 - x0);
                let n = thi - tlo;
                let yy = (y as isize + dr) as usize;
                let q =
                    &input[((yy * w) as isize + (x0 + tlo) as isize + dc) as usize * 3..][..n * 3];
                let (q, _) = q.as_chunks::<3>();
                // The weights first, over plain floats, then the
                // accumulation: together in one loop the weight's
                // arithmetic is stuck at the pace of the three-wide
                // gather beside it.
                let wrow = &mut wrow[tlo..thi];
                for ((wt, &hs), &d) in wrow
                    .iter_mut()
                    .zip(&hsum[tlo..thi])
                    .zip(&drow[tlo + r..thi + r])
                {
                    *wt = weight((hs + p.center * d) * p.norm - 2.0);
                }
                let arow = &mut acc[ty * tw + tlo..ty * tw + thi];
                for ((a, &wt), q) in arow.iter_mut().zip(&*wrow).zip(q) {
                    a[0] += wt * q[0];
                    a[1] += wt * q[1];
                    a[2] += wt * q[2];
                    a[3] += wt;
                }
            }
            if ty + span < eh {
                let (o, n) = (old * ew, wrap(old + span) * ew);
                row(ty + span, &mut ring[n..n + ew]);
                for x in 0..ew {
                    vsum[x] += ring[n + x] - ring[o + x];
                }
            }
            old = wrap(old + 1);
        }
    }
}

/// The reference's weight, `exp2(-max(0, e))`, from the dissimilarity
/// already scaled and floored.
///
/// `f32::exp2` is a call into the C library, and a call in the middle of
/// the accumulation keeps that loop scalar and spills around it: it was
/// a third of the whole means. The weight only ever divides into a
/// weighted mean, so a polynomial whose every value is within two and a
/// half parts in ten million of the right one — 2.24e-7 at the worst,
/// measured over every `f32` the dissimilarity can reach — bounds the
/// mean's error at twice that, which is far under what the eventual
/// sample holds; and the loop vectorizes. `2^x = 2^n * 2^f` with `n` the nearest
/// integer, `f` the half unit left over, and `2^f` the degree-6 Taylor
/// series, whose first missing term is 1.2e-7 of the value at the worst
/// `f`.
#[inline(always)]
fn weight(e: f32) -> f32 {
    // At or above zero the weight is one: `f` comes out zero and the
    // series' constant term is exact.
    //
    // The floor is a departure from `exp2`, which goes on to the
    // subnormals and then to zero: a dissimilarity past 126 weighs
    // 2^-126 here where it used to weigh nothing. It cannot show,
    // because the zero offset is always in the search window and its own
    // difference is zero, so it contributes a weight of exactly one and
    // the denominator is never less than that; a floor weight is at most
    // 2^-126 of the answer. Clamping also keeps the scale's exponent a
    // normal one, so no value the loop makes is subnormal.
    let x = (-e).clamp(-126.0, 0.0);
    // Rounding to the nearest integer by adding 1.5 * 2^23, which pushes
    // the fraction off the end of the mantissa: `ceil` and `round` are
    // calls into the C library on a plain x86-64 target, which no loop
    // survives. The same sum carries `n` in its low bits, so the scale
    // `2^n` is those bits shifted into an exponent.
    const MAGIC: f32 = 12_582_912.0;
    let t = x + MAGIC;
    let f = x - (t - MAGIC);
    let scale = f32::from_bits(t.to_bits().wrapping_add(127) << 23);
    // `2^f` over the half unit `f` covers, by a degree-6 polynomial in
    // it, grouped so the multiplies do not queue up behind one another.
    // The coefficients are a minimax fit rather than the Taylor series,
    // which costs nothing and is worth a great deal: Taylor's seventh
    // term, the one left off, is 1.2e-7 of the value at the worst `f`,
    // where the fit is out by 2e-9 and the f32 arithmetic of evaluating
    // it is all that is left. The constant term is one exactly, so a
    // dissimilarity under the floor weighs exactly one.
    let f2 = f * f;
    let a = 1.0 + std::f32::consts::LN_2 * f;
    let b = 0.240_226_48 + 0.055_503_324 * f;
    let c = 0.009_618_438 + 0.001_339_886_7 * f;
    scale * (a + f2 * b + (f2 * f2) * (c + 0.000_153_533_3 * f2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::noise::tests::Gauss;

    #[test]
    fn scattering_spreads_without_duplicates() {
        let dense = offsets(7, 0.0);
        assert_eq!(dense.len(), 225);
        assert!(dense.contains(&(0, 0)) && dense.contains(&(7, -7)));
        let spread = offsets(7, 1.0);
        assert_eq!(spread.len(), 225);
        let mut sorted = spread.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 225, "no duplicate offsets at full scattering");
        let far = spread
            .iter()
            .map(|&(r, c)| r.abs().max(c.abs()))
            .max()
            .unwrap();
        assert!(far > 40 && far < 100, "spread reaches {far}");
        assert!(spread.contains(&(0, 0)));
    }

    #[test]
    fn auto_parameters_are_flat_over_the_noise() {
        for sigma in [0.003, 0.03, 0.05, 0.1] {
            assert_eq!(auto_patch_radius(sigma), 1);
            assert_eq!(auto_scattering(sigma), 0.0);
            assert_eq!(auto_strength(sigma), AUTO_STRENGTH);
        }
    }

    #[test]
    fn a_tile_matches_the_plain_definition() {
        // Small noisy image, every pixel's output against the textbook
        // weighted mean computed directly; and a whole image tiled
        // against one tile.
        let (w, h) = (23, 19);
        let mut g = Gauss(3);
        let input: Vec<f32> = (0..w * h * 3)
            .map(|i| 5.0 + ((i / 3 % w) as f32 * 0.7).sin() + 0.4 * g.next())
            .collect();
        let offsets = offsets(2, 0.0);
        let center_weight = 0.3;
        // Every patch size the ring of difference rows has a different
        // shape for: none at all, the default, and larger.
        for r in [0usize, 1, 2] {
            let patch = ((2 * r + 1) * (2 * r + 1)) as f32;
            let p = Params {
                width: w,
                height: h,
                radius: r,
                norm: NORM / patch / 0.3 / (1.0 + center_weight),
                center: center_weight * patch,
                offsets: &offsets,
            };
            let mut scratch = Scratch::default();
            let mut tiled = vec![0.0f32; w * h * 3];
            {
                let mut rows: Vec<&mut [f32]> = tiled.chunks_mut(w * 3).collect();
                denoise_tile(&mut rows, &input, &p, &mut scratch, (0, w), (0, h));
            }
            let at = |y: isize, x: isize| -> Option<&[f32]> {
                (x >= 0 && y >= 0 && x < w as isize && y < h as isize)
                    .then(|| &input[(y as usize * w + x as usize) * 3..][..3])
            };
            for y in 0..h as isize {
                for x in 0..w as isize {
                    let mut sum = [0.0f32; 4];
                    for &(dr, dc) in &offsets {
                        let Some(q) = at(y + dr, x + dc) else {
                            continue;
                        };
                        // The center pair's own difference, weighted.
                        let mut d = p.center
                            * at(y, x)
                                .unwrap()
                                .iter()
                                .zip(q)
                                .map(|(a, b)| (a - b) * (a - b))
                                .sum::<f32>();
                        for ky in -(r as isize)..=r as isize {
                            for kx in -(r as isize)..=r as isize {
                                if let (Some(a), Some(b)) =
                                    (at(y + ky, x + kx), at(y + dr + ky, x + dc + kx))
                                {
                                    for c in 0..3 {
                                        d += (a[c] - b[c]) * (a[c] - b[c]);
                                    }
                                }
                            }
                        }
                        let e = d * p.norm - 2.0;
                        let wt = if e <= 0.0 { 1.0 } else { (-e).exp2() };
                        for c in 0..3 {
                            sum[c] += wt * q[c];
                        }
                        sum[3] += wt;
                    }
                    for c in 0..3 {
                        let want = sum[c] / sum[3];
                        let got = tiled[(y as usize * w + x as usize) * 3 + c];
                        assert!(
                            (got - want).abs() < 1e-4,
                            "r{r} ({x},{y}) c{c}: {got} vs {want}"
                        );
                    }
                }
            }
            // Tiles agree with the whole.
            let mut a = vec![0.0f32; 10 * 7 * 3];
            {
                let mut rows: Vec<&mut [f32]> = a.chunks_mut(10 * 3).collect();
                denoise_tile(&mut rows, &input, &p, &mut scratch, (0, 10), (0, 7));
            }
            for ty in 0..7 {
                for tx in 0..10 {
                    for c in 0..3 {
                        assert!(
                            (a[(ty * 10 + tx) * 3 + c] - tiled[(ty * w + tx) * 3 + c]).abs() < 1e-5,
                            "r{r} tile ({tx},{ty}) c{c}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_means_leave_the_coarse_bands_and_take_the_fine_ones() {
        let noise = unit_noise(100_000, 7);
        let mean: f32 = noise.iter().sum::<f32>() / noise.len() as f32;
        let var: f32 = noise.iter().map(|v| v * v).sum::<f32>() / noise.len() as f32;
        assert!(
            mean.abs() < 0.01 && (var - 1.0).abs() < 0.02,
            "{mean} {var}"
        );
        let stats = NlmStats {
            strength: 2.0,
            patch_radius: 2,
            scattering: 0.0,
            center_weight: DEFAULT_CENTER_WEIGHT,
            patches: 225,
            max_shift: 7,
        };
        let left = residual_band_ratios(&stats, DEFAULT_SEARCH);
        assert!(left[0] < 0.02 && left[1] < 0.02, "fine bands {left:?}");
        assert!(
            left[2] < 0.1 && left[3] > 0.2 && left[3] < 0.5,
            "middle {left:?}"
        );
        assert!(left[5] > 0.9, "coarse {left:?}");
        assert!(
            left.windows(2).all(|w| w[0] <= w[1] + 0.05),
            "monotone {left:?}"
        );
    }

    #[test]
    fn flattens_noise_and_keeps_an_edge() {
        let (w, h) = (96, 64);
        let model = NoiseModel {
            a: [2e-3; 3],
            b: [1e-5; 3],
        };
        let mut g = Gauss(11);
        let truth: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let v = if i % w < w / 2 { 0.1 } else { 0.4 };
                [v, v, v]
            })
            .collect();
        let mut noisy: Vec<f32> = truth
            .iter()
            .enumerate()
            .map(|(i, &v)| (v + g.next() * model.sigma(i % 3, v)).max(0.0))
            .collect();
        let before = truth
            .iter()
            .zip(&noisy)
            .map(|(t, n)| (t - n) * (t - n))
            .sum::<f32>();
        // A patch radius of 1: with larger patches a patch shifted one
        // pixel across an edge still matches on most of its area and
        // the edge softens, which is the method, not a fault.
        let options = NlmOptions {
            patch_radius: Some(1),
            ..NlmOptions::default()
        };
        let stats = denoise_nlm(&mut noisy, w, h, &model, 1.0, &options);
        assert_eq!(stats.patches, 225);
        let after = truth
            .iter()
            .zip(&noisy)
            .map(|(t, n)| (t - n) * (t - n))
            .sum::<f32>();
        assert!(after < before * 0.1, "error {before} -> {after}");
        // The edge stays a step: the pixel either side of it is still
        // within a small fraction of its side's level.
        for y in 4..h - 4 {
            let l = noisy[(y * w + w / 2 - 1) * 3];
            let r = noisy[(y * w + w / 2) * 3];
            assert!(
                (l - 0.1).abs() < 0.03 && (r - 0.4).abs() < 0.05,
                "row {y}: {l} | {r}"
            );
        }
    }

    /// The values a synthetic noisy patch came out as before the speed
    /// work of §128, when the weight was `f32::exp2`. Everything the
    /// rewrite did to the box sums and the scratch is exact; the
    /// polynomial weight is not, and this is what that costs. The worst
    /// sample here moved by 4.4e-7 of itself, four of the last bits.
    /// That follows from the weight's own bound rather than from any
    /// cancellation — the polynomial's error varies with the fractional
    /// part, so it is not a factor the weights share — but a weighted
    /// mean whose every weight is within a relative `d` of the right one
    /// is itself within about twice `d`, and `d` here is 2.3e-7. The
    /// tolerance is 1e-5, twenty times that and still under a 16-bit
    /// sample's own step.
    #[test]
    fn the_means_hold_the_values_they_had_before_the_speed_work() {
        const GOLDEN: [f32; 24] = [
            1.304_65e-1,
            3.555_714_8e-1,
            1.632_430_6e-1,
            4.220_314_3e-1,
            6.914_874e-2,
            4.230_199e-1,
            1.197_163_2e-1,
            4.126_179_2e-1,
            1.738_393_8e-1,
            3.343_805_7e-1,
            1.630_213e-1,
            4.101_994_3e-1,
            1.366_392_4e-1,
            4.154_531e-1,
            7.882_365e-2,
            4.160_629e-1,
            1.341_975_5e-1,
            3.900_334_2e-1,
            1.649_331_3e-1,
            3.414_178e-1,
            1.649_338_3e-1,
            4.075_431e-1,
            1.218_260_8e-1,
            4.150_473_8e-1,
        ];
        const GOLDEN_MEAN: f64 = 0.263_257_006;

        let (w, h) = (37usize, 29usize);
        let model = NoiseModel {
            a: [2e-3, 1.5e-3, 2.5e-3],
            b: [1e-5, 2e-5, 1.5e-5],
        };
        let mut s: u32 = 3;
        let mut g = move || {
            let mut sum = 0.0f32;
            for _ in 0..12 {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                sum += (s >> 8) as f32 / (1u32 << 24) as f32;
            }
            sum - 6.0
        };
        let mut rgb: Vec<f32> = (0..w * h * 3)
            .map(|i| {
                let p = i / 3;
                let (x, y) = (p % w, p / w);
                let base = if x < w / 2 { 0.12 } else { 0.38 } + 0.05 * ((y as f32) * 0.3).sin();
                (base + g() * 0.02).max(0.0)
            })
            .collect();
        denoise_nlm(&mut rgb, w, h, &model, 1.5, &NlmOptions::default());
        for (k, &want) in GOLDEN.iter().enumerate() {
            let got = rgb[(k * 1277) % (w * h * 3)];
            assert!((got - want).abs() < 1e-5, "sample {k}: {got} vs {want}");
        }
        let mean = rgb.iter().map(|&v| v as f64).sum::<f64>() / rgb.len() as f64;
        assert!((mean - GOLDEN_MEAN).abs() < 1e-7, "mean {mean}");
    }

    /// The weight against the library's `exp2`, over the whole range the
    /// dissimilarity reaches, at the seams where the fraction the series
    /// covers changes sign, and past the floor.
    #[test]
    fn the_weight_follows_exp2() {
        let mut worst = 0.0f32;
        let mut check = |e: f32| {
            let got = weight(e);
            if e <= 0.0 {
                assert_eq!(got, 1.0, "at {e} the weight must be exactly one");
                return;
            }
            // Past 126 the floor takes over and `exp2` is no longer what
            // this claims to be.
            if e <= 126.0 {
                let want = (-e).exp2();
                worst = worst.max(((got - want) / want).abs());
            }
        };
        for i in 0..200_001 {
            check((i as f32) * 1e-3 - 100.0);
        }
        // `f` reaches +/- 0.5 at every half integer, where the series is
        // furthest from its center.
        for k in 0..126 {
            for d in [0.499_99, 0.5, 0.500_01] {
                check(k as f32 + d);
            }
        }
        for e in [1e-6, 125.9, 126.0] {
            check(e);
        }
        // Sweeping every `f32` in the range says 2.24e-7, under four of
        // the last bits; the bound leaves room for a `exp2` of another
        // platform's own last bit.
        assert!(worst < 3.5e-7, "worst relative error {worst}");

        // The floor, and that it is a floor: everything past 126 weighs
        // what 126 does, which is 2^-126 and not zero.
        assert_eq!(weight(126.0), f32::from_bits(1 << 23));
        assert_eq!(weight(126.000_1), weight(126.0));
        assert_eq!(weight(149.0), weight(126.0));
        assert_eq!(weight(1e9), weight(126.0));
        assert!(weight(f32::INFINITY) == weight(126.0));
        // The zero offset is always in the search window and its own
        // difference is zero, so every pixel has a weight of exactly one
        // in its sum: the floor can never be the whole denominator.
        assert_eq!(weight(-2.0), 1.0);

        // It never rises with `e`, bar a last bit at a seam.
        let mut last = weight(0.0);
        for i in 0..130_001 {
            let got = weight(i as f32 * 1e-3);
            assert!(got <= last * (1.0 + 1e-6), "rose at {}: {last} to {got}", i);
            last = got;
        }
    }
}
