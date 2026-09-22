//! Segment-based highlight reconstruction: a second pass after
//! [`super::highlights`] ("inpaint opposed").
//!
//! Ported from darktable's `src/iop/hlreconstruct/segbased.c` and
//! `segmentation.c` (copyright 2022-2026 darktable developers,
//! GPL-3.0-or-later), version 2 of the algorithm by Hanno Schwalm with
//! Iain and garagecoder of the G'MIC team. The plane reduction, the
//! segmentation with its morphology and flood fill, the candidate
//! weighting and the correction are theirs; the buffers and the threading
//! are ours.
//!
//! The opposed pass fills every clipped photosite from its neighborhood
//! plus one chrominance offset for the whole frame. This pass works per
//! clipped region instead. Each color plane is reduced to 3x3 blocks in
//! cube-root space, the clipped blocks are joined into segments (a
//! dilation to bridge small gaps, then a flood fill that also collects
//! the unclipped blocks on the border), and each segment looks for the
//! smoothest unclipped spot among its own blocks. The difference between
//! that spot's reading and its opposed average is the segment's
//! chrominance, applied to every clipped photosite of the segment in
//! place of the global offset. Segments without a convincing spot keep
//! the opposed result.
//!
//! Not ported: the "rebuild" modes, which invent luminance for regions
//! where all three channels are clipped from the border gradients and a
//! distance transform, off by default in darktable; and the mask views.
//! One quirk is kept as found: the reference's candidate weight includes
//! a factor that is always 1, so the weight is the smoothness alone.

use rayon::prelude::*;

use super::highlights::opposed_root;
use crate::error::{Error, Result};
use crate::raw::{CfaColor, CfaPattern};

/// How the segments are formed and judged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentOptions {
    /// Radius of the dilation that joins nearby clipped blocks into one
    /// segment, 0 to 8 blocks; above 3 an erosion by the excess follows,
    /// making it a closing. darktable's "combine".
    pub combine: usize,
    /// How poor a candidate spot may be and still be used, 0 to 1: a
    /// spot's weight must exceed one minus this. darktable's
    /// "candidating".
    pub candidating: f32,
}

impl Default for SegmentOptions {
    fn default() -> Self {
        Self {
            combine: 2,
            candidating: 0.4,
        }
    }
}

/// What the pass found and did.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SegmentStats {
    /// Segments found per color plane.
    pub segments: [usize; 3],
    /// Segments that found a candidate spot, per color plane.
    pub candidates: [usize; 3],
    /// Photosites whose opposed value was replaced.
    pub replaced: usize,
}

/// Blocks of unused plane around the picture, so the morphology and the
/// candidate windows never leave the buffer.
const BORDER: usize = 8;
/// Flag on a segment id marking an unclipped block on the segment's
/// border, collected by the flood fill.
const ID_MASK: u32 = 0x40000;
/// Fewer clipped blocks than this and there is nothing to segment.
const MIN_CLIPPED: usize = 20;
/// Segments allowed per plane: 250 per megapixel, at least 256.
const PIXELS_PER_SEGMENT: usize = 4000;
/// Segments with fewer blocks than this are not segments.
const MIN_SEGMENT_BLOCKS: usize = 4;

/// Refine `opposed` (the output of [`super::highlights::inpaint_opposed`]
/// for `original`) segment by segment. Both are the white-balanced
/// mosaic; `clips` is the level at which each channel is treated as
/// saturated, as for the opposed pass. Returns the new mosaic and what
/// was measured.
pub fn inpaint_segments(
    original: &[f32],
    opposed: &[f32],
    width: usize,
    height: usize,
    pattern: &CfaPattern,
    clips: [f32; 3],
    options: &SegmentOptions,
) -> Result<(Vec<f32>, SegmentStats)> {
    if !pattern.is_rgb() {
        return Err(Error::Unsupported(format!(
            "highlight reconstruction needs an RGB pattern, got {pattern:?}"
        )));
    }
    if original.len() != width * height || opposed.len() != width * height {
        return Err(Error::Unsupported(format!(
            "{width}x{height} mosaic with {} and {} samples",
            original.len(),
            opposed.len()
        )));
    }
    let mut out = opposed.to_vec();
    let mut stats = SegmentStats::default();
    if width < 12 || height < 12 {
        return Ok((out, stats));
    }
    let color = |x: usize, y: usize| -> usize {
        pattern
            .color_at(y, x)
            .rgb_index()
            .expect("RGB pattern checked above")
    };

    // The planes: one value per 3x3 block, the block centered on a green
    // photosite so every block holds five greens and two of each other.
    let round2 = |n: usize| n.div_ceil(2) * 2;
    let pw = round2(width / 3) + 2 * BORDER;
    let ph = round2(height / 3) + 2 * BORDER;
    let plane_index = |row: usize, col: usize| (BORDER + row / 3) * pw + col / 3 + BORDER;
    let xshift = if pattern.color_at(0, 0) == CfaColor::Green {
        1
    } else {
        2
    };
    let cube_clips = clips.map(f32::cbrt);
    let limit = ((width * height) / PIXELS_PER_SEGMENT).max(256);

    let mut plane = vec![vec![0.0f32; pw * ph]; 3];
    let mut refavg = vec![vec![0.0f32; pw * ph]; 3];
    let mut segs: Vec<Segmentation> = (0..3)
        .map(|_| Segmentation::new(pw, ph, BORDER + 1, limit))
        .collect();
    let mut any_clipped = 0usize;
    for row in (1..height - 1).filter(|r| r % 3 == 1) {
        for col in (1..width - 1).filter(|c| c % 3 == xshift) {
            let mut sum = [0.0f32; 3];
            let mut cnt = [0u32; 3];
            for dy in row - 1..row + 2 {
                for dx in col - 1..col + 2 {
                    let c = color(dx, dy);
                    sum[c] += opposed[dy * width + dx];
                    cnt[c] += 1;
                }
            }
            let mean: [f32; 3] = std::array::from_fn(|c| {
                if cnt[c] > 0 {
                    (sum[c] / cnt[c] as f32).cbrt()
                } else {
                    0.0
                }
            });
            let o = plane_index(row, col);
            for c in 0..3 {
                plane[c][o] = mean[c];
                refavg[c][o] = 0.5 * (mean[(c + 1) % 3] + mean[(c + 2) % 3]);
                if mean[c] > cube_clips[c] {
                    segs[c].data[o] = 1;
                    any_clipped += 1;
                }
            }
        }
    }
    if any_clipped < MIN_CLIPPED {
        return Ok((out, stats));
    }
    for p in plane.iter_mut() {
        extend_border(p, pw, ph, BORDER);
    }

    // Segments per plane, then each segment's candidate.
    segs.par_iter_mut()
        .zip(plane.par_iter())
        .zip(refavg.par_iter())
        .enumerate()
        .for_each(|(c, ((seg, plane), refavg))| {
            seg.combine(options.combine);
            seg.segmentize();
            seg.find_candidates(plane, refavg, cube_clips[c], options.candidating);
        });
    for (c, seg) in segs.iter().enumerate() {
        stats.segments[c] = seg.nr as usize - 2;
        stats.candidates[c] = (2..seg.nr)
            .filter(|&id| seg.val1[id as usize] != 0.0)
            .count();
    }

    // Every clipped photosite in a segment with a candidate: the opposed
    // average here plus the segment's own offset, in cube-root space.
    let replaced: usize = out
        .par_chunks_mut(width)
        .enumerate()
        .map(|(row, orow)| {
            if row == 0 || row == height - 1 {
                return 0;
            }
            let mut replaced = 0;
            for col in 1..width - 1 {
                let c = color(col, row);
                let inval = original[row * width + col].max(0.0);
                if inval <= clips[c] {
                    continue;
                }
                let o = plane_index(row, col);
                let pid = segs[c].id_at(o) as usize;
                if pid == 0 {
                    continue;
                }
                let candidate = segs[c].val1[pid];
                if candidate == 0.0 {
                    continue;
                }
                let reference = segs[c].val2[pid];
                let root = opposed_root(original, width, height, pattern, col, row, c);
                let v = root + candidate - reference;
                orow[col] = inval.max(v * v * v);
                replaced += 1;
            }
            replaced
        })
        .sum();
    stats.replaced = replaced;
    Ok((out, stats))
}

/// Copy the innermost real row and column outwards over the border.
fn extend_border(mask: &mut [f32], width: usize, height: usize, border: usize) {
    for row in border..height - border {
        let idx = row * width;
        for i in 0..border {
            mask[idx + i] = mask[idx + border];
            mask[idx + width - i - 1] = mask[idx + width - border - 1];
        }
    }
    for col in 0..width {
        let inner = col.clamp(border, width - border - 1);
        let top = mask[border * width + inner];
        let bottom = mask[(height - border - 1) * width + inner];
        for i in 0..border {
            mask[col + i * width] = top;
            mask[col + (height - i - 1) * width] = bottom;
        }
    }
}

/// Segments of one plane: `data` holds 0 for clear, 1 for clipped and
/// not yet visited, an id from 2 for a segment's blocks, and an id with
/// [`ID_MASK`] for an unclipped block on a segment's border.
struct Segmentation {
    width: usize,
    height: usize,
    border: usize,
    slots: usize,
    /// Next id to hand out; the count found is `nr - 2`.
    nr: u32,
    data: Vec<u32>,
    tmp: Vec<u32>,
    xmin: Vec<usize>,
    xmax: Vec<usize>,
    ymin: Vec<usize>,
    ymax: Vec<usize>,
    /// The candidate's 5x5 average, cube-root space; 0 for no candidate.
    val1: Vec<f32>,
    /// The opposed average at the candidate, cube-root space.
    val2: Vec<f32>,
}

impl Segmentation {
    fn new(width: usize, height: usize, border: usize, slots: usize) -> Self {
        let slots = slots.clamp(256, ID_MASK as usize - 2);
        Self {
            width,
            height,
            border,
            slots,
            nr: 2,
            data: vec![0; width * height],
            tmp: vec![0; width * height],
            xmin: vec![0; slots],
            xmax: vec![0; slots],
            ymin: vec![0; slots],
            ymax: vec![0; slots],
            val1: vec![0.0; slots],
            val2: vec![0.0; slots],
        }
    }

    /// The segment a block belongs to, border blocks included; 0 for none.
    #[inline]
    fn id_at(&self, loc: usize) -> u32 {
        if loc >= self.width * (self.height - self.border) {
            return 0;
        }
        let id = self.data[loc] & (ID_MASK - 1);
        if id > 1 && id < self.nr { id } else { 0 }
    }

    fn fill_border(data: &mut [u32], width: usize, height: usize, border: usize, value: u32) {
        let di = (height - border - 1) * width;
        for i in 0..border * width {
            data[i] = value;
            data[i + di] = value;
        }
        for row in border..height - border {
            let j = row * width;
            for i in 0..border {
                data[j + i] = value;
                data[j + i + width - border] = value;
            }
        }
    }

    /// Join nearby clipped blocks: dilate by `radius`, and above 3 erode
    /// by the excess.
    fn combine(&mut self, radius: usize) {
        let radius = radius.min(8);
        let (w, h, b) = (self.width, self.height, self.border);
        Self::fill_border(&mut self.data, w, h, b, 0);
        morph(&self.data, &mut self.tmp, w, h, b, radius, true);
        if radius > 3 {
            Self::fill_border(&mut self.tmp, w, h, b, 1);
            morph(&self.tmp, &mut self.data, w, h, b, radius - 3, false);
        } else {
            self.data.copy_from_slice(&self.tmp);
        }
        Self::fill_border(&mut self.data, w, h, b, 0);
    }

    /// Flood-fill every clipped block into a segment.
    fn segmentize(&mut self) {
        let mut stack = Vec::with_capacity(1024);
        let mut id = 2u32;
        'scan: for row in self.border..self.height - self.border {
            for col in self.border..self.width - self.border {
                if id as usize >= self.slots - 2 {
                    break 'scan;
                }
                if self.data[row * self.width + col] == 1 && self.flood(row, col, id, &mut stack) {
                    id += 1;
                }
            }
        }
    }

    /// darktable's scanline flood fill from one clipped block. Marks the
    /// segment's blocks with `id` and the unclipped blocks it touches
    /// with `id | ID_MASK`; segments of fewer than four blocks are undone.
    fn flood(&mut self, yin: usize, xin: usize, id: u32, stack: &mut Vec<(usize, usize)>) -> bool {
        let (w, h, border) = (self.width, self.height, self.border);
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (xin, xin, yin, yin);
        let mut count = 0usize;
        stack.clear();
        stack.push((xin, yin));
        // Mark an unclipped neighbor as border, if it is clear and not
        // itself in the outer margin.
        let mut mark = |d: &mut [u32], x: usize, y: usize, ok: bool| {
            let rp = y * w + x;
            if ok && d[rp] == 0 {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
                d[rp] = ID_MASK | id;
            }
        };
        while let Some((x, y)) = stack.pop() {
            let d = &mut self.data;
            if d[y * w + x] != 1 {
                continue;
            }
            let (y_up, y_down) = (y - 1, y + 1);
            d[y * w + x] = id;
            count += 1;
            let (first_up, first_down);
            if y_up >= border && d[y_up * w + x] == 1 {
                stack.push((x, y_up));
                first_up = true;
            } else {
                mark(d, x, y_up, x > border + 1);
                first_up = false;
            }
            if y_down < h - border && d[y_down * w + x] == 1 {
                stack.push((x, y_down));
                first_down = true;
            } else {
                mark(d, x, y_down, y_down < h - border - 2);
                first_down = false;
            }
            let (mut last_up, mut last_down) = (first_up, first_down);

            // Rightwards along the run.
            let mut xr = x + 1;
            while xr < w - border && d[y * w + xr] == 1 {
                d[y * w + xr] = id;
                count += 1;
                if y_up >= border && d[y_up * w + xr] == 1 {
                    if !last_up {
                        stack.push((xr, y_up));
                        last_up = true;
                    }
                } else {
                    mark(d, xr, y_up, y_up > border + 1);
                    last_up = false;
                }
                if y_down < h - border && d[y_down * w + xr] == 1 {
                    if !last_down {
                        stack.push((xr, y_down));
                        last_down = true;
                    }
                } else {
                    mark(d, xr, y_down, y_down < h - border - 2);
                    last_down = false;
                }
                xr += 1;
            }
            mark(d, xr, y, xr < w - border - 2);

            // Leftwards.
            let mut xl = x - 1;
            last_up = first_up;
            last_down = first_down;
            while xl >= border && d[y * w + xl] == 1 {
                d[y * w + xl] = id;
                count += 1;
                if y_up >= border && d[y_up * w + xl] == 1 {
                    if !last_up {
                        stack.push((xl, y_up));
                        last_up = true;
                    }
                } else {
                    mark(d, xl, y_up, y_up > border + 1);
                    last_up = false;
                }
                if y_down < h - border && d[y_down * w + xl] == 1 {
                    if !last_down {
                        stack.push((xl, y_down));
                        last_down = true;
                    }
                } else {
                    mark(d, xl, y_down, y_down < h - border - 2);
                    last_down = false;
                }
                xl -= 1;
            }
            mark(d, xl, y, xl > border + 1);
        }

        if count < MIN_SEGMENT_BLOCKS {
            for row in min_y..=max_y {
                for col in min_x..=max_x {
                    let loc = row * w + col;
                    if self.data[loc] == id {
                        self.data[loc] = 1;
                    } else if self.data[loc] == (id | ID_MASK) {
                        self.data[loc] = 0;
                    }
                }
            }
            false
        } else {
            let i = id as usize;
            self.xmin[i] = min_x;
            self.xmax[i] = max_x;
            self.ymin[i] = min_y;
            self.ymax[i] = max_y;
            self.val1[i] = 0.0;
            self.val2[i] = 0.0;
            self.nr += 1;
            true
        }
    }

    /// For every segment, the unclipped block with the smoothest
    /// surroundings; its 5x5 average and its opposed average become the
    /// segment's correction.
    fn find_candidates(&mut self, plane: &[f32], refavg: &[f32], clip: f32, badlevel: f32) {
        let (w, h, border) = (self.width, self.height, self.border);
        for id in 2..self.nr {
            let i = id as usize;
            self.val1[i] = 0.0;
            self.val2[i] = 0.0;
            if self.ymax[i] - self.ymin[i] <= 2 || self.xmax[i] - self.xmin[i] <= 2 {
                continue;
            }
            let mut best = 0usize;
            let mut best_weight = 0.0f32;
            for row in (border + 2).max(self.ymin[i].saturating_sub(2))
                ..(h - border - 2).min(self.ymax[i] + 3)
            {
                for col in (border + 2).max(self.xmin[i].saturating_sub(2))
                    ..(w - border - 2).min(self.xmax[i] + 3)
                {
                    let pos = row * w + col;
                    if self.id_at(pos) != id || plane[pos] >= clip {
                        continue;
                    }
                    let on_border = self.data[pos] & ID_MASK != 0;
                    let weight = smoothness(plane, pos, w) * if on_border { 1.0 } else { 0.75 };
                    if weight > best_weight {
                        best_weight = weight;
                        best = pos;
                    }
                }
            }
            if best == 0 || best_weight <= 1.0 - badlevel {
                continue;
            }
            let mut sum = 0.0f32;
            let mut wsum = 0.0f32;
            for (dy, wrow) in BINOMIAL_5.iter().enumerate() {
                for (dx, &wt) in wrow.iter().enumerate() {
                    let pos = best + dy * w + dx - 2 * w - 2;
                    if plane[pos] < clip {
                        sum += plane[pos] * wt;
                        wsum += wt;
                    }
                }
            }
            let average = sum / wsum.max(1.0);
            if average > 0.125 * clip {
                self.val1[i] = average.min(clip);
                self.val2[i] = refavg[best];
            }
        }
    }
}

/// The candidate weight: one minus ten times the local standard
/// deviation's square root, floored at zero, over the reference's
/// 21-block cross.
fn smoothness(p: &[f32], loc: usize, w: usize) -> f32 {
    let at = |dx: isize, dy: isize| p[(loc as isize + dy * w as isize + dx) as usize];
    let values: [f32; 21] = std::array::from_fn(|n| at(CROSS_21[n].0, CROSS_21[n].1));
    let mean = values.iter().sum::<f32>() / 21.0;
    let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / 21.0;
    (1.0 - 10.0 * var.sqrt().sqrt()).max(0.0)
}

const CROSS_21: [(isize, isize); 21] = [
    (-1, -2),
    (0, -2),
    (1, -2),
    (-2, -1),
    (-1, -1),
    (0, -1),
    (1, -1),
    (2, -1),
    (-2, 0),
    (-1, 0),
    (0, 0),
    (1, 0),
    (2, 0),
    (-2, 1),
    (-1, 1),
    (0, 1),
    (1, 1),
    (2, 1),
    (-1, 2),
    (0, 2),
    (1, 2),
];

const BINOMIAL_5: [[f32; 5]; 5] = [
    [1.0, 4.0, 6.0, 4.0, 1.0],
    [4.0, 16.0, 24.0, 16.0, 4.0],
    [6.0, 24.0, 36.0, 24.0, 6.0],
    [4.0, 16.0, 24.0, 16.0, 4.0],
    [1.0, 4.0, 6.0, 4.0, 1.0],
];

/// Dilate (`grow`) or erode a 0/1 plane by the reference's disc of
/// `radius` rings, inside the border.
fn morph(
    src: &[u32],
    dst: &mut [u32],
    width: usize,
    height: usize,
    border: usize,
    radius: usize,
    grow: bool,
) {
    let rings = &RINGS[..radius.clamp(1, RINGS.len())];
    dst.par_chunks_mut(width)
        .enumerate()
        .skip(border)
        .take(height - 2 * border)
        .for_each(|(row, drow)| {
            for (col, d) in drow
                .iter_mut()
                .enumerate()
                .take(width - border)
                .skip(border)
            {
                let i = (row * width + col) as isize;
                let hit =
                    |&(dx, dy): &(isize, isize)| src[(i + dy * width as isize + dx) as usize] != 0;
                let set = if grow {
                    rings.iter().any(|ring| ring.iter().any(hit))
                } else {
                    rings.iter().all(|ring| ring.iter().all(hit))
                };
                *d = set as u32;
            }
        });
}

/// The reference's disc, ring by ring: ring 1 is the full 3x3, each
/// further ring the blocks it adds. Rings 1 to 5 are shared by dilation
/// and erosion; erosion never goes beyond 5. Ring 8's one lopsided entry
/// in the reference is made symmetric here, and its four repeats of ring
/// 7 dropped.
const RINGS: [&[(isize, isize)]; 8] = [
    &[
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (0, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ],
    &[
        (-1, -2),
        (0, -2),
        (1, -2),
        (-2, -1),
        (2, -1),
        (-2, 0),
        (2, 0),
        (-2, 1),
        (2, 1),
        (-1, 2),
        (0, 2),
        (1, 2),
    ],
    &[
        (-2, -3),
        (-1, -3),
        (0, -3),
        (1, -3),
        (2, -3),
        (-3, -2),
        (-2, -2),
        (2, -2),
        (3, -2),
        (-3, -1),
        (3, -1),
        (-3, 0),
        (3, 0),
        (-3, 1),
        (3, 1),
        (-3, 2),
        (-2, 2),
        (2, 2),
        (3, 2),
        (-2, 3),
        (-1, 3),
        (0, 3),
        (1, 3),
        (2, 3),
    ],
    &[
        (-2, -4),
        (-1, -4),
        (0, -4),
        (1, -4),
        (2, -4),
        (-3, -3),
        (3, -3),
        (-4, -2),
        (4, -2),
        (-4, -1),
        (4, -1),
        (-4, 0),
        (4, 0),
        (-4, 1),
        (4, 1),
        (-4, 2),
        (4, 2),
        (-3, 3),
        (3, 3),
        (-2, 4),
        (-1, 4),
        (0, 4),
        (1, 4),
        (2, 4),
    ],
    &[
        (-2, -5),
        (-1, -5),
        (0, -5),
        (1, -5),
        (2, -5),
        (-4, -4),
        (-3, -4),
        (3, -4),
        (4, -4),
        (-4, -3),
        (4, -3),
        (-5, -2),
        (5, -2),
        (-5, -1),
        (5, -1),
        (-5, 0),
        (5, 0),
        (-5, 1),
        (5, 1),
        (-5, 2),
        (5, 2),
        (-4, 3),
        (4, 3),
        (-4, 4),
        (-3, 4),
        (3, 4),
        (4, 4),
        (-2, 5),
        (-1, 5),
        (0, 5),
        (1, 5),
        (2, 5),
    ],
    &[
        (-2, -6),
        (-1, -6),
        (0, -6),
        (1, -6),
        (2, -6),
        (-4, -5),
        (-3, -5),
        (3, -5),
        (4, -5),
        (-5, -4),
        (5, -4),
        (-5, -3),
        (5, -3),
        (-6, -2),
        (6, -2),
        (-6, -1),
        (6, -1),
        (-6, 0),
        (6, 0),
        (-6, 1),
        (6, 1),
        (-6, 2),
        (6, 2),
        (-5, 3),
        (5, 3),
        (-5, 4),
        (5, 4),
        (-4, 5),
        (-3, 5),
        (3, 5),
        (4, 5),
        (-2, 6),
        (-1, 6),
        (0, 6),
        (1, 6),
        (2, 6),
    ],
    &[
        (-3, -7),
        (-2, -7),
        (-1, -7),
        (0, -7),
        (1, -7),
        (2, -7),
        (3, -7),
        (-4, -6),
        (-3, -6),
        (3, -6),
        (4, -6),
        (-6, -5),
        (-5, -5),
        (5, -5),
        (6, -5),
        (-6, -4),
        (6, -4),
        (-7, -3),
        (-6, -3),
        (6, -3),
        (7, -3),
        (-7, -2),
        (7, -2),
        (-7, -1),
        (7, -1),
        (-7, 0),
        (7, 0),
        (-7, 1),
        (7, 1),
        (-7, 2),
        (7, 2),
        (-7, 3),
        (-6, 3),
        (6, 3),
        (7, 3),
        (-6, 4),
        (6, 4),
        (-6, 5),
        (-5, 5),
        (5, 5),
        (6, 5),
        (-4, 6),
        (-3, 6),
        (3, 6),
        (4, 6),
        (-3, 7),
        (-2, 7),
        (-1, 7),
        (0, 7),
        (1, 7),
        (2, 7),
        (3, 7),
    ],
    &[
        (-4, -8),
        (-3, -8),
        (-2, -8),
        (-1, -8),
        (0, -8),
        (1, -8),
        (2, -8),
        (3, -8),
        (4, -8),
        (-6, -7),
        (-5, -7),
        (-4, -7),
        (4, -7),
        (5, -7),
        (6, -7),
        (-6, -6),
        (-5, -6),
        (5, -6),
        (6, -6),
        (-7, -5),
        (7, -5),
        (-8, -4),
        (-7, -4),
        (7, -4),
        (8, -4),
        (-8, -3),
        (8, -3),
        (-8, -2),
        (8, -2),
        (-8, -1),
        (8, -1),
        (-8, 0),
        (8, 0),
        (-8, 1),
        (8, 1),
        (-8, 2),
        (8, 2),
        (-8, 3),
        (8, 3),
        (-8, 4),
        (-7, 4),
        (7, 4),
        (8, 4),
        (-7, 5),
        (7, 5),
        (-6, 6),
        (-5, 6),
        (5, 6),
        (6, 6),
        (-6, 7),
        (-5, 7),
        (-4, 7),
        (4, 7),
        (5, 7),
        (6, 7),
        (-4, 8),
        (-3, 8),
        (-2, 8),
        (-1, 8),
        (0, 8),
        (1, 8),
        (2, 8),
        (3, 8),
        (4, 8),
    ],
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::highlights::{CLIP_MAGIC, inpaint_opposed};

    fn mosaic(width: usize, height: usize, f: impl Fn(usize, usize, usize) -> f32) -> Vec<f32> {
        let p = CfaPattern::rggb();
        (0..width * height)
            .map(|i| {
                let (x, y) = (i % width, i / width);
                f(x, y, p.color_at(y, x).rgb_index().unwrap())
            })
            .collect()
    }

    #[test]
    fn rings_are_symmetric_and_distinct() {
        let mut seen = std::collections::HashSet::new();
        for (r, ring) in RINGS.iter().enumerate() {
            for &(dx, dy) in ring.iter() {
                assert!(
                    seen.insert((dx, dy)),
                    "ring {}: {:?} twice",
                    r + 1,
                    (dx, dy)
                );
                assert!(
                    ring.contains(&(-dx, dy)) && ring.contains(&(dx, -dy)),
                    "ring {}: {:?}",
                    r + 1,
                    (dx, dy)
                );
                assert!(dx.abs().max(dy.abs()) as usize <= r + 1);
            }
        }
    }

    #[test]
    fn flood_fill_finds_blobs_and_their_borders() {
        let (w, h, b) = (40, 30, 9);
        let mut seg = Segmentation::new(w, h, b, 256);
        // A 4x3 blob, a lone block and a 1x3 blob; the reference keeps
        // segments of four blocks or more.
        for y in 12..15 {
            for x in 12..16 {
                seg.data[y * w + x] = 1;
            }
        }
        seg.data[20 * w + 20] = 1;
        for x in 25..28 {
            seg.data[18 * w + x] = 1;
        }
        seg.segmentize();
        assert_eq!(seg.nr, 3, "one segment");
        assert_eq!(
            (seg.xmin[2], seg.xmax[2], seg.ymin[2], seg.ymax[2]),
            (11, 16, 11, 15)
        );
        assert_eq!(seg.id_at(13 * w + 13), 2);
        assert_eq!(seg.id_at(13 * w + 11), 2, "border block joins the segment");
        assert!(seg.data[13 * w + 11] & ID_MASK != 0);
        assert_eq!(seg.id_at(20 * w + 20), 0);
        assert_eq!(seg.data[20 * w + 20], 1, "small blobs are left as found");
        assert_eq!(seg.data[18 * w + 25], 1);
    }

    #[test]
    fn combine_bridges_a_gap_of_three() {
        let (w, h, b) = (40, 30, 9);
        let mut seg = Segmentation::new(w, h, b, 256);
        for y in 12..16 {
            for x in 12..16 {
                seg.data[y * w + x] = 1;
                seg.data[y * w + x + 7] = 1;
            }
        }
        seg.combine(2);
        seg.segmentize();
        assert_eq!(seg.nr, 3, "the two blobs join through the gap");
        let mut apart = Segmentation::new(w, h, b, 256);
        for y in 12..16 {
            for x in 12..16 {
                apart.data[y * w + x] = 1;
                apart.data[y * w + x + 7] = 1;
            }
        }
        apart.combine(0);
        apart.segmentize();
        assert_eq!(apart.nr, 4, "a 3x3 dilation alone leaves them apart");
    }

    #[test]
    fn unclipped_images_pass_through() {
        let cfa = mosaic(60, 60, |x, _, c| {
            [0.6, 0.9, 0.4][c] * (0.5 + x as f32 / 120.0)
        });
        let clips = [2.0f32, 1.0, 2.0].map(|g| g * CLIP_MAGIC);
        let (out, stats) = inpaint_segments(
            &cfa,
            &cfa,
            60,
            60,
            &CfaPattern::rggb(),
            clips,
            &SegmentOptions::default(),
        )
        .unwrap();
        assert_eq!(out, cfa);
        assert_eq!(stats, SegmentStats::default());
    }

    #[test]
    fn each_segment_gets_its_own_chrominance() {
        // Two lamps of different color on flat surrounds just below the
        // clip, green clipped inside both. One global offset can only suit
        // one of them; per segment, both. The surrounds are exactly flat:
        // the reference's candidate weight wants a spot smoother than a
        // few tenths of a percent across five blocks, which a wall or a
        // sky gives and a smooth synthetic falloff does not.
        let (w, h) = (240, 120);
        let gains = [2.0f32, 1.0, 2.0];
        let clips = gains.map(|g| g * CLIP_MAGIC);
        let lamps = [
            (60.0f32, [0.98f32, 0.9, 0.68], [1.3f32, 1.2, 0.91]),
            (180.0, [0.54, 0.9, 0.54], [0.72, 1.2, 0.72]),
        ];
        let truth = |x: usize, y: usize, c: usize| -> f32 {
            let blend = ((x as f32 - 110.0) / 20.0).clamp(0.0, 1.0);
            let mut v = lamps[0].1[c] * (1.0 - blend) + lamps[1].1[c] * blend;
            for (cx, base, top) in lamps {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - 60.0).powi(2)).sqrt();
                if d < 25.0 {
                    v += top[c] - base[c];
                }
            }
            v
        };
        let cfa = mosaic(w, h, |x, y, c| truth(x, y, c).min(clips[c] / CLIP_MAGIC));
        let pattern = CfaPattern::rggb();
        let (opposed, _) = inpaint_opposed(&cfa, w, h, &pattern, clips).unwrap();
        let (out, stats) = inpaint_segments(
            &cfa,
            &opposed,
            w,
            h,
            &pattern,
            clips,
            &SegmentOptions::default(),
        )
        .unwrap();
        assert_eq!(stats.segments[1], 2, "{stats:?}");
        assert_eq!(stats.candidates[1], 2, "{stats:?}");
        assert!(stats.replaced > 100, "{stats:?}");
        let error = |img: &[f32]| {
            let mut e = 0.0f64;
            let mut n = 0;
            for y in 0..h {
                for x in 0..w {
                    if pattern.color_at(y, x) == CfaColor::Green && cfa[y * w + x] >= clips[1] {
                        e += (img[y * w + x] - truth(x, y, 1)).powi(2) as f64;
                        n += 1;
                    }
                }
            }
            (e / n as f64).sqrt()
        };
        let (before, after) = (error(&opposed), error(&out));
        assert!(after < 0.6 * before, "rms error {before} -> {after}");
    }
}
