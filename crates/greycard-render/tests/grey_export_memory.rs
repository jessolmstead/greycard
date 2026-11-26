//! A grey export's cost in memory over a 100 MP frame against a color
//! one's: the process's peak, read from `/proc/self/status`, after the
//! frame is rendered for a 16-bit TIFF. Ignored: it allocates the frame
//! (1.2 GB) and is a measurement, run by hand, one process a setting:
//! `cargo test -p greycard-render --release --test grey_export_memory -- --ignored --nocapture`
//! with `GREYCARD_GREY_EXPORT` `grey` for Gray Gamma 2.2, `floats` for
//! the frame's finished floats held whole and reduced after, as the
//! grey export first did, or anything else for sRGB.

use greycard_core::image::WorkingImage;
use greycard_edit::Edit;
use greycard_render::export::{Format, Pixels, Settings, Space, render};
use greycard_render::finish::{Baked, Source, finish_reaching};

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
fn peak_memory_of_a_100_mp_grey_export() {
    let (w, h) = (12240usize, 8160usize);
    let mut image = WorkingImage::new(w, h);
    for (i, v) in image.data.iter_mut().enumerate() {
        let p = i / 3;
        let (x, y) = (p % w, p / w);
        *v = 0.05 + 0.4 * (((x / 7) ^ (y / 5)) & 1) as f32 + 0.01 * (i % 3) as f32;
    }
    let mut edit = Edit::default();
    edit.grain.amount = 0.3;
    // The mixer's hue mean is planes of the frame's own, made before the
    // output and larger than it, which would hide the output's share.
    edit.mixer.enabled = false;
    let how = std::env::var("GREYCARD_GREY_EXPORT").unwrap_or_default();
    let space = match how.as_str() {
        "grey" => Space::GreyGamma22,
        _ => Space::Srgb,
    };
    let settings = Settings {
        format: Format::Tiff,
        space,
        ..Settings::default()
    };
    let before = peak_kib();
    let started = std::time::Instant::now();
    let samples = if how == "floats" {
        let global = Baked::global(&edit, Source::SCENE);
        let floats = finish_reaching(
            &image,
            1.0,
            None,
            &global,
            &[],
            |x, y| ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / w as f32),
            |_, _| (0.0, None),
            None,
            None,
            None,
            &Space::Srgb.matrix(),
            |v: f32| v,
        );
        let grey: Vec<u16> = floats
            .as_chunks::<3>()
            .0
            .iter()
            .map(|[r, g, b]| {
                (Space::GreyGamma22.recode(0.2126 * r + 0.7152 * g + 0.0722 * b) * 65535.0).round()
                    as u16
            })
            .collect();
        grey.len()
    } else {
        let r = render(
            &image,
            &edit,
            &greycard_edit::NoStores,
            (w as u32, h as u32),
            &settings,
            &Default::default(),
            1.0,
            None,
            Source::SCENE,
            None,
            None,
        );
        match r.pixels {
            Pixels::Sixteen(p) => p.len(),
            Pixels::Eight(p) => p.len(),
        }
    };
    let after = peak_kib();
    eprintln!(
        "100 MP TIFF, {how:?}: peak {:.0} MB before the render, {:.0} MB after \
         ({:.0} MB over the frame), {:.1} s",
        before as f64 / 1024.0,
        after as f64 / 1024.0,
        (after - before) as f64 / 1024.0,
        started.elapsed().as_secs_f64()
    );
    assert!(samples >= w * h);
}
