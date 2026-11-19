//! Oklab and luminance as the learned fill computes them (SPEC.md,
//! stage 2 and "Arithmetic order").
//!
//! These are not core's [`crate::color::Oklab`]: the fill was trained
//! with its own matrices (the bench's `oklab.json`, Rec.2020 straight
//! to LMS), a signed power with a 1e-9 offset where core takes a cube
//! root, and torch's float32 inverses, whose first column is 1 - 2^-24,
//! 1, and 1 + 2^-23 where core's `LAB_TO_LMS` has 1 exactly. Each
//! constant is the fixture's `oklab/` value, bit for bit.

/// Rec.2020's luma weights, (0.2627, 0.678, 0.0593) as f32.
pub const LUMA: [f32; 3] = [
    f32::from_bits(0x3e86_809d),
    f32::from_bits(0x3f2d_9168),
    f32::from_bits(0x3d72_e48f),
];

/// Linear Rec.2020 to Oklab's LMS, rows.
pub const TO_LMS: [[f32; 3]; 3] = [
    [
        f32::from_bits(0x3f1d_e2ac),
        f32::from_bits(0x3eb8_6c84),
        f32::from_bits(0x3cbc_e218),
    ],
    [
        f32::from_bits(0x3e87_be60),
        f32::from_bits(0x3f22_c654),
        f32::from_bits(0x3dca_d3dc),
    ],
    [
        f32::from_bits(0x3dcc_ff1b),
        f32::from_bits(0x3e50_c87c),
        f32::from_bits(0x3f32_2dfe),
    ],
];

/// Ottosson's rooted LMS to Lab, rows.
pub const LMS_TO_LAB: [[f32; 3]; 3] = [
    [
        f32::from_bits(0x3e57_8152),
        f32::from_bits(0x3f4b_2a89),
        f32::from_bits(0xbb85_6ece),
    ],
    [
        f32::from_bits(0x3ffd_2f0e),
        f32::from_bits(0xc01b_6e0e),
        f32::from_bits(0x3ee6_b438),
    ],
    [
        f32::from_bits(0x3cd4_34b4),
        f32::from_bits(0x3f48_63bb),
        f32::from_bits(0xbf4f_0560),
    ],
];

/// [`TO_LMS`]'s inverse as torch's float32 `linalg.inv` gives it.
pub const FROM_LMS: [[f32; 3]; 3] = [
    [
        f32::from_bits(0x4008_f533),
        f32::from_bits(0xbf9f_8b0c),
        f32::from_bits(0x3dda_0a80),
    ],
    [
        f32::from_bits(0xbf62_7da6),
        f32::from_bits(0x400a_725e),
        f32::from_bits(0xbe8e_97a1),
    ],
    [
        f32::from_bits(0xbd46_ff19),
        f32::from_bits(0xbee8_ac92),
        f32::from_bits(0x3fc0_631d),
    ],
];

/// [`LMS_TO_LAB`]'s inverse as torch's float32 `linalg.inv` gives it.
pub const FROM_LAB: [[f32; 3]; 3] = [
    [
        f32::from_bits(0x3f7f_ffff),
        f32::from_bits(0x3eca_ecc9),
        f32::from_bits(0x3e5c_fbaa),
    ],
    [
        f32::from_bits(0x3f80_0000),
        f32::from_bits(0xbdd8_308b),
        f32::from_bits(0xbd82_c5fa),
    ],
    [
        f32::from_bits(0x3f80_0001),
        f32::from_bits(0xbdb7_437c),
        f32::from_bits(0xbfa5_4f66),
    ],
];

/// The root's power, f32(1/3) = 0.33333334.
pub const ROOT: f32 = 1.0 / 3.0;

/// One row of a matrix product in torch's CPU order, (v0 m0 + v1 m1) +
/// v2 m2, with no fused multiply-add (Rust never contracts).
#[inline]
fn row(m: &[f32; 3], v: [f32; 3]) -> f32 {
    (v[0] * m[0] + v[1] * m[1]) + v[2] * m[2]
}

#[inline]
fn product(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [row(&m[0], v), row(&m[1], v), row(&m[2], v)]
}

/// Luminance Y = (r 0.2627 + g 0.678) + b 0.0593 (SPEC, "Arithmetic
/// order").
#[inline]
pub fn luma(rgb: [f32; 3]) -> f32 {
    row(&LUMA, rgb)
}

/// sign(l) (|l| + 1e-9)^(1/3), the power taken in f64 and rounded:
/// within torch's 2 ulp (SPEC, "The cube root"). sign(0) is 0.
#[inline]
pub fn root(l: f32) -> f32 {
    let p = ((l.abs() + 1e-9f32) as f64).powf(ROOT as f64) as f32;
    if l > 0.0 {
        p
    } else if l < 0.0 {
        -p
    } else {
        0.0
    }
}

/// Linear Rec.2020 to Oklab: lightness, a, b.
#[inline]
pub fn of(rgb: [f32; 3]) -> [f32; 3] {
    product(&LMS_TO_LAB, product(&TO_LMS, rgb).map(root))
}

/// Oklab back to linear Rec.2020: ((lab FROM_LABᵀ)³) FROM_LMSᵀ, the
/// cube as (x x) x.
#[inline]
pub fn rgb(lab: [f32; 3]) -> [f32; 3] {
    product(&FROM_LMS, product(&FROM_LAB, lab).map(|l| (l * l) * l))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::highlight_fill::fixtures::Fixture;

    #[test]
    fn the_constants_are_the_fixtures() {
        let f = Fixture::load("oklab");
        let flat = |m: &[[f32; 3]; 3]| m.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>();
        let bits = |n: &str| f.f32(n).iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(flat(&TO_LMS), bits("to_lms"));
        assert_eq!(flat(&LMS_TO_LAB), bits("lms_to_lab"));
        assert_eq!(flat(&FROM_LMS), bits("from_lms"));
        assert_eq!(flat(&FROM_LAB), bits("from_lab"));
        assert_eq!(LUMA.map(f32::to_bits).to_vec(), bits("luma"));
        assert_eq!(ROOT.to_bits(), 0.333_333_34f32.to_bits());
    }

    #[test]
    fn the_round_trip_holds() {
        for c in [[0.2, 0.5, 0.9], [1.0, 1.0, 1.0], [-0.01, 0.3, 0.02]] {
            let back = rgb(of(c));
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 1e-5, "{c:?} -> {back:?}");
            }
        }
        assert_eq!(root(0.0), 0.0);
    }
}
