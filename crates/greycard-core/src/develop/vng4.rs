//! VNG4: Variable Number of Gradients, with the two greens kept apart.
//!
//! Ported from RawTherapee's `rtengine/vng4_demosaic_RT.cc`, Ingo Weyrich's
//! speed-optimized version of the VNG interpolation in Dave Coffin's dcraw
//! (after Chang, Cheung and Pau, "Color filter array recovery using a
//! threshold-based variable number of gradients", 1999), GPL-3.0-or-later.
//! The gradient terms, the neighbor order, the threshold, the four-color
//! treatment of green and the red/blue step are theirs; the buffer layout,
//! the border handling and the threading are ours.
//!
//! At every pixel eight directional gradients are summed from absolute
//! differences of same-color raw samples around it (dcraw's 64 terms,
//! filtered per CFA phase), a threshold of `min + max / 2` selects the
//! directions with low gradient, and the missing green is the pixel's own
//! value plus the mean color difference over those directions. The "4" is
//! dcraw's four-color mode: the greens on red rows and on blue rows are
//! treated as separate colors, so at a green site the output is the mean
//! of the sample and the other green interpolated from its neighbors. That
//! is what makes VNG4 immune to the maze pattern that green imbalance and
//! noise give sharper algorithms, and what makes it soft: it is the
//! secondary demosaic for flat, noisy regions in [`super::dual`].
//!
//! Red and blue then follow the reference's simple step: color differences
//! against the VNG green, diagonally at the opposite color's sites and
//! along the row or column at green sites.
//!
//! The outer three pixels, which the reference hands to a separate border
//! routine, are bilinear here.

use rayon::prelude::*;

use super::bilinear_pixel;
pub use super::rcd::is_bayer;
use crate::error::{Error, Result};
use crate::raw::{CfaColor, CfaPattern};

/// Pixels on each side the reference does not interpolate.
const BORDER: usize = 3;

/// dcraw's gradient terms: two sample offsets `(y1, x1)`, `(y2, x2)`, the
/// weight as a power of two, and the directions (a bit per [`CHOOD`]
/// entry) their difference contributes to.
const TERMS: [(i8, i8, i8, i8, u8, u8); 64] = [
    (-2, -2, 0, -1, 0, 0x01),
    (-2, -2, 0, 0, 1, 0x01),
    (-2, -1, -1, 0, 0, 0x01),
    (-2, -1, 0, -1, 0, 0x02),
    (-2, -1, 0, 0, 0, 0x03),
    (-2, -1, 0, 1, 1, 0x01),
    (-2, 0, 0, -1, 0, 0x06),
    (-2, 0, 0, 0, 1, 0x02),
    (-2, 0, 0, 1, 0, 0x03),
    (-2, 1, -1, 0, 0, 0x04),
    (-2, 1, 0, -1, 1, 0x04),
    (-2, 1, 0, 0, 0, 0x06),
    (-2, 1, 0, 1, 0, 0x02),
    (-2, 2, 0, 0, 1, 0x04),
    (-2, 2, 0, 1, 0, 0x04),
    (-1, -2, -1, 0, 0, 0x80),
    (-1, -2, 0, -1, 0, 0x01),
    (-1, -2, 1, -1, 0, 0x01),
    (-1, -2, 1, 0, 1, 0x01),
    (-1, -1, -1, 1, 0, 0x88),
    (-1, -1, 1, -2, 0, 0x40),
    (-1, -1, 1, -1, 0, 0x22),
    (-1, -1, 1, 0, 0, 0x33),
    (-1, -1, 1, 1, 1, 0x11),
    (-1, 0, -1, 2, 0, 0x08),
    (-1, 0, 0, -1, 0, 0x44),
    (-1, 0, 0, 1, 0, 0x11),
    (-1, 0, 1, -2, 1, 0x40),
    (-1, 0, 1, -1, 0, 0x66),
    (-1, 0, 1, 0, 1, 0x22),
    (-1, 0, 1, 1, 0, 0x33),
    (-1, 0, 1, 2, 1, 0x10),
    (-1, 1, 1, -1, 1, 0x44),
    (-1, 1, 1, 0, 0, 0x66),
    (-1, 1, 1, 1, 0, 0x22),
    (-1, 1, 1, 2, 0, 0x10),
    (-1, 2, 0, 1, 0, 0x04),
    (-1, 2, 1, 0, 1, 0x04),
    (-1, 2, 1, 1, 0, 0x04),
    (0, -2, 0, 0, 1, 0x80),
    (0, -1, 0, 1, 1, 0x88),
    (0, -1, 1, -2, 0, 0x40),
    (0, -1, 1, 0, 0, 0x11),
    (0, -1, 2, -2, 0, 0x40),
    (0, -1, 2, -1, 0, 0x20),
    (0, -1, 2, 0, 0, 0x30),
    (0, -1, 2, 1, 1, 0x10),
    (0, 0, 0, 2, 1, 0x08),
    (0, 0, 2, -2, 1, 0x40),
    (0, 0, 2, -1, 0, 0x60),
    (0, 0, 2, 0, 1, 0x20),
    (0, 0, 2, 1, 0, 0x30),
    (0, 0, 2, 2, 1, 0x10),
    (0, 1, 1, 0, 0, 0x44),
    (0, 1, 1, 2, 0, 0x10),
    (0, 1, 2, -1, 1, 0x40),
    (0, 1, 2, 0, 0, 0x60),
    (0, 1, 2, 1, 0, 0x20),
    (0, 1, 2, 2, 0, 0x10),
    (1, -2, 1, 0, 0, 0x80),
    (1, -1, 1, 1, 0, 0x88),
    (1, 0, 1, 2, 0, 0x08),
    (1, 0, 2, -1, 0, 0x40),
    (1, 0, 2, 1, 0, 0x10),
];

/// The eight neighbors, in the order the gradient masks index them.
const CHOOD: [(i8, i8); 8] = [
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
    (1, 0),
    (1, -1),
    (0, -1),
];

/// A gradient term after filtering for one CFA phase. The directions it
/// contributes to are kept as ones and zeros rather than as a bit mask, so
/// the accumulation over the eight is a multiply and an add per direction
/// with no branch: for the finite samples the pipeline hands the demosaic,
/// `x * 1.0` and `x + 0.0` are exact, so the sums are the bits the branch
/// gave. A difference that was not finite would not be, since `NaN * 0.0`
/// and `inf * 0.0` are NaN where the branch left the direction alone;
/// [`Mosaic::green`] says so in a debug build.
#[derive(Clone, Copy)]
struct Term {
    d1: (isize, isize),
    d2: (isize, isize),
    weight: f32,
    grads: [f32; 8],
}

/// The mosaic in dcraw's four-color view: red 0, green on red rows 1,
/// blue 2, green on blue rows 3.
struct Mosaic<'a> {
    samples: &'a [f32],
    width: usize,
    /// Four-color index by phase `[y & 1][x & 1]`.
    color: [[usize; 2]; 2],
    /// Gradient terms by phase.
    terms: [[Vec<Term>; 2]; 2],
}

impl Mosaic<'_> {
    fn new<'a>(samples: &'a [f32], width: usize, pattern: &CfaPattern) -> Mosaic<'a> {
        let mut color = [[0usize; 2]; 2];
        for (y, row) in color.iter_mut().enumerate() {
            let has_red = (0..2).any(|x| pattern.color_at(y, x) == CfaColor::Red);
            for (x, c) in row.iter_mut().enumerate() {
                *c = match pattern.color_at(y, x) {
                    CfaColor::Red => 0,
                    CfaColor::Blue => 2,
                    CfaColor::Green if has_red => 1,
                    CfaColor::Green => 3,
                    CfaColor::Other(_) => unreachable!("Bayer pattern checked by caller"),
                };
            }
        }
        let fc = |y: isize, x: isize| color[y.rem_euclid(2) as usize][x.rem_euclid(2) as usize];
        let terms_for = |py: isize, px: isize| -> Vec<Term> {
            let mut out = Vec::new();
            for &(y1, x1, y2, x2, weight, grads) in &TERMS {
                let (y1, x1, y2, x2) = (y1 as isize, x1 as isize, y2 as isize, x2 as isize);
                let c = fc(py + y1, px + x1);
                if fc(py + y2, px + x2) != c {
                    continue;
                }
                let diag = if fc(py, px + 1) == c && fc(py + 1, px) == c {
                    2
                } else {
                    1
                };
                if (y1 - y2).abs() == diag && (x1 - x2).abs() == diag {
                    continue;
                }
                out.push(Term {
                    d1: (y1, x1),
                    d2: (y2, x2),
                    weight: (1u32 << weight) as f32,
                    grads: std::array::from_fn(|g| ((grads >> g) & 1) as f32),
                });
            }
            out
        };
        let terms = [
            [terms_for(0, 0), terms_for(0, 1)],
            [terms_for(1, 0), terms_for(1, 1)],
        ];
        Mosaic {
            samples,
            width,
            color,
            terms,
        }
    }

    #[inline]
    fn fc(&self, y: usize, x: usize) -> usize {
        self.color[y & 1][x & 1]
    }

    #[inline]
    fn raw(&self, y: usize, x: usize) -> f32 {
        self.samples[y * self.width + x]
    }

    #[inline]
    fn at(&self, y: usize, x: usize, dy: isize, dx: isize) -> f32 {
        self.raw((y as isize + dy) as usize, (x as isize + dx) as usize)
    }

    /// Color `c` at a pixel one in from the edge: the sample itself where
    /// the pixel has that color, else the reference's first linear
    /// interpolation, the 3x3 mean with cardinal neighbors weighted 2 and
    /// diagonal ones 1.
    fn plane(&self, c: usize, y: usize, x: usize) -> f32 {
        if self.fc(y, x) == c {
            return self.raw(y, x);
        }
        let mut sum = 0.0;
        let mut weight = 0.0;
        for dy in -1..=1isize {
            for dx in -1..=1isize {
                if (dy == 0 && dx == 0)
                    || self.fc((y as isize + dy) as usize, (x as isize + dx) as usize) != c
                {
                    continue;
                }
                let w = (1u32 << ((dy == 0) as u32 + (dx == 0) as u32)) as f32;
                sum += w * self.at(y, x, dy, dx);
                weight += w;
            }
        }
        sum / weight
    }

    /// The VNG green at an interior pixel. `greens` must hold rows `y-1`
    /// to `y+1` of the two interpolated green planes.
    fn green(&self, y: usize, x: usize, greens: &Greens) -> f32 {
        let mut gval = [0.0f32; 8];
        for t in &self.terms[y & 1][x & 1] {
            let diff =
                (self.at(y, x, t.d1.0, t.d1.1) - self.at(y, x, t.d2.0, t.d2.1)).abs() * t.weight;
            // The branchless sum below is the bit-for-bit equal of a
            // branch per direction only while this is finite, which the
            // levels the pipeline normalizes to 0..1 make it.
            debug_assert!(diff.is_finite(), "gradient term at ({x},{y}) is {diff}");
            for (v, g) in gval.iter_mut().zip(&t.grads) {
                *v += diff * g;
            }
        }
        let lo = gval.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = gval.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let thold = lo + hi * 0.5;

        let own = self.raw(y, x);
        let c = self.fc(y, x);
        let is_green = c & 1 == 1;
        let mut sum0 = 0.0f32;
        let mut sum1 = 0.0f32;
        let mut num = 0usize;
        for (g, &(dy, dx)) in CHOOD.iter().enumerate() {
            if gval[g] > thold {
                continue;
            }
            let (dy, dx) = (dy as isize, dx as isize);
            let (ny, nx) = ((y as isize + dy) as usize, (x as isize + dx) as usize);
            // The pixel two steps on has this pixel's color: with it, the
            // neighbor's own-color estimate.
            sum0 += own + self.at(y, x, 2 * dy, 2 * dx);
            sum1 += if is_green {
                greens.at(c ^ 2, ny, nx)
            } else {
                greens.at(1, ny, nx) + greens.at(3, ny, nx)
            };
            num += 1;
        }
        if is_green {
            sum0 *= 0.5;
        }
        (own + (sum1 - sum0) / (2 * num) as f32).max(0.0)
    }
}

/// The two interpolated green planes over three rows: what
/// [`Mosaic::green`] reads at its eight neighbors.
///
/// Each of those values is [`Mosaic::plane`]'s, which every pixel within
/// one step of it asks for again; held for a row and its two neighbors,
/// each is computed once. Three rows and not a band's worth because the
/// band is a thread's and the frame is large: two planes of three rows is
/// 200 KB where a band's would be megabytes. The rows sit in the tail of
/// the band's own buffer, so a band is one allocation as it was.
struct Greens<'a> {
    /// Rows by `y % CACHE_HEIGHT`, color 1 then color 3, `width` each.
    rows: &'a mut [f32],
    width: usize,
    /// The rows held, `lo..hi`. Outside it a row's slot holds another
    /// row's values and nothing would say so.
    lo: usize,
    hi: usize,
}

/// Rows of each plane the cache holds: the row being interpolated and one
/// on each side. A row's slot is its number modulo this, so no more than
/// this many may be live at once.
const CACHE_HEIGHT: usize = 3;
/// Rows of the plane cache in a band's buffer, past its green: the two
/// green planes over [`CACHE_HEIGHT`] rows.
const CACHE_ROWS: usize = 2 * CACHE_HEIGHT;

impl Greens<'_> {
    #[inline]
    fn index(&self, c: usize, y: usize, x: usize) -> usize {
        ((y % CACHE_HEIGHT) * 2 + (c >> 1)) * self.width + x
    }

    /// Row `y` of both planes, for every pixel the interior reads: the
    /// edge columns are never a neighbor of an interpolated pixel. Rows
    /// arrive in order, the oldest falling out of the window as they do.
    fn fill(&mut self, mosaic: &Mosaic, y: usize) {
        debug_assert!(
            self.hi == 0 || y == self.hi,
            "the cache rolls forward a row at a time: {y} after {}",
            self.hi
        );
        for c in [1usize, 3] {
            let base = self.index(c, y, 0);
            for x in 1..self.width - 1 {
                self.rows[base + x] = mosaic.plane(c, y, x);
            }
        }
        self.hi = y + 1;
        self.lo = self.hi.saturating_sub(CACHE_HEIGHT);
    }

    #[inline]
    fn at(&self, c: usize, y: usize, x: usize) -> f32 {
        debug_assert!(
            (self.lo..self.hi).contains(&y),
            "green plane row {y} is not among the {}..{} the cache holds",
            self.lo,
            self.hi
        );
        self.rows[self.index(c, y, x)]
    }
}

/// Rows per band when the demosaic runs in bands: the green is computed
/// a row beyond each band on both sides, so wider bands repeat less.
pub(crate) const BAND: usize = 64;

/// Demosaic a Bayer mosaic with VNG4. `samples` are one per pixel, levels
/// normalized and white balance applied. Values are clamped to
/// `0..=max(1, largest sample)` so unclipped highlights survive.
///
/// Images too small for the interior fall back to bilinear.
pub fn demosaic_vng4(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
) -> Result<Vec<f32>> {
    let vng = Vng4::new(samples, width, height, pattern)?;
    let mut out = vec![0.0f32; width * height * 3];
    out.par_chunks_mut(BAND * width * 3)
        .enumerate()
        .for_each(|(k, rows)| {
            let y0 = k * BAND;
            vng.rows(y0, y0 + rows.len() / (width * 3), rows);
        });
    Ok(out)
}

/// The demosaic set up for a frame, run over any band of rows, so a
/// consumer that blends the result away band by band (the dual demosaic)
/// never holds a second frame.
pub(crate) struct Vng4<'a> {
    mosaic: Mosaic<'a>,
    samples: &'a [f32],
    pattern: &'a CfaPattern,
    width: usize,
    height: usize,
    ceiling: f32,
    /// Too small for the gradients: everything is bilinear.
    small: bool,
}

impl<'a> Vng4<'a> {
    pub(crate) fn new(
        samples: &'a [f32],
        width: usize,
        height: usize,
        pattern: &'a CfaPattern,
    ) -> Result<Self> {
        if !is_bayer(pattern) {
            return Err(Error::Unsupported(format!(
                "VNG4 needs a 2x2 Bayer pattern, got {pattern}"
            )));
        }
        Ok(Self {
            mosaic: Mosaic::new(samples, width, pattern),
            samples,
            pattern,
            width,
            height,
            // The largest sample, in parallel: the maximum is exact and
            // does not care in what order it is taken.
            ceiling: samples
                .par_iter()
                .copied()
                .fold(|| 1.0f32, f32::max)
                .reduce(|| 1.0f32, f32::max),
            small: width < 2 * BORDER + 2 || height < 2 * BORDER + 2,
        })
    }

    fn bilinear(&self, y: usize, x: usize) -> [f32; 3] {
        bilinear_pixel(self.samples, self.width, self.height, self.pattern, y, x)
    }

    /// Rows `y0..y1` of the green: the gradients where they fit, bilinear
    /// on the border. The rolling plane cache is why this is a row loop
    /// and not a per-pixel call.
    fn greens(&self, y0: usize, out: &mut [f32], rows: &mut [f32]) {
        let (width, height) = (self.width, self.height);
        debug_assert_eq!(rows.len(), CACHE_ROWS * width);
        let mut cache = Greens {
            rows,
            width,
            lo: 0,
            hi: 0,
        };
        // The first plane row any interior pixel of this band will read.
        let mut next = y0.saturating_sub(1).max(1);
        for (r, row) in out.chunks_exact_mut(width).enumerate() {
            let y = y0 + r;
            let interior = !self.small && (2..height - 2).contains(&y);
            if interior {
                while next <= y + 1 {
                    cache.fill(&self.mosaic, next);
                    next += 1;
                }
            }
            for (x, g) in row.iter_mut().enumerate() {
                *g = if interior && (2..width - 2).contains(&x) {
                    self.mosaic.green(y, x, &cache)
                } else {
                    self.bilinear(y, x)[1]
                };
            }
        }
    }

    /// Rows `y0..y1` of the result into `out`, which holds exactly those
    /// rows of interleaved RGB.
    pub(crate) fn rows(&self, y0: usize, y1: usize, out: &mut [f32]) {
        let (width, height) = (self.width, self.height);
        debug_assert_eq!(out.len(), (y1 - y0) * width * 3);
        let mosaic = &self.mosaic;
        let ceiling = self.ceiling;

        // The band's green and a row beyond it on each side.
        let g0 = y0.saturating_sub(1);
        let g1 = (y1 + 1).min(height);
        let mut band = vec![0.0f32; (g1 - g0 + CACHE_ROWS) * width];
        let (green, cache) = band.split_at_mut((g1 - g0) * width);
        self.greens(g0, green, cache);
        let gr = |y: usize, x: usize| green[(y - g0) * width + x];

        for (r, row) in out.chunks_exact_mut(width * 3).enumerate() {
            let y = y0 + r;
            let edge_row = self.small || y < BORDER || y >= height - BORDER;
            // Which of red and blue this row holds.
            let row_color = if mosaic.fc(y, 0) == 2 || mosaic.fc(y, 1) == 2 {
                2
            } else {
                0
            };
            let other_color = 2 - row_color;
            for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                if edge_row || x < BORDER || x >= width - BORDER {
                    *px = self.bilinear(y, x);
                    continue;
                }
                let g = gr(y, x);
                let (own, other) = if mosaic.fc(y, x) & 1 == 0 {
                    // Red or blue site: the opposite color across the
                    // four diagonals.
                    let mut rb = 0.0;
                    for (dy, dx) in [(-1, -1), (-1, 1), (1, -1), (1, 1)] {
                        let (ny, nx) = ((y as isize + dy) as usize, (x as isize + dx) as usize);
                        rb += mosaic.raw(ny, nx) - gr(ny, nx);
                    }
                    (mosaic.raw(y, x), (g + rb * 0.25).max(0.0))
                } else {
                    // Green site: the row's color along the row, the other
                    // along the column.
                    let h = g
                        + (mosaic.raw(y, x - 1) - gr(y, x - 1) + mosaic.raw(y, x + 1)
                            - gr(y, x + 1))
                            * 0.5;
                    let v = g
                        + (mosaic.raw(y - 1, x) - gr(y - 1, x) + mosaic.raw(y + 1, x)
                            - gr(y + 1, x))
                            * 0.5;
                    (h.max(0.0), v.max(0.0))
                };
                px[row_color] = own.min(ceiling);
                px[1] = g.min(ceiling);
                px[other_color] = other.min(ceiling);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::demosaic_bilinear;
    use super::*;

    fn mosaic_of(rgb: &[f32], w: usize, h: usize, pattern: &CfaPattern) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let c = pattern.color_at(i / w, i % w).rgb_index().unwrap();
                rgb[i * 3 + c]
            })
            .collect()
    }

    /// A deterministic mosaic with a ramp, an edge, a checker and noise.
    fn synthetic(w: usize, h: usize, pattern: &CfaPattern) -> Vec<f32> {
        let mut seed = 0x9E3779B97F4A7C15u64;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
        };
        (0..w * h)
            .map(|i| {
                let (y, x) = (i / w, i % w);
                let c = pattern.color_at(y, x).rgb_index().unwrap();
                let base = 0.1 + 0.004 * x as f32 + 0.003 * y as f32;
                let edge = if x > w / 2 { 0.4 } else { 0.0 };
                let checker = if (x / 3 + y / 3) % 2 == 0 { 0.05 } else { 0.0 };
                let tint = [0.9, 1.0, 0.8][c];
                ((base + edge + checker) * tint + 0.01 * noise()).max(0.0)
            })
            .collect()
    }

    fn fnv(v: &[f32]) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for x in v {
            h ^= x.to_bits() as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// The bits, not just the behavior. The gradient sums and the plane
    /// cache are rearrangements meant to preserve the arithmetic exactly,
    /// so what comes out is pinned: these are the hashes the first version
    /// of the port gave, with a branch per direction and a plane
    /// interpolated afresh at every neighbor of every pixel.
    #[test]
    fn every_phase_is_bit_for_bit_what_the_port_first_gave() {
        let want = [
            ((0, 0), 0xe642174fa4cc5afau64),
            ((1, 0), 0x3f5f2c927ae472dc),
            ((0, 1), 0x3aedd72633875c9c),
            ((1, 1), 0xf7dce90bddea3c67),
        ];
        let (w, h) = (97, 71);
        for ((sx, sy), want) in want {
            let pattern = CfaPattern::rggb().shifted(sx, sy);
            let cfa = synthetic(w, h, &pattern);
            let out = demosaic_vng4(&cfa, w, h, &pattern).unwrap();
            assert_eq!(fnv(&out), want, "shift ({sx},{sy})");
        }
    }

    #[test]
    fn four_color_view_and_terms_follow_the_pattern() {
        let m = Mosaic::new(&[0.0; 16], 4, &CfaPattern::rggb());
        assert_eq!(m.color, [[0, 1], [3, 2]]);
        let m = Mosaic::new(&[0.0; 16], 4, &CfaPattern::rggb().shifted(1, 0));
        assert_eq!(m.color, [[1, 0], [2, 3]]);
        // Every phase keeps some terms and drops some.
        for row in &m.terms {
            for t in row {
                assert!(t.len() > 20 && t.len() < 64, "{}", t.len());
            }
        }
    }

    #[test]
    fn flat_field_is_flat() {
        let (w, h) = (16, 12);
        let pattern = CfaPattern::rggb();
        let rgb: Vec<f32> = (0..w * h).flat_map(|_| [0.5, 1.0, 0.25]).collect();
        let cfa = mosaic_of(&rgb, w, h, &pattern);
        let out = demosaic_vng4(&cfa, w, h, &pattern).unwrap();
        for (a, b) in out.iter().zip(&rgb) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
    }

    #[test]
    fn a_neutral_ramp_is_reproduced_exactly() {
        // Every estimate VNG makes is linear in the samples, so a plane
        // survives, whichever corner the pattern starts in.
        let (w, h) = (24, 20);
        let rgb: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let v = 0.1 + 0.02 * (i % w) as f32 + 0.015 * (i / w) as f32;
                [v, v, v]
            })
            .collect();
        for (sx, sy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let pattern = CfaPattern::rggb().shifted(sx, sy);
            let cfa = mosaic_of(&rgb, w, h, &pattern);
            let out = demosaic_vng4(&cfa, w, h, &pattern).unwrap();
            for y in BORDER..h - BORDER {
                for x in BORDER..w - BORDER {
                    for c in 0..3 {
                        let i = (y * w + x) * 3 + c;
                        assert!(
                            (out[i] - rgb[i]).abs() < 1e-4,
                            "({x},{y}) c{c}: {} vs {} for shift ({sx},{sy})",
                            out[i],
                            rgb[i]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn follows_an_edge_better_than_bilinear() {
        // A vertical edge in every channel. VNG4 still bleeds a little,
        // because the other green at a neighbor is read from diagonals
        // that straddle the edge, but less than bilinear.
        let (w, h) = (32, 16);
        let pattern = CfaPattern::rggb();
        let rgb: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                if i % w < 16 {
                    [0.2, 0.3, 0.1]
                } else {
                    [0.8, 0.9, 0.7]
                }
            })
            .collect();
        let cfa = mosaic_of(&rgb, w, h, &pattern);
        let vng = demosaic_vng4(&cfa, w, h, &pattern).unwrap();
        let bil = demosaic_bilinear(&cfa, w, h, &pattern);
        let err = |out: &[f32]| -> f32 {
            let mut e = 0.0;
            for y in BORDER..h - BORDER {
                for x in BORDER..w - BORDER {
                    for c in 0..3 {
                        let i = (y * w + x) * 3 + c;
                        e += (out[i] - rgb[i]).abs();
                    }
                }
            }
            e
        };
        assert!(
            err(&vng) < 0.85 * err(&bil),
            "vng {} bilinear {}",
            err(&vng),
            err(&bil)
        );
    }

    #[test]
    fn rejects_what_it_cannot_do() {
        let cfa = vec![0.5; 16];
        let p = CfaPattern::new(2, 2, vec![CfaColor::Red; 4]).unwrap();
        assert!(demosaic_vng4(&cfa, 4, 4, &p).is_err());
        // Too small for the interior: bilinear, not a panic.
        assert!(demosaic_vng4(&cfa, 4, 4, &CfaPattern::rggb()).is_ok());
    }
}
