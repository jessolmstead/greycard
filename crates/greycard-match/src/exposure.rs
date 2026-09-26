//! The frame's brightness against the camera's, in stops.
//!
//! The camera's JPEG sits at its own exposure per scene. Solved per
//! frame and applied through the Exposure slider before the shared
//! table is fitted, the look carries the style and the brightness
//! stays where the user can see it.

use crate::color::{decode3, luminance};
use crate::pairs::Pairs;

/// The median over the kept blocks of `log2(camera / develop)` in
/// linear luminance: positive when the camera is brighter. `None`
/// when there are no blocks.
pub fn offset_stops(pairs: &Pairs) -> Option<f32> {
    let mut ratios: Vec<f32> = pairs
        .pairs
        .iter()
        .map(|p| {
            let r = luminance(decode3(p.render)).max(1e-6);
            let j = luminance(decode3(p.jpeg)).max(1e-6);
            (j / r).log2()
        })
        .collect();
    if ratios.is_empty() {
        return None;
    }
    ratios.sort_by(f32::total_cmp);
    let n = ratios.len();
    Some(if n % 2 == 1 {
        ratios[n / 2]
    } else {
        (ratios[n / 2 - 1] + ratios[n / 2]) / 2.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::encode3;
    use crate::pairs::Pair;

    #[test]
    fn a_camera_one_stop_darker_reads_minus_one() {
        let pairs = Pairs {
            pairs: (0..5)
                .map(|i| {
                    let lin = 0.1 + 0.1 * i as f32;
                    Pair {
                        render: encode3([lin; 3]),
                        jpeg: encode3([lin / 2.0; 3]),
                        row: 0,
                        col: i,
                    }
                })
                .collect(),
            rows: 1,
            cols: 5,
            total: 5,
        };
        let off = offset_stops(&pairs).unwrap();
        assert!((off + 1.0).abs() < 1e-3, "{off}");
    }

    #[test]
    fn no_blocks_is_no_offset() {
        let pairs = Pairs {
            pairs: vec![],
            rows: 0,
            cols: 0,
            total: 0,
        };
        assert_eq!(offset_stops(&pairs), None);
    }
}
