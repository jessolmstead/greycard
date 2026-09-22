//! Turn a full-color reference image into what a Bayer sensor would have
//! recorded of it, and the sRGB encoding the metrics score in.

use greycard_core::raw::{CfaColor, CfaPattern};

/// Keep one channel per pixel according to the pattern.
pub fn mosaic(rgb: &[f32], width: usize, height: usize, pattern: &CfaPattern) -> Vec<f32> {
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let c = pattern
                .color_at(y, x)
                .rgb_index()
                .expect("Bayer patterns are RGB");
            out.push(rgb[(y * width + x) * 3 + c]);
        }
    }
    out
}

/// A 2x2 Bayer pattern from its usual four-letter name.
pub fn bayer_pattern(name: &str) -> Option<CfaPattern> {
    let colors: Option<Vec<CfaColor>> = name
        .chars()
        .map(|c| match c.to_ascii_uppercase() {
            'R' => Some(CfaColor::Red),
            'G' => Some(CfaColor::Green),
            'B' => Some(CfaColor::Blue),
            _ => None,
        })
        .collect();
    let colors = colors?;
    if colors.len() != 4 {
        return None;
    }
    CfaPattern::new(2, 2, colors).ok()
}

pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mosaic_picks_the_pattern_channel() {
        // 2x2 image, every pixel (1, 2, 3).
        let rgb: Vec<f32> = [1.0, 2.0, 3.0].repeat(4);
        let cfa = mosaic(&rgb, 2, 2, &bayer_pattern("RGGB").unwrap());
        assert_eq!(cfa, vec![1.0, 2.0, 2.0, 3.0]);
        let cfa = mosaic(&rgb, 2, 2, &bayer_pattern("BGGR").unwrap());
        assert_eq!(cfa, vec![3.0, 2.0, 2.0, 1.0]);
        assert!(bayer_pattern("RGB").is_none());
        assert!(bayer_pattern("RGGX").is_none());
    }

    #[test]
    fn srgb_round_trips() {
        for i in 0..=255 {
            let v = i as f32 / 255.0;
            assert!((srgb_encode(srgb_decode(v)) - v).abs() < 1e-5, "{i}");
        }
    }
}
