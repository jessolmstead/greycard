//! A DNG's bad-pixel lists on the mosaic: `FixBadPixelsConstant` and
//! `FixBadPixelsList`, the photosites a maker knows to be bad, each made
//! again from its same-color neighbors.
//!
//! The SDK (`dng_bad_pixels.cpp`) averages fixed neighbor sets of a
//! Bayer mosaic. Here a bad photosite takes the median of the good ones
//! of its color within two photosites, or within four when none is
//! that near, read from the frame's own pattern, so an X-Trans mosaic
//! is repaired as a Bayer one is and a cluster's members never feed one
//! another. A photosite with no good neighbor of its color in reach is
//! left. On a monochrome sensor's plane every neighbor is of its color,
//! so the good ones adjacent to it are read first.

use rayon::prelude::*;

use crate::opcodes::BadPixels;
use crate::raw::{CfaColor, CfaPattern};

/// Mark the photosites `fix` lists in `bad`, a flag a photosite of a
/// buffer `width` by `height` whose first photosite is at (`x`, `y`) in
/// the list's image. A constant list marks nothing here: its test is
/// on the values, the caller's.
pub fn mark_listed(
    bad: &mut [bool],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    fix: &BadPixels,
) {
    let BadPixels::List { points, rects, .. } = fix else {
        return;
    };
    let (x, y) = (x as i64, y as i64);
    let (w, h) = (width as i64, height as i64);
    let mut mark = |top: i64, left: i64, bottom: i64, right: i64| {
        let (top, bottom) = ((top - y).clamp(0, h), (bottom - y).clamp(0, h));
        let (left, right) = ((left - x).clamp(0, w), (right - x).clamp(0, w));
        for row in top..bottom {
            let start = (row * w) as usize;
            bad[start + left as usize..start + right.max(left) as usize].fill(true);
        }
    };
    for &[row, col] in points {
        let (row, col) = (i64::from(row), i64::from(col));
        mark(row, col, row + 1, col + 1);
    }
    for &[top, left, bottom, right] in rects {
        mark(
            i64::from(top),
            i64::from(left),
            i64::from(bottom),
            i64::from(right),
        );
    }
}

/// The farthest a repair reads, photosites each way.
const MAX_REACH: usize = 4;

/// A monochrome sensor's plane as a pattern: every photosite is of one
/// color.
fn one_color() -> CfaPattern {
    CfaPattern {
        width: 1,
        height: 1,
        colors: vec![CfaColor::Green],
    }
}

/// Make every photosite `bad` flags again from its good same-color
/// neighbors: of a mosaic's `pattern` as seen from the buffer's corner,
/// or, with none, of a monochrome sensor's plane, where the ones
/// beside it are read first. Answers how many were made again. A
/// mosaic is read the same whatever its pattern, one of a single color
/// too.
pub fn repair(
    samples: &mut [f32],
    width: usize,
    height: usize,
    pattern: Option<&CfaPattern>,
    bad: &[bool],
) -> usize {
    let read: &[f32] = samples;
    let plane = one_color();
    let (pattern, reaches): (&CfaPattern, &[usize]) = match pattern {
        Some(pattern) => (pattern, &[2, MAX_REACH]),
        None => (&plane, &[1, 2, MAX_REACH]),
    };
    let fixes: Vec<(usize, f32)> = bad
        .par_iter()
        .enumerate()
        .filter(|(_, b)| **b)
        .filter_map(|(i, _)| {
            let (x, y) = (i % width, i / width);
            let color = pattern.color_at(y, x);
            reaches
                .iter()
                .find_map(|&reach| {
                    median(read, (width, height), pattern, bad, color, (x, y), reach)
                })
                .map(|v| (i, v))
        })
        .collect();
    for &(i, v) in &fixes {
        samples[i] = v;
    }
    fixes.len()
}

/// The median of the good photosites of `color` within `reach` of
/// (`x`, `y`), or `None` when there is none.
fn median(
    samples: &[f32],
    (width, height): (usize, usize),
    pattern: &CfaPattern,
    bad: &[bool],
    color: CfaColor,
    (x, y): (usize, usize),
    reach: usize,
) -> Option<f32> {
    let mut found = [0.0f32; (2 * MAX_REACH + 1) * (2 * MAX_REACH + 1)];
    let mut n = 0;
    for ny in y.saturating_sub(reach)..(y + reach + 1).min(height) {
        for nx in x.saturating_sub(reach)..(x + reach + 1).min(width) {
            let i = ny * width + nx;
            if !bad[i] && pattern.color_at(ny, nx) == color {
                found[n] = samples[i];
                n += 1;
            }
        }
    }
    let found = &mut found[..n];
    if found.is_empty() {
        return None;
    }
    found.sort_unstable_by(f32::total_cmp);
    let mid = n / 2;
    Some(if n % 2 == 1 {
        found[mid]
    } else {
        (found[mid - 1] + found[mid]) / 2.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listed point on a flat red takes the red, not the greens beside
    /// it; the photosites not listed are left to the bit.
    #[test]
    fn a_listed_point_takes_its_colors_median() {
        let (w, h) = (8, 8);
        let pattern = CfaPattern::rggb();
        let mut s: Vec<f32> = (0..w * h)
            .map(|i| match pattern.color_at(i / w, i % w) {
                CfaColor::Red => 0.3,
                CfaColor::Blue => 0.1,
                _ => 0.6,
            })
            .collect();
        let before = s.clone();
        s[4 * w + 4] = 1.0; // a red site, stuck at the white
        let mut bad = vec![false; w * h];
        let fix = BadPixels::List {
            phase: 0,
            points: vec![[4, 4]],
            rects: vec![],
        };
        mark_listed(&mut bad, w, h, 0, 0, &fix);
        assert_eq!(bad.iter().filter(|b| **b).count(), 1);
        assert_eq!(repair(&mut s, w, h, Some(&pattern), &bad), 1);
        assert_eq!(s, before);
    }

    /// A list in the coordinates of a larger image marks its point where
    /// the buffer has it, and a rectangle clipped to the buffer; one
    /// wholly outside, or written backward, marks nothing.
    #[test]
    fn a_list_is_placed_by_the_buffers_corner() {
        let (w, h) = (6, 4);
        let mut bad = vec![false; w * h];
        let fix = BadPixels::List {
            phase: 0,
            points: vec![[11, 12], [0, 0], [-3, 5]],
            rects: vec![[9, 14, 20, 30], [10, 10, 9, 12], [100, 100, 101, 101]],
        };
        mark_listed(&mut bad, w, h, 10, 8, &fix);
        let marked: Vec<usize> = (0..w * h).filter(|&i| bad[i]).collect();
        // (11, 12) is row 3 col 2; the first rectangle rows 1.. and
        // columns 4.. of the buffer.
        assert_eq!(
            marked,
            vec![
                w + 4,
                w + 5,
                2 * w + 4,
                2 * w + 5,
                3 * w + 2,
                3 * w + 4,
                3 * w + 5
            ]
        );
    }

    /// A cluster's members never feed one another: a 3x3 block of bad
    /// photosites on a flat mosaic comes back flat, the reach widening
    /// for the center that has no good neighbor of its color within two.
    #[test]
    fn a_cluster_is_made_from_the_good_around_it() {
        let (w, h) = (12, 12);
        let pattern = CfaPattern::rggb();
        let mut s = vec![0.5f32; w * h];
        let mut bad = vec![false; w * h];
        let fix = BadPixels::List {
            phase: 0,
            points: vec![],
            rects: vec![[4, 4, 9, 9]],
        };
        mark_listed(&mut bad, w, h, 0, 0, &fix);
        for (v, b) in s.iter_mut().zip(&bad) {
            if *b {
                *v = 0.0;
            }
        }
        assert_eq!(repair(&mut s, w, h, Some(&pattern), &bad), 25);
        assert!(s.iter().all(|&v| v == 0.5), "{s:?}");
    }

    /// An X-Trans mosaic is repaired by its own pattern.
    #[test]
    fn an_xtrans_site_takes_its_own_color() {
        let (w, h) = (12, 12);
        let pattern = CfaPattern::xtrans();
        let level = |c: CfaColor| match c {
            CfaColor::Red => 0.2,
            CfaColor::Blue => 0.4,
            _ => 0.7,
        };
        let mut s: Vec<f32> = (0..w * h)
            .map(|i| level(pattern.color_at(i / w, i % w)))
            .collect();
        let before = s.clone();
        let mut bad = vec![false; w * h];
        for i in [5 * w + 5, 6 * w + 7, 7 * w + 3] {
            s[i] = 1.0;
            bad[i] = true;
        }
        assert_eq!(repair(&mut s, w, h, Some(&pattern), &bad), 3);
        assert_eq!(s, before);
    }

    /// A monochrome plane's point takes the median of the eight beside
    /// it, and a 3x3 block's center, with none good beside it, of the
    /// sixteen round the block.
    #[test]
    fn a_monochrome_site_takes_the_ones_beside_it() {
        let (w, h) = (11, 11);
        // Rings about (5, 5): 0.5 beside it, 0.3 two out, 0.1 past that.
        let ring = |i: usize| (i % w).abs_diff(5).max((i / w).abs_diff(5));
        let shot: Vec<f32> = (0..w * h)
            .map(|i| [1.0, 0.5, 0.3, 0.1, 0.1, 0.1][ring(i)])
            .collect();
        let center = 5 * w + 5;
        for (size, want) in [(0, 0.5), (1, 0.3)] {
            let mut s = shot.clone();
            let bad: Vec<bool> = (0..w * h).map(|i| ring(i) <= size).collect();
            let count = bad.iter().filter(|b| **b).count();
            assert_eq!(repair(&mut s, w, h, None, &bad), count);
            assert_eq!(s[center], want);
            for i in (0..w * h).filter(|&i| !bad[i]) {
                assert_eq!(s[i], shot[i]);
            }
        }
        // At the corner, the three beside it: 0.2, 0.4, and 0.9.
        let mut s = vec![0.0; 16];
        (s[1], s[4], s[5]) = (0.2, 0.9, 0.4);
        let mut bad = [false; 16];
        bad[0] = true;
        assert_eq!(repair(&mut s, 4, 4, None, &bad), 1);
        assert_eq!(s[0], 0.4);
    }

    /// Rings about (5, 5): 1.0 at it, 0.5 beside it, 0.3 two out, 0.1
    /// past that.
    fn rings(w: usize, h: usize) -> Vec<f32> {
        let ring = |i: usize| (i % w).abs_diff(5).max((i / w).abs_diff(5));
        (0..w * h)
            .map(|i| [1.0, 0.5, 0.3, 0.1, 0.1, 0.1][ring(i)])
            .collect()
    }

    /// A mosaic is read as a mosaic whatever its pattern: one of a
    /// single color, as a hostile file may declare, takes the median
    /// within two, never the plane's ones beside it first.
    #[test]
    fn a_one_color_mosaic_is_not_a_plane() {
        let (w, h) = (11, 11);
        let mut bad = vec![false; w * h];
        bad[5 * w + 5] = true;
        let mut mosaic = rings(w, h);
        assert_eq!(repair(&mut mosaic, w, h, Some(&one_color()), &bad), 1);
        // Within two: eight at 0.5 and sixteen at 0.3.
        assert_eq!(mosaic[5 * w + 5], 0.3);
        let mut plane = rings(w, h);
        assert_eq!(repair(&mut plane, w, h, None, &bad), 1);
        assert_eq!(plane[5 * w + 5], 0.5);
    }

    /// A whole bad row on a monochrome plane: each photosite has the
    /// rows above and below beside it, an even count (six, or four at
    /// either end), and takes their middle two's mean.
    #[test]
    fn a_monochrome_planes_bad_row() {
        let (w, h) = (6, 5);
        // Row 1 at 0.2 + 0.1 x, row 3 at 0.6 + 0.1 x; row 2 stuck.
        let shot: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                match y {
                    1 => 0.2 + 0.1 * x as f32,
                    3 => 0.6 + 0.1 * x as f32,
                    _ => 0.0,
                }
            })
            .collect();
        let mut s = shot.clone();
        let mut bad = vec![false; w * h];
        let fix = BadPixels::List {
            phase: 0,
            points: vec![],
            rects: vec![[2, 0, 3, 6]],
        };
        mark_listed(&mut bad, w, h, 0, 0, &fix);
        assert_eq!(bad.iter().filter(|b| **b).count(), w);
        s[2 * w..3 * w].fill(1.0);
        assert_eq!(repair(&mut s, w, h, None, &bad), w);
        for x in 0..w {
            // The middle two are the highest above and the lowest below.
            let above = 0.2 + 0.1 * (x + 1).min(w - 1) as f32;
            let below = 0.6 + 0.1 * x.saturating_sub(1) as f32;
            let want = (above + below) / 2.0;
            assert!((s[2 * w + x] - want).abs() < 1e-6, "{x}: {}", s[2 * w + x]);
        }
        for i in (0..w * h).filter(|&i| !bad[i]) {
            assert_eq!(s[i], shot[i]);
        }
    }

    /// A rectangle listed on a monochrome plane: a 2x3 block on a flat
    /// plane comes back flat, its members read from the good ones
    /// round it and never from one another.
    #[test]
    fn a_monochrome_planes_listed_rect() {
        let (w, h) = (9, 8);
        let mut s = vec![0.4f32; w * h];
        let mut bad = vec![false; w * h];
        let fix = BadPixels::List {
            phase: 0,
            points: vec![],
            rects: vec![[3, 2, 5, 5]],
        };
        mark_listed(&mut bad, w, h, 0, 0, &fix);
        assert_eq!(bad.iter().filter(|b| **b).count(), 6);
        for (v, b) in s.iter_mut().zip(&bad) {
            if *b {
                *v = 0.9;
            }
        }
        assert_eq!(repair(&mut s, w, h, None, &bad), 6);
        assert!(s.iter().all(|&v| v == 0.4), "{s:?}");
    }
}
