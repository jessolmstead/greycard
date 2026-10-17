//! Sites round a photosite, read from the mosaic's pattern rather than
//! assumed: what the stages before the demosaic need of a pattern whose
//! sites of a color do not sit at one offset from every site, as they
//! do on Bayer.
//!
//! On X-Trans the nearest red to a red is two columns over at one site
//! and a row and two columns over at the next, and the set round a site
//! is lopsided: the five reds in a red site's 5x5 have their centroid
//! off the site. A plain mean of them reads the picture a fraction of a
//! pixel away, which on a gradient is a bias, not noise, and on an edge
//! is a shift. [`planar_weights`] weights a set of sites so that their
//! mean is exact on any plane (a constant and both gradients), with the
//! least sum of squared weights, which is the least noise such a mean
//! can carry. Every table here is built once from the pattern, a cell
//! per site of its period. Ours, not a port.

use crate::raw::CfaPattern;

/// Something for each site of a pattern's period, read at any sensor
/// position.
#[derive(Debug, Clone)]
pub struct Phases<T> {
    width: usize,
    height: usize,
    cells: Vec<T>,
}

impl<T> Phases<T> {
    /// `f(row, col)` for each site of `pattern`'s period.
    pub fn build(pattern: &CfaPattern, mut f: impl FnMut(usize, usize) -> T) -> Self {
        let (width, height) = (pattern.width, pattern.height);
        let cells = (0..height)
            .flat_map(|y| (0..width).map(move |x| (y, x)))
            .map(|(y, x)| f(y, x))
            .collect();
        Self {
            width,
            height,
            cells,
        }
    }

    /// The entry for sensor position (`row`, `col`).
    #[inline]
    pub fn at(&self, row: usize, col: usize) -> &T {
        &self.cells[(row % self.height) * self.width + col % self.width]
    }

    /// The entries for sensor row `row`, one a column of the period:
    /// for a loop along the row, which cycles them rather than take a
    /// remainder at every site.
    pub fn row(&self, row: usize) -> &[T] {
        let start = (row % self.height) * self.width;
        &self.cells[start..start + self.width]
    }
}

/// The offsets of `color`'s sites (an RGB index) within `radius` of
/// (`row`, `col`) either way, row-major, the site itself left out.
pub fn of_color(
    pattern: &CfaPattern,
    row: usize,
    col: usize,
    color: usize,
    radius: usize,
) -> Vec<(isize, isize)> {
    let r = radius as isize;
    // Read the pattern at positive positions: the period added to the
    // site keeps every offset in range.
    let (y, x) = (
        (row % pattern.height + pattern.height * (radius + 1)) as isize,
        (col % pattern.width + pattern.width * (radius + 1)) as isize,
    );
    (-r..=r)
        .flat_map(|dy| (-r..=r).map(move |dx| (dy, dx)))
        .filter(|&d| d != (0, 0))
        .filter(|&(dy, dx)| {
            pattern
                .color_at((y + dy) as usize, (x + dx) as usize)
                .rgb_index()
                == Some(color)
        })
        .collect()
}

/// The weights of the least sum of squares that make a weighted sum
/// of the samples at `offsets` exact on a plane: they sum to one, and
/// their first moments along each axis are zero. `None` when the sites
/// do not span a plane (fewer than three, or all in a line).
///
/// With `A` the three rows of ones, row offsets and column offsets over
/// the sites, the weights are `A^T (A A^T)^-1 e1`: the minimum-norm
/// solution of `A w = e1`, in double precision.
pub fn planar_weights(offsets: &[(isize, isize)]) -> Option<Vec<f32>> {
    let rows: Vec<[f64; 3]> = offsets
        .iter()
        .map(|&(dy, dx)| [1.0, dy as f64, dx as f64])
        .collect();
    let mut m = [[0.0f64; 3]; 3];
    for r in &rows {
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += r[i] * r[j];
            }
        }
    }
    let lambda = solve3(m, [1.0, 0.0, 0.0])?;
    Some(
        rows.iter()
            .map(|r| (r[0] * lambda[0] + r[1] * lambda[1] + r[2] * lambda[2]) as f32)
            .collect(),
    )
}

/// `m x = b` for a symmetric 3x3 by Cramer's rule; `None` when `m` is
/// singular, relative to its size.
fn solve3(m: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(&m);
    let scale = m.iter().flatten().fold(0.0f64, |a, v| a.max(v.abs()));
    if scale == 0.0 || d.abs() <= 1e-9 * scale * scale * scale {
        return None;
    }
    let mut x = [0.0f64; 3];
    for (k, xk) in x.iter_mut().enumerate() {
        let mut mk = m;
        for (row, bv) in mk.iter_mut().zip(b) {
            row[k] = bv;
        }
        *xk = det(&mk) / d;
    }
    Some(x)
}

/// A weighted read of the mosaic: a row offset, a column offset and a
/// weight.
pub type Tap = (isize, isize, f32);

/// For each site of `pattern` and each color, the taps that estimate
/// that color there: the site's own sample for its own color; else the
/// color's sites in the 3x3 round it with [`planar_weights`] when they
/// span a plane, and in the 5x5 when they do not. On X-Trans the 3x3
/// serves green at every red and blue site (five greens) and red at
/// blue and blue at red (three each); red and blue at a green site take
/// the 5x5. Reads reach two sites out.
pub fn interpolation(pattern: &CfaPattern) -> Phases<[Vec<Tap>; 3]> {
    Phases::build(pattern, |y, x| {
        let own = pattern.color_at(y, x).rgb_index();
        std::array::from_fn(|c| {
            if own == Some(c) {
                return vec![(0, 0, 1.0)];
            }
            [1, 2]
                .into_iter()
                .find_map(|radius| {
                    let offsets = of_color(pattern, y, x, c, radius);
                    let weights = planar_weights(&offsets)?;
                    Some(
                        offsets
                            .iter()
                            .zip(weights)
                            .map(|(&(dy, dx), w)| (dy, dx, w))
                            .collect(),
                    )
                })
                .unwrap_or_default()
        })
    })
}

/// Taps as offsets into a buffer `width` wide.
pub fn linear(taps: &[Tap], width: usize) -> Vec<(isize, f32)> {
    taps.iter()
        .map(|&(dy, dx, w)| (dy * width as isize + dx, w))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::CfaColor;

    #[test]
    fn planar_weights_are_exact_on_a_plane_and_refuse_a_line() {
        let offsets = [(-1, -1), (-1, 1), (0, -1), (0, 1), (1, 0)];
        let w = planar_weights(&offsets).unwrap();
        let sum: f32 = w.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6, "{w:?}");
        for axis in 0..2 {
            let moment: f32 = offsets
                .iter()
                .zip(&w)
                .map(|(&(dy, dx), w)| w * if axis == 0 { dy } else { dx } as f32)
                .sum();
            assert!(moment.abs() < 1e-6, "{axis}: {w:?}");
        }
        // Four symmetric sites: the plain mean is already the answer.
        let w = planar_weights(&[(0, -2), (0, 2), (-2, 0), (2, 0)]).unwrap();
        assert!(w.iter().all(|v| (v - 0.25).abs() < 1e-6), "{w:?}");
        assert!(planar_weights(&[(0, -1), (0, 1)]).is_none());
        assert!(planar_weights(&[(0, -1), (0, 1), (0, 2)]).is_none());
        assert!(planar_weights(&[]).is_none());
    }

    #[test]
    fn of_color_reads_the_pattern_at_any_phase() {
        let p = CfaPattern::xtrans();
        // The red at (1, 0): its reds in the 5x5.
        let mut reds = of_color(&p, 1, 0, 0, 2);
        reds.sort();
        assert_eq!(reds, [(-2, 1), (-1, -2), (0, 2), (1, -2), (2, 1)]);
        // The same site a period on reads the same.
        let mut again = of_color(&p, 7, 6, 0, 2);
        again.sort();
        assert_eq!(again, reds);
        for &(dy, dx) in &reds {
            assert_eq!(
                p.color_at((7 + dy) as usize, (6 + dx) as usize),
                CfaColor::Red
            );
        }
    }

    /// Every estimate the table makes is exact on a plane, at every
    /// site of X-Trans at every phase and of Bayer.
    #[test]
    fn interpolation_is_exact_on_a_plane() {
        let mut patterns = vec![CfaPattern::rggb(), CfaPattern::rggb().shifted(1, 0)];
        for dy in 0..6 {
            for dx in 0..6 {
                patterns.push(CfaPattern::xtrans().shifted(dx, dy));
            }
        }
        let plane = |y: isize, x: isize| 0.3 + 0.01 * y as f32 - 0.007 * x as f32;
        for p in &patterns {
            let table = interpolation(p);
            for y in 10..16usize {
                for x in 10..16usize {
                    for (c, taps) in table.at(y, x).iter().enumerate() {
                        assert!(!taps.is_empty(), "{p}: ({y}, {x}) color {c}");
                        let mut v = 0.0f32;
                        for &(dy, dx, w) in taps {
                            let (sy, sx) = (y as isize + dy, x as isize + dx);
                            assert_eq!(p.color_at(sy as usize, sx as usize).rgb_index(), Some(c));
                            assert!(dy.abs() <= 2 && dx.abs() <= 2);
                            v += w * plane(sy, sx);
                        }
                        let want = plane(y as isize, x as isize);
                        assert!(
                            (v - want).abs() < 1e-5,
                            "{p}: ({y}, {x}) {c}: {v} vs {want}"
                        );
                    }
                }
            }
        }
    }
}
