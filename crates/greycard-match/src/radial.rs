//! The lightness left after the look, against the radius from the
//! frame's center. The camera's JPEG carries its own vignetting
//! correction and the develop carries the lens profile's; where they
//! differ, the residual runs with radius, and that is a lens finding
//! and not the look's. Measured here, reported, and kept out of the
//! table.

use crate::color::{decode3, oklab};
use crate::fit::Model;
use crate::pairs::Pairs;

/// `dL = a + b r² + c r⁴` over the kept blocks, `dL` the model's
/// Oklab lightness minus the camera's. `None` with fewer than three
/// blocks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radial {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    /// The fitted difference between center and corner: `b·2 + c·4`.
    pub corner_minus_center: f32,
}

pub fn radial(model: &Model, sets: &[&Pairs]) -> Option<Radial> {
    // Normal equations on [1, r², r⁴].
    let mut ata = [[0.0f64; 3]; 3];
    let mut atb = [0.0f64; 3];
    let mut n = 0usize;
    for set in sets {
        for p in &set.pairs {
            let r = set.radius(p) as f64;
            let row = [1.0, r * r, r * r * r * r];
            let dl = (oklab(decode3(model.apply(p.render)))[0] - oklab(decode3(p.jpeg))[0]) as f64;
            for i in 0..3 {
                for j in 0..3 {
                    ata[i][j] += row[i] * row[j];
                }
                atb[i] += row[i] * dl;
            }
            n += 1;
        }
    }
    if n < 3 {
        return None;
    }
    let x = solve3(ata, atb)?;
    Some(Radial {
        a: x[0] as f32,
        b: x[1] as f32,
        c: x[2] as f32,
        corner_minus_center: (x[1] * 2.0 + x[2] * 4.0) as f32,
    })
}

fn solve3(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    // Gaussian elimination with partial pivoting.
    let mut m = [[0.0f64; 4]; 3];
    for i in 0..3 {
        m[i][..3].copy_from_slice(&a[i]);
        m[i][3] = b[i];
    }
    for col in 0..3 {
        let pivot = (col..3).max_by(|&i, &j| m[i][col].abs().total_cmp(&m[j][col].abs()))?;
        if m[pivot][col].abs() < 1e-18 {
            return None;
        }
        m.swap(col, pivot);
        let pivot_row = m[col];
        for (row, r) in m.iter_mut().enumerate() {
            if row != col {
                let f = r[col] / pivot_row[col];
                for (v, p) in r.iter_mut().zip(&pivot_row).skip(col) {
                    *v -= f * p;
                }
            }
        }
    }
    Some([m[0][3] / m[0][0], m[1][3] / m[1][1], m[2][3] / m[2][2]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::encode3;
    use crate::pairs::Pair;

    #[test]
    fn a_synthetic_radial_term_is_recovered() {
        // A grid of grey blocks, the camera's lightness falling with
        // radius: L_cam = L_dev - 0.02 r² - 0.005 r⁴ in Oklab. The
        // identity model then leaves dL = +0.02 r² + 0.005 r⁴.
        let (rows, cols) = (20usize, 30usize);
        let mut pairs = Vec::new();
        let base_l = 0.6f32;
        for row in 0..rows {
            for col in 0..cols {
                let y = (row as f32 - rows as f32 / 2.0) / (rows as f32 / 2.0);
                let x = (col as f32 - cols as f32 / 2.0) / (cols as f32 / 2.0);
                let r2 = y * y + x * x;
                let l_cam = base_l - 0.02 * r2 - 0.005 * r2 * r2;
                // Grey with Oklab lightness L has linear value L³.
                let dev = encode3([base_l.powi(3); 3]);
                let cam = encode3([l_cam.powi(3); 3]);
                pairs.push(Pair {
                    render: dev,
                    jpeg: cam,
                    row,
                    col,
                });
            }
        }
        let set = Pairs {
            pairs,
            rows,
            cols,
            total: rows * cols,
        };
        let r = radial(&Model::identity(), &[&set]).unwrap();
        assert!(r.a.abs() < 1e-3, "{r:?}");
        assert!((r.b - 0.02).abs() < 2e-3, "{r:?}");
        assert!((r.c - 0.005).abs() < 2e-3, "{r:?}");
        assert!((r.corner_minus_center - 0.06).abs() < 5e-3, "{r:?}");
    }

    #[test]
    fn too_few_blocks_is_none() {
        let set = Pairs {
            pairs: vec![],
            rows: 1,
            cols: 1,
            total: 1,
        };
        assert_eq!(radial(&Model::identity(), &[&set]), None);
    }
}
