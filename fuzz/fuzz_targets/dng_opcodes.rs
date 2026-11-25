//! A DNG's opcode list, read as each of the three lists and what it
//! asks applied to a small buffer: the gain maps and the sample maps to
//! one plane and three, the bad photosites to a mosaic and to a
//! monochrome plane, the warps to RGB. Never a panic, and samples that
//! start between 0 and 1 stay finite, the sample maps' within 0 and 1.
//! The first four bytes place the buffer in the lists' image; the rest
//! is the list.
#![no_main]

use greycard_core::develop::bad_pixels::{mark_listed, repair};
use greycard_core::develop::gain_map::{MapPlacement, apply_gain_maps};
use greycard_core::develop::remap::apply_remap;
use greycard_core::develop::warp::{WarpPlacement, apply_warps};
use greycard_core::opcodes::{MosaicOp, corrections};
use greycard_core::raw::CfaPattern;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((head, list)) = data.split_first_chunk::<4>() else {
        return;
    };
    let read = corrections(Some(list), Some(list), Some(list));
    let (width, height) = (7, 5);
    let at = MapPlacement {
        x: usize::from(head[0] % 8),
        y: usize::from(head[1] % 8),
        image_width: usize::from(head[2] % 32),
        image_height: usize::from(head[3] % 32),
    };
    let start = |n: usize| -> Vec<f32> { (0..n).map(|i| (i % 11) as f32 / 10.0).collect() };
    let maps: Vec<_> = read.gain_maps().cloned().collect();
    for cpp in [1, 3] {
        let mut samples = start(width * height * cpp);
        let held = apply_gain_maps(&mut samples, width, cpp, at, &maps);
        assert!(held <= samples.len() * maps.len());
        assert!(samples.iter().all(|s| s.is_finite()), "{samples:?}");
        for op in &read.mosaic {
            if let MosaicOp::Remap(map) = op {
                let moved = apply_remap(&mut samples, width, cpp, at, map);
                assert!(moved <= samples.len());
                assert!(
                    samples.iter().all(|s| (0.0..=1.0).contains(s)),
                    "{samples:?}"
                );
            }
        }
    }

    let mut bad = vec![false; width * height];
    for op in &read.mosaic {
        if let MosaicOp::BadPixels(_, fix) = op {
            mark_listed(&mut bad, width, height, at.x, at.y, fix);
        }
    }
    // A mosaic's, and a monochrome sensor's plane's.
    for pattern in [Some(CfaPattern::rggb()), None] {
        let mut mosaic = start(width * height);
        repair(&mut mosaic, width, height, pattern.as_ref(), &bad);
        assert!(mosaic.iter().all(|s| s.is_finite()), "{mosaic:?}");
    }

    let mut rgb = start(width * height * 3);
    apply_warps(
        &mut rgb,
        width,
        height,
        WarpPlacement::binned(at, 1),
        &read.warps,
    );
    assert!(rgb.iter().all(|s| s.is_finite()), "{rgb:?}");
});
