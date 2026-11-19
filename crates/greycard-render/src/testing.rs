//! Helpers the tests here share with the editor's own: a scratch
//! folder's removal, a camera profile for the masks' white balances,
//! a camera body by name and a watermark picture. Built for this
//! crate's tests and, behind the `testing` feature, for the editor's.

use std::path::PathBuf;

pub use greycard_core::testing::remove_dir_retry;

/// A camera profile with a warm and a daylight calibration, the
/// matrices of an ordinary sensor (a little wider than Rec.2020 in its
/// reds, a little narrower in its blues), for the masks' white
/// balances in tests that have no raw: the profile and a develop at
/// 5500 K, what [`greycard_edit::WhiteShift`] is made from.
pub fn white_shift() -> greycard_edit::WhiteShift {
    let frame = frame();
    let profile = greycard_core::color::profile_from_frame(&frame).expect("two calibrations");
    let base = greycard_core::color::white_balance_at(
        &profile,
        greycard_core::TempTint {
            cct: 5500.0,
            duv: 0.0,
        },
    )
    .expect("daylight resolves");
    greycard_edit::WhiteShift::new(profile, &base)
}

/// A two by two frame of an ordinary sensor with a warm and a daylight
/// calibration, and no DNG tags: [`white_shift`]'s.
pub fn frame() -> greycard_core::RawFrame {
    use greycard_core::raw::{
        Calibration, CfaPattern, LevelPattern, Levels, Samples, SensorLayout,
    };
    let calibration = |illuminant: u16, m: [f32; 9]| Calibration {
        illuminant,
        color_matrix: m.to_vec(),
        forward_matrix: None,
    };
    greycard_core::RawFrame {
        make: "Test".into(),
        model: "Cam".into(),
        other_names: Vec::new(),
        width: 2,
        height: 2,
        channels: 1,
        layout: SensorLayout::Cfa(CfaPattern::rggb()),
        samples: Samples::U16(vec![0; 4]),
        levels: Levels {
            black: LevelPattern::uniform(0.0, 1),
            white: LevelPattern::uniform(1.0, 1),
            black_deltas: None,
            saturation: None,
        },
        as_shot_coefficients: Some([2.0, 1.0, 1.6]),
        calibrations: vec![
            calibration(
                17,
                [
                    1.1564, -0.4626, -0.1081, -0.4229, 1.2260, 0.2187, -0.0711, 0.1500, 0.6268,
                ],
            ),
            calibration(
                21,
                [
                    0.9766, -0.2953, -0.1254, -0.4276, 1.2116, 0.2433, -0.0437, 0.1336, 0.5131,
                ],
            ),
        ],
        crop: None,
        orientation: Default::default(),
        shot: Default::default(),
        dng: Default::default(),
    }
}

/// A picture `size` high and twice as wide, clear but for a white
/// bar across its middle half, written as a PNG under `dir`.
pub fn bar_png(dir: &std::path::Path, size: u32) -> PathBuf {
    let mut img = image::RgbaImage::new(size * 2, size);
    for (_, y, p) in img.enumerate_pixels_mut() {
        let inside = y >= size / 4 && y < size * 3 / 4;
        p.0 = if inside {
            [255, 255, 255, 255]
        } else {
            [0, 0, 0, 0]
        };
    }
    let path = dir.join("mark.png");
    img.save(&path).unwrap();
    path
}

/// The body the GFX 100S II's raws name, as the decoder reads them.
pub const GFX: (&str, &str) = ("FUJIFILM", "GFX100S II");
