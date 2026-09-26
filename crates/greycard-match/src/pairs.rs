//! Block pairs: both pictures averaged over cells, with the cells that
//! carry texture, lie outside the JPEG's frame, or clip on either side
//! dropped. What is left is color and tone, sampled where the maker's
//! sharpening and noise reduction do not show.

use crate::BLOCK;
use crate::color::{decode3, luminance};
use crate::register::{Picture, Plane};

/// One kept block: the develop's mean, the camera's mean, both in
/// encoded sRGB, and where the block sits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pair {
    pub render: [f32; 3],
    pub jpeg: [f32; 3],
    /// The block's row and column in the block grid.
    pub row: usize,
    pub col: usize,
}

/// The kept blocks of one frame, and the grid they came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Pairs {
    pub pairs: Vec<Pair>,
    /// Blocks down and across.
    pub rows: usize,
    pub cols: usize,
    /// How many blocks the grid held before the cuts.
    pub total: usize,
}

/// The luminance-weighted standard deviation a block may have on
/// either side and still count as flat.
pub const TEXTURE_MAX: f32 = 0.035;
/// A channel mean at or over this, on either side, is clipped.
pub const CLIP_HIGH: f32 = 0.97;
/// A channel mean at or under this, on either side, is black.
pub const CLIP_LOW: f32 = 0.01;

/// Per-block means and standard deviations of an encoded picture,
/// over `block`-pixel cells, the remainder at the right and bottom
/// dropped.
pub fn block_stats(p: &Picture, block: usize) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, usize, usize) {
    let rows = p.height / block;
    let cols = p.width / block;
    let n = (block * block) as f32;
    let mut means = Vec::with_capacity(rows * cols);
    let mut stds = Vec::with_capacity(rows * cols);
    for by in 0..rows {
        for bx in 0..cols {
            let mut sum = [0.0f64; 3];
            let mut sq = [0.0f64; 3];
            for y in by * block..(by + 1) * block {
                for x in bx * block..(bx + 1) * block {
                    let v = p.at(y, x);
                    for k in 0..3 {
                        sum[k] += v[k] as f64;
                        sq[k] += (v[k] as f64) * (v[k] as f64);
                    }
                }
            }
            let mean = sum.map(|s| (s / n as f64) as f32);
            let std = [0, 1, 2].map(|k| {
                let var = sq[k] / n as f64 - (sum[k] / n as f64).powi(2);
                var.max(0.0).sqrt() as f32
            });
            means.push(mean);
            stds.push(std);
        }
    }
    (means, stds, rows, cols)
}

/// Per-block means of a plane, the same grid as [`block_stats`].
pub fn block_means(p: &Plane, block: usize) -> Vec<f32> {
    let rows = p.height / block;
    let cols = p.width / block;
    let n = (block * block) as f32;
    let mut means = Vec::with_capacity(rows * cols);
    for by in 0..rows {
        for bx in 0..cols {
            let mut sum = 0.0f32;
            for y in by * block..(by + 1) * block {
                for x in bx * block..(bx + 1) * block {
                    sum += p.at(y, x);
                }
            }
            means.push(sum / n);
        }
    }
    means
}

/// The block pairs of a render and the JPEG already laid over it,
/// with `coverage` saying which render pixels the JPEG reached
/// ([`Picture::coverage`]).
pub fn pairs(render: &Picture, laid: &Picture, coverage: &Plane) -> Pairs {
    assert_eq!((render.width, render.height), (laid.width, laid.height));
    let (rm, rs, rows, cols) = block_stats(render, BLOCK);
    let (jm, js, _, _) = block_stats(laid, BLOCK);
    let inside = block_means(coverage, BLOCK);
    let mut out = Vec::new();
    for i in 0..rows * cols {
        let smooth = luminance(rs[i]) < TEXTURE_MAX && luminance(js[i]) < TEXTURE_MAX;
        let covered = inside[i] > 0.999;
        let unclipped = max3(rm[i]) < CLIP_HIGH
            && max3(jm[i]) < CLIP_HIGH
            && min3(rm[i]) > CLIP_LOW
            && min3(jm[i]) > CLIP_LOW;
        if smooth && covered && unclipped {
            out.push(Pair {
                render: rm[i],
                jpeg: jm[i],
                row: i / cols,
                col: i % cols,
            });
        }
    }
    Pairs {
        pairs: out,
        rows,
        cols,
        total: rows * cols,
    }
}

fn max3(v: [f32; 3]) -> f32 {
    v[0].max(v[1]).max(v[2])
}

fn min3(v: [f32; 3]) -> f32 {
    v[0].min(v[1]).min(v[2])
}

impl Pairs {
    /// The render side, decoded to linear.
    pub fn render_linear(&self) -> Vec<[f32; 3]> {
        self.pairs.iter().map(|p| decode3(p.render)).collect()
    }

    /// The camera side, decoded to linear.
    pub fn jpeg_linear(&self) -> Vec<[f32; 3]> {
        self.pairs.iter().map(|p| decode3(p.jpeg)).collect()
    }

    /// A block's radius from the frame's center, taken at the block's
    /// own center: 0 in the middle, 1 at the midpoint of any edge, √2
    /// at a corner.
    pub fn radius(&self, p: &Pair) -> f32 {
        let (h, w) = (self.rows as f32, self.cols as f32);
        let y = (p.row as f32 + 0.5 - h / 2.0) / (h / 2.0);
        let x = (p.col as f32 + 0.5 - w / 2.0) / (w / 2.0);
        (y * y + x * x).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(width: usize, height: usize, v: [f32; 3]) -> Picture {
        Picture::filled(width, height, v)
    }

    #[test]
    fn a_flat_pair_is_kept_and_its_means_are_the_values() {
        let render = flat(BLOCK * 3, BLOCK * 2, [0.4, 0.5, 0.6]);
        let laid = flat(BLOCK * 3, BLOCK * 2, [0.45, 0.5, 0.55]);
        let cov = Plane {
            width: BLOCK * 3,
            height: BLOCK * 2,
            data: vec![1.0; BLOCK * 3 * BLOCK * 2],
        };
        let p = pairs(&render, &laid, &cov);
        assert_eq!((p.rows, p.cols, p.total), (2, 3, 6));
        assert_eq!(p.pairs.len(), 6);
        assert!((p.pairs[0].render[0] - 0.4).abs() < 1e-6);
        assert!((p.pairs[0].jpeg[2] - 0.55).abs() < 1e-6);
        assert_eq!((p.pairs[4].row, p.pairs[4].col), (1, 1));
    }

    #[test]
    fn texture_clipping_and_the_border_drop_a_block() {
        let (w, h) = (BLOCK * 3, BLOCK);
        let mut render = flat(w, h, [0.5; 3]);
        // Block 0: a checkerboard, textured.
        for y in 0..BLOCK {
            for x in 0..BLOCK {
                let v = if (x + y) % 2 == 0 { 0.2 } else { 0.8 };
                render.data[y * w + x] = [v; 3];
            }
        }
        // Block 1: clipped white on the camera side.
        let mut laid = flat(w, h, [0.5; 3]);
        for y in 0..BLOCK {
            for x in BLOCK..2 * BLOCK {
                laid.data[y * w + x] = [0.99; 3];
            }
        }
        // Block 2: outside the JPEG's coverage.
        let mut cov = vec![1.0; w * h];
        for y in 0..BLOCK {
            cov[y * w + 2 * BLOCK] = 0.0;
        }
        let cov = Plane {
            width: w,
            height: h,
            data: cov,
        };
        let p = pairs(&render, &laid, &cov);
        assert_eq!(p.total, 3);
        assert!(p.pairs.is_empty(), "{:?}", p.pairs);
    }

    #[test]
    fn radius_is_zero_at_the_center_and_root_two_at_a_corner() {
        // An odd grid, so one block sits exactly on the center.
        let p = Pairs {
            pairs: vec![],
            rows: 21,
            cols: 41,
            total: 861,
        };
        let center = Pair {
            render: [0.0; 3],
            jpeg: [0.0; 3],
            row: 10,
            col: 20,
        };
        let corner = Pair {
            row: 0,
            col: 0,
            ..center
        };
        let edge = Pair {
            row: 0,
            col: 20,
            ..center
        };
        assert!(p.radius(&center).abs() < 1e-6);
        // The corner block's center sits half a block in from the
        // corner on both axes.
        let expect = ((1.0 - 0.5 / 10.5f32).powi(2) + (1.0 - 0.5 / 20.5f32).powi(2)).sqrt();
        assert!((p.radius(&corner) - expect).abs() < 1e-5);
        assert!((p.radius(&edge) - (1.0 - 0.5 / 10.5)).abs() < 1e-5);
    }
}
