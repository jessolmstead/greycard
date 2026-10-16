//! Markesteijn's demosaic for Fujifilm's 6x6 X-Trans mosaic.
//!
//! Ported from RawTherapee's `rtengine/xtrans_demosaic.cc` (dev branch,
//! code dated 2018-04-18), GPL-3.0-or-later: `xtrans_interpolate` and
//! `xtransborder_interpolate`. The algorithm is Frank Markesteijn's,
//! first published in Dave Coffin's dcraw (`xtrans_interpolate`, 2012),
//! adapted to RawTherapee and optimized by Ingo Weyrich in 2014. The
//! algorithm, its constants, its tiling (114-pixel tiles stepping by 98)
//! and its pass structure are theirs; the buffers, the threading and the
//! guards at the edges are ours.
//!
//! What it does on each tile, after RawTherapee's order:
//!
//! 1. A green hexagon is mapped around every site once for the pattern:
//!    six greens round a red or blue site, the nearest of each color
//!    round a green one. The darkest and brightest of the six bound every
//!    green estimate made at that site.
//! 2. Green at red and blue sites in four directions (horizontal,
//!    vertical and the two diagonals), each from the hexagon's greens
//!    along that direction, bounded as above.
//! 3. With three passes, green again from the now complete neighbors,
//!    and the passes after the first work on a second set of four
//!    directions, eight in all.
//! 4. Red and blue at the solitary green sites (the center of each 3x3
//!    block) along each direction, then red at blue sites and blue at
//!    red, then red and blue in the 2x2 blocks of green, all as color
//!    differences against green.
//! 5. Each direction's picture in YPbPr, its second derivative along the
//!    direction, and from that a homogeneity map: how many of a site's
//!    3x3 neighbors are within eight times the smallest derivative. The
//!    5x5 sums pick the directions to average at each site.
//!
//! Departures from the reference:
//!
//! - Both pass counts judge homogeneity in YPbPr. RawTherapee uses CIELab
//!   through the camera matrix for three passes; darktable's port of the
//!   same algorithm uses YPbPr for every pass count, and this engine's
//!   demosaics take no camera matrix. The samples are white-balanced
//!   camera RGB, which is what YPbPr assumes of them.
//! - The homogeneity maps are zeroed for each tile, as dcraw has them.
//!   RawTherapee's maps share their memory with the same tile's YPbPr
//!   buffer, and in the tiles along all four edges of the frame the 5x5
//!   sums read entries the maps never computed, so they read the bytes
//!   of YPbPr floats. With three passes that moves the sites eight to
//!   ten rows or columns in from the frame's edge (by up to 0.43 on a
//!   real frame); one pass's border of eleven covers them. Everywhere
//!   else the port is bit-exact against RawTherapee, which a test pins.
//! - The border's 3x3 weighted mean falls back to a 5x5 plain one for a
//!   color the 3x3 does not hold (a frame corner can be all green), and
//!   to the site's own value where neither does, rather than divide by
//!   zero; a frame too small for one tile is all border.
//! - The result is clamped to `0..=max(1, largest sample)`, as the
//!   engine's other demosaics clamp, so highlights above 1.0 survive.

use rayon::prelude::*;

use crate::error::{Error, Result};
use crate::raw::CfaPattern;

/// Tile size, as in the reference.
const TS: usize = 114;
/// Half the tile: the green bounds keep one entry a pair of columns.
const TSH: usize = TS / 2;
/// Pixels in one direction's tile.
const PLANE: usize = TS * TS;
/// Below this either way there is no room for a tile: the whole frame
/// takes the border interpolation.
const MIN_SIZE: usize = 32;

/// How many passes of Markesteijn's algorithm to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Passes {
    /// One pass in four directions: about half the time of three.
    One,
    /// Three passes, the last two refining green in four more
    /// directions: the sharper, and the engine's choice.
    Three,
}

impl Passes {
    fn count(self) -> usize {
        match self {
            Passes::One => 1,
            Passes::Three => 3,
        }
    }

    /// The border the tiles leave to [`border_interpolate`], as the
    /// reference has it.
    fn border(self) -> usize {
        match self {
            Passes::One => 11,
            Passes::Three => 8,
        }
    }
}

/// Demosaic an X-Trans mosaic with Markesteijn's algorithm. `samples`
/// are one per pixel, levels normalized and white balance applied;
/// `pattern` is X-Trans as seen from the samples' origin.
pub fn demosaic_markesteijn(
    samples: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    passes: Passes,
) -> Result<Vec<f32>> {
    if !pattern.is_xtrans() {
        return Err(Error::Unsupported(format!(
            "Markesteijn needs the 6x6 X-Trans pattern, got {pattern}"
        )));
    }
    if samples.len() != width * height {
        return Err(Error::Unsupported(format!(
            "{width}x{height} mosaic with {} samples",
            samples.len()
        )));
    }
    let ceiling = samples.iter().copied().fold(1.0f32, f32::max);
    let layout = Layout::new(pattern, width);
    let mut out = vec![0.0f32; width * height * 3];
    if width < MIN_SIZE || height < MIN_SIZE {
        border_interpolate(samples, width, height, &layout, width.max(height), &mut out);
    } else {
        interior(samples, width, height, &layout, passes, &mut out);
        border_interpolate(samples, width, height, &layout, passes.border(), &mut out);
    }
    out.par_iter_mut().for_each(|v| *v = v.clamp(0.0, ceiling));
    Ok(out)
}

/// The pattern as the algorithm reads it, and the hexagons round each
/// phase of it.
struct Layout {
    /// 0 red, 1 green, 2 blue.
    colors: [[usize; 6]; 6],
    /// Offsets of the green hexagon round each site of the 3x3 phase:
    /// `[0]` in the mosaic's strides, `[1]` in a tile's.
    allhex: [[[[isize; 8]; 3]; 3]; 2],
    /// The solitary green's phase: the center of a 3x3 block.
    sgrow: usize,
    sgcol: usize,
    /// Rows of the 3x3 phase with two greens, whose other sites are
    /// every third column.
    right_shift: [bool; 3],
}

impl Layout {
    // The loops index as the reference's do, so the two read alike.
    #[allow(clippy::needless_range_loop)]
    fn new(pattern: &CfaPattern, width: usize) -> Self {
        let colors: [[usize; 6]; 6] = std::array::from_fn(|r| {
            std::array::from_fn(|c| pattern.color_at(r, c).rgb_index().expect("X-Trans is RGB"))
        });
        let isgreen = |row: usize, col: usize| colors[row % 3][col % 3] == 1;
        const ORTH: [isize; 12] = [1, 0, 0, 1, -1, 0, 0, -1, 1, 0, 0, 1];
        const PATT: [[isize; 16]; 2] = [
            [0, 1, 0, -1, 2, 0, -1, 0, 1, 1, 1, -1, 0, 0, 0, 0],
            [0, 1, 0, -2, 1, 0, -2, 0, 1, 1, -2, -2, 1, -1, -1, 1],
        ];
        let mut allhex = [[[[0isize; 8]; 3]; 3]; 2];
        let (mut sgrow, mut sgcol) = (0, 0);
        for row in 0..3 {
            for col in 0..3 {
                let gint = usize::from(isgreen(row, col));
                let mut ng = 0;
                for d in (0..10).step_by(2) {
                    let r = (row as isize + ORTH[d] + 6) as usize;
                    let c = (col as isize + ORTH[d + 2] + 6) as usize;
                    if isgreen(r, c) {
                        ng = 0;
                    } else {
                        ng += 1;
                    }
                    if ng == 4 {
                        // Four non-green sites round it: the solitary
                        // green.
                        sgrow = row;
                        sgcol = col;
                    }
                    if ng == gint + 1 {
                        for c in 0..8 {
                            let v =
                                ORTH[d] * PATT[gint][c * 2] + ORTH[d + 1] * PATT[gint][c * 2 + 1];
                            let h = ORTH[d + 2] * PATT[gint][c * 2]
                                + ORTH[d + 3] * PATT[gint][c * 2 + 1];
                            let at = c ^ ((gint * 2) & d);
                            allhex[0][row][col][at] = h + v * width as isize;
                            allhex[1][row][col][at] = h + v * TS as isize;
                        }
                    }
                }
            }
        }
        let right_shift =
            std::array::from_fn(|row| (0..3).filter(|&col| isgreen(row, col)).count() == 2);
        Self {
            colors,
            allhex,
            sgrow,
            sgcol,
            right_shift,
        }
    }

    #[inline]
    fn fcol(&self, row: usize, col: usize) -> usize {
        self.colors[row % 6][col % 6]
    }

    #[inline]
    fn isgreen(&self, row: usize, col: usize) -> bool {
        self.colors[row % 3][col % 3] == 1
    }

    /// The first non-green column of `row` at or after `from`, short of
    /// `to`.
    fn first_non_green(&self, row: usize, from: usize, to: usize) -> usize {
        let mut col = from;
        while col < to && self.isgreen(row, col) {
            col += 1;
        }
        col
    }
}

/// One band of tiles: the rows it writes.
struct Band {
    top: usize,
    start: usize,
    end: usize,
}

/// The tiled interior, every band of tiles on its own thread. Each band
/// writes rows of its own, so the output splits between them.
fn interior(
    samples: &[f32],
    width: usize,
    height: usize,
    layout: &Layout,
    passes: Passes,
    out: &mut [f32],
) {
    let bands: Vec<Band> = (3..height - 19)
        .step_by(TS - 16)
        .map(|top| {
            let (_, end) = written(top, height);
            Band {
                top,
                start: top + top.min(8),
                end: top + end,
            }
        })
        .collect();
    let mut slices = Vec::with_capacity(bands.len());
    let first = bands.first().map_or(0, |b| b.start);
    let mut rest = &mut out[first * width * 3..];
    for (i, band) in bands.iter().enumerate() {
        debug_assert!(i == 0 || bands[i - 1].end == band.start);
        let (mine, after) = rest.split_at_mut((band.end - band.start) * width * 3);
        slices.push(mine);
        rest = after;
    }
    bands.par_iter().zip(slices).for_each(|(band, slice)| {
        let mut scratch = Scratch::new(passes);
        for left in (3..width - 19).step_by(TS - 16) {
            scratch.tile(samples, width, height, layout, band, left, slice);
        }
    });
}

/// The extent of a tile at `top` along an edge of `size`: where its
/// sites stop being read (tile-relative), and where it stops writing.
fn written(top: usize, size: usize) -> (usize, usize) {
    let read = (top + TS).min(size - 3) - top;
    let write = if size - top < TS + 4 {
        size - top + 2
    } else {
        read
    };
    (read, write - 8)
}

/// A tile's buffers, kept for the band.
struct Scratch {
    passes: Passes,
    ndir: usize,
    /// `ndir` directions of a tile of RGB.
    rgb: Vec<f32>,
    /// The bounds on green, one a pair of columns.
    green_bounds: Vec<(f32, f32)>,
    /// Y, Pb, Pr of one direction, a tile less four on each side.
    yuv: Vec<f32>,
    /// Each direction's derivative, a tile less five on each side.
    drv: Vec<f32>,
    homo: Vec<u8>,
    homosum: Vec<u8>,
}

impl Scratch {
    fn new(passes: Passes) -> Self {
        let ndir = 4 << usize::from(passes.count() > 1);
        Self {
            passes,
            ndir,
            rgb: vec![0.0; ndir * PLANE * 3],
            green_bounds: vec![(0.0, 0.0); TS * TSH],
            yuv: vec![0.0; 3 * (TS - 8) * (TS - 8)],
            drv: vec![0.0; ndir * (TS - 10) * (TS - 10)],
            homo: vec![0; ndir * PLANE],
            homosum: vec![0; ndir * PLANE],
        }
    }

    /// One tile at (`band.top`, `left`), written into `out`, the band's
    /// rows. The loops index as the reference's do, so the two read
    /// alike.
    #[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
    fn tile(
        &mut self,
        samples: &[f32],
        width: usize,
        height: usize,
        layout: &Layout,
        band: &Band,
        left: usize,
        out: &mut [f32],
    ) {
        let top = band.top;
        let mrow = (top + TS).min(height - 3);
        let mcol = (left + TS).min(width - 3);
        let ndir = self.ndir;
        let raw = |at: isize| samples[at as usize];

        // The bounds on green at every non-green site: the darkest and
        // brightest of its hexagon. RawTherapee's shortcut: a pair of
        // neighboring sites shares the first one's.
        let gb = &mut self.green_bounds;
        let hex_bounds = |pix: isize, hex: &[isize; 8]| {
            let mut min = f32::MAX;
            let mut max = 0.0f32;
            for &h in &hex[..6] {
                let v = raw(pix + h);
                min = min.min(v);
                max = max.max(v);
            }
            (min, max)
        };
        for row in top..mrow {
            let leftstart = layout.first_non_green(row, left, mcol);
            let coloffset = if layout.right_shift[row % 3] {
                3
            } else {
                1 + (layout.fcol(row, leftstart + 1) & 1)
            };
            let slot = |col: usize| (row - top) * TSH + ((col - left) >> 1);
            let pix = |col: usize| (row * width + col) as isize;
            if coloffset == 3 {
                let hex = &layout.allhex[0][row % 3][leftstart % 3];
                for col in (leftstart..mcol).step_by(3) {
                    gb[slot(col)] = hex_bounds(pix(col), hex);
                }
            } else {
                let mut col = leftstart;
                if coloffset == 2 {
                    let hex = &layout.allhex[0][row % 3][col % 3];
                    gb[slot(col)] = hex_bounds(pix(col), hex);
                    col += 2;
                }
                let hex = &layout.allhex[0][row % 3][col % 3];
                while col + 1 < mcol {
                    let b = hex_bounds(pix(col), hex);
                    gb[slot(col)] = b;
                    gb[slot(col + 1)] = b;
                    col += 3;
                }
                if col < mcol {
                    gb[slot(col)] = hex_bounds(pix(col), hex);
                }
            }
        }
        let bounds =
            |gb: &[(f32, f32)], row: usize, col: usize| gb[(row - top) * TSH + ((col - left) >> 1)];
        let lim = |v: f32, (min, max): (f32, f32)| v.min(max).max(min);

        // The mosaic into the first four directions.
        let rgb = &mut self.rgb;
        rgb[..PLANE * 3].fill(0.0);
        for row in top..mrow {
            for col in left..mcol {
                let p = (row - top) * TS + (col - left);
                rgb[p * 3 + layout.fcol(row, col)] = samples[row * width + col];
            }
        }
        for d in 1..4 {
            rgb.copy_within(..PLANE * 3, d * PLANE * 3);
        }

        // Green horizontally, vertically and along both diagonals.
        for row in top..mrow {
            let leftstart = layout.first_non_green(row, left, mcol);
            let mut coloffset = if layout.right_shift[row % 3] {
                3
            } else {
                1 + (layout.fcol(row, leftstart + 1) & 1)
            };
            let estimates = |col: usize, hex: &[isize; 8]| -> [f32; 4] {
                let pix = (row * width + col) as isize;
                let p = |k: isize| raw(pix + k);
                let mut color = [0.0f32; 4];
                color[0] = 0.6796875 * (p(hex[1]) + p(hex[0]))
                    - 0.1796875 * (p(2 * hex[1]) + p(2 * hex[0]));
                color[1] = 0.87109375 * p(hex[3])
                    + p(hex[2]) * 0.12890625
                    + 0.359375 * (p(0) - p(-hex[2]));
                for c in 0..2 {
                    color[2 + c] = 0.640625 * p(hex[4 + c])
                        + 0.359375 * p(-2 * hex[4 + c])
                        + 0.12890625 * (2.0 * p(0) - p(3 * hex[4 + c]) - p(-3 * hex[4 + c]));
                }
                color
            };
            if coloffset == 3 {
                let hex = &layout.allhex[0][row % 3][leftstart % 3];
                for col in (leftstart..mcol).step_by(3) {
                    let color = estimates(col, hex);
                    let b = bounds(gb, row, col);
                    let p = (row - top) * TS + (col - left);
                    for (c, v) in color.into_iter().enumerate() {
                        rgb[(c * PLANE + p) * 3 + 1] = lim(v, b);
                    }
                }
            } else {
                let hexmod = [
                    &layout.allhex[0][row % 3][leftstart % 3],
                    &layout.allhex[0][row % 3][(leftstart + coloffset) % 3],
                ];
                let mut col = leftstart;
                let mut hexindex = 0;
                while col < mcol {
                    let color = estimates(col, hexmod[hexindex]);
                    let b = bounds(gb, row, col);
                    let p = (row - top) * TS + (col - left);
                    for (c, v) in color.into_iter().enumerate() {
                        rgb[((c ^ 1) * PLANE + p) * 3 + 1] = lim(v, b);
                    }
                    col += coloffset;
                    coloffset ^= 3;
                    hexindex ^= 1;
                }
            }
        }

        // `rix` is a site's index in pixels, its direction included;
        // these read and write one channel of it.
        let at = |rix: isize, ch: usize| rix as usize * 3 + ch;
        let tile_px = |row: usize, col: usize| ((row - top) * TS + (col - left)) as isize;
        let ts = TS as isize;
        let plane = PLANE as isize;
        let mut base = 0isize;
        for pass in 0..self.passes.count() {
            if pass == 1 {
                // The second set of four directions starts as the first.
                rgb.copy_within(..4 * PLANE * 3, 4 * PLANE * 3);
                base = 4 * plane;
            }

            // Green again, from the interpolated values of closer sites.
            if pass > 0 {
                for row in top + 2..mrow - 2 {
                    let leftstart = layout.first_non_green(row, left + 2, mcol - 2);
                    let mut coloffset = if layout.right_shift[row % 3] {
                        3
                    } else {
                        1 + (layout.fcol(row, leftstart + 1) & 1)
                    };
                    let mut f = layout.fcol(row, leftstart);
                    let refine = |rgb: &mut [f32], rix: isize, hex: isize, f: usize, b| {
                        let val = 0.33333333
                            * (rgb[at(rix - 2 * hex, 1)]
                                + 2.0 * (rgb[at(rix + hex, 1)] - rgb[at(rix + hex, f)])
                                - rgb[at(rix - 2 * hex, f)])
                            + rgb[at(rix, f)];
                        rgb[at(rix, 1)] = lim(val, b);
                    };
                    if coloffset == 3 {
                        let hex = &layout.allhex[1][row % 3][leftstart % 3];
                        let mut col = leftstart;
                        while col < mcol - 2 {
                            let b = bounds(gb, row, col);
                            for d in 3..6 {
                                let rix = base + (d as isize - 2) * plane + tile_px(row, col);
                                refine(rgb, rix, hex[d], f, b);
                            }
                            col += coloffset;
                            f ^= 2;
                        }
                    } else {
                        let hexmod = [
                            &layout.allhex[1][row % 3][leftstart % 3],
                            &layout.allhex[1][row % 3][(leftstart + coloffset) % 3],
                        ];
                        let mut col = leftstart;
                        let mut hexindex = 0;
                        while col < mcol - 2 {
                            let hex = hexmod[hexindex];
                            let b = bounds(gb, row, col);
                            for d in 3..6 {
                                let rix =
                                    base + (((d - 2) ^ 1) as isize) * plane + tile_px(row, col);
                                refine(rgb, rix, hex[d], f, b);
                            }
                            col += coloffset;
                            coloffset ^= 3;
                            f ^= coloffset & 2;
                            hexindex ^= 1;
                        }
                    }
                }
            }

            // Red and blue at the solitary green sites.
            let sgstartcol = (left - layout.sgcol + 4) / 3 * 3 + layout.sgcol;
            let mut color = [[0.0f32; 6]; 3];
            let mut row = (top - layout.sgrow + 4) / 3 * 3 + layout.sgrow;
            while row < mrow - 2 {
                let mut col = sgstartcol;
                let mut h = layout.fcol(row, col + 1);
                while col < mcol - 2 {
                    let mut rix = base + tile_px(row, col);
                    let mut diff = [0.0f32; 6];
                    let mut i = 1isize;
                    for d in 0..6 {
                        for c in 0..2 {
                            let o = i << c;
                            let g =
                                2.0 * rgb[at(rix, 1)] - rgb[at(rix + o, 1)] - rgb[at(rix - o, 1)];
                            color[h][d] = g + rgb[at(rix + o, h)] + rgb[at(rix - o, h)];
                            if d > 1 {
                                let e =
                                    rgb[at(rix + o, 1)] - rgb[at(rix - o, 1)] - rgb[at(rix + o, h)]
                                        + rgb[at(rix - o, h)];
                                diff[d] += e * e + g * g;
                            }
                            h ^= 2;
                        }
                        if d > 2 && d & 1 == 1 && diff[d - 1] < diff[d] {
                            for c in 0..2 {
                                color[c * 2][d] = color[c * 2][d - 1];
                            }
                        }
                        if d & 1 == 1 || d < 2 {
                            for c in 0..2 {
                                rgb[at(rix, c * 2)] = 0.5 * color[c * 2][d];
                            }
                            rix += plane;
                        }
                        i ^= ts ^ 1;
                        h ^= 2;
                    }
                    col += 3;
                    h ^= 2;
                }
                row += 3;
            }

            // Red at blue sites and blue at red.
            for row in top + 3..mrow - 3 {
                let leftstart = layout.first_non_green(row, left + 3, mcol - 1);
                let c = if !(row - layout.sgrow).is_multiple_of(3) {
                    ts
                } else {
                    1
                };
                let h = 3 * (c ^ ts ^ 1);
                let fill = |rgb: &mut [f32], col: usize, f: usize| {
                    let mut rix = base + tile_px(row, col);
                    for d in 0..4isize {
                        let g = rgb[at(rix, 1)];
                        let i = if d > 1
                            || (d ^ c) & 1 != 0
                            || ((g - rgb[at(rix + c, 1)]).abs() + (g - rgb[at(rix - c, 1)]).abs())
                                < 2.0
                                    * ((g - rgb[at(rix + h, 1)]).abs()
                                        + (g - rgb[at(rix - h, 1)]).abs())
                        {
                            c
                        } else {
                            h
                        };
                        rgb[at(rix, f)] = g + 0.5
                            * (rgb[at(rix + i, f)] + rgb[at(rix - i, f)]
                                - rgb[at(rix + i, 1)]
                                - rgb[at(rix - i, 1)]);
                        rix += plane;
                    }
                };
                let mut f = 2 - layout.fcol(row, leftstart);
                let mut col = leftstart;
                if layout.right_shift[row % 3] {
                    while col < mcol - 3 {
                        fill(rgb, col, f);
                        col += 3;
                        f ^= 2;
                    }
                } else {
                    let mut coloffset = if layout.fcol(row, leftstart + 1) == 1 {
                        2
                    } else {
                        1
                    };
                    while col < mcol - 3 {
                        fill(rgb, col, f);
                        col += coloffset;
                        coloffset ^= 3;
                        f ^= coloffset & 2;
                    }
                }
            }

            // Red and blue in the 2x2 blocks of green.
            let mut topstart = top + 2;
            while topstart < mrow - 2 && (topstart - layout.sgrow).is_multiple_of(3) {
                topstart += 1;
            }
            let mut leftstart = left + 2;
            while leftstart < mcol - 2 && (leftstart - layout.sgcol).is_multiple_of(3) {
                leftstart += 1;
            }
            let coloffsetstart = 2 - (layout.fcol(topstart, leftstart + 1) & 1);
            for row in topstart..mrow - 2 {
                if (row - layout.sgrow).is_multiple_of(3) {
                    continue;
                }
                let hexmod = [
                    &layout.allhex[1][row % 3][leftstart % 3],
                    &layout.allhex[1][row % 3][(leftstart + coloffsetstart) % 3],
                ];
                let mut col = leftstart;
                let mut coloffset = coloffsetstart;
                let mut hexindex = 0;
                while col < mcol - 2 {
                    let mut rix = base + tile_px(row, col);
                    let hex = hexmod[hexindex];
                    for d in (0..ndir).step_by(2) {
                        let (h0, h1) = (hex[d], hex[d + 1]);
                        if h0 + h1 != 0 {
                            let g = 3.0 * rgb[at(rix, 1)]
                                - 2.0 * rgb[at(rix + h0, 1)]
                                - rgb[at(rix + h1, 1)];
                            for c in [0, 2] {
                                rgb[at(rix, c)] =
                                    (g + 2.0 * rgb[at(rix + h0, c)] + rgb[at(rix + h1, c)])
                                        * 0.33333333;
                            }
                        } else {
                            let g =
                                2.0 * rgb[at(rix, 1)] - rgb[at(rix + h0, 1)] - rgb[at(rix + h1, 1)];
                            for c in [0, 2] {
                                rgb[at(rix, c)] =
                                    (g + rgb[at(rix + h0, c)] + rgb[at(rix + h1, c)]) * 0.5;
                            }
                        }
                        rix += plane;
                    }
                    col += coloffset;
                    coloffset ^= 3;
                    hexindex ^= 1;
                }
            }
        }

        // Each direction in YPbPr (ITU-R BT.2020), and its second
        // derivative along the direction.
        let (mut mrow, mut mcol) = (mrow - top, mcol - left);
        const LW: usize = TS - 8;
        const DW: usize = TS - 10;
        let dir = [1, TS, TS + 1, TS - 1];
        let yuv = &mut self.yuv;
        let drv = &mut self.drv;
        for d in 0..ndir {
            for row in 4..mrow - 4 {
                for col in 4..mcol - 4 {
                    let p = (d * PLANE + row * TS + col) * 3;
                    let (r, g, b) = (rgb[p], rgb[p + 1], rgb[p + 2]);
                    let y = 0.2627 * r + 0.6780 * g + 0.0593 * b;
                    let o = (row - 4) * LW + (col - 4);
                    yuv[o] = y;
                    yuv[LW * LW + o] = (b - y) * 0.56433;
                    yuv[2 * LW * LW + o] = (r - y) * 0.67815;
                }
            }
            let f = if dir[d & 3] == 1 { 1 } else { dir[d & 3] - 8 };
            for row in 5..mrow - 5 {
                for col in 5..mcol - 5 {
                    let o = (row - 4) * LW + (col - 4);
                    let second = |plane: usize| {
                        let v = &yuv[plane * LW * LW..];
                        let s = 2.0 * v[o] - v[o + f] - v[o - f];
                        s * s
                    };
                    drv[d * DW * DW + (row - 5) * DW + (col - 5)] =
                        second(0) + second(1) + second(2);
                }
            }
        }

        // Homogeneity maps: how many of the 3x3 round a site are within
        // eight times the least derivative of any direction there.
        let homo = &mut self.homo;
        homo.fill(0);
        for row in 6..mrow - 6 {
            for col in 6..mcol - 6 {
                let drv_at =
                    |d: usize, r: usize, c: usize| drv[d * DW * DW + (r - 5) * DW + (c - 5)];
                let tr = (0..ndir)
                    .map(|d| drv_at(d, row, col))
                    .fold(f32::MAX, f32::min)
                    * 8.0;
                for d in 0..ndir {
                    let mut count = 0u8;
                    for r in row - 1..=row + 1 {
                        for c in col - 1..=col + 1 {
                            count += u8::from(drv_at(d, r, c) <= tr);
                        }
                    }
                    homo[d * PLANE + row * TS + col] = count;
                }
            }
        }

        if height - top < TS + 4 {
            mrow = height - top + 2;
        }
        if width - left < TS + 4 {
            mcol = width - left + 2;
        }

        // Their 5x5 sums, by columns of five then rows of five.
        let (row0, col0) = (top.min(8), left.min(8));
        let homosum = &mut self.homosum;
        let mut columns = [0u8; TS];
        for d in 0..ndir {
            let homo = &homo[d * PLANE..(d + 1) * PLANE];
            for row in row0..mrow - 8 {
                for (col, sum) in columns.iter_mut().enumerate().take(mcol - 6).skip(col0 - 2) {
                    *sum = (row - 2..=row + 2).map(|r| homo[r * TS + col]).sum();
                }
                for col in col0..mcol - 8 {
                    homosum[d * PLANE + row * TS + col] = columns[col - 2..=col + 2].iter().sum();
                }
            }
        }

        // The average of the most homogeneous directions.
        for row in row0..mrow - 8 {
            let line = &mut out[(row + top - band.start) * width * 3..][..width * 3];
            for col in col0..mcol - 8 {
                let mut hm = [0u8; 8];
                for (d, h) in hm.iter_mut().enumerate().take(ndir) {
                    *h = homosum[d * PLANE + row * TS + col];
                }
                let mut max = hm[..ndir].iter().copied().max().unwrap_or(0);
                max -= max >> 3;
                for d in 4..ndir {
                    if hm[d - 4] < hm[d] {
                        hm[d - 4] = 0;
                    } else if hm[d - 4] > hm[d] {
                        hm[d] = 0;
                    }
                }
                let mut avg = [0.0f32; 3];
                let mut n = 0.0f32;
                for (d, &h) in hm.iter().enumerate().take(ndir) {
                    if h >= max {
                        let p = (d * PLANE + row * TS + col) * 3;
                        for c in 0..3 {
                            avg[c] += rgb[p + c];
                        }
                        n += 1.0;
                    }
                }
                let px = &mut line[(col + left) * 3..][..3];
                for c in 0..3 {
                    px[c] = (avg[c] / n).max(0.0);
                }
            }
        }
    }
}

/// The frame's outer `border` sites, from a weighted mean of each color
/// in their 3x3 neighborhood, as the reference's border routine: the
/// tiles do not reach them.
fn border_interpolate(
    samples: &[f32],
    width: usize,
    height: usize,
    layout: &Layout,
    border: usize,
    out: &mut [f32],
) {
    const WEIGHT: [[f32; 3]; 3] = [[0.25, 0.5, 0.25], [0.5, 0.0, 0.5], [0.25, 0.5, 0.25]];
    out.par_chunks_mut(width * 3)
        .enumerate()
        .for_each(|(row, line)| {
            let inner_row = row >= border && row + border < height;
            for col in 0..width {
                if inner_row && col >= border && col + border < width {
                    continue;
                }
                let mut sum = [0.0f32; 3];
                let mut weight = [0.0f32; 3];
                for y in row.saturating_sub(1)..=(row + 1).min(height - 1) {
                    for x in col.saturating_sub(1)..=(col + 1).min(width - 1) {
                        let w = WEIGHT[y + 1 - row][x + 1 - col];
                        let f = layout.fcol(y, x);
                        sum[f] += samples[y * width + x] * w;
                        weight[f] += w;
                    }
                }
                let own = layout.fcol(row, col);
                let value = samples[row * width + col];
                let px = &mut line[col * 3..col * 3 + 3];
                for c in 0..3 {
                    px[c] = if c == own {
                        value
                    } else if weight[c] > 0.0 {
                        sum[c] / weight[c]
                    } else {
                        wider_mean(samples, width, height, layout, row, col, c).unwrap_or(value)
                    };
                }
            }
        });
}

/// The plain mean of color `c` in the 5x5 round (`row`, `col`), for a
/// border site whose 3x3 holds none of it.
fn wider_mean(
    samples: &[f32],
    width: usize,
    height: usize,
    layout: &Layout,
    row: usize,
    col: usize,
    c: usize,
) -> Option<f32> {
    let mut sum = 0.0f32;
    let mut n = 0u32;
    for y in row.saturating_sub(2)..=(row + 2).min(height - 1) {
        for x in col.saturating_sub(2)..=(col + 2).min(width - 1) {
            if layout.fcol(y, x) == c {
                sum += samples[y * width + x];
                n += 1;
            }
        }
    }
    (n > 0).then(|| sum / n as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::CfaColor;

    fn flat(width: usize, height: usize, pattern: &CfaPattern, rgb: [f32; 3]) -> Vec<f32> {
        (0..width * height)
            .map(|i| rgb[pattern.color_at(i / width, i % width).rgb_index().unwrap()])
            .collect()
    }

    /// `f(y, x)` of every channel, mosaicked.
    fn mosaic(
        width: usize,
        height: usize,
        pattern: &CfaPattern,
        f: impl Fn(usize, usize, usize) -> f32,
    ) -> Vec<f32> {
        (0..width * height)
            .map(|i| {
                let (y, x) = (i / width, i % width);
                f(y, x, pattern.color_at(y, x).rgb_index().unwrap())
            })
            .collect()
    }

    const BOTH: [Passes; 2] = [Passes::One, Passes::Three];

    #[test]
    fn recognizes_x_trans_at_every_phase_and_nothing_else() {
        for dy in 0..6 {
            for dx in 0..6 {
                assert!(
                    CfaPattern::xtrans().shifted(dx, dy).is_xtrans(),
                    "{dx},{dy}"
                );
            }
        }
        assert!(!CfaPattern::rggb().is_xtrans());
        assert!(
            !CfaPattern::new(6, 6, vec![CfaColor::Green; 36])
                .unwrap()
                .is_xtrans()
        );
        // One site changed.
        let mut odd = CfaPattern::xtrans();
        odd.colors[7] = CfaColor::Red;
        assert!(!odd.is_xtrans());
        assert!(demosaic_markesteijn(&[0.0; 4], 2, 2, &CfaPattern::rggb(), Passes::Three).is_err());
    }

    /// The layout of the canonical phase: the solitary green is the
    /// 3x3 block's center, and its rows of two greens are the outer two.
    #[test]
    fn the_layout_finds_the_solitary_green() {
        let layout = Layout::new(&CfaPattern::xtrans(), 64);
        assert_eq!((layout.sgrow, layout.sgcol), (1, 1));
        assert_eq!(layout.right_shift, [true, false, true]);
        for dy in 0..6 {
            for dx in 0..6 {
                let l = Layout::new(&CfaPattern::xtrans().shifted(dx, dy), 64);
                assert_eq!(
                    ((l.sgrow + dy) % 3, (l.sgcol + dx) % 3),
                    (1, 1),
                    "{dx},{dy}"
                );
            }
        }
    }

    #[test]
    fn a_flat_field_stays_flat_at_every_phase() {
        let want = [0.5, 1.0, 0.25];
        for passes in BOTH {
            for dy in 0..6 {
                for dx in 0..6 {
                    let pattern = CfaPattern::xtrans().shifted(dx, dy);
                    let (w, h) = (150, 130);
                    let cfa = flat(w, h, &pattern, want);
                    let out = demosaic_markesteijn(&cfa, w, h, &pattern, passes).unwrap();
                    for (i, px) in out.as_chunks::<3>().0.iter().enumerate() {
                        for c in 0..3 {
                            assert!(
                                (px[c] - want[c]).abs() < 1e-5,
                                "{passes:?} at {dx},{dy}, pixel {} {}: {px:?}",
                                i % w,
                                i / w
                            );
                        }
                    }
                }
            }
        }
    }

    /// A picture mosaicked at one phase and demosaiced is the same
    /// picture, moved, as the picture moved and mosaicked at the shifted
    /// phase: the algorithm reads its pattern from the crop's origin.
    ///
    /// On a frame of one tile, away from its edges, the two agree. Where
    /// two tiles meet the answer depends a little on where the tiles
    /// fall on the picture, as the reference's does: a tile computes red
    /// and blue from its third column in, the 2x2 greens two columns
    /// further in read what was left at zero, and through the
    /// derivatives and the 5x5 homogeneity sums that reaches the eighth
    /// column, the first a tile writes. It moves only which directions
    /// are averaged there; on this scene, which swings 0.6 over thirty
    /// pixels, by under 0.01.
    #[test]
    fn a_shifted_pattern_gives_the_shifted_picture() {
        let scene = |y: usize, x: usize, c: usize| {
            let (y, x) = (y as f32, x as f32);
            let base = 0.4 + 0.3 * (x * 0.21 + y * 0.13).sin() * (y * 0.07).cos();
            base * [0.8, 1.0, 0.6][c] + 0.05 * c as f32
        };
        for ((w, h), tolerance) in [((112, 106), 1e-5), ((170, 150), 1e-2)] {
            for passes in BOTH {
                let pattern = CfaPattern::xtrans();
                let whole =
                    demosaic_markesteijn(&mosaic(w, h, &pattern, scene), w, h, &pattern, passes)
                        .unwrap();
                for dy in 0..6 {
                    for dx in 0..6 {
                        let shifted = pattern.shifted(dx, dy);
                        let (sw, sh) = (w - dx, h - dy);
                        let cfa = mosaic(sw, sh, &shifted, |y, x, c| scene(y + dy, x + dx, c));
                        let out = demosaic_markesteijn(&cfa, sw, sh, &shifted, passes).unwrap();
                        let mut worst = 0.0f32;
                        for y in 20..sh - 20 {
                            for x in 20..sw - 20 {
                                for c in 0..3 {
                                    let a = out[(y * sw + x) * 3 + c];
                                    let b = whole[((y + dy) * w + x + dx) * 3 + c];
                                    worst = worst.max((a - b).abs());
                                }
                            }
                        }
                        assert!(
                            worst < tolerance,
                            "{w}x{h} {passes:?} shifted {dx},{dy}: off by {worst}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_smooth_gradient_comes_back() {
        let (w, h) = (200, 150);
        let scene = |y: usize, x: usize, c: usize| {
            0.1 + 0.6 * x as f32 / w as f32 + 0.2 * y as f32 / h as f32 + 0.05 * c as f32
        };
        for passes in BOTH {
            for (dx, dy) in [(0, 0), (2, 5), (4, 1)] {
                let pattern = CfaPattern::xtrans().shifted(dx, dy);
                let out =
                    demosaic_markesteijn(&mosaic(w, h, &pattern, scene), w, h, &pattern, passes)
                        .unwrap();
                let border = passes.border();
                for y in border..h - border {
                    for x in border..w - border {
                        for c in 0..3 {
                            let (got, want) = (out[(y * w + x) * 3 + c], scene(y, x, c));
                            assert!(
                                (got - want).abs() < 2e-3,
                                "{passes:?} at {dx},{dy}: ({x},{y}) channel {c} {got} want {want}"
                            );
                        }
                    }
                }
                // The border is the weighted mean: on a gradient, within
                // a step of the slope.
                for y in [0, h - 1] {
                    for x in 0..w {
                        for c in 0..3 {
                            let (got, want) = (out[(y * w + x) * 3 + c], scene(y, x, c));
                            assert!((got - want).abs() < 0.02, "border ({x},{y}) {got} {want}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn edges_are_finite_and_in_range() {
        let (w, h) = (97, 61);
        let pattern = CfaPattern::xtrans().shifted(5, 3);
        // A hard edge and a bright square on a dark field.
        let cfa = mosaic(w, h, &pattern, |y, x, c| {
            let v = if x > 40 { 0.9 } else { 0.05 };
            if (20..30).contains(&y) && (60..70).contains(&x) {
                [1.6, 1.0, 1.2][c]
            } else {
                v * [1.0, 0.8, 0.5][c]
            }
        });
        for passes in BOTH {
            let out = demosaic_markesteijn(&cfa, w, h, &pattern, passes).unwrap();
            assert!(out.iter().all(|v| v.is_finite() && (0.0..=1.6).contains(v)));
        }
    }

    #[test]
    fn highlights_above_one_survive() {
        let pattern = CfaPattern::xtrans();
        let cfa = flat(64, 64, &pattern, [2.0, 1.0, 1.5]);
        let out = demosaic_markesteijn(&cfa, 64, 64, &pattern, Passes::Three).unwrap();
        let px = &out[(32 * 64 + 32) * 3..][..3];
        assert!(
            (px[0] - 2.0).abs() < 1e-4 && (px[2] - 1.5).abs() < 1e-4,
            "{px:?}"
        );
    }

    #[test]
    fn tiny_images_do_not_panic_and_stay_flat() {
        let want = [0.5, 1.0, 0.25];
        for (w, h) in [
            (1, 1),
            (2, 3),
            (5, 5),
            (6, 6),
            (7, 31),
            (31, 40),
            (32, 32),
            (40, 33),
        ] {
            for (dx, dy) in (0..6).flat_map(|dy| (0..6).map(move |dx| (dx, dy))) {
                let pattern = CfaPattern::xtrans().shifted(dx, dy);
                let cfa = flat(w, h, &pattern, want);
                for passes in BOTH {
                    let out = demosaic_markesteijn(&cfa, w, h, &pattern, passes).unwrap();
                    assert_eq!(out.len(), w * h * 3);
                    for px in out.as_chunks::<3>().0 {
                        let own_only = w < 3 || h < 3;
                        for c in 0..3 {
                            assert!(
                                own_only || (px[c] - want[c]).abs() < 1e-5,
                                "{w}x{h} at {dx},{dy} {passes:?}: {px:?}"
                            );
                        }
                        assert!(px.iter().all(|v| v.is_finite()));
                    }
                }
            }
        }
    }

    include!("xtrans_golden.rs");

    /// The scene the golden values were made from: ramps folded into
    /// steps, a hard edge at column 120, every value an exact binary
    /// fraction so the mosaic is the same bits wherever it is built.
    fn golden_scene() -> (usize, usize, CfaPattern, Vec<f32>) {
        let (w, h) = (170, 150);
        let pattern = CfaPattern::xtrans().shifted(2, 5);
        let samples = (0..w * h)
            .map(|i| {
                let (y, x) = (i / w, i % w);
                let c = pattern.color_at(y, x).rgb_index().unwrap();
                let mut n = ((x * x + 3 * y * y) / 7 + (x * y) / 5 + 97 * c) % 1024;
                if x > 120 {
                    n = (n + 300) % 1024;
                }
                n as f32 / 1024.0
            })
            .collect();
        (w, h, pattern, samples)
    }

    /// The port against RawTherapee's own code, run on the golden scene
    /// (four tiles, a pattern off its origin) with the homogeneity maps
    /// zeroed as here: sampled sites, the tiles' seams and the frame's
    /// border among them, and two sums over every site but the frame's
    /// corners, where the border's fallback departs on purpose.
    #[test]
    fn the_port_matches_rawtherapee_on_the_golden_scene() {
        let (w, h, pattern, samples) = golden_scene();
        let corner = |x: usize, y: usize| (x < 2 || x >= w - 2) && (y < 2 || y >= h - 2);
        for (passes, golden, sums) in [
            (Passes::One, &GOLDEN_ONE, GOLDEN_ONE_SUMS),
            (Passes::Three, &GOLDEN_THREE, GOLDEN_THREE_SUMS),
        ] {
            let out = demosaic_markesteijn(&samples, w, h, &pattern, passes).unwrap();
            for (&(x, y), want) in GOLDEN_POINTS.iter().zip(golden) {
                let got = &out[(y * w + x) * 3..][..3];
                for c in 0..3 {
                    assert!(
                        (got[c] - want[c]).abs() < 1e-6,
                        "{passes:?} ({x},{y}) channel {c}: {} against {}",
                        got[c],
                        want[c]
                    );
                }
            }
            let (mut s1, mut s2) = (0.0f64, 0.0f64);
            for (i, px) in out.as_chunks::<3>().0.iter().enumerate() {
                if corner(i % w, i / w) {
                    continue;
                }
                for (c, &v) in px.iter().enumerate() {
                    s1 += f64::from(v);
                    s2 += f64::from(v) * ((i * 3 + c) % 97) as f64;
                }
            }
            assert!(
                (s1 - sums.0).abs() < 1e-6 * sums.0,
                "{passes:?} sum {s1} against {}",
                sums.0
            );
            assert!(
                (s2 - sums.1).abs() < 1e-6 * sums.1,
                "{passes:?} weighted {s2} against {}",
                sums.1
            );
        }
    }
}
