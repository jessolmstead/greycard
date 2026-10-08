//! sRGB in and out, Oklab, and the ΔE the fit is measured in.

use greycard_core::color::{LAB_TO_LMS, LMS_TO_LAB, SRGB_TO_LMS, invert3};
pub use greycard_core::color::{srgb_decode, srgb_encode};

/// Rec.709's luminance weights on linear sRGB.
pub const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

pub fn decode3(v: [f32; 3]) -> [f32; 3] {
    v.map(srgb_decode)
}

pub fn encode3(v: [f32; 3]) -> [f32; 3] {
    v.map(|c| srgb_encode(c.max(0.0)))
}

pub fn luminance(lin: [f32; 3]) -> f32 {
    lin[0] * LUMA[0] + lin[1] * LUMA[1] + lin[2] * LUMA[2]
}

pub fn mul3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

/// Linear sRGB to Oklab, negatives clipped first. A ΔE of 0.02 or so
/// is a just-visible step.
pub fn oklab(lin: [f32; 3]) -> [f32; 3] {
    let lms = mul3(&SRGB_TO_LMS, lin.map(|c| c.max(0.0)));
    mul3(&LMS_TO_LAB, lms.map(f32::cbrt))
}

/// Oklab back to linear sRGB, unclipped.
pub fn from_oklab(lab: [f32; 3]) -> [f32; 3] {
    let lms = mul3(&LAB_TO_LMS, lab).map(|v| v * v * v);
    mul3(&lms_to_srgb(), lms)
}

/// The inverse of [`SRGB_TO_LMS`].
fn lms_to_srgb() -> [[f32; 3]; 3] {
    static INVERSE: std::sync::OnceLock<[[f32; 3]; 3]> = std::sync::OnceLock::new();
    *INVERSE.get_or_init(|| invert3(SRGB_TO_LMS).expect("Oklab's matrix inverts"))
}

/// The distance in Oklab between two linear sRGB colors.
pub fn delta_e(a_lin: [f32; 3], b_lin: [f32; 3]) -> f32 {
    let a = oklab(a_lin);
    let b = oklab(b_lin);
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The mean ΔE over pairs of linear colors.
pub fn mean_delta_e(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    assert_eq!(a.len(), b.len());
    if a.is_empty() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| delta_e(*x, *y)).sum::<f32>() / a.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_has_no_chroma_and_white_is_one() {
        let w = oklab([1.0, 1.0, 1.0]);
        assert!((w[0] - 1.0).abs() < 1e-3, "{w:?}");
        assert!(w[1].abs() < 1e-3 && w[2].abs() < 1e-3, "{w:?}");
        let g = oklab([0.18, 0.18, 0.18]);
        assert!(g[1].abs() < 1e-3 && g[2].abs() < 1e-3, "{g:?}");
    }

    #[test]
    fn oklab_goes_back() {
        for c in [
            [0.2, 0.5, 0.1],
            [0.9, 0.05, 0.3],
            [0.18, 0.18, 0.18],
            [1.0, 1.0, 1.0],
        ] {
            let back = from_oklab(oklab(c));
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 1e-5, "{c:?} came back {back:?}");
            }
        }
    }

    #[test]
    fn encode_and_decode_round_trip() {
        for v in [0.0, 0.001, 0.01, 0.18, 0.5, 1.0] {
            assert!((srgb_decode(srgb_encode(v)) - v).abs() < 1e-6);
        }
    }

    #[test]
    fn delta_e_is_zero_on_itself_and_symmetric() {
        let a = [0.2, 0.5, 0.1];
        let b = [0.25, 0.45, 0.12];
        assert_eq!(delta_e(a, a), 0.0);
        assert!((delta_e(a, b) - delta_e(b, a)).abs() < 1e-7);
    }
}
