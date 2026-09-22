//! How a demosaic is scored.
//!
//! Everything is computed on 8-bit sRGB, the domain the reference sets are
//! in and the one the literature reports, over an interior region that
//! leaves `border` pixels out on every side, since edge handling is not
//! what is being compared.
//!
//! - **PSNR** per channel and **CPSNR** (one MSE over all three channels),
//!   in dB against a 255 peak.
//! - **Zipper**: Lu and Tan's measure (IEEE TIP 2003). For each pixel the
//!   8-neighbor closest to it in CIELAB is found in the reference; the
//!   pixel is "zippered" if that pair's color distance changed by more
//!   than 2.5 in the output. Reported as a percentage of interior pixels.
//! - **ΔE**: mean CIE76 color difference, and its chroma part on its own
//!   (`Δab`), which is where false color shows up.
//! - **Coarse**: the root mean square of the lightness error averaged
//!   over [`COARSE_BLOCK`]-square blocks, in L* units: the error a
//!   denoiser leaves at the scale of a blotch rather than a grain, which
//!   PSNR barely sees and the eye sees first once the grain is gone.

/// One image's scores.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scores {
    pub psnr: [f64; 3],
    pub cpsnr: f64,
    pub zipper_percent: f64,
    pub delta_e: f64,
    pub delta_ab: f64,
    pub coarse: f64,
}

impl Scores {
    pub fn mean(all: &[Scores]) -> Scores {
        let n = all.len().max(1) as f64;
        let mut m = Scores::default();
        for s in all {
            for c in 0..3 {
                m.psnr[c] += s.psnr[c] / n;
            }
            m.cpsnr += s.cpsnr / n;
            m.zipper_percent += s.zipper_percent / n;
            m.delta_e += s.delta_e / n;
            m.delta_ab += s.delta_ab / n;
            m.coarse += s.coarse / n;
        }
        m
    }
}

pub const ZIPPER_THRESHOLD: f64 = 2.5;
/// Side of the blocks the coarse error is averaged over, in pixels.
pub const COARSE_BLOCK: usize = 16;

/// Score `output` against `reference`; both interleaved 8-bit RGB.
pub fn score(
    reference: &[u8],
    output: &[u8],
    width: usize,
    height: usize,
    border: usize,
) -> Scores {
    assert_eq!(reference.len(), width * height * 3);
    assert_eq!(output.len(), reference.len());
    assert!(
        width > 2 * border && height > 2 * border,
        "border eats the image"
    );

    let (x0, x1, y0, y1) = (border, width - border, border, height - border);
    let count = ((x1 - x0) * (y1 - y0)) as f64;

    let mut sq = [0.0f64; 3];
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * width + x) * 3;
            for c in 0..3 {
                let d = reference[i + c] as f64 - output[i + c] as f64;
                sq[c] += d * d;
            }
        }
    }
    let psnr = sq.map(|s| psnr_from_mse(s / count));
    let cpsnr = psnr_from_mse(sq.iter().sum::<f64>() / (3.0 * count));

    // Lab images for the color measures.
    let ref_lab = to_lab(reference);
    let out_lab = to_lab(output);
    let lab_at = |lab: &[[f64; 3]], x: usize, y: usize| lab[y * width + x];

    let mut zipper = 0usize;
    let mut de_sum = 0.0;
    let mut dab_sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let r = lab_at(&ref_lab, x, y);
            let o = lab_at(&out_lab, x, y);
            de_sum += delta_e(r, o);
            dab_sum += ((r[1] - o[1]).powi(2) + (r[2] - o[2]).powi(2)).sqrt();

            // Nearest neighbor in the reference, by color.
            let mut best = (f64::INFINITY, (x, y));
            for (dx, dy) in NEIGHBORS {
                let (nx, ny) = (x.wrapping_add_signed(dx), y.wrapping_add_signed(dy));
                if nx >= width || ny >= height {
                    continue;
                }
                let d = delta_e(r, lab_at(&ref_lab, nx, ny));
                if d < best.0 {
                    best = (d, (nx, ny));
                }
            }
            let (nx, ny) = best.1;
            let after = delta_e(o, lab_at(&out_lab, nx, ny));
            if (after - best.0).abs() > ZIPPER_THRESHOLD {
                zipper += 1;
            }
        }
    }

    // Lightness error over whole blocks of the interior.
    let mut coarse_sq = 0.0;
    let mut blocks = 0usize;
    for by in (y0..y1).step_by(COARSE_BLOCK) {
        for bx in (x0..x1).step_by(COARSE_BLOCK) {
            if by + COARSE_BLOCK > y1 || bx + COARSE_BLOCK > x1 {
                continue;
            }
            let mut sum = 0.0;
            for y in by..by + COARSE_BLOCK {
                for x in bx..bx + COARSE_BLOCK {
                    sum += lab_at(&out_lab, x, y)[0] - lab_at(&ref_lab, x, y)[0];
                }
            }
            let mean = sum / (COARSE_BLOCK * COARSE_BLOCK) as f64;
            coarse_sq += mean * mean;
            blocks += 1;
        }
    }

    Scores {
        psnr,
        cpsnr,
        zipper_percent: 100.0 * zipper as f64 / count,
        delta_e: de_sum / count,
        delta_ab: dab_sum / count,
        coarse: (coarse_sq / blocks.max(1) as f64).sqrt(),
    }
}

const NEIGHBORS: [(isize, isize); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

fn psnr_from_mse(mse: f64) -> f64 {
    if mse <= 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

fn delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// 8-bit sRGB to CIELAB, D65 white.
fn to_lab(rgb: &[u8]) -> Vec<[f64; 3]> {
    // 8-bit input, so a table beats recomputing the power curve.
    let linear: Vec<f64> = (0..=255)
        .map(|i| {
            let v = i as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        })
        .collect();
    rgb.as_chunks::<3>()
        .0
        .iter()
        .map(|px| {
            let (r, g, b) = (
                linear[px[0] as usize],
                linear[px[1] as usize],
                linear[px[2] as usize],
            );
            // sRGB primaries, D65.
            let x = 0.4124564 * r + 0.3575761 * g + 0.1804375 * b;
            let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
            let z = 0.0193339 * r + 0.1191920 * g + 0.9503041 * b;
            let f = |t: f64| {
                if t > 216.0 / 24389.0 {
                    t.cbrt()
                } else {
                    (24389.0 / 27.0 * t + 16.0) / 116.0
                }
            };
            let (fx, fy, fz) = (f(x / 0.95047), f(y), f(z / 1.08883));
            [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_score_perfectly() {
        let img: Vec<u8> = (0..16 * 16 * 3).map(|i| (i * 7 % 256) as u8).collect();
        let s = score(&img, &img, 16, 16, 2);
        assert!(s.cpsnr.is_infinite() && s.psnr.iter().all(|p| p.is_infinite()));
        assert_eq!(s.zipper_percent, 0.0);
        assert_eq!(s.delta_e, 0.0);
    }

    #[test]
    fn psnr_matches_the_textbook_value() {
        // Every sample off by 1: MSE 1, PSNR 48.13 dB.
        let a = vec![100u8; 8 * 8 * 3];
        let b = vec![101u8; 8 * 8 * 3];
        let s = score(&a, &b, 8, 8, 1);
        assert!((s.cpsnr - 48.1308).abs() < 1e-3, "{}", s.cpsnr);
        assert!((s.psnr[1] - 48.1308).abs() < 1e-3);
        // Uniform shift: no zipper, nonzero ΔE, no chroma change.
        assert_eq!(s.zipper_percent, 0.0);
        assert!(s.delta_e > 0.0 && s.delta_ab < 1e-6);
    }

    #[test]
    fn lab_white_and_grey() {
        let lab = to_lab(&[255, 255, 255, 0, 0, 0, 128, 128, 128]);
        assert!(
            (lab[0][0] - 100.0).abs() < 1e-3 && lab[0][1].abs() < 1e-2 && lab[0][2].abs() < 1e-2
        );
        assert!(lab[1][0].abs() < 1e-6);
        assert!((lab[2][0] - 53.585).abs() < 0.05, "{}", lab[2][0]);
    }

    #[test]
    fn coarse_sees_a_blotch_and_not_a_grain() {
        // A grey image; the output once with every other pixel a step
        // up and down (a grain, which averages out), once with the left
        // half a step up (a blotch, which does not).
        let (w, h) = (64, 48);
        let reference = vec![128u8; w * h * 3];
        let mut grain = reference.clone();
        for (i, px) in grain.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let v = if (i % w + i / w) % 2 == 0 { 138 } else { 118 };
            px.fill(v);
        }
        let mut blotch = reference.clone();
        for (i, px) in blotch.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            if i % w < w / 2 {
                px.fill(138);
            }
        }
        let g = score(&reference, &grain, w, h, 0);
        let b = score(&reference, &blotch, w, h, 0);
        assert!(g.coarse < 0.2, "grain coarse {}", g.coarse);
        // Ten 8-bit steps at mid grey are about four L* units, over
        // half the blocks: the root mean square is that over root two.
        assert!(
            b.coarse > 2.0 && b.coarse < 3.5,
            "blotch coarse {}",
            b.coarse
        );
        assert!(b.psnr[0] > g.psnr[0], "PSNR ranks the blotch better");
    }

    #[test]
    fn zipper_counts_broken_neighbor_pairs() {
        // Flat grey reference; output alternates columns light/dark inside.
        let w = 10;
        let a = vec![128u8; w * w * 3];
        let mut b = a.clone();
        for y in 0..w {
            for x in 0..w {
                let v = if x % 2 == 0 { 100 } else { 156 };
                for c in 0..3 {
                    b[(y * w + x) * 3 + c] = v;
                }
            }
        }
        let s = score(&a, &b, w, w, 1);
        assert!(s.zipper_percent > 99.0, "{}", s.zipper_percent);
    }
}
