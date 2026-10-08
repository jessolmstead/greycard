//! Lateral chromatic aberration correction on the Bayer mosaic.
//!
//! Ported from RawTherapee's `rtengine/CA_correct_RT.cc` (dev branch, code
//! dated 2018-09-08), GPL-3.0-or-later. Copyright (c) 2008-2010 Emil
//! Martinec <ejmartin@uchicago.edu>; copyright (c) 2018 Ingo Weyrich for
//! the speedups, the iterated correction and the color-shift avoidance.
//! The linear solver is Henry Guennadi Levkin's, as used there. The
//! algorithm and its constants are theirs; this port takes the scalar,
//! automatic branch. Buffers, threading and edge reflection are ours.
//!
//! The idea: lateral CA displaces red and blue relative to green by a
//! smooth, slowly varying amount across the frame. In each 128-pixel tile,
//! green is interpolated at the red and blue sites, and the sub-pixel
//! offset that minimizes the variance of the red-minus-green (and
//! blue-minus-green) difference along each axis is solved for in closed
//! form, since that variance is quadratic in the offset. Tiles vote,
//! outliers are dropped by a 3x3 median and a variance gate, and a
//! fourth-order polynomial in tile position is fitted to the survivors.
//! Red and blue are then resampled from where the polynomial says they
//! really landed, with a few guards against overshoot. Two iterations
//! tighten the result, and a last step blurs the ratio of old to new
//! values over a wide radius and applies it, so the local average color
//! is what it was before.
//!
//! Departures: consistent corner reflection, zeroed buffers, a triple box
//! blur standing in for the reference's recursive Gaussian in the
//! color-shift step, and a reduced-order fit (used with fewer than 32
//! voting tiles) built from the right entries of the normal matrix, which
//! the reference gets wrong. And one in evaluation order: green at a red
//! or blue site, the reference's weighted mean of its four green
//! neighbors `(wtu*u + wtd*d + wtl*l + wtr*r) / W` with `W` the sum of
//! the weights, is taken as `u + (wtd*(d - u) + wtl*(l - u) +
//! wtr*(r - u)) / W`, which is the same mean (`wtu*u + wtd*d + wtl*l +
//! wtr*r = W*u + wtd*(d - u) + wtl*(l - u) + wtr*(r - u)`). Rounded in
//! floats the reference's form leaves a flat or clipped patch's greens
//! within an ulp of their value, and its gradients rounding residues
//! instead of zeros, and another implementation's rounding (a GPU's
//! fused multiply-adds) leaves different ones: their energy straddles
//! the vote's gate, so the two cast votes on different tiles and fit
//! different corrections. In this form four equal greens give their
//! value exactly, and the rounding of near-equal ones is a share of
//! their differences rather than of their values.
//!
//! The pieces marked as shared (the tile constants, `reflect`,
//! `origins`, `vote_of`, `fit_votes`, `resample_for`, `shift_factor`,
//! `box_radius`) are public so that a GPU implementation reads the
//! same numbers and takes the same decisions this reference does; a
//! difference between the two is then rounding, not a constant. The
//! decisions between the data-parallel stages (the median, the gate,
//! the fit and the solve) are `fit_votes`, which a GPU implementation
//! calls on the votes it reads back; `measure_votes` gives the
//! reference's own votes for a check against them.

use rayon::prelude::*;

pub use super::rcd::is_bayer;
use crate::error::{Error, Result};
use crate::raw::{CfaColor, CfaPattern};

/// Tile side including margins. Shared.
pub const TS: usize = 128;
/// Margin on each side of a tile. Shared.
pub const BORDER: usize = 8;
/// A tile's interior, which is what it votes over and what it corrects.
/// Shared.
pub const STEP: usize = TS - 2 * BORDER;
/// A frame narrower or shorter than this has too few tiles for a fit
/// worth trusting and is left alone. Shared.
pub const MIN_SIDE: usize = 2 * TS;

/// Shared.
pub const EPS: f32 = 1e-5;
/// Shared.
pub const EPS2: f32 = 1e-10;
/// Blocks whose shift squared exceeds this many times the variance of all
/// blocks' shifts do not vote in the fit. Shared.
pub const AUTO_STRENGTH: f32 = 8.0;
/// Largest shift, in pixels, the correction will apply. Shared.
pub const SHIFT_LIMIT: f64 = 3.99;
/// Sigma of the blur over the color-shift factors, in half-resolution
/// pixels. Shared.
pub const SHIFT_BLUR_SIGMA: f32 = 30.0;
/// Below this a sample is too small for a color-shift factor. Shared.
pub const SHIFT_TINY: f32 = 1.0 / 65535.0;

/// How to run the correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaOptions {
    /// Passes of measure-and-correct. The reference defaults to 2.
    pub iterations: usize,
    /// Keep the local average of red and blue what it was, so the
    /// correction cannot tint large areas.
    pub avoid_color_shift: bool,
}

impl Default for CaOptions {
    fn default() -> Self {
        Self {
            iterations: 2,
            avoid_color_shift: true,
        }
    }
}

/// What the correction found.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CaStats {
    /// Whether a fit was made and applied on the last iteration.
    pub corrected: bool,
    /// Tiles that voted in the last fit, the smaller of red and blue.
    pub blocks: usize,
    /// Order of the polynomial in the last fit: 4, or 2 with few votes.
    pub order: usize,
    /// Largest fitted shift magnitude in pixels, red then blue, over the
    /// first iteration: how much aberration there was to correct.
    pub max_shift: [f32; 2],
}

/// Correct lateral CA in a white-balanced Bayer mosaic. Returns the
/// corrected mosaic (green untouched) and what was measured.
pub fn correct_ca(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    options: &CaOptions,
) -> Result<(Vec<f32>, CaStats)> {
    if !is_bayer(pattern) {
        return Err(Error::Unsupported(format!(
            "CA correction needs a 2x2 Bayer pattern, got {pattern}"
        )));
    }
    if samples.len() != width * height {
        return Err(Error::Unsupported(format!(
            "{width}x{height} mosaic with {} samples",
            samples.len()
        )));
    }
    let mut current = samples.to_vec();
    let mut stats = CaStats::default();
    if width < MIN_SIDE || height < MIN_SIDE {
        return Ok((current, stats));
    }
    for it in 0..options.iterations.max(1) {
        let (next, pass) = one_pass(&current, width, height, pattern)?;
        if it == 0 {
            stats.max_shift = pass.max_shift;
        }
        stats.corrected = pass.corrected;
        stats.blocks = pass.blocks;
        stats.order = pass.order;
        if !pass.corrected {
            break;
        }
        current = next;
    }
    if options.avoid_color_shift && stats.corrected {
        avoid_color_shift(samples, &mut current, width, height, pattern);
    }
    Ok((current, stats))
}

struct Image<'a> {
    samples: &'a [f32],
    width: usize,
    height: usize,
    pattern: &'a CfaPattern,
}

impl Image<'_> {
    fn at(&self, row: isize, col: isize) -> f32 {
        self.samples[reflect(row, self.height) * self.width + reflect(col, self.width)]
    }
}

/// The reference's edge handling: a read past an edge mirrors about the
/// edge pixel. Shared.
pub fn reflect(i: isize, n: usize) -> usize {
    let n = n as isize;
    let r = if i < 0 {
        -i
    } else if i >= n {
        2 * n - 2 - i
    } else {
        i
    };
    r.clamp(0, n - 1) as usize
}

/// Tile origins along one axis: from minus the margin, stepping by the
/// interior, while the origin is inside the image. Shared.
pub fn origins(n: usize) -> Vec<isize> {
    (0..)
        .map(|k| -(BORDER as isize) + (k * STEP) as isize)
        .take_while(|&o| o < n as isize)
        .collect()
}

/// One tile's vote: the shift that minimizes color-difference variance,
/// per color (red, blue) and direction (vertical, horizontal), plus the
/// weight the reference assigns to the block. Shared.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BlockVote {
    pub shift: [[f32; 2]; 2],
    pub weight: f32,
}

/// A tile's quadratic-fit sums of color-difference variance against
/// interpolation position, `[direction][term][color]`: the terms are
/// the color difference squared, its product with the green gradient,
/// and the gradient squared, each weighted; the directions vertical
/// then horizontal; the colors red then blue. Accumulated in single
/// precision over the tile's interior in row-major order, the greens
/// skipped. Shared.
pub type Coefficients = [[[f32; 2]; 3]; 2];

/// The vote from a tile's sums: the reference's scaling, then the
/// quotient per color and direction. Shared.
#[allow(clippy::needless_range_loop)]
pub fn vote_of(coeff: Coefficients) -> BlockVote {
    let coeff = scaled(coeff);
    let mut vote = BlockVote::default();
    for c in 0..2 {
        for dir in 0..2 {
            if coeff[dir][2][c] > EPS2 {
                vote.shift[c][dir] = coeff[dir][1][c] / coeff[dir][2][c];
                // The reference overwrites this per color and
                // direction, so blue-horizontal is what survives.
                vote.weight = coeff[dir][2][c] / (EPS + coeff[dir][0][c]);
            } else {
                vote.shift[c][dir] = 17.0;
                vote.weight = 0.0;
            }
        }
    }
    vote
}

/// A tile's sums as the vote reads them, after the reference's
/// scaling: the third term is the gradient energy the vote's gate
/// holds against [`EPS2`]. Shared.
pub fn scaled(mut coeff: Coefficients) -> Coefficients {
    for dir in &mut coeff {
        for (k, row) in dir.iter_mut().enumerate() {
            for v in row.iter_mut() {
                *v *= 0.25;
                if k == 1 {
                    *v *= 0.3125;
                } else if k == 2 {
                    *v *= 0.3125 * 0.3125;
                }
            }
        }
    }
    coeff
}

/// The bilinear resample's parameters for one tile's fitted shifts,
/// per color: the integer part toward zero, the next integer away from
/// it, the fraction between, and the direction (two pixels, to the
/// same color) to look for the opposite sample. Shared.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Resample {
    pub vfloor: [isize; 2],
    pub vceil: [isize; 2],
    pub vfrac: [f32; 2],
    pub hfloor: [isize; 2],
    pub hceil: [isize; 2],
    pub hfrac: [f32; 2],
    pub dir_v: [isize; 2],
    pub dir_h: [isize; 2],
}

/// [`Resample`] for a tile's shifts, `[color][direction]`. Shared.
#[allow(clippy::needless_range_loop)]
pub fn resample_for(shifts: [[f64; 2]; 2]) -> Resample {
    let mut r = Resample::default();
    for c in 0..2 {
        let (sv, sh) = (shifts[c][0], shifts[c][1]);
        let (mut f, mut ce) = (sv.floor() as isize, sv.ceil() as isize);
        if sv < 0.0 {
            std::mem::swap(&mut f, &mut ce);
        }
        r.vfloor[c] = f;
        r.vceil[c] = ce;
        r.vfrac[c] = (sv - f as f64).abs() as f32;
        let (mut f, mut ce) = (sh.floor() as isize, sh.ceil() as isize);
        if sh < 0.0 {
            std::mem::swap(&mut f, &mut ce);
        }
        r.hfloor[c] = f;
        r.hceil[c] = ce;
        r.hfrac[c] = (sh - f as f64).abs() as f32;
        r.dir_v[c] = if sv > 0.0 { 2 } else { -2 };
        r.dir_h[c] = if sh > 0.0 { 2 } else { -2 };
    }
    r
}

/// Per-thread working buffers at tile resolution. Values indexed by site;
/// the reference packs red and blue into half-size arrays.
struct Scratch {
    cfa: Vec<f32>,
    g: Vec<f32>,
    rbhpfv: Vec<f32>,
    rbhpfh: Vec<f32>,
    rblpfv: Vec<f32>,
    rblpfh: Vec<f32>,
    grblpfv: Vec<f32>,
    grblpfh: Vec<f32>,
    grbdiff: Vec<f32>,
    gshift: Vec<f32>,
    /// Each corrected sample's [`Conditioning`], kept when `trace` is set.
    cond: Vec<Conditioning>,
    /// When traced: the largest mosaic value a green here is made of (the
    /// site's own and its four neighbors'), and the largest a shifted
    /// green is made of (that at each of its bilinear corners).
    around: Vec<f32>,
    around_shift: Vec<f32>,
    trace: bool,
    /// Each green interpolated past the picture's edge moved by this
    /// share of itself, for [`measure_coefficients_edged`]; zero in the
    /// correction.
    edge_nudge: f32,
}

impl Scratch {
    fn new() -> Self {
        Self::tracing(false)
    }

    fn tracing(trace: bool) -> Self {
        let f = || vec![0.0f32; TS * TS];
        Self {
            cond: if trace {
                vec![Conditioning::default(); TS * TS]
            } else {
                Vec::new()
            },
            around: if trace { f() } else { Vec::new() },
            around_shift: if trace { f() } else { Vec::new() },
            trace,
            edge_nudge: 0.0,
            cfa: f(),
            g: f(),
            rbhpfv: f(),
            rbhpfh: f(),
            rblpfv: f(),
            rblpfh: f(),
            grblpfv: f(),
            grblpfh: f(),
            grbdiff: f(),
            gshift: f(),
        }
    }

    /// Load the tile and interpolate green at red and blue sites with the
    /// reference's directional weights. Returns the tile extent.
    fn load(&mut self, image: &Image<'_>, top: isize, left: isize) -> (usize, usize) {
        let ts = TS as isize;
        let bottom = (top + ts).min(image.height as isize + BORDER as isize);
        let right = (left + ts).min(image.width as isize + BORDER as isize);
        let rr1 = (bottom - top) as usize;
        let cc1 = (right - left) as usize;
        for rr in 0..rr1 {
            for cc in 0..cc1 {
                let v = image.at(top + rr as isize, left + cc as isize);
                self.cfa[rr * TS + cc] = v;
                self.g[rr * TS + cc] = v;
            }
        }
        let is_green =
            |rr: usize, cc: usize| image.pattern.color_at(rr & 1, cc & 1) == CfaColor::Green;
        let cfa = &self.cfa;
        let at = |i: usize, o: isize| cfa[(i as isize + o) as usize];
        let (width, height) = (image.width as isize, image.height as isize);
        let nudge = self.edge_nudge;
        let past_edge = |rr: usize, cc: usize| {
            let (y, x) = (top + rr as isize, left + cc as isize);
            y < 0 || x < 0 || y >= height || x >= width
        };
        for rr in 3..rr1 - 3 {
            for cc in 3..cc1 - 3 {
                if is_green(rr, cc) {
                    continue;
                }
                let i = rr * TS + cc;
                let s = |o| at(i, o);
                // Green neighbors are one step away, same-color two.
                let wtu = 1.0
                    / sq(EPS
                        + (s(ts) - s(-ts)).abs()
                        + (s(0) - s(-2 * ts)).abs()
                        + (s(-ts) - s(-3 * ts)).abs());
                let wtd = 1.0
                    / sq(EPS
                        + (s(-ts) - s(ts)).abs()
                        + (s(0) - s(2 * ts)).abs()
                        + (s(ts) - s(3 * ts)).abs());
                let wtl = 1.0
                    / sq(EPS + (s(1) - s(-1)).abs() + (s(0) - s(-2)).abs() + (s(-1) - s(-3)).abs());
                let wtr = 1.0
                    / sq(EPS + (s(-1) - s(1)).abs() + (s(0) - s(2)).abs() + (s(1) - s(3)).abs());
                // The weighted mean of the four greens as the upper one
                // and the weighted differences from it (see the module
                // doc): four equal greens give that green exactly.
                let up = s(-ts);
                self.g[i] = up
                    + (wtd * (s(ts) - up) + wtl * (s(-1) - up) + wtr * (s(1) - up))
                        / (wtu + wtd + wtl + wtr);
                if nudge != 0.0 && past_edge(rr, cc) {
                    self.g[i] *= 1.0 + nudge;
                }
            }
        }
        (rr1, cc1)
    }

    /// Pass one on a tile: the block's shift vote.
    #[allow(clippy::needless_range_loop)]
    fn measure(&mut self, image: &Image<'_>, top: isize, left: isize) -> BlockVote {
        vote_of(self.coefficients(image, top, left))
    }

    /// A tile's quadratic-fit sums, which [`vote_of`] makes its vote of.
    fn coefficients(&mut self, image: &Image<'_>, top: isize, left: isize) -> Coefficients {
        let (rr1, cc1) = self.load(image, top, left);
        let ts = TS as isize;
        let is_green =
            |rr: usize, cc: usize| image.pattern.color_at(rr & 1, cc & 1) == CfaColor::Green;
        let color_index = |rr: usize, cc: usize| match image.pattern.color_at(rr & 1, cc & 1) {
            CfaColor::Red => 0,
            _ => 1,
        };

        // High- and low-pass filters of the color difference along each axis.
        for rr in 4..rr1 - 4 {
            for cc in 4..cc1 - 4 {
                if is_green(rr, cc) {
                    continue;
                }
                let i = rr * TS + cc;
                let g = |o: isize| self.g[(i as isize + o) as usize];
                let c = |o: isize| self.cfa[(i as isize + o) as usize];
                let d = |o: isize| g(o) - c(o);
                self.rbhpfv[i] = ((d(0) - d(4 * ts)).abs() + (d(-4 * ts) - d(0)).abs()
                    - (d(-4 * ts) - d(4 * ts)).abs())
                .abs();
                self.rbhpfh[i] =
                    ((d(0) - d(4)).abs() + (d(-4) - d(0)).abs() - (d(-4) - d(4)).abs()).abs();
                let glpfv = 2.0 * g(0) + g(2 * ts) + g(-2 * ts);
                let glpfh = 2.0 * g(0) + g(2) + g(-2);
                let clpfv = 2.0 * c(0) + c(2 * ts) + c(-2 * ts);
                let clpfh = 2.0 * c(0) + c(2) + c(-2);
                self.rblpfv[i] = 0.25 * (glpfv - clpfv).abs();
                self.rblpfh[i] = 0.25 * (glpfh - clpfh).abs();
                self.grblpfv[i] = 0.25 * (glpfv + clpfv);
                self.grblpfh[i] = 0.25 * (glpfh + clpfh);
            }
        }

        // Quadratic-fit coefficients of color-difference variance against
        // interpolation position, per direction and color.
        let mut coeff = Coefficients::default();
        for rr in 8..rr1 - 8 {
            for cc in 8..cc1 - 8 {
                if is_green(rr, cc) {
                    continue;
                }
                let k = color_index(rr, cc);
                let i = rr * TS + cc;
                let g = |o: isize| self.g[(i as isize + o) as usize];
                let b = |buf: &[f32], o: isize| buf[(i as isize + o) as usize];
                let deltgrb = self.cfa[i] - g(0);

                let gdiff =
                    (g(ts) - g(-ts)) + 0.3 * (g(ts + 1) - g(-ts + 1) + g(ts - 1) - g(-ts - 1));
                let gradwt = (b(&self.rbhpfv, 0)
                    + 0.5 * (b(&self.rbhpfv, 2) + b(&self.rbhpfv, -2)))
                    * (b(&self.grblpfv, -2 * ts) + b(&self.grblpfv, 2 * ts))
                    / (EPS
                        + 0.1 * (b(&self.grblpfv, -2 * ts) + b(&self.grblpfv, 2 * ts))
                        + b(&self.rblpfv, -2 * ts)
                        + b(&self.rblpfv, 2 * ts));
                coeff[0][0][k] += gradwt * deltgrb * deltgrb;
                coeff[0][1][k] += gradwt * gdiff * deltgrb;
                coeff[0][2][k] += gradwt * gdiff * gdiff;

                let gdiff =
                    (g(1) - g(-1)) + 0.3 * (g(1 + ts) - g(-1 + ts) + g(1 - ts) - g(-1 - ts));
                let gradwt = (b(&self.rbhpfh, 0)
                    + 0.5 * (b(&self.rbhpfh, 2 * ts) + b(&self.rbhpfh, -2 * ts)))
                    * (b(&self.grblpfh, -2) + b(&self.grblpfh, 2))
                    / (EPS
                        + 0.1 * (b(&self.grblpfh, -2) + b(&self.grblpfh, 2))
                        + b(&self.rblpfh, -2)
                        + b(&self.rblpfh, 2));
                coeff[1][0][k] += gradwt * deltgrb * deltgrb;
                coeff[1][1][k] += gradwt * gdiff * deltgrb;
                coeff[1][2][k] += gradwt * gdiff * gdiff;
            }
        }
        coeff
    }

    /// Pass two on a tile: resample red and blue by the fitted shift.
    /// Returns the corrected interior.
    fn correct(
        &mut self,
        image: &Image<'_>,
        top: isize,
        left: isize,
        shifts: [[f64; 2]; 2],
    ) -> TileOut {
        let (rr1, cc1) = self.load(image, top, left);
        let ts = TS as isize;
        let is_green =
            |rr: usize, cc: usize| image.pattern.color_at(rr & 1, cc & 1) == CfaColor::Green;
        let color_index = |rr: usize, cc: usize| match image.pattern.color_at(rr & 1, cc & 1) {
            CfaColor::Red => 0,
            _ => 1,
        };

        let Resample {
            vfloor,
            vceil,
            vfrac,
            hfloor,
            hceil,
            hfrac,
            dir_v,
            dir_h,
        } = resample_for(shifts);

        if self.trace {
            let ts = TS;
            for rr in 1..rr1 - 1 {
                for cc in 1..cc1 - 1 {
                    let i = rr * ts + cc;
                    self.around[i] = [i, i - 1, i + 1, i - ts, i + ts]
                        .map(|j| self.cfa[j].abs())
                        .into_iter()
                        .fold(0.0, f32::max);
                }
            }
        }

        // Green at each red/blue site's optical position, and the color
        // difference there.
        for rr in 4..rr1 - 4 {
            for cc in 4..cc1 - 4 {
                if is_green(rr, cc) {
                    continue;
                }
                let c = color_index(rr, cc);
                let i = rr * TS + cc;
                let g = |dr: isize, dc: isize| {
                    self.g[((rr as isize + dr) * ts + cc as isize + dc) as usize]
                };
                let gint_floor = intp(hfrac[c], g(vfloor[c], hceil[c]), g(vfloor[c], hfloor[c]));
                let gint_ceil = intp(hfrac[c], g(vceil[c], hceil[c]), g(vceil[c], hfloor[c]));
                let gint = intp(vfrac[c], gint_ceil, gint_floor);
                self.grbdiff[i] = gint - self.cfa[i];
                self.gshift[i] = gint;
                if self.trace {
                    let a = |dr: isize, dc: isize| {
                        self.around[((rr as isize + dr) * ts + cc as isize + dc) as usize]
                    };
                    self.around_shift[i] = [
                        a(vfloor[c], hceil[c]),
                        a(vfloor[c], hfloor[c]),
                        a(vceil[c], hceil[c]),
                        a(vceil[c], hfloor[c]),
                    ]
                    .into_iter()
                    .fold(0.0, f32::max);
                }
            }
        }
        let hfrac = hfrac.map(|f| f * 0.5);
        let vfrac = vfrac.map(|f| f * 0.5);

        // Interpolate the color difference back to the grid and apply.
        for rr in 8..rr1 - 8 {
            for cc in 8..cc1 - 8 {
                if is_green(rr, cc) {
                    continue;
                }
                let c = color_index(rr, cc);
                let i = rr * TS + cc;
                let (dv, dh) = (dir_v[c], dir_h[c]);
                let gd = |dr: isize, dc: isize| {
                    self.grbdiff[((rr as isize + dr) * ts + cc as isize + dc) as usize]
                };
                let gs = |dr: isize, dc: isize| {
                    self.gshift[((rr as isize + dr) * ts + cc as isize + dc) as usize]
                };
                let g0 = self.g[i];
                let old = g0 - self.cfa[i];

                let h_floor = intp(hfrac[c], gd(0, -dh), gd(0, 0));
                let h_ceil = intp(hfrac[c], gd(-dv, -dh), gd(-dv, 0));
                let mut int = intp(vfrac[c], h_ceil, h_floor);
                let rbint = g0 - int;
                let cur = self.cfa[i];
                if self.trace {
                    let at = [(0, 0), (0, -dh), (-dv, 0), (-dv, -dh)];
                    let shifted = at.map(|(r, c)| (gs(r, c), gd(r, c)));
                    let around = at
                        .map(|(r, c)| {
                            self.around_shift[((rr as isize + r) * ts + cc as isize + c) as usize]
                        })
                        .into_iter()
                        .fold(self.around[i], f32::max);
                    self.cond[i] = Conditioning::of(g0, cur, int, shifted, around);
                }
                if (rbint - cur).abs() < 0.25 * (rbint + cur) {
                    if old.abs() > int.abs() {
                        self.cfa[i] = rbint;
                    }
                } else {
                    // Weight by how far green at the shifted points is
                    // from green here.
                    let p0 = 1.0 / (EPS + (g0 - gs(0, 0)).abs());
                    let p1 = 1.0 / (EPS + (g0 - gs(0, -dh)).abs());
                    let p2 = 1.0 / (EPS + (g0 - gs(-dv, 0)).abs());
                    let p3 = 1.0 / (EPS + (g0 - gs(-dv, -dh)).abs());
                    int = (p0 * gd(0, 0) + p1 * gd(0, -dh) + p2 * gd(-dv, 0) + p3 * gd(-dv, -dh))
                        / (p0 + p1 + p2 + p3);
                    if old.abs() > int.abs() {
                        self.cfa[i] = g0 - int;
                    }
                }
                // Overshot the correction: desaturate instead.
                if old * int < 0.0 {
                    self.cfa[i] = g0 - 0.5 * (old + int);
                }
                if self.trace {
                    debug_assert_eq!(
                        self.cond[i].value.to_bits(),
                        self.cfa[i].to_bits(),
                        "the conditioning's guards are these"
                    );
                }
            }
        }

        let rows = rr1.saturating_sub(2 * BORDER);
        let cols = cc1.saturating_sub(2 * BORDER);
        let mut data = vec![0.0f32; rows * cols];
        let mut cond = Vec::new();
        for r in 0..rows {
            let from = (r + BORDER) * TS + BORDER;
            data[r * cols..(r + 1) * cols].copy_from_slice(&self.cfa[from..from + cols]);
            if self.trace {
                cond.extend_from_slice(&self.cond[from..from + cols]);
            }
        }
        if self.trace {
            self.cond.fill(Conditioning::default());
        }
        TileOut {
            row0: (top + BORDER as isize) as usize,
            col0: (left + BORDER as isize) as usize,
            rows,
            cols,
            data,
            cond,
        }
    }
}

struct TileOut {
    row0: usize,
    col0: usize,
    rows: usize,
    cols: usize,
    data: Vec<f32>,
    /// The interior's conditioning, when traced; else empty.
    cond: Vec<Conditioning>,
}

/// How a red or blue sample of a pass stands to rounding in the values
/// it is resampled from (the green here and at the shifted points, and
/// the color differences there), for a check of another
/// implementation that rounds them differently. An error of `e` in
/// each moves the sample by up to `gain * e` while no guard flips;
/// `flips` holds, for each of the three guards (the branch between
/// the bilinear and the weighted color difference, keeping or taking
/// the correction, and desaturating an overshoot), how large an error
/// would flip it and what the sample would be then. `scale` is the
/// largest of those values and of the mosaic values the greens among
/// them are interpolated from (an interpolated green is its upper
/// neighbor plus weighted differences, and rounds as a share of
/// those, which may be far larger than the green: a dark site by a
/// bright one), what their rounding is a share of. A
/// green sample, or one outside every tile's interior, has the
/// default: no gain and no flip, so it is exact. Shared.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conditioning {
    pub scale: f32,
    pub gain: f32,
    pub flips: [Flip; 3],
    /// The sample, before it is kept at or above zero.
    pub value: f32,
}

/// One guard of a resampled sample taken the other way: how large an
/// error in the sample's values would do it, and the sample then, with
/// its own gain. Shared.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flip {
    pub margin: f32,
    pub value: f32,
    pub gain: f32,
}

impl Default for Conditioning {
    fn default() -> Self {
        let none = Flip {
            margin: f32::INFINITY,
            value: 0.0,
            gain: 0.0,
        };
        Self {
            scale: 0.0,
            gain: 0.0,
            flips: [none; 3],
            value: 0.0,
        }
    }
}

impl Conditioning {
    /// The sample `cur` resampled with green `g0` here, the bilinear
    /// color difference `bilinear`, and the shifted greens and color
    /// differences, each guard as the pass decides it or flipped.
    fn of(g0: f32, cur: f32, bilinear: f32, shifted: [(f32, f32); 4], around: f32) -> Self {
        let old = g0 - cur;
        let weights = shifted.map(|(gs, _)| 1.0 / (EPS + (g0 - gs).abs()));
        let total: f32 = weights.iter().sum();
        let [(_, d0), (_, d1), (_, d2), (_, d3)] = shifted;
        let weighted =
            (weights[0] * d0 + weights[1] * d1 + weights[2] * d2 + weights[3] * d3) / total;
        let rbint = g0 - bilinear;
        let bilinear_branch = (rbint - cur).abs() < 0.25 * (rbint + cur);
        // A weight moves by its square times an error in the greens it
        // reads, here and shifted; the bilinear mean of the
        // differences carries their error once.
        let weighted_gain = 1.0
            + 2.0
                * weights
                    .iter()
                    .zip(&shifted)
                    .map(|(p, (_, d))| p * p * (d - weighted).abs())
                    .sum::<f32>()
                / total;
        let int_of = |bilinear_branch: bool| {
            if bilinear_branch {
                (bilinear, 1.0)
            } else {
                (weighted, weighted_gain)
            }
        };
        // The sample and its gain with each guard as decided, or
        // flipped.
        let decide = |bilinear_branch: bool, flip_take: bool, flip_over: bool| {
            let (int, int_gain) = int_of(bilinear_branch);
            let take = (old.abs() > int.abs()) != flip_take;
            if (old * int < 0.0) != flip_over {
                (g0 - 0.5 * (old + int), 0.5 + 0.5 * int_gain)
            } else if take {
                (g0 - int, 1.0 + int_gain)
            } else {
                (cur, 0.0)
            }
        };
        let flip = |margin: f32, (value, gain): (f32, f32)| Flip {
            margin,
            value,
            gain,
        };
        let (int, int_gain) = int_of(bilinear_branch);
        let (value, gain) = decide(bilinear_branch, false, false);
        let scale = shifted
            .iter()
            .map(|&(gs, d)| gs.abs().max((gs - d).abs()))
            .fold(g0.abs().max(cur.abs()).max(around), f32::max);
        Self {
            scale,
            gain,
            flips: [
                flip(
                    (0.25 * (rbint + cur) - (rbint - cur).abs()).abs() / 2.5,
                    decide(!bilinear_branch, false, false),
                ),
                flip(
                    (old.abs() - int.abs()).abs() / (1.0 + int_gain),
                    decide(bilinear_branch, true, false),
                ),
                flip(
                    (int.abs() / int_gain).min(old.abs()),
                    decide(bilinear_branch, false, true),
                ),
            ],
            value,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct PassStats {
    corrected: bool,
    blocks: usize,
    order: usize,
    max_shift: [f32; 2],
}

/// Why [`fit_votes`] made no fit. Shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoFit {
    /// No tile voted a shift under two pixels in some color and
    /// direction.
    NoVotes,
    /// Fewer than ten tiles passed the gate in red or in blue.
    TooFew { blocks: usize },
    /// The normal equations had no solution.
    Singular { blocks: usize },
}

impl NoFit {
    /// The tiles that passed the gate, the smaller of red and blue;
    /// what the stats report.
    pub fn blocks(self) -> usize {
        match self {
            Self::NoVotes => 0,
            Self::TooFew { blocks } | Self::Singular { blocks } => blocks,
        }
    }
}

/// The polynomial in block position fitted to the votes. Shared.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    /// `[color][direction]`, `order * order` coefficients in use.
    coefficients: [[[f64; 16]; 2]; 2],
    /// 4, or 2 with fewer than 32 voting tiles.
    pub order: usize,
    /// Tiles that voted, the smaller of red and blue.
    pub blocks: usize,
    /// Largest fitted shift magnitude in pixels, red then blue, over
    /// the interior blocks.
    pub max_shift: [f32; 2],
    /// How close the nearest tile came to the variance gate, either
    /// side of it: the smallest, over every tile, color and direction,
    /// of the median shift squared's distance from the gate as a
    /// fraction of the gate. The gate is a cliff (one tile crossing it
    /// changes the fit and so every pixel), so a check of another
    /// implementation wants to see this margin beside its vote
    /// differences.
    pub gate_margin: f64,
}

impl Fit {
    /// The shift at block `(v, h)` of the bordered grid, `[color]
    /// [direction]`, clamped to [`SHIFT_LIMIT`]. A tile's block is its
    /// index along each axis plus one. Shared.
    #[allow(clippy::needless_range_loop)]
    pub fn shift_at(&self, v: usize, h: usize) -> [[f64; 2]; 2] {
        let mut out = [[0.0f64; 2]; 2];
        for c in 0..2 {
            for dir in 0..2 {
                let mut s = 0.0;
                let mut pow_v = 1.0;
                for i in 0..self.order {
                    let mut pow_h = pow_v;
                    for j in 0..self.order {
                        s += pow_h * self.coefficients[c][dir][self.order * i + j];
                        pow_h *= h as f64;
                    }
                    pow_v *= v as f64;
                }
                out[c][dir] = s.clamp(-SHIFT_LIMIT, SHIFT_LIMIT);
            }
        }
        out
    }
}

/// The decisions between the data-parallel stages, from the tiles'
/// votes in row-major order (`tiles_down` rows of `tiles_across`): the
/// variance of the plausible shifts, the 3x3 median over a grid with a
/// one-block border copied from the nearest interior, the variance
/// gate, the weighted normal equations in double, and the solve.
/// Shared.
pub fn fit_votes(
    votes: &[BlockVote],
    tiles_down: usize,
    tiles_across: usize,
) -> std::result::Result<Fit, NoFit> {
    assert_eq!(votes.len(), tiles_down * tiles_across);
    // Block grid with a one-block border on every side for the median.
    let vblsz = tiles_down + 2;
    let hblsz = tiles_across + 2;
    let mut shifts = vec![[[0.0f32; 2]; 2]; vblsz * hblsz];
    let mut weights = vec![0.0f32; vblsz * hblsz];
    let mut sum = [[0.0f64; 2]; 2];
    let mut sumsq = [[0.0f64; 2]; 2];
    let mut count = [[0.0f64; 2]; 2];
    for (i, vote) in votes.iter().enumerate() {
        let (v, h) = (i / tiles_across + 1, i % tiles_across + 1);
        shifts[v * hblsz + h] = vote.shift;
        weights[v * hblsz + h] = vote.weight;
        for c in 0..2 {
            for dir in 0..2 {
                let s = vote.shift[c][dir];
                if s.abs() < 2.0 {
                    sum[c][dir] += f64::from(s);
                    sumsq[c][dir] += f64::from(s * s);
                    count[c][dir] += 1.0;
                }
            }
        }
    }
    let mut var = [[0.0f64; 2]; 2];
    for c in 0..2 {
        for dir in 0..2 {
            if count[c][dir] == 0.0 {
                return Err(NoFit::NoVotes);
            }
            var[c][dir] = sumsq[c][dir] / count[c][dir] - (sum[c][dir] / count[c][dir]).powi(2);
        }
    }

    // Fill the border blocks from the nearest interior ones.
    for v in 1..vblsz - 1 {
        shifts[v * hblsz] = shifts[v * hblsz + 2];
        shifts[v * hblsz + hblsz - 1] = shifts[v * hblsz + hblsz - 3];
    }
    for h in 0..hblsz {
        shifts[h] = shifts[2 * hblsz + h];
        shifts[(vblsz - 1) * hblsz + h] = shifts[(vblsz - 3) * hblsz + h];
    }

    // Least-squares normal equations for a polynomial in block position.
    let mut polyord = 4usize;
    let mut polymat = [[[0.0f64; 256]; 2]; 2];
    let mut shiftmat = [[[0.0f64; 16]; 2]; 2];
    let mut numblox = [0usize; 2];
    let mut gate_margin = f64::INFINITY;
    for v in 1..vblsz - 1 {
        for h in 1..hblsz - 1 {
            for c in 0..2 {
                let mut med = [0.0f32; 2];
                for dir in 0..2 {
                    let mut p = [0.0f32; 9];
                    for (k, (dv, dh)) in [
                        (-1isize, -1isize),
                        (-1, 0),
                        (-1, 1),
                        (0, -1),
                        (0, 0),
                        (0, 1),
                        (1, -1),
                        (1, 0),
                        (1, 1),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        p[k] = shifts
                            [(v as isize + dv) as usize * hblsz + (h as isize + dh) as usize][c]
                            [dir];
                    }
                    med[dir] = median9(p);
                }
                for dir in 0..2 {
                    let gate = f64::from(AUTO_STRENGTH) * var[c][dir];
                    if gate > 0.0 {
                        gate_margin =
                            gate_margin.min((f64::from(sq(med[dir])) - gate).abs() / gate);
                    }
                }
                if f64::from(sq(med[0])) > f64::from(AUTO_STRENGTH) * var[c][0]
                    || f64::from(sq(med[1])) > f64::from(AUTO_STRENGTH) * var[c][1]
                {
                    continue;
                }
                numblox[c] += 1;
                let w = f64::from(weights[v * hblsz + h]);
                let (vf, hf) = (v as f64, h as f64);
                for dir in 0..2 {
                    let mut pow_v_i = 1.0;
                    for i in 0..4 {
                        let mut pow_h_j = 1.0;
                        for j in 0..4 {
                            let mut pow_v_m = pow_v_i;
                            for m in 0..4 {
                                let mut pow_h_n = pow_h_j;
                                for n in 0..4 {
                                    polymat[c][dir][16 * (4 * i + j) + (4 * m + n)] +=
                                        pow_v_m * pow_h_n * w;
                                    pow_h_n *= hf;
                                }
                                pow_v_m *= vf;
                            }
                            shiftmat[c][dir][4 * i + j] +=
                                pow_v_i * pow_h_j * f64::from(med[dir]) * w;
                            pow_h_j *= hf;
                        }
                        pow_v_i *= vf;
                    }
                }
            }
        }
    }
    let blocks = numblox[0].min(numblox[1]);
    if blocks < 32 {
        polyord = 2;
        if blocks < 10 {
            return Err(NoFit::TooFew { blocks });
        }
    }
    // The basis functions in use, as indices into the 16-parameter
    // layout: all of them, or for the reduced fit {1, h, v, vh}. (The
    // reference's reduced fit reads the wrong entries of its full normal
    // matrix; this builds the four-parameter system properly.)
    let basis: Vec<usize> = if polyord == 4 {
        (0..16).collect()
    } else {
        vec![0, 1, 4, 5]
    };
    let numpar = basis.len();
    let mut fit = [[[0.0f64; 16]; 2]; 2];
    for c in 0..2 {
        for dir in 0..2 {
            let mut m = vec![0.0f64; numpar * numpar];
            let mut b = vec![0.0f64; numpar];
            for (a, &ia) in basis.iter().enumerate() {
                for (bb, &ib) in basis.iter().enumerate() {
                    m[a * numpar + bb] = polymat[c][dir][16 * ia + ib];
                }
                b[a] = shiftmat[c][dir][ia];
            }
            match solve(numpar, &mut m, &mut b) {
                Some(x) => fit[c][dir][..numpar].copy_from_slice(&x),
                None => return Err(NoFit::Singular { blocks }),
            }
        }
    }
    let mut fit = Fit {
        coefficients: fit,
        order: polyord,
        blocks,
        max_shift: [0.0; 2],
        gate_margin,
    };
    let mut max_shift = [0.0f32; 2];
    for v in 1..vblsz - 1 {
        for h in 1..hblsz - 1 {
            let s = fit.shift_at(v, h);
            for c in 0..2 {
                let mag = (s[c][0] * s[c][0] + s[c][1] * s[c][1]).sqrt() as f32;
                max_shift[c] = max_shift[c].max(mag);
            }
        }
    }
    fit.max_shift = max_shift;
    Ok(fit)
}

/// The reference's votes on a mosaic, one a tile in row-major order
/// (`origins(height).len()` rows of `origins(width).len()`), as the
/// first pass of [`correct_ca`] measures them; for checking another
/// implementation's votes tile by tile. Shared.
pub fn measure_votes(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) -> Result<Vec<BlockVote>> {
    if !is_bayer(pattern) {
        return Err(Error::Unsupported(format!(
            "CA correction needs a 2x2 Bayer pattern, got {pattern}"
        )));
    }
    if samples.len() != width * height {
        return Err(Error::Unsupported(format!(
            "{width}x{height} mosaic with {} samples",
            samples.len()
        )));
    }
    let image = Image {
        samples,
        width,
        height,
        pattern,
    };
    let tops = origins(height);
    let lefts = origins(width);
    let tiles: Vec<(isize, isize)> = tops
        .iter()
        .flat_map(|&t| lefts.iter().map(move |&l| (t, l)))
        .collect();
    Ok(tiles
        .par_iter()
        .map_init(Scratch::new, |scratch, &(t, l)| {
            scratch.measure(&image, t, l)
        })
        .collect())
}

/// [`measure_votes`]'s sums before the votes are made of them: a tile's
/// [`Coefficients`] in the same order. Shared.
pub fn measure_coefficients(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) -> Result<Vec<Coefficients>> {
    coefficients_of(samples, width, height, pattern, 0.0)
}

/// [`measure_coefficients`] with every green interpolated past the
/// picture's edge moved by `nudge` of itself: for telling the votes that
/// another implementation's rounding of those greens alone can move. A
/// green past the edge mirrors one inside, made of the same samples,
/// and the reference's rounding can leave the two exactly equal where
/// another implementation's leaves them an ulp apart; the high-pass
/// that weights a site's terms is then exactly zero on one path and a
/// residue on the other, and a residue times a clipped edge's gradient
/// moves a vote, or casts one. No change to the mosaic shows it, since
/// a reflected read is the same sample, and an ulp either way does:
/// the residue cancels for one sign, so a check takes both. Inside the
/// picture it is the reference. Shared.
pub fn measure_coefficients_edged(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    nudge: f32,
) -> Result<Vec<Coefficients>> {
    coefficients_of(samples, width, height, pattern, nudge)
}

fn coefficients_of(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    edge_nudge: f32,
) -> Result<Vec<Coefficients>> {
    if !is_bayer(pattern) {
        return Err(Error::Unsupported(format!(
            "CA correction needs a 2x2 Bayer pattern, got {pattern}"
        )));
    }
    if samples.len() != width * height {
        return Err(Error::Unsupported(format!(
            "{width}x{height} mosaic with {} samples",
            samples.len()
        )));
    }
    let image = Image {
        samples,
        width,
        height,
        pattern,
    };
    let tiles: Vec<(isize, isize)> = origins(height)
        .iter()
        .flat_map(|&t| origins(width).into_iter().map(move |l| (t, l)))
        .collect();
    Ok(tiles
        .par_iter()
        .map_init(
            || Scratch {
                edge_nudge,
                ..Scratch::new()
            },
            |scratch, &(t, l)| scratch.coefficients(&image, t, l),
        )
        .collect())
}

/// Measure, fit and correct once.
fn one_pass(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) -> Result<(Vec<f32>, PassStats)> {
    let image = Image {
        samples,
        width,
        height,
        pattern,
    };
    let tops = origins(height);
    let lefts = origins(width);
    let block_of = |t: isize, l: isize| {
        let v = (t + BORDER as isize) as usize / STEP + 1;
        let h = (l + BORDER as isize) as usize / STEP + 1;
        (v, h)
    };
    let tiles: Vec<(isize, isize)> = tops
        .iter()
        .flat_map(|&t| lefts.iter().map(move |&l| (t, l)))
        .collect();

    // In tile order, row-major, which is the order `fit_votes` wants.
    let votes: Vec<((usize, usize), BlockVote)> = tiles
        .par_iter()
        .map_init(Scratch::new, |scratch, &(t, l)| {
            (block_of(t, l), scratch.measure(&image, t, l))
        })
        .collect();

    let votes: Vec<BlockVote> = votes.into_iter().map(|(_, v)| v).collect();
    let fit = match fit_votes(&votes, tops.len(), lefts.len()) {
        Ok(fit) => fit,
        Err(no) => {
            return Ok((
                samples.to_vec(),
                PassStats {
                    blocks: no.blocks(),
                    ..Default::default()
                },
            ));
        }
    };
    let (blocks, polyord, max_shift) = (fit.blocks, fit.order, fit.max_shift);
    let (out, _) = resample(&image, &fit, false);
    Ok((
        out,
        PassStats {
            corrected: true,
            blocks,
            order: polyord,
            max_shift,
        },
    ))
}

/// A pass's resample of red and blue by the shifts `fit` gives each
/// tile, kept at or above zero, and each sample's conditioning when
/// `trace` is set.
fn resample(image: &Image<'_>, fit: &Fit, trace: bool) -> (Vec<f32>, Vec<Conditioning>) {
    let (width, height) = (image.width, image.height);
    let block_of = |t: isize, l: isize| {
        let v = (t + BORDER as isize) as usize / STEP + 1;
        let h = (l + BORDER as isize) as usize / STEP + 1;
        (v, h)
    };
    let (tops, lefts) = (origins(height), origins(width));
    let tiles: Vec<(isize, isize)> = tops
        .iter()
        .flat_map(|&t| lefts.iter().map(move |&l| (t, l)))
        .collect();
    let outs: Vec<TileOut> = tiles
        .par_iter()
        .map_init(
            || Scratch::tracing(trace),
            |scratch, &(t, l)| {
                let (v, h) = block_of(t, l);
                scratch.correct(image, t, l, fit.shift_at(v, h))
            },
        )
        .collect();
    let mut out = image.samples.to_vec();
    let mut cond = if trace {
        vec![Conditioning::default(); width * height]
    } else {
        Vec::new()
    };
    // A last column of tiles starting within a margin of the right
    // edge has no interior, and its offset is past the edge.
    for tile in outs.into_iter().filter(|t| t.cols > 0) {
        for r in 0..tile.rows {
            let at = (tile.row0 + r) * width + tile.col0;
            let from = r * tile.cols;
            out[at..at + tile.cols].copy_from_slice(&tile.data[from..from + tile.cols]);
            if trace {
                cond[at..at + tile.cols].copy_from_slice(&tile.cond[from..from + tile.cols]);
            }
        }
    }
    // The reference keeps corrected values at or above zero.
    out.par_iter_mut().for_each(|v| *v = v.max(0.0));
    (out, cond)
}

/// One pass's resample of `samples` by `fit`, as the correction's own
/// pass makes it from its fit, and each sample's [`Conditioning`]: for
/// checking another implementation's resample on the fit it made.
/// Shared.
pub fn resample_with(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    fit: &Fit,
) -> Result<(Vec<f32>, Vec<Conditioning>)> {
    if !is_bayer(pattern) {
        return Err(Error::Unsupported(format!(
            "CA correction needs a 2x2 Bayer pattern, got {pattern}"
        )));
    }
    if samples.len() != width * height {
        return Err(Error::Unsupported(format!(
            "{width}x{height} mosaic with {} samples",
            samples.len()
        )));
    }
    let image = Image {
        samples,
        width,
        height,
        pattern,
    };
    Ok(resample(&image, fit, true))
}

/// Keep the local average of each of red and blue what it was: the ratio
/// of old to new at every red and blue site, blurred widely, multiplies
/// the corrected value. What [`correct_ca`] does after its passes when
/// asked to avoid a color shift. Shared.
pub fn avoid_color_shift(
    original: &[f32],
    corrected: &mut [f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) {
    let (hw, hh) = (width.div_ceil(2), height.div_ceil(2));
    let mut factor = [vec![1.0f32; hw * hh], vec![1.0f32; hw * hh]];
    // A factor row covers two mosaic rows; rows in parallel.
    let [red, blue] = &mut factor;
    red.par_chunks_mut(hw)
        .zip(blue.par_chunks_mut(hw))
        .enumerate()
        .for_each(|(fy, (fr, fb))| {
            for y in (2 * fy)..(2 * fy + 2).min(height) {
                for x in 0..width {
                    let f = match pattern.color_at(y, x) {
                        CfaColor::Red => &mut *fr,
                        CfaColor::Blue => &mut *fb,
                        _ => continue,
                    };
                    f[x / 2] = shift_factor(original[y * width + x], corrected[y * width + x]);
                }
            }
        });
    for f in factor.iter_mut() {
        gaussian_blur_approx(f, hw, hh, SHIFT_BLUR_SIGMA);
    }
    corrected
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                let c = match pattern.color_at(y, x) {
                    CfaColor::Red => 0,
                    CfaColor::Blue => 1,
                    _ => continue,
                };
                *v *= factor[c][(y / 2) * hw + x / 2];
            }
        });
}

/// The color-shift factor at a red or blue site: the old value over the
/// new, within a stop either way, or one where either is too small to
/// say. Shared.
#[inline]
pub fn shift_factor(old: f32, new: f32) -> f32 {
    if new <= SHIFT_TINY || old <= SHIFT_TINY {
        1.0
    } else {
        (old / new).clamp(0.5, 2.0)
    }
}

/// The radius of each of the three box blurs that stand in for a
/// Gaussian of `sigma`: the odd width with the same variance over the
/// three passes, halved. Shared.
pub fn box_radius(sigma: f32) -> usize {
    let w = ((12.0 * sigma * sigma / 3.0 + 1.0).sqrt() as usize) | 1;
    w / 2
}

/// Three box blurs of equal width approximate a Gaussian closely enough
/// for a factor map this smooth.
fn gaussian_blur_approx(data: &mut [f32], width: usize, height: usize, sigma: f32) {
    let radius = box_radius(sigma);
    let mut tmp = vec![0.0f32; width * height];
    // The passes commute, so the three row passes go first, then the
    // three column passes as row passes over the transpose.
    box_blur_rows(data, &mut tmp, width, radius);
    box_blur_rows(&tmp, data, width, radius);
    box_blur_rows(data, &mut tmp, width, radius);
    transpose(&tmp, data, width, height);
    box_blur_rows(data, &mut tmp, height, radius);
    box_blur_rows(&tmp, data, height, radius);
    box_blur_rows(data, &mut tmp, height, radius);
    transpose(&tmp, data, height, width);
}

fn box_blur_rows(src: &[f32], dst: &mut [f32], width: usize, radius: usize) {
    dst.par_chunks_mut(width)
        .zip(src.par_chunks(width))
        .for_each(|(out, row)| box_blur_line(row, out, 1, width, radius));
}

/// `src` of `width` by `height` into `dst` of `height` by `width`.
fn transpose(src: &[f32], dst: &mut [f32], width: usize, height: usize) {
    dst.par_chunks_mut(height).enumerate().for_each(|(x, col)| {
        for (y, v) in col.iter_mut().enumerate() {
            *v = src[y * width + x];
        }
    });
}

/// Box blur of one line with edge clamping, by running sum: the window's
/// sum once, then a sample in and a sample out per step. (A GPU
/// implementation runs the same running sum, one thread a line, so that
/// its rounding is this one's.)
fn box_blur_line(src: &[f32], dst: &mut [f32], stride: usize, n: usize, radius: usize) {
    let get = |i: isize| src[i.clamp(0, n as isize - 1) as usize * stride];
    let mut sum = 0.0f32;
    for k in -(radius as isize)..=(radius as isize) {
        sum += get(k);
    }
    let norm = 1.0 / (2 * radius + 1) as f32;
    for i in 0..n {
        dst[i * stride] = sum * norm;
        sum += get(i as isize + radius as isize + 1) - get(i as isize - radius as isize);
    }
}

/// Gaussian elimination with partial pivoting; `None` if singular.
fn solve(n: usize, m: &mut [f64], b: &mut [f64]) -> Option<Vec<f64>> {
    for k in 0..n.saturating_sub(1) {
        let mut max = m[k * n + k].abs();
        let mut pivot = k;
        for i in k + 1..n {
            if max < m[i * n + k].abs() {
                max = m[i * n + k].abs();
                pivot = i;
            }
        }
        if pivot != k {
            for i in k..n {
                m.swap(k * n + i, pivot * n + i);
            }
            b.swap(k, pivot);
        }
        if m[k * n + k] == 0.0 {
            return None;
        }
        for j in k + 1..n {
            let f = -m[j * n + k] / m[k * n + k];
            for i in k..n {
                m[j * n + i] += f * m[k * n + i];
            }
            b[j] += f * b[k];
        }
    }
    if m[(n - 1) * n + n - 1] == 0.0 {
        return None;
    }
    let mut x = vec![0.0f64; n];
    for k in (0..n).rev() {
        let mut v = b[k];
        for i in k + 1..n {
            v -= m[k * n + i] * x[i];
        }
        x[k] = v / m[k * n + k];
    }
    Some(x)
}

#[inline]
fn sq(x: f32) -> f32 {
    x * x
}

/// `a * b + (1 - a) * c`, the reference's `intp`.
#[inline]
fn intp(a: f32, b: f32, c: f32) -> f32 {
    a * (b - c) + c
}

fn median9(mut p: [f32; 9]) -> f32 {
    p.sort_by(f32::total_cmp);
    p[4]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth random texture with plenty of edges: sums of a few sine
    /// gratings at different angles and frequencies, in 0.2..0.8.
    fn texture(width: usize, height: usize) -> Vec<f32> {
        (0..width * height)
            .map(|i| {
                let (x, y) = ((i % width) as f32, (i / width) as f32);
                let v = (x * 0.11).sin() * (y * 0.07).cos()
                    + (x * 0.031 + y * 0.052).sin()
                    + ((x * 0.9 + y * 0.37) * 0.23).sin() * 0.5
                    + ((x - y) * 0.017).cos() * 0.7;
                0.5 + 0.3 * v / 3.2
            })
            .collect()
    }

    /// Bilinear sample with edge clamping.
    fn sample(img: &[f32], width: usize, height: usize, x: f32, y: f32) -> f32 {
        let x = x.clamp(0.0, (width - 1) as f32);
        let y = y.clamp(0.0, (height - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |xx: usize, yy: usize| img[yy * width + xx];
        (1.0 - fy) * ((1.0 - fx) * p(x0, y0) + fx * p(x1, y0))
            + fy * ((1.0 - fx) * p(x0, y1) + fx * p(x1, y1))
    }

    /// Mosaic a grey texture with lateral CA: red magnified about the
    /// center by `1 + red`, blue by `1 + blue`, so the displacement grows
    /// linearly with distance from the center as it does optically.
    fn aberrated(width: usize, height: usize, red: f32, blue: f32) -> (Vec<f32>, Vec<f32>) {
        let tex = texture(width, height);
        let pattern = CfaPattern::rggb();
        let (cx, cy) = ((width - 1) as f32 / 2.0, (height - 1) as f32 / 2.0);
        let mut clean = vec![0.0f32; width * height];
        let mut shifted = vec![0.0f32; width * height];
        for y in 0..height {
            for x in 0..width {
                let i = y * width + x;
                clean[i] = tex[i];
                let scale = match pattern.color_at(y, x) {
                    CfaColor::Red => 1.0 + red,
                    CfaColor::Blue => 1.0 + blue,
                    _ => 1.0,
                };
                let sx = cx + (x as f32 - cx) / scale;
                let sy = cy + (y as f32 - cy) / scale;
                shifted[i] = sample(&tex, width, height, sx, sy);
            }
        }
        (clean, shifted)
    }

    fn rms_error(
        a: &[f32],
        b: &[f32],
        width: usize,
        height: usize,
        pattern: &CfaPattern,
        color: CfaColor,
    ) -> f32 {
        let (mut sum, mut n) = (0.0f64, 0usize);
        for y in 32..height - 32 {
            for x in 32..width - 32 {
                if pattern.color_at(y, x) != color {
                    continue;
                }
                let d = f64::from(a[y * width + x] - b[y * width + x]);
                sum += d * d;
                n += 1;
            }
        }
        (sum / n as f64).sqrt() as f32
    }

    #[test]
    fn solver_inverts_a_small_system() {
        let mut m = vec![2.0, 1.0, -1.0, -3.0, -1.0, 2.0, -2.0, 1.0, 2.0];
        let mut b = vec![8.0, -11.0, -3.0];
        let x = solve(3, &mut m, &mut b).unwrap();
        assert!(
            (x[0] - 2.0).abs() < 1e-9 && (x[1] - 3.0).abs() < 1e-9 && (x[2] + 1.0).abs() < 1e-9,
            "{x:?}"
        );
        let mut m = vec![1.0, 2.0, 2.0, 4.0];
        let mut b = vec![1.0, 2.0];
        assert!(solve(2, &mut m, &mut b).is_none());
        assert_eq!(median9([5.0, 1.0, 9.0, 3.0, 7.0, 2.0, 8.0, 4.0, 6.0]), 5.0);
    }

    #[test]
    fn box_blur_preserves_a_constant_and_smooths_a_spike() {
        let (w, h) = (40, 30);
        let mut flat = vec![0.7f32; w * h];
        gaussian_blur_approx(&mut flat, w, h, 3.0);
        assert!(flat.iter().all(|v| (v - 0.7).abs() < 1e-5));
        let mut spike = vec![0.0f32; w * h];
        spike[15 * w + 20] = 1.0;
        gaussian_blur_approx(&mut spike, w, h, 2.0);
        let total: f32 = spike.iter().sum();
        assert!((total - 1.0).abs() < 1e-3, "{total}");
        assert!(spike[15 * w + 20] < 0.1 && spike[15 * w + 20] > spike[15 * w + 25]);
    }

    #[test]
    fn unaberrated_mosaic_is_left_nearly_alone() {
        let (w, h) = (400, 320);
        let (clean, _) = aberrated(w, h, 0.0, 0.0);
        let (out, stats) =
            correct_ca(&clean, w, h, &CfaPattern::rggb(), &CaOptions::default()).unwrap();
        assert!(
            stats.max_shift[0] < 0.15 && stats.max_shift[1] < 0.15,
            "{stats:?}"
        );
        let p = CfaPattern::rggb();
        let err = rms_error(&clean, &out, w, h, &p, CfaColor::Red);
        assert!(err < 2e-3, "{err} {stats:?}");
    }

    #[test]
    fn radial_aberration_is_measured_and_removed() {
        // 0.5% magnification of red and -0.4% of blue: about 1.3 and 1.0
        // pixels of displacement at the corners of a 400x320 frame.
        let (w, h) = (400, 320);
        let (clean, shifted) = aberrated(w, h, 0.005, -0.004);
        let p = CfaPattern::rggb();
        let (out, stats) = correct_ca(&shifted, w, h, &p, &CaOptions::default()).unwrap();
        assert!(stats.corrected, "{stats:?}");
        assert!(
            stats.max_shift[0] > 0.4 && stats.max_shift[0] < 2.0,
            "{stats:?}"
        );
        for color in [CfaColor::Red, CfaColor::Blue] {
            let before = rms_error(&clean, &shifted, w, h, &p, color);
            let after = rms_error(&clean, &out, w, h, &p, color);
            assert!(
                after < 0.5 * before,
                "{color:?}: {before} -> {after} {stats:?}"
            );
        }
        // Green is never touched.
        for y in 0..h {
            for x in 0..w {
                if p.color_at(y, x) == CfaColor::Green {
                    assert_eq!(out[y * w + x], shifted[y * w + x]);
                }
            }
        }
    }

    /// A width a few pixels past a tile origin: the last column of
    /// tiles starts inside the mosaic but its interior lies wholly past
    /// the right edge, and has nothing to write. Writing its empty rows
    /// at their offsets ran past the end of the mosaic on the last row
    /// and panicked, for any width 105 to 111 past a multiple of the
    /// interior (112): a frame of such a width crashed the CPU
    /// correction wherever it made a fit (the GPU's, which the editor
    /// takes when it has a device, did not).
    #[test]
    fn a_last_tile_column_with_no_interior_writes_nothing() {
        let p = CfaPattern::rggb();
        for w in [STEP * 5 - BORDER + 1, STEP * 5 - 1, STEP * 6 - 3] {
            let h = 320;
            let last = *origins(w).last().unwrap() + BORDER as isize;
            assert!(
                last >= w as isize,
                "{w}: the last tile's interior starts past the edge"
            );
            let (clean, shifted) = aberrated(w, h, 0.005, -0.004);
            let (out, stats) = correct_ca(&shifted, w, h, &p, &CaOptions::default()).unwrap();
            assert!(stats.corrected, "{w}: {stats:?}");
            // Corrected as at any other width.
            for color in [CfaColor::Red, CfaColor::Blue] {
                let before = rms_error(&clean, &shifted, w, h, &p, color);
                let after = rms_error(&clean, &out, w, h, &p, color);
                assert!(after < 0.5 * before, "{w} {color:?}: {before} -> {after}");
            }
        }
    }

    /// The resample on its own, on the fit a pass makes, is that pass;
    /// each sample's conditioning holds the value it took, and green
    /// has none.
    #[test]
    fn the_traced_resample_is_the_pass() {
        let (w, h) = (400, 320);
        let p = CfaPattern::rggb();
        let (_, shifted) = aberrated(w, h, 0.005, -0.004);
        let one = CaOptions {
            iterations: 1,
            avoid_color_shift: false,
        };
        let (pass, _) = correct_ca(&shifted, w, h, &p, &one).unwrap();
        let votes = measure_votes(&shifted, w, h, &p).unwrap();
        let sums = measure_coefficients(&shifted, w, h, &p).unwrap();
        assert_eq!(votes, sums.into_iter().map(vote_of).collect::<Vec<_>>());
        let fit = fit_votes(&votes, origins(h).len(), origins(w).len()).unwrap();
        let (out, cond) = resample_with(&shifted, w, h, &p, &fit).unwrap();
        assert_eq!(out, pass);
        for (i, c) in cond.iter().enumerate() {
            if p.color_at(i / w, i % w) == CfaColor::Green {
                assert_eq!(*c, Conditioning::default());
            } else {
                assert!(c.scale > 0.0 && c.value.max(0.0) == out[i], "{i}: {c:?}");
            }
        }
    }

    /// A flat mosaic, and a patch inside a textured one where green is
    /// flat and red and blue are not (a highlight whose green channel
    /// clipped first), have exactly zero gradient energy and cast no
    /// vote: four equal green neighbors give their value as the green
    /// between them, not that value rounded through weights that differ
    /// from site to site with red and blue, so the green gradients the
    /// energy is made of are zero. In RawTherapee's own order of
    /// evaluation the patch's greens land an ulp either side of their
    /// value, and its tiles have the residues' energy.
    #[test]
    #[allow(clippy::needless_range_loop)]
    fn a_flat_patch_has_no_gradient_energy() {
        let p = CfaPattern::rggb();
        let (w, h) = (600, 520);
        let level = 0.8137;
        // The tiles kept, each checked to have no energy and no vote.
        let silent = |sums: &[Coefficients], keep: &dyn Fn(usize) -> bool| -> usize {
            let mut n = 0;
            for (i, s) in sums.iter().enumerate().filter(|(i, _)| keep(*i)) {
                n += 1;
                let (energy, vote) = (scaled(*s), vote_of(*s));
                for dir in 0..2 {
                    for c in 0..2 {
                        assert_eq!(energy[dir][2][c], 0.0, "tile {i}'s energy ({c}, {dir})");
                        assert_eq!(vote.shift[c][dir], 17.0, "tile {i} voted ({c}, {dir})");
                    }
                }
            }
            n
        };
        let flat = vec![level; w * h];
        let sums = measure_coefficients(&flat, w, h, &p).unwrap();
        assert_eq!(silent(&sums, &|_| true), sums.len());
        // A patch covering whole tiles, its green flat, the texture
        // around it.
        let (_, mut mosaic) = aberrated(w, h, 0.005, -0.004);
        let (x0, y0, x1, y1) = (100, 90, 480, 430);
        for y in y0..y1 {
            for x in x0..x1 {
                if p.color_at(y, x) == CfaColor::Green {
                    mosaic[y * w + x] = level;
                }
            }
        }
        let (tops, lefts) = (origins(h), origins(w));
        // A tile reads the mosaic from its origin over its side.
        let inside = |i: usize| {
            let (t, l) = (tops[i / lefts.len()], lefts[i % lefts.len()]);
            let within =
                |o: isize, lo: usize, hi: usize| o >= lo as isize && o + TS as isize <= hi as isize;
            within(t, y0, y1) && within(l, x0, x1)
        };
        let sums = measure_coefficients(&mosaic, w, h, &p).unwrap();
        assert_eq!(silent(&sums, &inside), 6, "the tiles inside the patch");
        assert!(
            mosaic
                .chunks(w)
                .skip(y0)
                .take(y1 - y0)
                .any(|row| row[x0..x1].iter().any(|&v| v != level)),
            "red and blue inside the patch are not flat"
        );
        assert!(
            sums.iter()
                .enumerate()
                .any(|(i, s)| !inside(i) && vote_of(*s).shift[0][0] != 17.0),
            "the texture around the patch votes"
        );
    }

    #[test]
    fn rejects_non_bayer_and_skips_small_images() {
        let xtrans = CfaPattern::new(6, 6, vec![CfaColor::Green; 36]).unwrap();
        assert!(correct_ca(&[0.0; 36], 6, 6, &xtrans, &CaOptions::default()).is_err());
        let small = vec![0.5f32; 100 * 100];
        let (out, stats) =
            correct_ca(&small, 100, 100, &CfaPattern::rggb(), &CaOptions::default()).unwrap();
        assert_eq!(out, small);
        assert!(!stats.corrected);
    }
}
