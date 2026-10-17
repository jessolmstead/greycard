//! The landscape's groups against the real model: set `GREYCARD_MODELS`
//! to a store that holds the Sky model (and SAM, for the sky as the Sky
//! shape makes it) and run with `--ignored`. Without the variable each
//! test returns at once and passes, as the other model tests do.
//!
//! `GREYCARD_LANDSCAPE_PREVIEWS` names a folder of the models'
//! previews, `<stem>.png` (the editor's, at most 2048 on the long
//! side), and beside each, optionally, its linear base as `<stem>.lin`
//! (b"GCLN", u32 width, u32 height, then f16 RGB in the working space,
//! of the preview's aspect) for the matte. `GREYCARD_LANDSCAPE_ONLY` is
//! a comma list of stems to run alone; `GREYCARD_LANDSCAPE_NONE` a
//! comma list of stems on which nothing may be offered;
//! `GREYCARD_LANDSCAPE_OUT` a folder for each shape made, as a PNG;
//! `GREYCARD_LANDSCAPE_MAX_SKY_OVERLAP` the share of the found sky a
//! group the sky caps may reach into (0.02 without it).

use std::path::{Path, PathBuf};
use std::time::Instant;

use greycard_ai::landscape::{self, Found, Group, Kind, Measure, Scene};
use greycard_ai::sky::{self, Frame};
use greycard_ai::{Mask, Provider, Rgb8, Sam, Sky, Store};
use greycard_core::image::WorkingImage;

fn read_lin(path: &Path) -> Option<WorkingImage> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 12 || &bytes[..4] != b"GCLN" {
        return None;
    }
    let w = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let h = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if bytes.len() != 12 + w * h * 6 {
        return None;
    }
    let mut image = WorkingImage::new(w, h);
    for (o, b) in image.data.iter_mut().zip(bytes[12..].as_chunks::<2>().0) {
        *o = half::f16::from_le_bytes(*b).to_f32();
    }
    Some(image)
}

/// The trial's core, for comparison: the soft map over 0.8 eroded,
/// whether or not the pixels are labeled the group.
fn trial_core(scene: &Scene, group: Group) -> f32 {
    let (w, h) = (scene.prior.width, scene.prior.height);
    let r = ((0.01 * w.max(h) as f32).round() as usize).max(1);
    let m: Vec<bool> = scene.soft(group).iter().map(|&v| v > 0.8).collect();
    sky::erode(&m, w, h, r).iter().filter(|&&v| v).count() as f32 / (w * h) as f32
}

fn list(name: &str) -> Option<Vec<String>> {
    std::env::var(name)
        .ok()
        .map(|s| s.split(',').map(str::to_string).collect())
}

/// The previews of the folder, by stem, `GREYCARD_LANDSCAPE_ONLY`'s
/// alone when it is set.
fn previews(dir: &Path) -> Vec<(String, PathBuf)> {
    let only = list("GREYCARD_LANDSCAPE_ONLY");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .map(|p| (p.file_stem().unwrap().to_string_lossy().into_owned(), p))
        .filter(|(s, _)| only.as_ref().is_none_or(|o| o.contains(s)))
        .collect();
    paths.sort();
    paths
}

fn picture(path: &Path) -> Rgb8 {
    let rgb = image::open(path).unwrap().to_rgb8();
    Rgb8::new(rgb.width() as usize, rgb.height() as usize, rgb.into_raw())
}

fn scene_of(model: &mut Sky, picture: &Rgb8) -> (Scene, f64, f64) {
    let t = Instant::now();
    let boxed = sky::letterbox(picture);
    let content = (boxed.width, boxed.height);
    let logits = model.logits(boxed.planes).expect("the model");
    let t_model = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let scene = Scene::label(&logits, content, picture.width, picture.height);
    (scene, t_model, t.elapsed().as_secs_f64())
}

/// Every frame of the folder: the offer, and each group's measure and
/// gate; with the linear base beside it, the sky as the Sky shape makes
/// it, and each group offered made and timed, with how much of that
/// sky it reaches into (over a quarter, where the sky is over a half).
/// Frames named in `GREYCARD_LANDSCAPE_NONE` must be offered nothing,
/// and Foliage, which the found sky cuts, must reach into no more than
/// `GREYCARD_LANDSCAPE_MAX_SKY_OVERLAP` of the sky on any frame.
#[test]
#[ignore]
fn the_landscape_over_real_frames() {
    let Some(store) = std::env::var_os("GREYCARD_MODELS").map(Store::at) else {
        return;
    };
    let Some(dir) = std::env::var_os("GREYCARD_LANDSCAPE_PREVIEWS").map(PathBuf::from) else {
        println!("no GREYCARD_LANDSCAPE_PREVIEWS: nothing to run over");
        return;
    };
    let none = list("GREYCARD_LANDSCAPE_NONE").unwrap_or_default();
    let max_overlap: f32 = std::env::var("GREYCARD_LANDSCAPE_MAX_SKY_OVERLAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.02);
    let out = std::env::var_os("GREYCARD_LANDSCAPE_OUT").map(PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out).unwrap();
    }
    let mut model = Sky::load(&store, &Provider::available()).expect("the Sky model");
    println!("Sky model on {}", model.provider().name());
    let mut sam = store
        .have(&greycard_ai::SAM)
        .then(|| Sam::load(&store, &Provider::available()).expect("SAM"));
    let mut wrong = Vec::new();
    for (stem, path) in previews(&dir) {
        let picture = picture(&path);
        let (w, h) = (picture.width, picture.height);
        let (scene, t_model, t_label) = scene_of(&mut model, &picture);
        let offers = scene.offers();
        let said: Vec<String> = offers
            .iter()
            .map(|o| {
                let name = match o.kind {
                    Kind::Sky => "sky",
                    Kind::Group(g) => g.name(),
                };
                format!("{name} {:.1}%", o.area * 100.0)
            })
            .collect();
        let groups: Vec<String> = Group::ALL
            .into_iter()
            .filter(|&g| scene.measure(g).area > 0.001)
            .map(|g| {
                let m = scene.measure(g);
                format!(
                    "{}:{:.1}%/{:.3}/c{:.1}(trial c{:.1}){}",
                    g.name(),
                    m.area * 100.0,
                    m.mean,
                    m.core * 100.0,
                    trial_core(&scene, g) * 100.0,
                    if scene.gate(g).is_ok() { "" } else { "x" }
                )
            })
            .collect();
        println!(
            "{stem}: model {t_model:.3}s labels {t_label:.3}s | sky {} | offered [{}] | {}",
            match sky::gate(&scene.prior) {
                Ok(()) => "passes".to_string(),
                Err(why) => why.to_string(),
            },
            said.join(", "),
            groups.join(" ")
        );
        if none.contains(&stem) && !offers.is_empty() {
            wrong.push(format!("{stem}: offered {offers:?}"));
        }
        let Some(linear) = read_lin(&path.with_extension("lin")) else {
            continue;
        };
        let luma = picture.luma();
        let frame = Frame {
            display: &picture,
            luma: &luma,
            linear: &linear,
        };
        // The sky as the Sky shape makes it with no picks (SAM's
        // outline when SAM is in the store), which is also what the
        // editor cuts Foliage by.
        let t = Instant::now();
        let the_sky = sky_shape(&scene, &frame, &picture, (w, h), &mut sam);
        let cut = the_sky.clone();
        if the_sky.is_some() {
            println!("  sky: made in {:.3}s", t.elapsed().as_secs_f64());
        }
        for offer in &offers {
            let Kind::Group(g) = offer.kind else {
                continue;
            };
            let t = Instant::now();
            let Found::Shape { matte, matted } =
                landscape::find(&scene, g, &frame, (w, h), cut.as_ref())
            else {
                panic!("{stem}: {} offered and not made", g.name());
            };
            let took = t.elapsed().as_secs_f64();
            let labels = scene.labeled(g);
            let n = labels.data.iter().filter(|&&v| v > 0.5).count().max(1) as f32;
            let (mut kept, mut spill) = (0usize, 0usize);
            for (m, l) in matte.data.iter().zip(&labels.data) {
                match (*m > 0.5, *l > 0.5) {
                    (true, true) => kept += 1,
                    (true, false) => spill += 1,
                    _ => {}
                }
            }
            let over = the_sky.as_ref().map(|s| sky_overlap(s, &matte));
            let both = the_sky.as_ref().map_or(0.0, |s| sky_both(s, &matte));
            let labeled = scene.prior.labeled_sky();
            let on_labels = sky_overlap(&labeled, &matte);
            let uncut = match (&the_sky, g.capped_by_sky()) {
                (Some(s), true) => match landscape::find(&scene, g, &frame, (w, h), None) {
                    Found::Shape { matte, .. } => Some(sky_overlap(s, &matte)),
                    Found::None(_) => None,
                },
                _ => None,
            };
            println!(
                "  {}: {} in {took:.3}s, recall {:.1}% spill {:.1}%, over the sky {}{}, both {:.2}%, \
                 over the labeled sky {:.2}%",
                g.name(),
                if matted { "matte" } else { "feathered" },
                kept as f32 / n * 100.0,
                spill as f32 / n * 100.0,
                over.map_or("-".to_string(), |o| format!("{:.2}%", o * 100.0)),
                uncut.map_or(String::new(), |o| format!(" (uncut {:.2}%)", o * 100.0)),
                both * 100.0,
                on_labels * 100.0,
            );
            let over = over.unwrap_or(0.0);
            if g.capped_by_sky() && (over > max_overlap || both > 0.0) {
                wrong.push(format!(
                    "{stem}: {} over {:.2}% of the sky, {:.2}% of the labeled sky, both {:.2}%",
                    g.name(),
                    over * 100.0,
                    on_labels * 100.0,
                    both * 100.0
                ));
            }
            if let Some(out) = &out {
                save(&out.join(format!("{stem}.{}.png", g.name())), &matte);
            }
        }
        if let (Some(out), Some(s)) = (&out, &the_sky) {
            save(&out.join(format!("{stem}.sky.png")), s);
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The four groups' measures, and so their gates, are the same on the
/// CPU and on WebGPU: on every frame of the folder (or
/// `GREYCARD_LANDSCAPE_ONLY`'s), the area, the mean and the core within
/// 2e-3 of each other, and every gate's verdict the same. A mean near
/// its bar is printed with its distance from it.
#[test]
#[ignore]
fn the_group_measures_are_the_same_on_the_cpu_and_on_webgpu() {
    let Some(store) = std::env::var_os("GREYCARD_MODELS").map(Store::at) else {
        return;
    };
    let Some(dir) = std::env::var_os("GREYCARD_LANDSCAPE_PREVIEWS").map(PathBuf::from) else {
        return;
    };
    if !Provider::available().contains(&Provider::WebGpu) {
        println!("no WebGPU here: nothing to compare");
        return;
    }
    let mut cpu = Sky::load(&store, &[Provider::Cpu]).expect("the Sky model on the CPU");
    let mut gpu = Sky::load(&store, &[Provider::WebGpu]).expect("the Sky model on WebGPU");
    assert_eq!(gpu.provider(), Provider::WebGpu);
    let mut worst = Measure::default();
    for (stem, path) in previews(&dir) {
        let picture = picture(&path);
        let (on_cpu, _, _) = scene_of(&mut cpu, &picture);
        let (on_gpu, _, _) = scene_of(&mut gpu, &picture);
        let mut said = Vec::new();
        for g in Group::ALL {
            let (a, b) = (on_cpu.measure(g), on_gpu.measure(g));
            let d = Measure {
                area: (a.area - b.area).abs(),
                mean: (a.mean - b.mean).abs(),
                core: (a.core - b.core).abs(),
            };
            worst.area = worst.area.max(d.area);
            worst.mean = worst.mean.max(d.mean);
            worst.core = worst.core.max(d.core);
            assert!(
                d.area < 2e-3 && d.mean < 2e-3 && d.core < 2e-3,
                "{stem}: {}: CPU {a:?}, WebGPU {b:?}",
                g.name()
            );
            assert_eq!(
                on_cpu.gate(g).is_ok(),
                on_gpu.gate(g).is_ok(),
                "{stem}: {}'s gate",
                g.name()
            );
            if a.area > 0.0 {
                said.push(format!(
                    "{} mean {:.4}/{:.4} ({:+.4} from its bar) core {:.4}/{:.4}",
                    g.name(),
                    a.mean,
                    b.mean,
                    a.mean - g.bar(),
                    a.core,
                    b.core
                ));
            }
        }
        assert_eq!(on_cpu.offers().len(), on_gpu.offers().len(), "{stem}");
        println!("{stem}: {}", said.join("; "));
    }
    println!(
        "worst CPU against WebGPU: area {:.2e}, mean {:.2e}, core {:.2e}",
        worst.area, worst.mean, worst.core
    );
}

/// The Sky shape's matte on a frame as the editor makes it with no
/// picks: SAM's outline when SAM is here, else the prior's labels.
fn sky_shape(
    scene: &Scene,
    frame: &Frame,
    picture: &Rgb8,
    size: (usize, usize),
    sam: &mut Option<Sam>,
) -> Option<Mask> {
    sky::gate(&scene.prior).ok()?;
    let mut times = sky::Times::default();
    let found = match sam.as_mut() {
        Some(s) => {
            let e = s.embed(picture).expect("SAM's embedding");
            let mut decode = |p: &[greycard_ai::Prompt]| s.decode(&e, p).map(|(m, _)| m);
            let decode: &mut sky::Decode = &mut decode;
            sky::find(&scene.prior, frame, &[], Some(decode), size, &mut times)
        }
        None => sky::find(&scene.prior, frame, &[], None, size, &mut times),
    };
    match found.expect("the sky") {
        sky::Found::Sky { matte, .. } => Some(matte),
        sky::Found::None(_) => None,
    }
}

/// Of the pixels the sky's matte holds over a half, the share `shape`
/// holds over a quarter: how far a shape reaches into the sky.
fn sky_overlap(sky: &Mask, shape: &Mask) -> f32 {
    let (mut n, mut over) = (0usize, 0usize);
    for (s, m) in sky.data.iter().zip(&shape.data) {
        if *s > 0.5 {
            n += 1;
            over += (*m > 0.25) as usize;
        }
    }
    over as f32 / n.max(1) as f32
}

/// Of the pixels the sky's matte holds over a half, the share where it
/// and `shape` both claim the pixel: their sum over 1.25. A pixel mixed
/// of sky and leaf is shared, not claimed twice.
fn sky_both(sky: &Mask, shape: &Mask) -> f32 {
    let (mut n, mut both) = (0usize, 0usize);
    for (s, m) in sky.data.iter().zip(&shape.data) {
        if *s > 0.5 {
            n += 1;
            both += (s + m > 1.25) as usize;
        }
    }
    both as f32 / n.max(1) as f32
}

fn save(path: &Path, m: &Mask) {
    image::GrayImage::from_raw(m.width as u32, m.height as u32, m.to_u8())
        .unwrap()
        .save(path)
        .unwrap();
}
