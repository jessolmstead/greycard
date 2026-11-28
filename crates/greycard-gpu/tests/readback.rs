//! The upload and its read back: the picture comes back to the bit, and
//! (ignored, by hand) what each costs in time at the editor's frame
//! sizes.
//!
//! Without a GPU adapter each test says so and returns; it does not
//! pass in silence.

use greycard_core::image::WorkingImage;
use greycard_gpu::Context;

fn context() -> Option<Context> {
    match Context::own() {
        Ok(c) => Some(c),
        Err(e) => {
            assert!(
                std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
                "the read-back tests have nothing to run on ({e}) and GREYCARD_REQUIRE_GPU is set"
            );
            println!("SKIPPED: the read-back tests have nothing to run on ({e})");
            None
        }
    }
}

/// A picture of every kind of float a channel can hold: signed, tiny,
/// huge, and each channel its own.
fn picture(width: usize, height: usize) -> WorkingImage {
    let data = (0..width * height * 3)
        .map(|i| {
            let x = (i as u32).wrapping_mul(2_654_435_761);
            f32::from_bits(0x3000_0000 + (x >> 4)) * if i % 7 == 0 { -1.0 } else { 1.0 }
        })
        .collect();
    WorkingImage::from_data(width, height, data).unwrap()
}

/// What goes up comes back to the bit, over a picture of several
/// bands of rows and a width that pads its rows.
#[test]
fn the_upload_reads_back_to_the_bit() {
    let Some(ctx) = context() else { return };
    let image = picture(1531, 2900);
    let up = ctx.upload(&image).unwrap();
    let back = ctx.download(&up).unwrap();
    assert_eq!((back.width, back.height), (image.width, image.height));
    let same = back
        .data
        .iter()
        .zip(&image.data)
        .all(|(a, b)| a.to_bits() == b.to_bits());
    assert!(same, "the picture changed on its way");
}

/// A picture in full floats holds three planes, 12 bytes a pixel, not
/// four channels of 16: the upload and what the local contrast and the
/// dehaze keep, by the device's own account where it gives one
/// (Vulkan and D3D12; not Metal).
#[test]
fn a_picture_in_full_floats_is_three_planes() {
    use greycard_core::develop::dehaze::DehazeOptions;
    use greycard_core::develop::local_contrast::LocalContrastOptions;
    let Some(ctx) = context() else { return };
    let (w, h) = (1024usize, 768usize);
    let image = picture(w, h)
        .data
        .iter()
        .map(|v| v.abs().min(1.0))
        .collect();
    let image = WorkingImage::from_data(w, h, image).unwrap();
    let up = ctx.upload(&image).unwrap();
    let lc = LocalContrastOptions {
        clarity: 0.5,
        ..Default::default()
    };
    let (after_lc, _) = ctx.local_contrast(&up, &lc, f32::INFINITY).unwrap();
    let model = ctx
        .dehaze_reduced(&up)
        .unwrap()
        .model(&DehazeOptions { amount: 0.6 })
        .expect("a model");
    let (after_dehaze, _) = ctx.dehaze(&up, &model).unwrap();
    let Some(report) = ctx.device().generate_allocator_report() else {
        println!("SKIPPED: the device gives no account of its allocations");
        return;
    };
    for label in [
        "developed, full floats",
        "after the local contrast, full floats",
        "after the dehaze, full floats",
    ] {
        let bytes: u64 = report
            .allocations
            .iter()
            .filter(|a| a.name == label)
            .map(|a| a.size)
            .sum();
        let per = bytes as f64 / (w * h) as f64;
        assert!((12.0..13.0).contains(&per), "{label}: {per:.2} B/px");
    }
    drop((up, after_lc, after_dehaze));
}

/// The upload and the read back timed at 24, 45, and 102 MP, three
/// runs each, the best kept.
///
///     VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json WGPU_BACKEND=vulkan \
///     cargo test --release -p greycard-gpu --test readback -- --ignored --nocapture
#[test]
#[ignore = "a measurement: run by hand"]
fn what_a_read_back_costs() {
    let Some(ctx) = context() else { return };
    eprintln!("adapter: {}", ctx.name());
    for (w, h) in [(6000, 4000), (8256, 5504), (12416, 8192)] {
        let image = picture(w, h);
        let mut up_best = f64::MAX;
        let mut down_best = f64::MAX;
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let up = ctx.upload(&image).unwrap();
            // The upload's copies done, the read back alone timed.
            let _ = ctx
                .device()
                .poll(greycard_gpu::wgpu::PollType::wait_indefinitely());
            up_best = up_best.min(t.elapsed().as_secs_f64());
            let t = std::time::Instant::now();
            let back = ctx.download(&up).unwrap();
            down_best = down_best.min(t.elapsed().as_secs_f64());
            assert_eq!(back.data.len(), image.data.len());
        }
        eprintln!(
            "{w}x{h} {:.1} MP: upload {:.0} ms, read back {:.0} ms ({:.2} GB/s of the picture's 12 B/px)",
            (w * h) as f64 / 1e6,
            up_best * 1e3,
            down_best * 1e3,
            (w * h * 12) as f64 / down_best / 1e9
        );
    }
}
