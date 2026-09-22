//! A model's mask, made at its own small square, brought to the
//! picture's edges: the guided filter of He, Sun and Tang (ECCV 2010),
//! with the picture's luma as the guide. Where the guide has an edge
//! the mask snaps to it; where it is flat the mask is smoothed.
//!
//! The filter itself is `greycard_core::guided`: the tone equalizer
//! wants it too and the engine may not depend on this crate.

use crate::image::{Mask, Rgb8};

impl Rgb8 {
    /// Luma, 0 to 1, from the encoded values (good enough for a guide).
    pub fn luma(&self) -> Vec<f32> {
        self.data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0)
            .collect()
    }
}

/// `mask` resampled to the guide's size and filtered against `guide`
/// (a luma plane `width`×`height`) with window radius `radius` and
/// regularizer `eps`. Output in 0 to 1.
pub fn refine(
    mask: &Mask,
    guide: &[f32],
    width: usize,
    height: usize,
    radius: usize,
    eps: f32,
) -> Mask {
    assert_eq!(guide.len(), width * height);
    let p = mask.resampled(width, height).data;
    let out = greycard_core::guided::filter(guide, Some(&p), width, height, radius, eps);
    Mask::new(
        width,
        height,
        out.into_iter().map(|v| v.clamp(0.0, 1.0)).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mask_snaps_to_the_guides_edge() {
        // A luma step at x = 32 of 64; a mask that is a wide ramp there.
        let (w, h) = (64usize, 16usize);
        let guide: Vec<f32> = (0..w * h)
            .map(|k| if k % w < 32 { 0.2 } else { 0.8 })
            .collect();
        let ramp: Vec<f32> = (0..w * h)
            .map(|k| ((k % w) as f32 - 20.0) / 24.0)
            .map(|v| v.clamp(0.0, 1.0))
            .collect();
        let mask = Mask::new(w, h, ramp.clone());
        let out = refine(&mask, &guide, w, h, 6, 1e-4);
        // Sharper: the values two texels either side of the edge are
        // further apart than the ramp's.
        let before = ramp[8 * w + 34] - ramp[8 * w + 29];
        let after = out.at(34, 8) - out.at(29, 8);
        assert!(after > before + 0.1, "before {before} after {after}");
        assert!(out.data.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn a_flat_guide_smooths_the_mask() {
        let (w, h) = (32usize, 8usize);
        let guide = vec![0.5f32; w * h];
        let mut spiky = vec![0.0f32; w * h];
        spiky[4 * w + 16] = 1.0;
        let out = refine(&Mask::new(w, h, spiky), &guide, w, h, 2, 1e-3);
        assert!(out.at(16, 4) < 0.2 && out.at(16, 4) > 0.0);
        assert!(out.at(15, 4) > 0.0);
    }
}
