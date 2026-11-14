//! The output sharpen's cost in memory over a 45 MP frame: the
//! process's peak, read from `/proc/self/status`, after a finish with
//! the sharpen on. Ignored: it allocates the frame (half a gigabyte)
//! and is a measurement, run by hand, one process a setting:
//! `cargo test -p greycard-render --release --test output_sharpen_memory -- --ignored --nocapture`
//! with `GREYCARD_SHARPEN_AMOUNT` 0 for the frame without it.

use greycard_core::image::WorkingImage;
use greycard_edit::Edit;
use greycard_render::finish::{Baked, Source, finish_with};

fn peak_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

#[test]
#[ignore]
fn peak_memory_of_a_45_mp_finish() {
    let (w, h) = (8256usize, 5504usize);
    let mut image = WorkingImage::new(w, h);
    for (i, v) in image.data.iter_mut().enumerate() {
        let p = i / 3;
        let (x, y) = (p % w, p / w);
        *v = 0.05 + 0.4 * (((x / 7) ^ (y / 5)) & 1) as f32 + 0.01 * (i % 3) as f32;
    }
    let amount: f32 = std::env::var("GREYCARD_SHARPEN_AMOUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let mut edit = Edit::default();
    edit.output_sharpen.amount = amount;
    edit.output_sharpen.radius = 3.0;
    let global = Baked::global(&edit, Source::SCENE);
    let before = peak_kib();
    let started = std::time::Instant::now();
    let out = finish_with(
        &image,
        None,
        &global,
        &[],
        |x, y| ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32),
        |_, _| (0.0, None),
        None,
        None,
        None,
        &greycard_render::export::Space::Srgb.matrix(),
        |v: f32| (v * 255.0).round() as u8,
    );
    let after = peak_kib();
    eprintln!(
        "45 MP finish, amount {amount}: peak {:.0} MB before the finish, {:.0} MB after \
         ({:.0} MB over the frame and its output), {:.1} s",
        before as f64 / 1024.0,
        after as f64 / 1024.0,
        (after - before) as f64 / 1024.0,
        started.elapsed().as_secs_f64()
    );
    assert_eq!(out.len(), w * h * 3);
}
