//! Where the input was clipped, and how: the regime of every pixel, the
//! clipped regions and their sizes, the band round each region's edge.
//!
//! The unit is the cell: a 2x2 quad of a Bayer mosaic, a 3x3 block of
//! an X-Trans one, each holding every color. A cell's regime is how many
//! of its colors have a clipped photosite: one (usually green, or red
//! under warm light), two, or all three, where nothing of the scene is
//! left to reconstruct from. Every pixel takes its cell's regime. A
//! region is a 4-connected run of cells with anything clipped; its size
//! in pixels sorts it as a small specular, a medium patch, or a large
//! area.
//!
//! What is not scored: cells with a photosite the original frame itself
//! had near its white (the truth is not truth there), two cells round
//! them, and a border of [`BORDER`] pixels.

use rayon::prelude::*;

use greycard_core::raw::CfaPattern;

/// Pixels left out at every edge.
pub const BORDER: usize = 8;
/// A pixel's regime when it is not scored.
pub const INVALID: u8 = 255;
/// The largest region, in pixels, that is a small specular: 16x16.
pub const SMALL_MAX: usize = 256;
/// The largest region that is a medium patch: 128x128.
pub const MEDIUM_MAX: usize = 16_384;
/// The width of the band either side of a region's edge, in pixels.
pub const RING: u8 = 3;
/// Cells round an unreliable cell that are left out with it.
const INVALID_REACH: usize = 2;

/// A region's size class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SizeBin {
    Small,
    Medium,
    Large,
}

impl SizeBin {
    pub const ALL: [SizeBin; 3] = [SizeBin::Small, SizeBin::Medium, SizeBin::Large];

    pub fn of(pixels: usize) -> Self {
        if pixels <= SMALL_MAX {
            SizeBin::Small
        } else if pixels <= MEDIUM_MAX {
            SizeBin::Medium
        } else {
            SizeBin::Large
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SizeBin::Small => "small",
            SizeBin::Medium => "medium",
            SizeBin::Large => "large",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// One clipped region.
#[derive(Debug, Clone, Copy)]
pub struct Region {
    pub pixels: usize,
    /// Pixel bounds, inclusive start, exclusive end: x0, y0, x1, y1.
    pub bounds: [usize; 4],
    pub bin: SizeBin,
}

/// Which band of a region's edge a pixel is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    None,
    /// Unclipped, within [`RING`] pixels of a clipped cell.
    Outside,
    /// Clipped, within [`RING`] pixels of an unclipped cell.
    Inside,
}

pub struct ClipMap {
    pub width: usize,
    pub height: usize,
    /// Cell side: 2 for Bayer, 3 for X-Trans.
    pub cell: usize,
    pub cells_wide: usize,
    pub cells_high: usize,
    /// Per photosite: the input was clipped there.
    pub clipped: Vec<bool>,
    /// Per cell: the colors with a clipped photosite, bit `c` for color `c`.
    pub cell_colors: Vec<u8>,
    /// Per cell: left out of every score.
    pub cell_invalid: Vec<bool>,
    /// Per cell: its region's index plus one, or 0.
    pub cell_region: Vec<u32>,
    pub regions: Vec<Region>,
    /// Per pixel: 0 unclipped, 1 to 3 the regime, [`INVALID`] not scored.
    pub regime: Vec<u8>,
    /// Per pixel: its band at a region's edge.
    pub band: Vec<Band>,
}

impl ClipMap {
    /// Map a mosaic: `clipped(i)` says the input's photosite `i` was
    /// clipped, `invalid(i)` that its truth cannot be trusted.
    pub fn build(
        width: usize,
        height: usize,
        pattern: &CfaPattern,
        clipped: impl Fn(usize) -> bool + Sync,
        invalid: impl Fn(usize) -> bool + Sync,
    ) -> Self {
        let cell = if pattern.width == 2 { 2 } else { 3 };
        let cw = width.div_ceil(cell);
        let ch = height.div_ceil(cell);
        let clipped: Vec<bool> = (0..width * height).into_par_iter().map(&clipped).collect();

        // Cells: the colors clipped in each, and whether any photosite
        // is unreliable.
        let per_row: Vec<(Vec<u8>, Vec<bool>)> = (0..ch)
            .into_par_iter()
            .map(|cy| {
                let mut colors = vec![0u8; cw];
                let mut bad = vec![false; cw];
                for y in cy * cell..((cy + 1) * cell).min(height) {
                    for x in 0..width {
                        let i = y * width + x;
                        let k = x / cell;
                        if clipped[i] {
                            let c = pattern.color_at(y, x).rgb_index().expect("an RGB pattern");
                            colors[k] |= 1 << c;
                        }
                        if invalid(i) {
                            bad[k] = true;
                        }
                    }
                }
                (colors, bad)
            })
            .collect();
        let mut cell_colors = Vec::with_capacity(cw * ch);
        let mut seed_bad = Vec::with_capacity(cw * ch);
        for (c, b) in per_row {
            cell_colors.extend(c);
            seed_bad.extend(b);
        }
        let cell_invalid = dilate(&seed_bad, cw, ch, INVALID_REACH);

        let (cell_region, regions) = label_regions(&cell_colors, cw, ch, cell, width, height);

        let regime: Vec<u8> = (0..width * height)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % width, i / width);
                if x < BORDER || y < BORDER || x + BORDER >= width || y + BORDER >= height {
                    return INVALID;
                }
                let k = (y / cell) * cw + x / cell;
                if cell_invalid[k] {
                    INVALID
                } else {
                    cell_colors[k].count_ones() as u8
                }
            })
            .collect();

        // The bands: distance from each pixel to the nearest pixel of the
        // other side, in pixels (chessboard), capped past the ring.
        let any = |i: usize| {
            let (x, y) = (i % width, i / width);
            cell_colors[(y / cell) * cw + x / cell] != 0
        };
        let to_clipped = chessboard(width, height, any);
        let to_unclipped = chessboard(width, height, |i| !any(i));
        let band = (0..width * height)
            .into_par_iter()
            .map(|i| {
                if regime[i] == INVALID {
                    Band::None
                } else if regime[i] == 0 && to_clipped[i] <= RING {
                    Band::Outside
                } else if regime[i] > 0 && to_unclipped[i] <= RING {
                    Band::Inside
                } else {
                    Band::None
                }
            })
            .collect();

        Self {
            width,
            height,
            cell,
            cells_wide: cw,
            cells_high: ch,
            clipped,
            cell_colors,
            cell_invalid,
            cell_region,
            regions,
            regime,
            band,
        }
    }

    /// The cell of pixel `i`.
    #[inline]
    pub fn cell_of(&self, i: usize) -> usize {
        let (x, y) = (i % self.width, i / self.width);
        (y / self.cell) * self.cells_wide + x / self.cell
    }

    /// The size class of pixel `i`'s region, if it has one.
    #[inline]
    pub fn bin(&self, i: usize) -> Option<SizeBin> {
        let r = self.cell_region[self.cell_of(i)];
        (r > 0).then(|| self.regions[r as usize - 1].bin)
    }

    /// Scored pixels per regime (1 to 3) and size class.
    pub fn counts(&self) -> [[usize; 3]; 3] {
        let mut out = [[0usize; 3]; 3];
        for i in 0..self.regime.len() {
            let r = self.regime[i];
            if (1..=3).contains(&r)
                && let Some(b) = self.bin(i)
            {
                out[r as usize - 1][b.index()] += 1;
            }
        }
        out
    }

    /// Scored clipped photosites.
    pub fn scored_clipped(&self) -> usize {
        (0..self.clipped.len())
            .filter(|&i| self.clipped[i] && (1..=3).contains(&self.regime[i]))
            .count()
    }
}

/// Grow a cell mask by `reach` cells in every direction.
fn dilate(mask: &[bool], w: usize, h: usize, reach: usize) -> Vec<bool> {
    let rows: Vec<bool> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            (x.saturating_sub(reach)..(x + reach + 1).min(w)).any(|xx| mask[y * w + xx])
        })
        .collect();
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            (y.saturating_sub(reach)..(y + reach + 1).min(h)).any(|yy| rows[yy * w + x])
        })
        .collect()
}

/// 4-connected regions of cells with anything clipped.
fn label_regions(
    colors: &[u8],
    cw: usize,
    ch: usize,
    cell: usize,
    width: usize,
    height: usize,
) -> (Vec<u32>, Vec<Region>) {
    let mut label = vec![0u32; cw * ch];
    let mut regions = Vec::new();
    let mut stack = Vec::new();
    for start in 0..cw * ch {
        if colors[start] == 0 || label[start] != 0 {
            continue;
        }
        let id = regions.len() as u32 + 1;
        label[start] = id;
        stack.push(start);
        let mut pixels = 0usize;
        let mut bounds = [usize::MAX, usize::MAX, 0, 0];
        while let Some(k) = stack.pop() {
            let (cx, cy) = (k % cw, k / cw);
            let (x0, y0) = (cx * cell, cy * cell);
            let (x1, y1) = ((x0 + cell).min(width), (y0 + cell).min(height));
            pixels += (x1 - x0) * (y1 - y0);
            bounds = [
                bounds[0].min(x0),
                bounds[1].min(y0),
                bounds[2].max(x1),
                bounds[3].max(y1),
            ];
            let mut visit = |n: usize| {
                if colors[n] != 0 && label[n] == 0 {
                    label[n] = id;
                    stack.push(n);
                }
            };
            if cx > 0 {
                visit(k - 1);
            }
            if cx + 1 < cw {
                visit(k + 1);
            }
            if cy > 0 {
                visit(k - cw);
            }
            if cy + 1 < ch {
                visit(k + cw);
            }
        }
        regions.push(Region {
            pixels,
            bounds,
            bin: SizeBin::of(pixels),
        });
    }
    (label, regions)
}

/// Chessboard distance from every pixel to the nearest where `inside`
/// holds, capped at [`RING`] + 1 (all that is asked of it).
fn chessboard(width: usize, height: usize, inside: impl Fn(usize) -> bool + Sync) -> Vec<u8> {
    let cap = RING + 1;
    let mut d: Vec<u8> = (0..width * height)
        .into_par_iter()
        .map(|i| if inside(i) { 0 } else { cap })
        .collect();
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            let mut v = d[i];
            if v == 0 {
                continue;
            }
            if x > 0 {
                v = v.min(d[i - 1] + 1);
            }
            if y > 0 {
                v = v.min(d[i - width] + 1);
                if x > 0 {
                    v = v.min(d[i - width - 1] + 1);
                }
                if x + 1 < width {
                    v = v.min(d[i - width + 1] + 1);
                }
            }
            d[i] = v.min(cap);
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let i = y * width + x;
            let mut v = d[i];
            if v == 0 {
                continue;
            }
            if x + 1 < width {
                v = v.min(d[i + 1] + 1);
            }
            if y + 1 < height {
                v = v.min(d[i + width] + 1);
                if x + 1 < width {
                    v = v.min(d[i + width + 1] + 1);
                }
                if x > 0 {
                    v = v.min(d[i + width - 1] + 1);
                }
            }
            d[i] = v.min(cap);
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::develop::clipcodes::clip_codes;
    use greycard_core::develop::highlights::CLIP_MAGIC;

    /// A 40x40 RGGB mosaic with one 6x6 patch where green alone clipped
    /// and one 4x4 where everything did, and one unreliable photosite.
    #[test]
    fn regimes_regions_and_bands_follow_the_clipping() {
        let (w, h) = (40usize, 40usize);
        let p = CfaPattern::rggb();
        let green_patch = |x: usize, y: usize| (10..16).contains(&x) && (10..16).contains(&y);
        let full_patch = |x: usize, y: usize| (24..28).contains(&x) && (24..28).contains(&y);
        let map = ClipMap::build(
            w,
            h,
            &p,
            |i| {
                let (x, y) = (i % w, i / w);
                let green = p.color_at(y, x).rgb_index() == Some(1);
                (green_patch(x, y) && green) || full_patch(x, y)
            },
            |i| i == 30 * w + 9,
        );
        assert_eq!(map.cell, 2);
        assert_eq!(map.regions.len(), 2);
        assert!(map.regions.iter().all(|r| r.bin == SizeBin::Small));
        assert_eq!(map.regime[12 * w + 12], 1);
        assert_eq!(map.regime[25 * w + 25], 3);
        assert_eq!(map.regime[20 * w + 20], 0);
        // The edge is left out, and so is the unreliable cell's
        // neighborhood.
        assert_eq!(map.regime[2 * w + 20], INVALID);
        assert_eq!(map.regime[30 * w + 12], INVALID);
        assert_eq!(map.band[12 * w + 8], Band::Outside);
        assert_eq!(map.band[12 * w + 10], Band::Inside);
        assert_eq!(map.band[20 * w + 20], Band::None);
        let counts = map.counts();
        assert_eq!(counts[0][0], 36);
        assert_eq!(counts[2][0], 16);
        assert_eq!(map.scored_clipped(), 18 + 16);
    }

    /// A mosaic of `w` by `h` normalized levels (white 1, saturation
    /// `headroom`), noise from a fixed seed, about a quarter of it at or
    /// past the clip, none within a hair of it.
    fn levels(w: usize, h: usize, headroom: f32, seed: u64) -> Vec<f32> {
        let mut state = seed;
        (0..w * h)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let u = (state >> 40) as f32 / (1u64 << 24) as f32;
                let mut n = u * 1.3 * headroom;
                if (n / headroom - CLIP_MAGIC).abs() < 1e-4 {
                    n += 2e-4 * headroom;
                }
                n.min(headroom)
            })
            .collect()
    }

    /// Core's codes are this map's: on Bayer at its four phases and
    /// X-Trans at several, at sizes no cell divides, with and without
    /// headroom, every cell's code is the count of its clipped colors.
    /// The map is built as the trial's data builder builds it, a
    /// normalized level over the headroom at or past the magic; core
    /// reads the white-balanced samples at opposed's clips.
    #[test]
    fn core_clip_codes_are_the_maps_cell_counts() {
        let gains = [2.1f32, 1.0, 1.6];
        let mut patterns: Vec<CfaPattern> = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .into_iter()
            .map(|(dx, dy)| CfaPattern::rggb().shifted(dx, dy))
            .collect();
        patterns.extend(
            [(0, 0), (2, 1), (5, 3)]
                .into_iter()
                .map(|(dx, dy)| CfaPattern::xtrans().shifted(dx, dy)),
        );
        let mut seed = 1;
        for p in &patterns {
            for (w, h) in [(37, 23), (36, 24), (1, 1), (4, 7)] {
                for headroom in [1.0f32, 1.1231] {
                    seed += 1;
                    let n = levels(w, h, headroom, seed);
                    let color = |i: usize| p.color_at(i / w, i % w).rgb_index().unwrap();
                    let samples: Vec<f32> = (0..w * h).map(|i| n[i] * gains[color(i)]).collect();
                    let map = ClipMap::build(w, h, p, |i| n[i] / headroom >= CLIP_MAGIC, |_| false);
                    let codes = clip_codes(&samples, w, h, p, gains, headroom);
                    assert_eq!(codes.cell, map.cell);
                    assert_eq!(
                        (codes.cells_wide, codes.cells_high),
                        (map.cells_wide, map.cells_high)
                    );
                    let counts: Vec<u8> = map
                        .cell_colors
                        .iter()
                        .map(|c| c.count_ones() as u8)
                        .collect();
                    assert_eq!(codes.codes, counts, "{p} {w}x{h} headroom {headroom}");
                    assert!((0..w * h).all(|i| codes.at(i % w, i / w) == counts[map.cell_of(i)]));
                }
            }
        }
    }

    /// At the threshold itself the two agree when the map reads the
    /// samples as opposed does: a photosite exactly at its clip, a step
    /// under, and a step over.
    #[test]
    fn core_clip_codes_agree_with_the_map_at_the_threshold() {
        let gains = [2.1f32, 1.0, 1.6];
        let p = CfaPattern::rggb();
        let (w, h) = (6, 2);
        for headroom in [1.0f32, 1.1231] {
            let clip = gains.map(|g| g * headroom * CLIP_MAGIC);
            // Cells: red at, under, and over its clip.
            let mut samples = vec![0.0f32; w * h];
            samples[0] = clip[0];
            samples[2] = clip[0].next_down();
            samples[4] = clip[0].next_up();
            let color = |i: usize| p.color_at(i / w, i % w).rgb_index().unwrap();
            let map = ClipMap::build(w, h, &p, |i| samples[i] >= clip[color(i)], |_| false);
            let codes = clip_codes(&samples, w, h, &p, gains, headroom);
            let counts: Vec<u8> = map
                .cell_colors
                .iter()
                .map(|c| c.count_ones() as u8)
                .collect();
            assert_eq!(codes.codes, counts);
            assert_eq!(codes.codes, vec![1, 0, 1]);
        }
    }

    #[test]
    fn size_classes_split_at_their_limits() {
        assert_eq!(SizeBin::of(256), SizeBin::Small);
        assert_eq!(SizeBin::of(257), SizeBin::Medium);
        assert_eq!(SizeBin::of(16_385), SizeBin::Large);
    }
}
