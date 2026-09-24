//! Against the real models: set `GREYCARD_MODELS` to a store root that
//! holds them and run with `--ignored`.

use greycard_ai::{
    Fill, Mask, Prompt, Provider, Rgb8, Rgbf, SUBJECT, SUBJECT_WEBGPU, Sam, Store, Subject,
};

fn store() -> Option<Store> {
    std::env::var_os("GREYCARD_MODELS").map(Store::at)
}

/// A dark red disc on light grey, `w`×`h`, centered at (cx, cy) in
/// fractions with radius `r` of the width.
fn disc(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> Rgb8 {
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let dx = (x as f32 + 0.5) / w as f32 - cx;
            let dy = (y as f32 + 0.5) / w as f32 - cy * h as f32 / w as f32;
            if dx * dx + dy * dy < r * r {
                data.extend([150, 30, 30]);
            } else {
                data.extend([200, 200, 200]);
            }
        }
    }
    Rgb8::new(w, h, data)
}

fn mean_in(mask: &greycard_ai::Mask, inside: impl Fn(f32, f32) -> bool) -> f32 {
    let (mut sum, mut n) = (0.0, 0);
    for y in 0..mask.height {
        for x in 0..mask.width {
            let (u, v) = (
                (x as f32 + 0.5) / mask.width as f32,
                (y as f32 + 0.5) / mask.height as f32,
            );
            if inside(u, v) {
                sum += mask.at(x, y);
                n += 1;
            }
        }
    }
    sum / n as f32
}

#[test]
#[ignore]
fn the_subject_model_finds_a_disc() {
    let Some(store) = store() else { return };
    let image = disc(768, 512, 0.4, 0.5, 0.15);
    let t = std::time::Instant::now();
    let mut subject = Subject::load(&store, &Provider::available()).expect("load");
    println!(
        "subject loaded on {} in {:.2}s",
        subject.provider().name(),
        t.elapsed().as_secs_f64()
    );
    let t = std::time::Instant::now();
    let mask = subject.mask(&image).expect("mask");
    println!("subject mask in {:.2}s", t.elapsed().as_secs_f64());
    let disc_in = |u: f32, v: f32| {
        let (dx, dy) = (u - 0.4, (v - 0.5) * 512.0 / 768.0);
        dx * dx + dy * dy < 0.12 * 0.12
    };
    let disc_out = |u: f32, v: f32| {
        let (dx, dy) = (u - 0.4, (v - 0.5) * 512.0 / 768.0);
        dx * dx + dy * dy > 0.2 * 0.2
    };
    let (inside, outside) = (mean_in(&mask, disc_in), mean_in(&mask, disc_out));
    println!("subject: inside {inside:.3} outside {outside:.3}");
    assert!(inside > 0.7 && outside < 0.2);
}

/// The picture for comparing the two Subject files: the one named by
/// `GREYCARD_SUBJECT_PICTURE` (a PNG or JPEG, say a develop of a real
/// frame), else the disc.
fn subject_picture() -> Rgb8 {
    match std::env::var_os("GREYCARD_SUBJECT_PICTURE") {
        Some(path) => {
            let rgb = image::open(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.to_string_lossy()))
                .to_rgb8();
            Rgb8::new(rgb.width() as usize, rgb.height() as usize, rgb.into_raw())
        }
        None => disc(768, 512, 0.4, 0.5, 0.15),
    }
}

/// How far apart two mattes are: the largest difference, the mean,
/// and the share of pixels on different sides of one half.
fn matte_difference(a: &Mask, b: &Mask) -> (f32, f32, f32) {
    let n = a.data.len() as f32;
    let (mut max, mut sum, mut flips) = (0f32, 0f32, 0f32);
    for (x, y) in a.data.iter().zip(&b.data) {
        max = max.max((x - y).abs());
        sum += (x - y).abs();
        flips += ((*x > 0.5) != (*y > 0.5)) as u8 as f32;
    }
    (max, sum / n, flips / n)
}

/// Load `model` on `providers` and make its matte of `picture`, three
/// warm runs after the first; returns the matte and the provider.
fn subject_matte(
    store: &Store,
    model: &'static greycard_ai::Model,
    providers: &[Provider],
    picture: &Rgb8,
) -> (Mask, Provider) {
    let t = std::time::Instant::now();
    let mut subject = Subject::load_model(store, model, providers).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (the WebGPU file is made by tools/ai/birefnet_webgpu.py rewrite, \
             into {}/{}/)",
            model.id,
            store.root().display(),
            model.id
        )
    });
    let load = t.elapsed().as_secs_f64();
    let t = std::time::Instant::now();
    let mut mask = subject.mask(picture).expect("mask");
    let first = t.elapsed().as_secs_f64();
    let mut warm = Vec::new();
    for _ in 0..3 {
        let t = std::time::Instant::now();
        mask = subject.mask(picture).expect("mask");
        warm.push(format!("{:.3}", t.elapsed().as_secs_f64()));
    }
    println!(
        "{} on {}: load {load:.2}s, first {first:.2}s, warm {}s",
        model.id,
        subject.provider().name(),
        warm.join(" ")
    );
    (mask, subject.provider())
}

/// The rewrite for WebGPU is the same function as the original: to
/// the bit on the CPU, and on WebGPU (where the build and the machine
/// offer it) within what fp16 on the card moves an edge.
#[test]
#[ignore]
fn the_webgpu_rewrite_answers_as_the_original() {
    let Some(store) = store() else { return };
    let picture = subject_picture();
    let (original, _) = subject_matte(&store, &SUBJECT, &[Provider::Cpu], &picture);
    let (rewrite, _) = subject_matte(&store, &SUBJECT_WEBGPU, &[Provider::Cpu], &picture);
    let (max, mean, flips) = matte_difference(&original, &rewrite);
    println!("CPU, original against rewrite: max {max:.3e}, mean {mean:.3e}, flips {flips:.3e}");
    assert!(
        max <= 1e-6,
        "the rewrite is not the same function on the CPU"
    );

    if !Provider::available().contains(&Provider::WebGpu) {
        println!("no WebGPU here; the CPU comparison is the whole test");
        return;
    }
    let (gpu, provider) = subject_matte(&store, &SUBJECT_WEBGPU, &[Provider::WebGpu], &picture);
    assert_eq!(provider, Provider::WebGpu);
    let (max, mean, flips) = matte_difference(&original, &gpu);
    println!(
        "original on the CPU against the rewrite on WebGPU: max {max:.3}, mean {mean:.2e}, \
         {:.3}% of pixels across one half",
        flips * 100.0
    );
    // Measured on a 45 MP portrait: mean 4.3e-4, 0.03% across; the
    // differences are an edge moved by fp16, never a region.
    assert!(mean < 3e-3, "mean difference {mean}");
    assert!(flips < 2e-3, "{flips} of the pixels change sides");
}

#[test]
#[ignore]
fn sam_masks_the_disc_from_a_click_or_a_box() {
    let Some(store) = store() else { return };
    let image = disc(768, 512, 0.6, 0.45, 0.12);
    let t = std::time::Instant::now();
    let mut sam = Sam::load(&store, &Provider::available()).expect("load");
    let (e, d) = sam.providers();
    println!(
        "sam loaded on {}/{} in {:.2}s",
        e.name(),
        d.name(),
        t.elapsed().as_secs_f64()
    );
    let t = std::time::Instant::now();
    let embedding = sam.embed(&image).expect("embed");
    println!("embedded in {:.3}s", t.elapsed().as_secs_f64());
    let disc_in = |u: f32, v: f32| {
        let (dx, dy) = (u - 0.6, (v - 0.45) * 512.0 / 768.0);
        dx * dx + dy * dy < 0.1 * 0.1
    };
    let disc_out = |u: f32, v: f32| {
        let (dx, dy) = (u - 0.6, (v - 0.45) * 512.0 / 768.0);
        dx * dx + dy * dy > 0.16 * 0.16
    };
    let t = std::time::Instant::now();
    let (mask, score) = sam
        .decode(
            &embedding,
            &[Prompt::Point {
                x: 0.6,
                y: 0.45,
                positive: true,
            }],
        )
        .expect("decode");
    println!(
        "point decoded in {:.3}s, score {score:.2}",
        t.elapsed().as_secs_f64()
    );
    let (inside, outside) = (mean_in(&mask, disc_in), mean_in(&mask, disc_out));
    println!("point: inside {inside:.3} outside {outside:.3}");
    assert!(inside > 0.8 && outside < 0.1);

    let (mask, score) = sam
        .decode(
            &embedding,
            &[Prompt::Box {
                x0: 0.45,
                y0: 0.25,
                x1: 0.75,
                y1: 0.65,
            }],
        )
        .expect("decode");
    let (inside, outside) = (mean_in(&mask, disc_in), mean_in(&mask, disc_out));
    println!("box: inside {inside:.3} outside {outside:.3} score {score:.2}");
    assert!(inside > 0.8 && outside < 0.1);

    // Two boxes are two objects, joined; the second, on the flat
    // background, must not spoil the first. In one call the model
    // answered with a mask a box.
    let (mask, score) = sam
        .decode(
            &embedding,
            &[
                Prompt::Box {
                    x0: 0.45,
                    y0: 0.25,
                    x1: 0.75,
                    y1: 0.65,
                },
                Prompt::Box {
                    x0: 0.05,
                    y0: 0.05,
                    x1: 0.2,
                    y1: 0.2,
                },
            ],
        )
        .expect("decode two boxes");
    let inside = mean_in(&mask, disc_in);
    println!("two boxes: inside {inside:.3} score {score:.2}");
    assert!(inside > 0.8);

    // Boxes with clicks: the click in a box goes with it, the others
    // make an object of their own. In one call the decoder's reshape
    // failed on this.
    let (mask, score) = sam
        .decode(
            &embedding,
            &[
                Prompt::Box {
                    x0: 0.45,
                    y0: 0.25,
                    x1: 0.75,
                    y1: 0.65,
                },
                Prompt::Box {
                    x0: 0.05,
                    y0: 0.05,
                    x1: 0.2,
                    y1: 0.2,
                },
                Prompt::Point {
                    x: 0.6,
                    y: 0.45,
                    positive: true,
                },
                Prompt::Point {
                    x: 0.9,
                    y: 0.9,
                    positive: true,
                },
                Prompt::Point {
                    x: 0.3,
                    y: 0.9,
                    positive: false,
                },
            ],
        )
        .expect("decode boxes and clicks");
    let inside = mean_in(&mask, disc_in);
    println!("boxes and clicks: inside {inside:.3} score {score:.2}");
    assert!(inside > 0.8);

    // A click on the background, with the disc pushed away.
    let (mask, _) = sam
        .decode(
            &embedding,
            &[
                Prompt::Point {
                    x: 0.15,
                    y: 0.5,
                    positive: true,
                },
                Prompt::Point {
                    x: 0.6,
                    y: 0.45,
                    positive: false,
                },
            ],
        )
        .expect("decode");
    let inside = mean_in(&mask, disc_in);
    println!("background: disc {inside:.3}");
    assert!(inside < 0.2);
}

/// Fetches whatever of the SAM model is missing from the store at
/// `GREYCARD_MODELS`: the download and the hash check, live. Set
/// `GREYCARD_FETCH=1` as well to run it.
#[test]
#[ignore]
fn the_store_fetches_and_checks_a_model() {
    let Some(store) = store() else { return };
    if std::env::var_os("GREYCARD_FETCH").is_none() {
        return;
    }
    let mut reports = 0;
    store
        .fetch(&greycard_ai::SAM, |p| {
            reports += 1;
            assert!(p.done <= p.total.max(p.done));
        })
        .expect("fetch");
    assert!(store.have(&greycard_ai::SAM));
    let note = std::fs::read_to_string(store.dir(&greycard_ai::SAM).join("LICENSE.txt")).unwrap();
    assert!(note.contains("Apache-2.0"));
    println!("{reports} progress reports");
}

/// Fetches every denoiser tier missing from the store at
/// `GREYCARD_MODELS`, live from the published weights, and checks it
/// against the registry's hash. Set `GREYCARD_FETCH=1` as well to run
/// it.
#[test]
#[ignore]
fn the_store_fetches_every_denoiser_tier() {
    let Some(store) = store() else { return };
    if std::env::var_os("GREYCARD_FETCH").is_none() {
        return;
    }
    for (name, model) in greycard_ai::DENOISERS {
        store.fetch(model, |_| {}).expect(name);
        assert!(store.have(model), "{name} not in the store after the fetch");
        let note = std::fs::read_to_string(store.dir(model).join("LICENSE.txt")).unwrap();
        assert!(note.contains("GPL-3.0-or-later"), "{name}: {note}");
    }
}

#[test]
#[ignore]
fn the_fill_model_continues_stripes_through_a_hole() {
    let Some(store) = store() else { return };
    let n = 512usize;
    // Vertical stripes, 32 px wide, with a hole in the middle that
    // holds a flat grey, so the model cannot just hand the hole back.
    let mut data = Vec::with_capacity(n * n * 3);
    for y in 0..n {
        for x in 0..n {
            let v = if (x / 32) % 2 == 0 { 0.85 } else { 0.25 };
            if (200..312).contains(&x) && (200..312).contains(&y) {
                data.extend([0.5, 0.5, 0.5]);
            } else {
                data.extend([v, v * 0.9, v * 0.7]);
            }
        }
    }
    let image = Rgbf::new(n, n, data);
    let hole: Vec<f32> = (0..n * n)
        .map(|i| {
            let (x, y) = (i % n, i / n);
            if (200..312).contains(&x) && (200..312).contains(&y) {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let t = std::time::Instant::now();
    let mut fill = Fill::load(&store, &Provider::available()).expect("load");
    println!(
        "fill loaded on {} in {:.2}s",
        fill.provider().name(),
        t.elapsed().as_secs_f64()
    );
    let t = std::time::Instant::now();
    let out = fill.fill(&image, &hole).expect("fill");
    println!("filled in {:.3}s", t.elapsed().as_secs_f64());
    // Outside the hole the picture comes back; inside, the stripes go on.
    let outside = (out.at(50, 50)[0] - image.at(50, 50)[0]).abs();
    let mut agree = 0;
    for y in 210..300 {
        for x in 210..300 {
            let want = (x / 32) % 2 == 0;
            if (out.at(x, y)[0] > 0.5) == want {
                agree += 1;
            }
        }
    }
    let agree = agree as f32 / (90.0 * 90.0);
    // The amplitude too: a blend with the blanked input would sit low.
    let (mut bright, mut dark, mut nb, mut nd) = (0.0f32, 0.0f32, 0, 0);
    for y in 210..300 {
        for x in 210..300 {
            if (x / 32) % 2 == 0 {
                bright += out.at(x, y)[0];
                nb += 1;
            } else {
                dark += out.at(x, y)[0];
                nd += 1;
            }
        }
    }
    let (bright, dark) = (bright / nb as f32, dark / nd as f32);
    println!(
        "outside error {outside:.3}, stripes agree {agree:.2}, bright {bright:.3} (0.85) dark {dark:.3} (0.25)"
    );
    assert!((bright - 0.85).abs() < 0.1 && (dark - 0.25).abs() < 0.1);
    assert!(outside < 0.1);
    assert!(agree > 0.8, "the stripes should continue: {agree}");
}

/// With `GREYCARD_FILL_DUMP` a directory holding `fill_in.png` and
/// `fill_hole.png` from the editor's dump: how much of the input shows
/// through the model's answer inside the hole. A diagnostic: a hole
/// that only half covers a thing gets that thing continued into it,
/// which correlates, so the bar is only that the answer is not the
/// input itself.
#[test]
#[ignore]
fn the_fill_does_not_show_its_input_through() {
    let Some(store) = store() else { return };
    let Some(dir) = std::env::var_os("GREYCARD_FILL_DUMP") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let input = image::open(dir.join("fill_in.png")).unwrap().into_rgb8();
    let hole = image::open(dir.join("fill_hole.png")).unwrap().into_luma8();
    let n = 512usize;
    let rgb = Rgbf::new(
        n,
        n,
        input.as_raw().iter().map(|&v| v as f32 / 255.0).collect(),
    );
    let mask: Vec<f32> = hole
        .as_raw()
        .iter()
        .map(|&v| if v > 0 { 1.0 } else { 0.0 })
        .collect();
    let mut fill = Fill::load(&store, &Provider::available()).expect("load");
    let out = fill.fill(&rgb, &mask).expect("fill");
    // Correlation between input and output inside the hole, after
    // taking each one's mean out.
    let idx: Vec<usize> = (0..n * n).filter(|&i| mask[i] > 0.0).collect();
    let mean = |v: &dyn Fn(usize) -> f32| idx.iter().map(|&i| v(i)).sum::<f32>() / idx.len() as f32;
    let (mi, mo) = (mean(&|i| rgb.data[i * 3]), mean(&|i| out.data[i * 3]));
    let (mut num, mut di, mut dout) = (0.0f32, 0.0f32, 0.0f32);
    for &i in &idx {
        let (a, b) = (rgb.data[i * 3] - mi, out.data[i * 3] - mo);
        num += a * b;
        di += a * a;
        dout += b * b;
    }
    let corr = num / (di * dout).sqrt().max(1e-9);
    println!(
        "hole pixels {}, input/output correlation inside the hole {corr:.3}",
        idx.len()
    );
    let data: Vec<u8> = out
        .data
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8)
        .collect();
    image::save_buffer(
        dir.join("fill_test_out.png"),
        &data,
        n as u32,
        n as u32,
        image::ExtendedColorType::Rgb8,
    )
    .unwrap();
    assert!(corr < 0.95, "the input came back: {corr}");
}
