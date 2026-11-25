//! A stack made again from the places it kept: blended where each
//! frame was put, nothing fitted, the same picture to the bit.

use greycard_core::CameraImage;
use greycard_core::error::Error;
use greycard_core::stack::{self, Options, Reference, Step};

/// Frame `k` of a stack of a texture, moved a little more each frame,
/// its finest detail fainter the farther it is from frame 2, as focus
/// moves.
fn frame(k: usize, w: usize, h: usize) -> CameraImage {
    let dx = k as f32 * 0.3;
    let sharp = 1.0 / (1.0 + (k as f32 - 2.0).abs());
    let data = (0..w * h)
        .flat_map(|i| {
            let (x, y) = ((i % w) as f32 + dx, (i / w) as f32);
            let v = 0.3
                + 0.05 * sharp * (x * 0.7).sin() * (y * 0.5).cos()
                + 0.08 * (x * 0.13).sin() * (y * 0.11).cos()
                + 0.05 * (x * 0.05 + y * 0.03).sin();
            [v * 0.6, v, v * 0.8]
        })
        .collect();
    CameraImage::from_data(w, h, data).unwrap()
}

#[test]
fn a_stack_placed_where_it_was_is_the_same_to_the_bit() {
    let (w, h) = (160, 120);
    let frames: Vec<CameraImage> = (0..5).map(|k| frame(k, w, h)).collect();
    let options = Options {
        reference: Reference::Index(2),
        ..Options::default()
    };
    let (first, report) = stack::stack_camera(&frames, &options).unwrap();
    let used = report.used();
    assert!(used >= 3, "{:#?}", report.frames);
    let placed: Vec<_> = report
        .frames
        .iter()
        .map(|f| f.used().then_some(f.transform))
        .collect();
    let reads = std::cell::Cell::new(0);
    let mut steps = Vec::new();
    let (again, placed_report) = stack::stack_camera_placed(
        2,
        &placed,
        |k| {
            reads.set(reads.get() + 1);
            Ok::<_, Error>(frames[k].clone())
        },
        |s| steps.push(s),
        &options,
    )
    .unwrap();
    assert!(
        first
            .data
            .iter()
            .zip(&again.data)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
    assert_eq!(placed_report.coverage, report.coverage);
    assert_eq!(reads.get(), used, "each frame the merge used read once");
    assert_eq!(steps.first(), Some(&Step::Blending { index: 1, of: used }));

    // A frame left out is not read, and is said dropped.
    let mut fewer = placed.clone();
    let out = (0..5).find(|&k| k != 2 && fewer[k].is_some()).unwrap();
    fewer[out] = None;
    reads.set(0);
    let (_, r) = stack::stack_camera_placed(
        2,
        &fewer,
        |k| {
            reads.set(reads.get() + 1);
            Ok::<_, Error>(frames[k].clone())
        },
        |_| {},
        &options,
    )
    .unwrap();
    assert_eq!((reads.get(), r.used()), (used - 1, used - 1));
    // The reference must be placed.
    fewer[2] = None;
    assert!(
        stack::stack_camera_placed(
            2,
            &fewer,
            |k| Ok::<_, Error>(frames[k].clone()),
            |_| {},
            &options
        )
        .is_err()
    );
}
