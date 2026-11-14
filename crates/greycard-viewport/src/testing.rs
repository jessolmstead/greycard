//! Pictures the scopes' tests here share with the window's tests of
//! their drawing, and a device of the tests' own. Built for this
//! crate's tests and, behind the `testing` feature, for the window's.

use greycard_gpu::wgpu as gpu;

/// A picture whose value rises with the column, grey.
pub fn ramp(w: usize, h: usize) -> Vec<[f32; 3]> {
    (0..w * h)
        .map(|i| {
            let v = (i % w) as f32 / (w - 1) as f32;
            [v; 3]
        })
        .collect()
}

/// A picture of one color.
pub fn flat(w: usize, h: usize, c: [f32; 3]) -> Vec<[f32; 3]> {
    vec![c; w * h]
}

/// A picture with something in every scope: hue across, value
/// down.
pub fn colors(w: usize, h: usize) -> Vec<[f32; 3]> {
    (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
            let t = x * std::f32::consts::TAU;
            [
                y * (0.5 + 0.5 * t.cos()),
                y * (0.5 + 0.5 * (t + 2.1).cos()),
                y * (0.5 + 0.5 * (t + 4.2).cos()),
            ]
        })
        .collect()
}

/// A device of our own, off any window; none without an adapter,
/// and the test says so rather than passing in silence.
pub fn device(what: &str) -> Option<(gpu::Device, gpu::Queue)> {
    let instance = greycard_gpu::instance();
    let got = pollster::block_on(instance.request_adapter(&gpu::RequestAdapterOptions {
        power_preference: gpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()
    .and_then(|adapter| {
        // Which device the tests ran on, once a run: the software
        // rasterizer and a real card round alike in most places and
        // not in all.
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| {
            let info = adapter.get_info();
            eprintln!("GPU tests on {} ({:?})", info.name, info.backend);
        });
        pollster::block_on(adapter.request_device(&gpu::DeviceDescriptor {
            label: Some("viewport test"),
            ..Default::default()
        }))
        .ok()
    });
    if got.is_none() {
        // Under CI the skip is a failure: the runner installs a
        // software adapter so that these tests run, and a run
        // that finds none has lost it, not earned a pass.
        assert!(
            std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
            "{what} has no GPU to run on and GREYCARD_REQUIRE_GPU is set"
        );
        eprintln!("SKIPPED: {what} has no GPU to run on");
        println!("SKIPPED: {what} has no GPU to run on");
    }
    got
}
