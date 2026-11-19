//! The fill's Gaussian blur (SPEC.md stage 10, `blur` and `gauss`, and
//! "Precision with an arbitrary f"): the synthesis's depth and
//! hand-over fields are made with it.

use rayon::prelude::*;

/// sg(σ) = max(0.5, σ) at S = 1, in f64.
#[inline]
pub fn sg(sigma: f64) -> f64 {
    sigma.max(0.5)
}

/// The normalized kernel for σ (already through [`sg`]): radius
/// r = int(3σ) in f64, k(t) = exp(−0.5·(t/σ₃₂)²) in f32 for t in
/// [−r, r], over its f32 sum.
pub fn kernel(sigma: f64) -> Vec<f32> {
    let r = (3.0 * sigma) as i64;
    let s = sigma as f32;
    let k: Vec<f32> = (-r..=r)
        .map(|t| {
            let u = t as f32 / s;
            (-0.5 * (u * u)).exp()
        })
        .collect();
    let total: f32 = k.iter().sum();
    k.iter().map(|v| v / total).collect()
}

/// `v` (`width` × `height`, row-major) blurred with σ (through [`sg`]):
/// a horizontal pass, then a vertical one, each with the edge
/// replicated, taps summed in order. Every output alone, so the same
/// on any thread count.
pub fn blur(v: &[f32], width: usize, height: usize, sigma: f64) -> Vec<f32> {
    let k = kernel(sg(sigma));
    let r = (k.len() / 2) as isize;
    let mut across = vec![0.0f32; v.len()];
    if width == 0 || height == 0 {
        return across;
    }
    let clamp = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;
    across
        .par_chunks_mut(width)
        .zip(v.par_chunks(width))
        .for_each(|(out, row)| {
            for (x, o) in out.iter_mut().enumerate() {
                let mut s = 0.0f32;
                for (j, &kj) in k.iter().enumerate() {
                    s += row[clamp(x as isize + j as isize - r, width)] * kj;
                }
                *o = s;
            }
        });
    let mut out = vec![0.0f32; v.len()];
    out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0.0f32;
            for (j, &kj) in k.iter().enumerate() {
                s += across[clamp(y as isize + j as isize - r, height) * width + x] * kj;
            }
            *o = s;
        }
    });
    out
}

/// blurN(v, m) = blur(v·m, 3) / max(blur(m, 3), 1e-3) · m (SPEC stage 10).
pub fn blur_normalized(v: &[f32], m: &[bool], width: usize, height: usize) -> Vec<f32> {
    let vm: Vec<f32> = v
        .iter()
        .zip(m)
        .map(|(&a, &b)| a * b as u32 as f32)
        .collect();
    let mf: Vec<f32> = m.iter().map(|&b| b as u32 as f32).collect();
    let mut top = blur(&vm, width, height, super::SIGMA_DEPTH);
    drop(vm);
    let bottom = blur(&mf, width, height, super::SIGMA_DEPTH);
    for ((t, &b), &m) in top.iter_mut().zip(&bottom).zip(&mf) {
        *t = *t / b.max(1e-3) * m;
    }
    top
}
