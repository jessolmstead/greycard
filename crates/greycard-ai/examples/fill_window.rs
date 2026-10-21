//! Fill a rectangle of a picture as the editor does (at the picture's
//! own pixels where the hole is shallow enough), and the old way beside it (the window squeezed to 512
//! and the answer stretched back), for a look:
//! `fill_window IMAGE X Y W H OUT_DIR` with `GREYCARD_MODELS` set
//! (and `FILL_CPU=1` for the CPU alone).
//!
//! Writes `new.png` and `old-512.png`, each the window round the
//! hole ([`greycard_ai::fill::window`]), and prints the window, the
//! tiles, and the time.

use std::time::Instant;

use greycard_ai::fill::{self, Window};
use greycard_ai::{Fill, Provider, Rgbf, Store};

fn save(img: &Rgbf, path: &std::path::Path) {
    let data: Vec<u8> = img
        .data
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    image::save_buffer(
        path,
        &data,
        img.width as u32,
        img.height as u32,
        image::ExtendedColorType::Rgb8,
    )
    .expect("write");
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 7 {
        eprintln!("usage: fill_window IMAGE X Y W H OUT_DIR");
        std::process::exit(2);
    }
    let n = |i: usize| a[i].parse::<usize>().expect("a number");
    let hole = Window {
        x: n(2),
        y: n(3),
        width: n(4),
        height: n(5),
    };
    let out = std::path::PathBuf::from(&a[6]);
    let rgb = image::open(&a[1]).expect("the picture").to_rgb8();
    let (iw, ih) = (rgb.width() as usize, rgb.height() as usize);
    // A rectangle is as deep as half its shorter side.
    let deep = hole.width.min(hole.height) as f32 / 2.0;
    let w = fill::window(hole, deep, iw, ih);
    let mut data = Vec::with_capacity(w.width * w.height * 3);
    let mut mask = Vec::with_capacity(w.width * w.height);
    for y in w.y..w.y + w.height {
        for x in w.x..w.x + w.width {
            let p = rgb.get_pixel(x as u32, y as u32);
            data.extend(p.0.map(|v| v as f32 / 255.0));
            let inside = (hole.x..hole.x + hole.width).contains(&x)
                && (hole.y..hole.y + hole.height).contains(&y);
            mask.push(if inside { 1.0 } else { 0.0 });
        }
    }
    let input = Rgbf::new(w.width, w.height, data);
    let store = Store::at(std::env::var_os("GREYCARD_MODELS").expect("GREYCARD_MODELS"));
    // FILL_CPU=1 for the CPU alone.
    let providers = if std::env::var_os("FILL_CPU").is_some() {
        vec![Provider::Cpu]
    } else {
        Provider::available()
    };
    let mut model = Fill::load(&store, &providers).expect("load");
    let t = Instant::now();
    let made = model
        .fill_window(&input, &mask, fill::depth(&mask, w.width, w.height))
        .expect("fill");
    println!(
        "window {}x{} at ({}, {}), filled at {:.3} ({}x{}) in {} tile(s) of up to {} on {}, {:.2}s",
        w.width,
        w.height,
        w.x,
        w.y,
        made.scale,
        made.size.0,
        made.size.1,
        made.tiles,
        fill::MAX,
        model.provider().name(),
        t.elapsed().as_secs_f64()
    );
    save(&made.image, &out.join("new.png"));
    // The old way: the window at 512 on its longer side.
    let s = 512.0 / w.width.max(w.height) as f32;
    let (sw, sh) = (
        ((w.width as f32 * s) as usize).next_multiple_of(8),
        ((w.height as f32 * s) as usize).next_multiple_of(8),
    );
    let small = input.resampled(sw, sh);
    let small_hole = greycard_ai::Mask::new(w.width, w.height, mask.clone())
        .resampled(sw, sh)
        .data;
    let filled = model.fill(&small, &small_hole).expect("fill at 512");
    let back = filled.resampled(w.width, w.height);
    let mut old = input.clone();
    for (i, &m) in mask.iter().enumerate() {
        if m > 0.0 {
            old.data[i * 3..i * 3 + 3].copy_from_slice(&back.data[i * 3..i * 3 + 3]);
        }
    }
    save(&old, &out.join("old-512.png"));
}
