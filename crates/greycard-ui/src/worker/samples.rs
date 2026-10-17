//! The real-frame suites: ignored, run on the sample raws and models.

use super::tests::{read_viewport, skipped, thumb_scratch};
use super::*;

/// What keeping the open frame's edit costs, on the raws of the
/// folder `GREYCARD_SAMPLES` names (ignored; run it in release with
/// `--ignored --nocapture`): each raw developed once on the CPU under
/// a brightening, then its pictures made as a save makes them, from
/// the CPU's picture (reduced, straightened, fitted, finished,
/// shrunk, encoded and kept) and, when there is a GPU, the same
/// picture read back from a viewport texture a whole factor smaller.
#[test]
#[ignore]
fn the_edit_s_pictures_of_the_samples() {
    let Some(dir) = std::env::var_os("GREYCARD_SAMPLES") else {
        eprintln!("GREYCARD_SAMPLES names no folder; nothing measured");
        return;
    };
    let mut raws: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| greycard_core::decode::is_raw_path(p))
        .collect();
    raws.sort();
    let gpu = greycard_gpu::Context::own()
        .inspect_err(|e| eprintln!("no GPU for the read back: {e}"))
        .ok();
    let scratch = thumb_scratch("edit-samples");
    for path in raws {
        let (input, _) = timed_open(&path).unwrap();
        let mut edit = Edit::default();
        edit.light.exposure = 1.0;
        let mut base = None;
        let deliver: Deliver = Arc::new(|_| {});
        let (_, image) = develop_job(
            &input,
            &edit,
            0,
            1,
            &mut base,
            &mut None,
            &mut Ai::new(),
            None,
            None,
            &mut None,
            &deliver,
        );
        let image = image.expect("a picture on the CPU");
        let b = base.as_ref().unwrap();
        let source = (image.width as u32, image.height as u32);
        let factor = crate::edited::factor_for(source, &edit.geometry);
        let finish = || crate::edited::Finish {
            edit: edit.clone(),
            turn: 0,
            source,
            full: source,
            rasters: Default::default(),
            clip_level: b.clip_level,
            guide: Some(b.guide.clone()),
            kind: b.source,
            white: b.white_shift(),
            detail: None,
        };
        let cache: ThumbCache = Arc::new(Mutex::new(Some(
            Thumbs::at(
                scratch.join("thumbs"),
                greycard_library::thumbs::DEFAULT_CAP,
            )
            .with_previews(2048, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
        )));
        let edited = crate::edited::Edited::new(cache);
        let hash = greycard_library::hash_file(&path).unwrap();
        let cpu = Instant::now();
        crate::edited::keep(
            &edited,
            crate::edited::Keeping {
                frame: Frame::file(path.clone()),
                hash: hash.clone(),
                key: 1,
                developed: crate::edited::Developed::Full(image.clone()),
                factor,
                read_back: None,
                marked: edited.mark(&hash, 1, crate::edited::EDITED_RECIPE).unwrap(),
                iso: None,
                finish: finish(),
            },
            |_| {},
        );
        let cpu = cpu.elapsed().as_secs_f64();
        let read_back = gpu.as_ref().map(|ctx| {
            let halves = Halves::from_image(&image, None);
            let texture = ctx.viewport_texture(source.0, source.1);
            ctx.queue().write_texture(
                greycard_gpu::wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: greycard_gpu::wgpu::Origin3d::ZERO,
                    aspect: greycard_gpu::wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&halves.pixels),
                greycard_gpu::wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(source.0 * 8),
                    rows_per_image: Some(source.1),
                },
                greycard_gpu::wgpu::Extent3d {
                    width: source.0,
                    height: source.1,
                    depth_or_array_layers: 1,
                },
            );
            ctx.queue().submit([]);
            let started = Instant::now();
            let reduced = crate::edited::read_back_reduced(ctx, &texture, factor).unwrap();
            let read = started.elapsed().as_secs_f64();
            let started = Instant::now();
            crate::edited::keep(
                &edited,
                crate::edited::Keeping {
                    frame: Frame::file(path.clone()),
                    hash: hash.clone(),
                    key: 2,
                    developed: crate::edited::Developed::Reduced(reduced),
                    factor,
                    read_back: Some(read),
                    marked: edited.mark(&hash, 2, crate::edited::EDITED_RECIPE).unwrap(),
                    iso: None,
                    finish: finish(),
                },
                |_| {},
            );
            (read, started.elapsed().as_secs_f64())
        });
        println!(
            "{}: {}x{} ({:.1} MP), factor {factor}: from the CPU's picture {:.0} ms{}",
            path.file_name().unwrap().to_string_lossy(),
            source.0,
            source.1,
            source.0 as f64 * source.1 as f64 / 1e6,
            cpu * 1e3,
            read_back
                .map(|(r, k)| format!(
                    "; from the GPU's, read back {:.0} ms and made {:.0} ms",
                    r * 1e3,
                    k * 1e3
                ))
                .unwrap_or_default(),
        );
    }
    crate::testing::remove_dir_retry(&scratch);
}

/// The Sky shape over real frames, the product's own path: each
/// file opened and developed as the worker does under the default
/// edit, then the Sky raster made by `Ai::raster` twice, once on
/// the providers this machine offers (WebGPU where there is one)
/// and once on the CPU alone, and the two compared. Set
/// `GREYCARD_SAMPLES` to a folder of frames (copies: nothing is
/// written beside them) and `GREYCARD_MODELS` to a store holding
/// the Sky model and SAM, and run with `--ignored`; without both
/// it passes at once. `GREYCARD_SKY_OUT` names a folder for each
/// frame's preview (`<stem>.jpg`), its Sky raster (`<stem>-sky.png`)
/// and `report.txt`; `GREYCARD_SKY_ONLY` a comma list of stems.
/// Fails if any frame with no sky gets a single pixel of sky: the
/// stems in `GREYCARD_SKY_NONE` (a comma list), else the five in the
/// user's set. A frame that will not open or develop is passed over
/// with a line.
#[test]
#[ignore]
fn the_sky_over_real_frames() {
    use greycard_ai::Provider;
    let (Some(samples), Some(models)) = (
        std::env::var_os("GREYCARD_SAMPLES"),
        std::env::var_os("GREYCARD_MODELS"),
    ) else {
        return;
    };
    let out = std::env::var_os("GREYCARD_SKY_OUT").map(PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out).unwrap();
    }
    let only: Option<Vec<String>> = std::env::var("GREYCARD_SKY_ONLY")
        .ok()
        .map(|s| s.split(',').map(str::to_string).collect());
    let no_sky: Vec<String> = std::env::var("GREYCARD_SKY_NONE").map_or_else(
        |_| {
            ["5M0A5135", "DSCF0153", "DSCF0186", "5M0A0957", "5M0A0952"]
                .map(str::to_string)
                .to_vec()
        },
        |s| s.split(',').map(str::to_string).collect(),
    );
    let store = greycard_ai::Store::at(models);
    let mut develop_ai = Ai::for_test(store.clone(), vec![Provider::Cpu]);
    let mut near = Ai::for_test(store.clone(), Provider::available());
    let mut cpu = Ai::for_test(store, vec![Provider::Cpu]);
    let mut frames: Vec<PathBuf> = std::fs::read_dir(&samples)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| greycard_core::decode::is_raw_path(p))
        .collect();
    frames.sort();
    let mut lines = Vec::new();
    let mut painted = Vec::new();
    let shape = Shape::Sky { picks: Vec::new() };
    let deliver: Deliver = Arc::new(|_| {});
    for path in frames {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        if only.as_ref().is_some_and(|o| !o.contains(&stem)) {
            continue;
        }
        let input = match open(&path) {
            Ok((input, _)) => input,
            Err(e) => {
                let line = format!("{stem}: passed over, it will not open: {e:#}");
                println!("{line}");
                lines.push(line);
                continue;
            }
        };
        let mut base = None;
        let t = Instant::now();
        let _ = develop_job(
            &input,
            &Edit::default(),
            0,
            1,
            &mut base,
            &mut None,
            &mut develop_ai,
            None,
            None,
            &mut None,
            &deliver,
        );
        let develop = t.elapsed().as_secs_f64();
        let Some(b) = base.as_ref() else {
            let line = format!("{stem}: passed over, it did not develop");
            println!("{line}");
            lines.push(line);
            continue;
        };
        let run = |ai: &mut Ai| {
            ai.forget(None);
            let t = Instant::now();
            let made = ai
                .raster(
                    b.stamp,
                    &b.image,
                    &b.edit,
                    b.source,
                    b.turn,
                    (1, 0),
                    &shape,
                    None,
                )
                .unwrap_or_else(|e| panic!("{stem}: {e}"));
            let total = t.elapsed().as_secs_f64();
            (made, total, ai.last_sky.clone().expect("a sky report"))
        };
        let (made, total, r) = run(&mut near);
        let (cpu_made, cpu_total, c) = run(&mut cpu);
        let data = made.raster.data();
        let cpu_data = cpu_made.raster.data();
        let n = data.len() as f64;
        let area = data.iter().filter(|&&v| v > 127).count() as f64 / n;
        let (mut max, mut sum, mut flips) = (0u8, 0u64, 0usize);
        for (a, c) in data.iter().zip(cpu_data) {
            max = max.max(a.abs_diff(*c));
            sum += a.abs_diff(*c) as u64;
            flips += ((*a > 127) != (*c > 127)) as usize;
        }
        if no_sky.contains(&stem) && data.iter().any(|&v| v > 0) {
            painted.push(stem.clone());
        }
        let stages = |r: &crate::ai::SkyReport, total: f64| {
            let s = r.stages;
            let known = r.load + r.prior + s.gate + s.seeds + s.outline + s.edge + r.raster;
            format!(
                "total {total:.2}s = preview {:.2} + prior {:.3} on {} + gate {:.3} + seeds {:.3} \
                 + outline {:.3} (SAM load {:.2}, embed {:.3} on {}, {} decodes) + edge {:.3} \
                 + raster {:.3}",
                total - known,
                r.prior,
                r.prior_on.map_or("-", |p| p.name()),
                s.gate,
                s.seeds,
                s.outline,
                r.sam_load,
                r.embed,
                r.sam_on.map_or("-", |p| p.name()),
                r.decodes,
                s.edge,
                r.raster,
            )
        };
        // The matte made from the frame brought down to the
        // preview's size instead of at its own, for the time.
        let at_preview = match (near.sky_prior(), &r.outline) {
            (Some(prior), Some(outline)) => {
                let (w, h) = (prior.width, prior.height);
                let mut small = WorkingImage::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        let (sx, sy) = (x * b.image.width / w, y * b.image.height / h);
                        let from = (sy * b.image.width + sx) * 3;
                        small.data[(y * w + x) * 3..][..3]
                            .copy_from_slice(&b.image.data[from..from + 3]);
                    }
                }
                let t = Instant::now();
                let _ = greycard_ai::matte::color_line(outline, prior, &small, &[], w, h);
                format!("{:.3}s at {w}x{h}", t.elapsed().as_secs_f64())
            }
            _ => "-".to_string(),
        };
        let line = format!(
            "{stem}: {} {:.1}% seeded {} | develop {develop:.1}s | {} | CPU: {} | \
             against the CPU: max {max}/255, mean {:.2e}, {:.4}% across a half | \
             matte at the preview's size {at_preview}, at the frame's {}x{}",
            match r.no_sky {
                Some(why) => format!("no sky ({why})"),
                None => "sky".to_string(),
            },
            area * 100.0,
            r.seeded,
            stages(&r, total),
            stages(&c, cpu_total),
            sum as f64 / n / 255.0,
            flips as f64 / n * 100.0,
            b.image.width,
            b.image.height,
        );
        println!("{line}");
        lines.push(line);
        if let Some(out) = &out {
            let rgb = crate::ai::preview(&b.image, &b.edit, b.source);
            image::save_buffer(
                out.join(format!("{stem}.jpg")),
                &rgb.data,
                rgb.width as u32,
                rgb.height as u32,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
            // The trimap the matte started from: sky white, in
            // question grey, not sky black; and the prior's labels.
            if let (Some(prior), Some(outline)) = (near.sky_prior(), &r.outline) {
                let map: Vec<u8> = greycard_ai::matte::trimap(outline, prior)
                    .into_iter()
                    .map(|k| match k {
                        greycard_ai::matte::Known::Sky => 255,
                        greycard_ai::matte::Known::Unknown => 128,
                        greycard_ai::matte::Known::NotSky => 0,
                    })
                    .collect();
                for (name, data) in [("trimap", &map), ("labels", &prior.labels)] {
                    image::save_buffer(
                        out.join(format!("{stem}-{name}.png")),
                        data,
                        prior.width as u32,
                        prior.height as u32,
                        image::ExtendedColorType::L8,
                    )
                    .unwrap();
                }
            }
            image::save_buffer(
                out.join(format!("{stem}-sky.png")),
                data,
                made.raster.width as u32,
                made.raster.height as u32,
                image::ExtendedColorType::L8,
            )
            .unwrap();
        }
    }
    if let Some(out) = &out {
        std::fs::write(out.join("report.txt"), lines.join("\n") + "\n").unwrap();
    }
    assert!(
        painted.is_empty(),
        "sky painted on frames with none: {painted:?}"
    );
}

/// The Landscape button over real frames, the product's own path:
/// each file opened and developed as the worker does under the default
/// edit, then the button's offer made by `Ai::landscape`, each shape
/// offered made by `Ai::raster`, and the button pressed again; the
/// model must run once a frame for all of it. Set `GREYCARD_SAMPLES` to
/// a folder of frames (copies: nothing is written beside them) and
/// `GREYCARD_MODELS` to a store holding the Sky model and SAM, and run
/// with `--ignored`; without both it passes at once.
/// `GREYCARD_LANDSCAPE_OUT` names a folder for each frame's preview
/// (`<stem>.jpg`) and each shape's raster (`<stem>-<kind>.png`), and
/// `report.txt`; `GREYCARD_LANDSCAPE_ONLY` a comma list of stems.
/// Fails if a stem in `GREYCARD_LANDSCAPE_NONE` (a comma list) is
/// offered anything, if an offered shape comes out empty, or if, where
/// Sky and Foliage are both offered, Foliage holds over a quarter of
/// more than `GREYCARD_LANDSCAPE_MAX_SKY_OVERLAP` (0.02 without it) of
/// the pixels the Sky holds over a half, or the two sum past 1.25
/// anywhere there.
#[test]
#[ignore]
fn the_landscape_over_real_frames() {
    use greycard_ai::Provider;
    let (Some(samples), Some(models)) = (
        std::env::var_os("GREYCARD_SAMPLES"),
        std::env::var_os("GREYCARD_MODELS"),
    ) else {
        return;
    };
    let out = std::env::var_os("GREYCARD_LANDSCAPE_OUT").map(PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out).unwrap();
    }
    let list = |name: &str| -> Option<Vec<String>> {
        std::env::var(name)
            .ok()
            .map(|s| s.split(',').map(str::to_string).collect())
    };
    let only = list("GREYCARD_LANDSCAPE_ONLY");
    let none = list("GREYCARD_LANDSCAPE_NONE").unwrap_or_default();
    let max_over: f64 = std::env::var("GREYCARD_LANDSCAPE_MAX_SKY_OVERLAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.02);
    let store = greycard_ai::Store::at(models);
    let mut develop_ai = Ai::for_test(store.clone(), vec![Provider::Cpu]);
    let mut ai = Ai::for_test(store, Provider::available());
    let mut frames: Vec<PathBuf> = std::fs::read_dir(&samples)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| greycard_core::decode::is_raw_path(p))
        .collect();
    frames.sort();
    let (mut lines, mut wrong) = (Vec::new(), Vec::new());
    let deliver: Deliver = Arc::new(|_| {});
    for path in frames {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        if only.as_ref().is_some_and(|o| !o.contains(&stem)) {
            continue;
        }
        let Ok((input, _)) = open(&path) else {
            println!("{stem}: passed over, it will not open");
            continue;
        };
        let mut base = None;
        let _ = develop_job(
            &input,
            &Edit::default(),
            0,
            1,
            &mut base,
            &mut None,
            &mut develop_ai,
            None,
            None,
            &mut None,
            &deliver,
        );
        let Some(b) = base.as_ref() else {
            println!("{stem}: passed over, it did not develop");
            continue;
        };
        ai.forget(Some(path.clone()));
        let runs = ai.scene_runs;
        let (offers, provider, seconds) = ai
            .landscape(b.stamp, &b.image, &b.edit, b.source)
            .unwrap_or_else(|e| panic!("{stem}: {e}"));
        let (mut made, mut rasters) = (Vec::new(), Vec::new());
        for (i, offer) in offers.iter().enumerate() {
            let kind = crate::panel::landscape::kind_of(offer);
            let shape = Shape::of_kind(kind);
            let t = Instant::now();
            let raster = ai
                .raster(
                    b.stamp,
                    &b.image,
                    &b.edit,
                    b.source,
                    b.turn,
                    (1, i),
                    &shape,
                    None,
                )
                .unwrap_or_else(|e| panic!("{stem}: {kind}: {e}"))
                .raster;
            let took = t.elapsed().as_secs_f64();
            let data = raster.data();
            let area = data.iter().filter(|&&v| v > 127).count() as f64 / data.len() as f64;
            if area == 0.0 {
                wrong.push(format!("{stem}: {kind} offered and empty"));
            }
            made.push(format!("{kind} {:.1}% in {took:.2}s", area * 100.0));
            if let Some(out) = &out {
                image::save_buffer(
                    out.join(format!("{stem}-{}.png", kind.to_lowercase())),
                    data,
                    raster.width as u32,
                    raster.height as u32,
                    image::ExtendedColorType::L8,
                )
                .unwrap();
            }
            rasters.push((kind, raster));
        }
        // Foliage against the sky the Sky shape found: of the pixels
        // the sky holds over a half, the share Foliage holds over a
        // quarter (`over`: a pixel of leaf and sky mixed counts), and
        // the share where the two sum past 1.25 (`both`: claimed
        // twice). Foliage is cut by this same sky, so none may be both.
        let of = |k: &str| rasters.iter().find(|(kind, _)| *kind == k).map(|r| &r.1);
        if let (Some(sky), Some(foliage)) = (of("Sky"), of("Foliage")) {
            let (mut n, mut over, mut both) = (0usize, 0usize, 0usize);
            for (&s, &f) in sky.data().iter().zip(foliage.data()) {
                if s > 127 {
                    n += 1;
                    over += (f > 63) as usize;
                    both += (s as u32 + f as u32 > 319) as usize;
                }
            }
            let (over, both) = (over as f64 / n.max(1) as f64, both as f64 / n.max(1) as f64);
            made.push(format!(
                "Foliage over {:.2}% of the sky, both {:.3}%",
                over * 100.0,
                both * 100.0
            ));
            if both > 0.001 || over > max_over {
                wrong.push(format!(
                    "{stem}: Foliage over {:.2}% of the sky, both {:.3}%",
                    over * 100.0,
                    both * 100.0
                ));
            }
        }
        // Pressed again: the offer from the scene kept.
        let (again, _, _) = ai
            .landscape(b.stamp, &b.image, &b.edit, b.source)
            .unwrap_or_else(|e| panic!("{stem}: {e}"));
        assert_eq!(again, offers, "{stem}: the second press");
        assert_eq!(
            ai.scene_runs,
            runs + 1,
            "{stem}: one model run for everything"
        );
        if none.contains(&stem) && !offers.is_empty() {
            wrong.push(format!("{stem}: offered {offers:?}"));
        }
        let line = format!(
            "{stem}: offered on {} in {seconds:.2}s [{}] | made: {}",
            provider.name(),
            offers
                .iter()
                .map(crate::panel::landscape::words_of)
                .collect::<Vec<_>>()
                .join(", "),
            made.join(", ")
        );
        println!("{line}");
        lines.push(line);
        if let Some(out) = &out {
            let rgb = crate::ai::preview(&b.image, &b.edit, b.source);
            image::save_buffer(
                out.join(format!("{stem}.jpg")),
                &rgb.data,
                rgb.width as u32,
                rgb.height as u32,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        }
    }
    if let Some(out) = &out {
        std::fs::write(out.join("report.txt"), lines.join("\n") + "\n").unwrap();
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The People mask on real frames, through the editor's own path:
/// each frame of `GREYCARD_PARTS_PICTURES` (the trial's JPEGs)
/// opened and developed, its people found, and the parts made on
/// it, with the models of `GREYCARD_MODELS`.
/// - On a portrait an Iris is two small pieces, and a part of the
///   one woman on it, as the panel keeps her, finds her there.
/// - Hers pasted onto her other frame finds her; a group's woman
///   pasted onto the portrait is never decided alone.
/// - On the group, each one's Top resolves alone on her own picture
///   and is hers: centered under her face, and no two overlapping.
/// - A person picked there is still that person after the picture
///   is turned or brightened, and her signature does not move.
/// - A person pasted to a picture of two or more people asks.
/// - With a Subject model in the store, a Whole person's edge is
///   as fine as Subject's, and on the group no two share more
///   than with SAM's edge alone.
///
/// Prints each part's seconds and provider; with
/// `GREYCARD_PARTS_OUT` set, writes each mask there as a PNG.
#[test]
#[ignore]
fn parts_on_real_frames() {
    use crate::ai::Unresolved;
    use greycard_ai::Provider;
    use greycard_edit::mask::{PARTS, Person, Turned};
    let Some(models) = std::env::var_os("GREYCARD_MODELS") else {
        return;
    };
    let dir = std::env::var_os("GREYCARD_PARTS_PICTURES")
        .map(PathBuf::from)
        .expect("GREYCARD_PARTS_PICTURES: the trial's JPEGs, with GREYCARD_MODELS set");
    let out = std::env::var_os("GREYCARD_PARTS_OUT").map(PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out).unwrap();
    }
    let store = greycard_ai::Store::at(models);
    // Whole person's edge is Subject's where the store has it.
    let edge = store.have(&greycard_ai::SUBJECT_WEBGPU) || store.have(&greycard_ai::SUBJECT);
    let mut develop_ai = Ai::for_test(store.clone(), vec![Provider::Cpu]);
    let mut ai = Ai::for_test(store, Provider::available());
    let deliver: Deliver = Arc::new(|_| {});
    // A frame developed under `edit` at `turn`, and the Parts side
    // told it is that file, as opening it tells the editor's.
    let mut develop = |stem: &str, edit: &Edit, turn: u8, ai: &mut Ai| {
        let path = dir.join(format!("{stem}.jpg"));
        let (input, _) = open(&path).unwrap_or_else(|e| panic!("{stem}: {e:#}"));
        let mut base = None;
        let _ = develop_job(
            &input,
            edit,
            turn,
            1,
            &mut base,
            &mut None,
            &mut develop_ai,
            None,
            None,
            &mut None,
            &deliver,
        );
        ai.forget(Some(path));
        base.unwrap_or_else(|| panic!("{stem} did not develop"))
    };
    let save = |name: &str, raster: &Raster| {
        if let Some(out) = &out {
            let height = raster.data().len() / greycard_edit::brush::RASTER_WIDTH;
            image::save_buffer(
                out.join(format!("{name}.png")),
                raster.data(),
                greycard_edit::brush::RASTER_WIDTH as u32,
                height as u32,
                image::ExtendedColorType::L8,
            )
            .unwrap();
        }
    };
    // Each frame's people as a shape stores them, for a sidecar
    // written by hand to look at.
    let dump = |stem: &str, people: &[crate::ai::Candidate]| {
        if let Some(out) = &out {
            let people: Vec<Person> = people.iter().map(|p| p.person()).collect();
            std::fs::write(
                out.join(format!("people-{stem}.json")),
                serde_json::to_string(&people).unwrap(),
            )
            .unwrap();
        }
    };
    let on = |r: &Raster| r.data().iter().filter(|&&v| v > 127).count();
    let share = |r: &Raster| on(r) as f64 / r.data().len() as f64;
    // A mask's soft band: its pixels between a tenth and nine
    // tenths for each pixel of its edge (over a half, a neighbor
    // under it).
    let band = |r: &Raster| {
        let (d, w) = (r.data(), greycard_edit::brush::RASTER_WIDTH);
        let h = d.len() / w;
        let soft = d.iter().filter(|&&v| v > 25 && v < 230).count();
        let mut edge = 0usize;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let under = |j: usize| d[j] <= 127;
                if d[i] > 127
                    && ((x > 0 && under(i - 1))
                        || (x + 1 < w && under(i + 1))
                        || (y > 0 && under(i - w))
                        || (y + 1 < h && under(i + w)))
                {
                    edge += 1;
                }
            }
        }
        soft as f64 / edge.max(1) as f64
    };
    let part = |label: &str, person: Option<Person>| {
        let p = PARTS.iter().find(|p| p.label == label).unwrap();
        Shape::Part {
            phrase: p.phrase.to_string(),
            route: p.route.clone(),
            person,
        }
    };
    let plain = Edit::default();

    // A portrait: every part of everyone, timed; the first run
    // pays for the load and the encoding.
    let b = develop("DSCF0835", &plain, 0, &mut ai);
    let (people, provider, seconds) = ai.people(b.stamp, &b.image, b.source, b.turn).unwrap();
    println!(
        "DSCF0835: {} people on {} in {seconds:.2}s (load and encode)",
        people.len(),
        provider.name()
    );
    assert_eq!(people.len(), 1, "one woman");
    dump("DSCF0835", &people);
    for (i, p) in PARTS.iter().enumerate() {
        let shape = part(p.label, None);
        let made = ai
            .raster(
                b.stamp,
                &b.image,
                &b.edit,
                b.source,
                b.turn,
                (1, i),
                &shape,
                None,
            )
            .unwrap_or_else(|e| panic!("{}: {e}", p.label));
        assert!(made.part.is_none());
        println!(
            "  {:<12} {:>6.2}s on {:<6} {:>6.2}% of the frame",
            p.label,
            made.seconds,
            made.provider.map_or("-", |p| p.name()),
            100.0 * share(&made.raster)
        );
        save(&format!("DSCF0835-{}", p.label), &made.raster);
        if p.label == "Iris" {
            let s = share(&made.raster);
            assert!(s > 0.0 && s < 0.002, "two irises, small: {s}");
        }
    }
    // A part chosen on the portrait is hers, the one person on it
    // kept as a pick is, and on her own picture it finds her.
    let her = people[0].person();
    assert!(her.picture.is_some());
    let made = ai
        .raster(
            b.stamp,
            &b.image,
            &b.edit,
            b.source,
            b.turn,
            (1, PARTS.len()),
            &part("Lips", Some(her.clone())),
            None,
        )
        .unwrap();
    assert!(made.part.is_none(), "hers on her own picture");
    assert!(share(&made.raster) > 0.0, "her lips");
    println!(
        "  Lips, hers: {:.2}s, {:.3}% of the frame",
        made.seconds,
        100.0 * share(&made.raster)
    );
    // Her whole, as the menu's first entry makes it on a portrait:
    // hers, and on a picture of one, all but everyone's.
    let everyone = ai
        .raster(
            b.stamp,
            &b.image,
            &b.edit,
            b.source,
            b.turn,
            (1, 0),
            &part("Whole person", None),
            None,
        )
        .unwrap()
        .raster;
    let made = ai
        .raster(
            b.stamp,
            &b.image,
            &b.edit,
            b.source,
            b.turn,
            (1, PARTS.len() + 1),
            &part("Whole person", Some(her.clone())),
            None,
        )
        .unwrap();
    assert!(made.part.is_none(), "her whole on her own picture");
    save("DSCF0835-person-hers", &made.raster);
    let both = made
        .raster
        .data()
        .iter()
        .zip(everyone.data())
        .filter(|&(&a, &b)| a > 127 && b > 127)
        .count();
    println!(
        "  Whole person, hers: {:.2}s, {:.2}% of the frame, {both} of everyone's {}",
        made.seconds,
        100.0 * share(&made.raster),
        on(&everyone)
    );
    assert!(share(&made.raster) > 0.05, "her, whole");
    assert!(both * 10 > on(&everyone) * 9, "the one person is everyone");
    // Her edge is Subject's: its soft band per edge pixel as
    // narrow as Subject's own, within a sixth (SAM's alone ran 1.3
    // times as wide), and Subject's raster the same run's. Without
    // Subject in the store, SAM's edge, and the status line says so.
    if edge {
        let subject = ai
            .raster(
                b.stamp,
                &b.image,
                &b.edit,
                b.source,
                b.turn,
                (1, PARTS.len() + 2),
                &Shape::Subject {},
                None,
            )
            .unwrap();
        assert_eq!(subject.seconds, 0.0, "Subject's matte, kept from her edge");
        let (hers, theirs) = (band(&made.raster), band(&subject.raster));
        println!("  Whole person's band {hers:.1}, Subject's {theirs:.1}");
        assert!(
            hers < 1.15 * theirs,
            "her band {hers:.1} against {theirs:.1}"
        );
    } else {
        println!(
            "  Whole person's band {:.1}, no Subject",
            band(&made.raster)
        );
        let note = made.note.as_deref().unwrap_or_default();
        assert!(note.contains("Subject model is not downloaded"), "{note}");
    }

    // The group: each person's Top on her own picture is decided
    // alone, and hers.
    let g = develop("5M0A4169", &plain, 0, &mut ai);
    let (group, _, seconds) = ai.people(g.stamp, &g.image, g.source, g.turn).unwrap();
    println!("5M0A4169: {} people in {seconds:.2}s", group.len());
    assert_eq!(group.len(), 5, "the group frame");
    assert!(group.iter().all(|p| p.picture.is_some()));
    dump("5M0A4169", &group);
    let w = greycard_edit::brush::RASTER_WIDTH;
    let aspect = g.image.height as f32 / g.image.width as f32;
    let mut tops = Vec::new();
    for (i, p) in group.iter().enumerate() {
        let shape = part("Top", Some(p.person()));
        let made = ai
            .raster(
                g.stamp,
                &g.image,
                &g.edit,
                g.source,
                g.turn,
                (2, i),
                &shape,
                None,
            )
            .unwrap();
        assert!(made.part.is_none(), "#{i} resolves on her own picture");
        save(&format!("5M0A4169-top-{i}"), &made.raster);
        // Her top's middle, across, is nearer her face than anyone
        // else's.
        let r = made.raster.data();
        let (mut sx, mut n) = (0.0f64, 0usize);
        for (k, &v) in r.iter().enumerate() {
            if v > 127 {
                sx += (k % w) as f64;
                n += 1;
            }
        }
        assert!(n > 0, "#{i} has a top");
        let mid = (sx / n as f64 / w as f64) as f32;
        let nearest = (0..group.len())
            .min_by(|&a, &b| {
                (group[a].at[0] - mid)
                    .abs()
                    .total_cmp(&(group[b].at[0] - mid).abs())
            })
            .unwrap();
        println!(
            "  Top of #{i}: {:.2}s, {:.2}% of the frame, nearest face #{nearest}",
            made.seconds,
            100.0 * share(&made.raster)
        );
        assert_eq!(nearest, i, "#{i}'s top is hers");
        tops.push(made.raster);
    }
    // No two the same, and none taking a piece of another's.
    for i in 0..tops.len() {
        for j in i + 1..tops.len() {
            let both = tops[i]
                .data()
                .iter()
                .zip(tops[j].data())
                .filter(|&(&a, &b)| a > 127 && b > 127)
                .count();
            let least = on(&tops[i]).min(on(&tops[j]));
            println!("  #{i} and #{j} overlap {both} of {least}");
            assert!(both * 50 < least, "#{i} and #{j} overlap {both} of {least}");
        }
    }

    // Each one whole, on her own picture: decided alone, over her
    // own face, and no two sharing much; All people holds them all.
    let mut wholes = Vec::new();
    for (i, p) in group.iter().enumerate() {
        let made = ai
            .raster(
                g.stamp,
                &g.image,
                &g.edit,
                g.source,
                g.turn,
                (9, i),
                &part("Whole person", Some(p.person())),
                None,
            )
            .unwrap();
        assert!(
            made.part.is_none(),
            "#{i} whole resolves on her own picture"
        );
        save(&format!("5M0A4169-person-{i}"), &made.raster);
        let r = made.raster.data();
        let h = r.len() / w;
        let [cx, cy] = [
            (p.face[0] + p.face[2]) / 2.0,
            (p.face[1] + p.face[3]) / 2.0 / aspect,
        ];
        let at = (cy * h as f32) as usize * w + (cx * w as f32) as usize;
        println!(
            "  Whole person #{i}: {:.2}s, {:.2}% of the frame, body {}, {} at her face",
            made.seconds,
            100.0 * share(&made.raster),
            p.body.is_some(),
            r[at]
        );
        assert!(r[at] > 127, "#{i} whole holds her face");
        wholes.push(made.raster);
    }
    // Each one's edge as fine as Subject's, within a sixth: SAM's
    // alone ran to 1.26 times as wide on these five.
    if edge {
        let subject = ai
            .raster(
                g.stamp,
                &g.image,
                &g.edit,
                g.source,
                g.turn,
                (9, group.len() + 1),
                &Shape::Subject {},
                None,
            )
            .unwrap();
        let theirs = band(&subject.raster);
        for (i, one) in wholes.iter().enumerate() {
            let hers = band(one);
            println!("  whole #{i}'s band {hers:.1}, Subject's {theirs:.1}");
            assert!(
                hers < 1.15 * theirs,
                "#{i}'s band {hers:.1} against {theirs:.1}"
            );
        }
    } else {
        for (i, one) in wholes.iter().enumerate() {
            println!("  whole #{i}'s band {:.1}, no Subject", band(one));
        }
    }
    // The same five with SAM's edge alone, under other keys.
    ai.sams_edge_only = true;
    let sams: Vec<Arc<Raster>> = group
        .iter()
        .enumerate()
        .map(|(i, p)| {
            ai.raster(
                g.stamp,
                &g.image,
                &g.edit,
                g.source,
                g.turn,
                (10, i),
                &part("Whole person", Some(p.person())),
                None,
            )
            .unwrap()
            .raster
        })
        .collect();
    ai.sams_edge_only = false;
    let overlap = |a: &Raster, b: &Raster| {
        a.data()
            .iter()
            .zip(b.data())
            .filter(|&(&a, &b)| a > 127 && b > 127)
            .count()
    };
    // No two share more than SAM's edges alone did, and not much.
    for i in 0..wholes.len() {
        for j in i + 1..wholes.len() {
            let both = overlap(&wholes[i], &wholes[j]);
            let before = overlap(&sams[i], &sams[j]);
            let least = on(&wholes[i]).min(on(&wholes[j]));
            println!("  whole #{i} and #{j} overlap {both} of {least}, SAM's {before}");
            assert!(
                both <= before && both * 20 < least,
                "whole #{i} and #{j} overlap {both} of {least}, SAM's {before}"
            );
        }
    }
    let made = ai
        .raster(
            g.stamp,
            &g.image,
            &g.edit,
            g.source,
            g.turn,
            (9, group.len()),
            &part("Whole person", None),
            None,
        )
        .unwrap();
    save("5M0A4169-person-all", &made.raster);
    for (i, one) in wholes.iter().enumerate() {
        let held = one
            .data()
            .iter()
            .zip(made.raster.data())
            .filter(|&(&a, &b)| a > 127 && b > 127)
            .count();
        println!("  All people holds {held} of #{i}'s {}", on(one));
        assert!(held * 10 > on(one) * 9, "All people holds #{i}");
    }

    // One of them after a global edit and a turn: still her, decided
    // on her own picture by where her face was, and her signature
    // the same under the brighter edit.
    let k = 2;
    let first = group[k].person();
    let mut bright = Edit::default();
    bright.light.exposure = 0.7;
    let e = develop("5M0A4169", &bright, 0, &mut ai);
    let (again, _, _) = ai.people(e.stamp, &e.image, e.source, e.turn).unwrap();
    for (a, b) in again.iter().zip(&group) {
        let most = a
            .signature
            .values
            .iter()
            .zip(&b.signature.values)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max);
        assert!(most <= 1e-3, "a signature moved by {most} under +0.7 EV");
    }
    let made = ai
        .raster(
            e.stamp,
            &e.image,
            &bright,
            e.source,
            e.turn,
            (7, 0),
            &part("Top", Some(first.clone())),
            None,
        )
        .unwrap();
    assert!(made.part.is_none(), "brighter: still decided");
    let both = made
        .raster
        .data()
        .iter()
        .zip(tops[k].data())
        .filter(|&(&a, &b)| a > 127 && b > 127)
        .count();
    assert!(both * 10 > on(&tops[k]) * 9, "brighter: the same top");
    println!("+0.7 EV: #{k} decided alone, the same top");

    let t = develop("5M0A4169", &plain, 1, &mut ai);
    let mut turned = part("Top", Some(first));
    turned.turn(Turned::new(1, 1.0 / aspect));
    let made = ai
        .raster(
            t.stamp,
            &t.image,
            &t.edit,
            t.source,
            t.turn,
            (8, 0),
            &turned,
            None,
        )
        .unwrap();
    assert!(made.part.is_none(), "turned: still decided");
    save("5M0A4169-top-turned", &made.raster);
    // Her top, turned: the same cells, a quarter clockwise.
    let tw = w;
    let th = made.raster.data().len() / tw;
    let (gw, gh) = (w, tops[k].data().len() / w);
    let mut hit = 0usize;
    for (idx, &v) in tops[k].data().iter().enumerate() {
        if v > 127 {
            let (x, y) = (idx % gw, idx / gw);
            let (nx, ny) = (
                ((1.0 - (y as f32 + 0.5) / gh as f32) * tw as f32) as usize,
                (((x as f32 + 0.5) / gw as f32) * th as f32) as usize,
            );
            if made.raster.data()[ny.min(th - 1) * tw + nx.min(tw - 1)] > 127 {
                hit += 1;
            }
        }
    }
    println!(
        "turned: #{k} decided alone, {hit} of {} cells",
        on(&tops[k])
    );
    assert!(hit * 10 > on(&tops[k]) * 8, "turned: the same top");

    // The woman of the portrait pasted onto the group: asked
    // about, empty, the people offered to pick from.
    let g = develop("5M0A4169", &plain, 0, &mut ai);
    let made = ai
        .raster(
            g.stamp,
            &g.image,
            &g.edit,
            g.source,
            g.turn,
            (3, 0),
            &part("Lips", Some(her.clone())),
            None,
        )
        .unwrap();
    match &made.part {
        Some(Unresolved::Ask(ask)) => {
            println!("pasted onto the group: asked, guess {:?}", ask.guess);
            assert_eq!(ask.people.len(), 5);
            assert_eq!(ask.why, crate::ai::Asking::Unsure);
        }
        other => panic!("pasted onto the group: {other:?}"),
    }
    assert!(made.raster.data().iter().all(|&v| v == 0));
    // And onto her other frame: found again, her lips.
    let o = develop("DSCF0848", &plain, 0, &mut ai);
    let made = ai
        .raster(
            o.stamp,
            &o.image,
            &o.edit,
            o.source,
            o.turn,
            (4, 0),
            &part("Lips", Some(her)),
            None,
        )
        .unwrap();
    println!(
        "pasted onto DSCF0848: {}, {:.3}% of the frame in {:.2}s",
        if made.part.is_none() {
            "decided"
        } else {
            "not decided"
        },
        100.0 * share(&made.raster),
        made.seconds
    );
    save("DSCF0848-lips-pasted", &made.raster);
    assert!(made.part.is_none() && share(&made.raster) > 0.0);
    // A group frame's person on the portrait: someone else, nobody.
    let b = develop("DSCF0835", &plain, 0, &mut ai);
    let made = ai
        .raster(
            b.stamp,
            &b.image,
            &b.edit,
            b.source,
            b.turn,
            (5, 0),
            &part("Hair", Some(group[0].person())),
            None,
        )
        .unwrap();
    println!(
        "group #0 pasted onto DSCF0835: {}",
        match &made.part {
            None => "decided".to_string(),
            Some(Unresolved::Nobody) => "nobody".to_string(),
            Some(Unresolved::Ask(a)) => format!("asked ({:?}), guess {:?}", a.why, a.guess),
        }
    );
    // Not her: asked as someone else's, the portrait's one face
    // outlined and no guess, never the stranger decided alone.
    match &made.part {
        Some(Unresolved::Ask(ask)) => {
            assert_eq!(ask.why, crate::ai::Asking::SomeoneElse);
            assert_eq!((ask.people.len(), ask.guess), (1, None));
        }
        other => panic!("group #0 pasted onto DSCF0835: {other:?}"),
    }
    assert!(made.raster.data().iter().all(|&v| v == 0));

    // A couple's person pasted onto the other couple: asked.
    let mut couples = Vec::new();
    for stem in ["4Z4A3846", "5M0A0504"] {
        let c = develop(stem, &plain, 0, &mut ai);
        let (people, _, _) = ai.people(c.stamp, &c.image, c.source, c.turn).unwrap();
        println!("{stem}: {} people", people.len());
        dump(stem, &people);
        couples.push((c, people));
    }
    let (c, _) = &couples[1];
    let made = ai
        .raster(
            c.stamp,
            &c.image,
            &c.edit,
            c.source,
            c.turn,
            (6, 0),
            &part("Lips", Some(couples[0].1[0].person())),
            None,
        )
        .unwrap();
    match &made.part {
        Some(Unresolved::Ask(ask)) => {
            println!("4Z4A3846 #0 onto 5M0A0504: asked, guess {:?}", ask.guess);
            assert_eq!(ask.why, crate::ai::Asking::Unsure);
        }
        other => panic!("4Z4A3846 #0 onto 5M0A0504: {other:?}"),
    }
}

/// The viewport against the export with the Dehaze on, on real
/// frames (`GREYCARD_SAMPLES`, a folder of raws; `GREYCARD_DEHAZE_ONLY`,
/// stems to keep, comma-separated). For each frame and each Detail
/// setting, the export's picture is developed on the CPU and the
/// viewport's on the GPU from the same base, so the CA correction
/// is the CPU's on both and what differs is the Detail section and
/// the sharpen alone. Both are finished the export's way (the edit's
/// look, sRGB, eight bits) and compared level by level; the dehaze's
/// airlight and mean transmission from each path are printed beside
/// them. A measurement, not a pass: it prints and holds nothing.
#[test]
#[ignore]
fn the_dehaze_viewport_against_the_export_on_real_frames() {
    let Some(samples) = std::env::var_os("GREYCARD_SAMPLES") else {
        eprintln!("set GREYCARD_SAMPLES to a folder of raws");
        return;
    };
    let only: Option<Vec<String>> = std::env::var("GREYCARD_DEHAZE_ONLY")
        .ok()
        .map(|s| s.split(',').map(str::to_string).collect());
    let Some(ctx) = greycard_gpu::Context::own().ok() else {
        skipped("the dehaze's viewport against its export");
        return;
    };
    let mut gpu = Some(ctx);
    let mut frames: Vec<PathBuf> = std::fs::read_dir(&samples)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| greycard_core::decode::is_raw_path(p))
        .collect();
    frames.sort();
    let deliver: Deliver = Arc::new(|_| {});
    let detail = |dehaze: f32, texture: f32, clarity: f32, sharpen: bool| {
        let mut edit = Edit::default();
        edit.detail.enabled = true;
        edit.detail.dehaze = dehaze;
        edit.detail.texture = texture;
        edit.detail.clarity = clarity;
        edit.sharpen.enabled = sharpen;
        edit
    };
    let cases = [
        ("dehaze +50", detail(0.5, 0.0, 0.0, true)),
        ("dehaze +60", detail(0.6, 0.0, 0.0, true)),
        (
            "dehaze +50, texture and clarity +50",
            detail(0.5, 0.5, 0.5, true),
        ),
        ("dehaze -50, clarity +30", detail(-0.5, 0.0, 0.3, true)),
        ("dehaze +100, sharpen off", detail(1.0, 0.0, 0.0, false)),
    ];
    for path in frames {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        if only.as_ref().is_some_and(|o| !o.contains(&stem)) {
            continue;
        }
        let Ok((input, _)) = open(&path) else {
            println!("{stem}: passed over, it will not open");
            continue;
        };
        let mut base = None;
        for (name, edit) in &cases {
            let run = |base: &mut Option<Base>, gpu: &mut Option<greycard_gpu::Context>| {
                develop_job(
                    &input,
                    edit,
                    0,
                    1,
                    base,
                    &mut None,
                    &mut Ai::new(),
                    None,
                    None,
                    gpu,
                    &deliver,
                )
            };
            let (cpu_outcome, cpu) = run(&mut base, &mut None);
            let (gpu_outcome, _) = run(&mut base, &mut gpu);
            assert!(gpu.is_some(), "{stem}: the GPU failed");
            let cpu = cpu.expect("the CPU's develop gives its picture");
            let Outcome::Developed {
                dehaze: cpu_dehaze, ..
            } = cpu_outcome
            else {
                panic!("{stem}: the CPU's develop failed");
            };
            let Outcome::Developed {
                image,
                dehaze: gpu_dehaze,
                ..
            } = gpu_outcome
            else {
                panic!("{stem}: the GPU's develop failed");
            };
            let viewport = match image {
                Developed::Texture(t) => read_viewport(gpu.as_ref().unwrap(), &t),
                Developed::Halves(h) => {
                    let data: Vec<f32> = h
                        .pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|px| [px[0].to_f32(), px[1].to_f32(), px[2].to_f32()])
                        .collect();
                    WorkingImage::from_data(h.width as usize, h.height as usize, data).unwrap()
                }
            };
            let b = base.as_ref().unwrap();
            let global = crate::finish::Baked::global(edit, b.source);
            let to_out = crate::export::Space::Srgb.matrix();
            let finish = |image: &WorkingImage| -> Vec<u8> {
                crate::finish::finish_with(
                    image,
                    None,
                    &global,
                    &[],
                    |x, y| (x as f32, y as f32),
                    |_, _| (0.0, None),
                    None,
                    None,
                    None,
                    &to_out,
                    |v| (v.clamp(0.0, 1.0) * 255.0).round() as u8,
                )
            };
            let (a, v) = (finish(&cpu), finish(&viewport));
            // The sharpen's dots in the blacks (§268): a pixel whose
            // brightest channel is over 0.8 in a 16-bit finish while
            // the 5x5 mean of its Rec.709 luma is under 0.08.
            let dots = |image: &WorkingImage| -> usize {
                let out: Vec<u16> = crate::finish::finish_with(
                    image,
                    None,
                    &global,
                    &[],
                    |x, y| (x as f32, y as f32),
                    |_, _| (0.0, None),
                    None,
                    None,
                    None,
                    &to_out,
                    |v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16,
                );
                let (w, h) = (image.width, image.height);
                let at = |i: usize| f32::from(out[i]) / 65535.0;
                let luma: Vec<f32> = (0..w * h)
                    .map(|i| 0.2126 * at(i * 3) + 0.7152 * at(i * 3 + 1) + 0.0722 * at(i * 3 + 2))
                    .collect();
                (0..w * h)
                    .filter(|&i| {
                        let (x, y) = (i % w, i / w);
                        let brightest = at(i * 3).max(at(i * 3 + 1)).max(at(i * 3 + 2));
                        if brightest <= 0.8 {
                            return false;
                        }
                        let (mut sum, mut n) = (0f32, 0f32);
                        for yy in y.saturating_sub(2)..(y + 3).min(h) {
                            for xx in x.saturating_sub(2)..(x + 3).min(w) {
                                sum += luma[yy * w + xx];
                                n += 1.0;
                            }
                        }
                        sum / n < 0.08
                    })
                    .count()
            };
            let mut counts = [0usize; 256];
            let mut sq = 0f64;
            for (x, y) in a.iter().zip(&v) {
                let d = x.abs_diff(*y);
                counts[d as usize] += 1;
                sq += f64::from(d) * f64::from(d);
            }
            let n = a.len();
            let worst = counts.iter().rposition(|&c| c > 0).unwrap_or(0);
            let off = n - counts[0];
            let mut seen = 0;
            let p999 = counts
                .iter()
                .position(|&c| {
                    seen += c;
                    seen as f64 >= 0.999 * n as f64
                })
                .unwrap_or(0);
            let fit = |d: &Option<DehazeStats>| {
                d.map_or("none".into(), |d| {
                    format!(
                        "A ({:.5}, {:.5}, {:.5}) t {:.5}",
                        d.airlight[0], d.airlight[1], d.airlight[2], d.transmission_mean
                    )
                })
            };
            println!(
                "{stem} {}x{}, {name}: {:.4}% of samples off, {:.4}% by 2 or more, \
                 99.9th percentile {p999}, worst {worst} levels, RMSE {:.4} levels; \
                 export {} viewport {}; dots, export {} viewport {}",
                cpu.width,
                cpu.height,
                100.0 * off as f64 / n as f64,
                100.0 * (off - counts[1]) as f64 / n as f64,
                (sq / n as f64).sqrt(),
                fit(&cpu_dehaze),
                fit(&gpu_dehaze),
                dots(&cpu),
                dots(&viewport),
            );
        }
    }
}
