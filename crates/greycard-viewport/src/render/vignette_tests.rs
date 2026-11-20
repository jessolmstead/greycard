//! The vignette in the scene through the shader: every edit made before
//! its place came in draws the same bytes as before.

use super::*;
use greycard_core::image::WorkingImage;
use greycard_edit::{DisplayCurve, Edit, Vignette};

use crate::testing::device;

/// FNV-1a over the bytes, which no release of anything changes.
fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Every vignette of a sweep (amounts both ways and none, the shape at
/// its ends, the switch off) under both display curves, in the scene,
/// drawn on lavapipe: the bytes the build before the place drew, by
/// their hash, taken from it. Ignored: the bytes are lavapipe's of one
/// Mesa (LLVM 23), and another device or release rounds its own way;
/// run by hand on lavapipe, with `--ignored`, after a change to the
/// shader's vignette.
#[test]
#[ignore = "lavapipe's bytes, by hand"]
fn the_scene_vignette_draws_as_before() {
    let Some((device, queue)) = device("the scene vignette's sweep") else {
        return;
    };
    let (w, h) = (40usize, 28usize);
    let mut image = WorkingImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let i = (y * w + x) * 3;
            let px = if v < 0.3 {
                [6.0 + 2.0 * u, 6.5, 7.0 - u]
            } else {
                [0.02 + 0.6 * u, 0.05 + 0.3 * v, 0.03 + 0.2 * u * v]
            };
            image.data[i..i + 3].copy_from_slice(&px);
        }
    }
    let mut renderer = Renderer::new(&device, &queue);
    renderer.upload(&Halves::from_image(&image, None));
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for curve in [DisplayCurve::Channels, DisplayCurve::Agx] {
        for amount in [-5.0, -2.0, -0.6, 0.0, 0.7, 3.0] {
            for (midpoint, feather, roundness) in
                [(0.5, 0.5, 0.0), (0.1, 0.0, -1.0), (0.8, 1.0, 1.0)]
            {
                for enabled in [true, false] {
                    let edit = Edit {
                        display_curve: curve,
                        vignette: Vignette {
                            enabled,
                            amount,
                            midpoint,
                            feather,
                            roundness,
                            ..Vignette::default()
                        },
                        ..Default::default()
                    };
                    let view = View {
                        center: (w as f32 / 2.0, h as f32 / 2.0),
                        plane: (w as f32, h as f32),
                        frame_size: (w as f32, h as f32),
                        ..View::with_look(&edit)
                    };
                    let target = renderer.render(w as u32, h as u32, &view).0;
                    let shown = renderer.read_back(&target).expect("read back");
                    let bytes: Vec<u8> = shown.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
                    hash = fnv(hash, &bytes);
                }
            }
        }
    }
    assert_eq!(hash, SCENE_HASH, "{hash:#018x}");
}

/// [`the_scene_vignette_draws_as_before`]'s hash on lavapipe, from the
/// build before the vignette's place.
const SCENE_HASH: u64 = 0x5f6d_139e_bb1d_e598;
