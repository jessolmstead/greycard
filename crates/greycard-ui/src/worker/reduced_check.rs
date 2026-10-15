//! The reduced develop checked against the full one: the CPU reference
//! is the full develop finished at 2048 as an export at that size is
//! (`export::fit`), and its 360 shrunk from that 2048 as every made size
//! is (`edited::shrunk`); the reduced develop's pictures (`reduced`, then
//! `edited::make`) must lie within a ΔE limit of them, over seeded random
//! edits in the manner of the GPU ops' sweeps. On the sample raws
//! (`GREYCARD_SAMPLES`), so ignored; `GREYCARD_REDUCED_CASES` sets the
//! cases a frame (18), `GREYCARD_REDUCED_SEED` the seed,
//! `GREYCARD_REDUCED_CSV` a file the numbers go into,
//! `GREYCARD_REDUCED_MEASURE=1` prints without holding the limit,
//! `GREYCARD_REDUCED_GRAIN=1` lays the grain on every case, and
//! `GREYCARD_REDUCED_HONEST=1` turns the capture sharpen and the
//! profiled denoise on in the reference alone, to say how far a real
//! export is from the thumbnail.

use super::*;
use greycard_core::color::Oklab;
use greycard_edit::mask::{Component, Shape};

/// splitmix64, as the GPU sweeps have it (`greycard-gpu/tests/common`).
struct Rng(u64);

impl Rng {
    fn new(seed: u64, k: u64) -> Self {
        let mut r = Rng(seed ^ k.wrapping_mul(0xD1B5_4A32_D192_ED03));
        r.next();
        r
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }

    /// A signed slider: each end a fifth of the time, zero a tenth, a
    /// small value three twentieths, anywhere otherwise.
    fn signed(&mut self, lo: f32, hi: f32) -> f32 {
        let u = self.unit();
        let v = self.unit();
        if u < 0.2 {
            lo
        } else if u < 0.4 {
            hi
        } else if u < 0.5 {
            0.0
        } else if u < 0.65 {
            let small = 0.05 * lo.abs().max(hi.abs()) * v * v;
            if self.chance(0.5) { -small } else { small }
        } else {
            lo + (hi - lo) * v
        }
    }
}

/// The limits, Oklab ΔE times 100, the mean over the picture and its
/// 99th percentile, at 2048 and at 360, set with headroom over the
/// measurement on 18 cases on each of five bodies (notes draft); and
/// the median of the means at 2048 over every case, which says the
/// typical edit is close and not only the worst one bounded.
const LIMIT_2048: (f32, f32) = (3.0, 18.0);
const LIMIT_360: (f32, f32) = (1.2, 10.0);
const LIMIT_MEDIAN_2048: f32 = 0.5;

/// An edit drawn for case `k` of `seed`: the ops the reduced develop
/// scales or places, each on with some chance, over a brightening.
fn drawn(seed: u64, k: u64, looks: &[String]) -> Edit {
    let mut r = Rng::new(seed, k);
    let mut edit = Edit::default();
    edit.sharpen.enabled = false;
    edit.noise.enabled = false;
    edit.grain = Default::default();
    edit.light.exposure = r.signed(-2.0, 2.0);
    edit.light.tone.contrast = 1.0 + r.signed(-0.5, 0.5);
    if r.chance(0.6) {
        edit.detail.texture = r.signed(-1.0, 1.0);
        edit.detail.clarity = r.signed(-1.0, 1.0);
    }
    if r.chance(0.5) {
        edit.detail.dehaze = r.signed(-1.0, 1.0);
    }
    if r.chance(0.4) {
        edit.lens.ca_red = r.signed(-0.005, 0.005);
        edit.lens.ca_blue = r.signed(-0.005, 0.005);
    }
    if r.chance(0.4) {
        edit.lens.defringe = true;
    }
    if r.chance(0.5) {
        edit.geometry.angle = r.signed(-10.0, 10.0);
    }
    if r.chance(0.5) {
        // Down to a tenth of the frame each way (the crop panel allows a
        // fiftieth, `geometry::MIN_CROP`): under about two thirds of a
        // 24 MP frame the reduced develop demosaics instead of binning.
        let w = r.range(0.1, 0.95);
        let h = r.range(0.1, 0.95);
        edit.geometry.crop = Some(greycard_edit::geometry::Crop {
            x: r.range(0.0, 1.0 - w),
            y: r.range(0.0, 1.0 - h),
            w,
            h,
        });
    }
    if r.chance(0.6) {
        let shape = if r.chance(0.5) {
            Shape::Radial {
                center: [r.range(0.2, 0.8), r.range(0.2, 0.6)],
                radius: [r.range(0.1, 0.4), r.range(0.1, 0.3)],
                angle: r.range(-45.0, 45.0),
                feather: r.range(0.0, 1.0),
            }
        } else {
            Shape::Linear {
                from: [r.range(0.0, 1.0), r.range(0.0, 0.7)],
                to: [r.range(0.0, 1.0), r.range(0.0, 0.7)],
            }
        };
        let mut a = greycard_edit::Adjustment {
            id: 1,
            name: "drawn".into(),
            ..Default::default()
        };
        a.mask.components.push(Component {
            shape,
            ..Default::default()
        });
        a.look.light.exposure = r.signed(-1.5, 1.5);
        a.look.light.tone.shadows = r.signed(-1.0, 1.0);
        edit.adjustments.push(a);
    }
    if r.chance(0.5) {
        edit.vignette.amount = r.signed(-2.0, 2.0);
        edit.vignette.midpoint = r.range(0.2, 0.8);
        edit.vignette.feather = r.range(0.0, 1.0);
    }
    if !looks.is_empty() && r.chance(0.5) {
        let name = looks[(r.next() % looks.len() as u64) as usize].clone();
        edit.look_lut.lut = greycard_edit::look::LutChoice::Named(name);
        edit.look_lut.strength = r.range(0.3, 1.0);
    }
    // The grain from a generator of its own, so the cases above are
    // the same with it as without; `GREYCARD_REDUCED_GRAIN` lays it on
    // every case.
    let mut g = Rng::new(seed ^ 0x6772_6169, k);
    if std::env::var_os("GREYCARD_REDUCED_GRAIN").is_some() || g.chance(0.35) {
        edit.grain.enabled = true;
        edit.grain.amount = g.range(0.2, 1.0);
        edit.grain.size = g.range(0.3, 2.0);
    }
    // A mask's Texture and Dehaze, from a generator of their own for
    // the same reason, so the masks' detail planes are made and
    // carried through the reduced develop as through the full one.
    let mut d = Rng::new(seed ^ 0x6465_7461, k);
    if let Some(a) = edit.adjustments.first_mut() {
        if d.chance(0.6) {
            a.look.detail.texture = d.signed(-1.0, 1.0);
        }
        if d.chance(0.5) {
            a.look.detail.dehaze = d.signed(-1.0, 1.0);
        }
    }
    edit
}

/// What one picture of the edit differs from the reference by: Oklab
/// ΔE times 100, its mean and its 99th percentile over the pixels.
fn delta_e(a: &[u8], b: &[u8]) -> (f32, f32) {
    let ok = Oklab::for_working_space();
    let to_working = RgbSpace::SRGB
        .to_space_matrix(
            &greycard_core::color::WORKING_SPACE,
            greycard_core::color::CAT,
        )
        .expect("sRGB reaches the working space")
        .rows
        .map(|row| row.map(|v| v as f32));
    let lab = |px: &[u8]| {
        let lin: [f32; 3] = std::array::from_fn(|c| srgb_decode(px[c] as f32 / 255.0));
        ok.of(greycard_core::color::apply3(&to_working, lin))
    };
    let mut d: Vec<f32> = a
        .as_chunks::<3>()
        .0
        .iter()
        .zip(b.as_chunks::<3>().0)
        .map(|(p, q)| {
            let (x, y) = (lab(p), lab(q));
            100.0 * ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()
        })
        .collect();
    let mean = d.iter().sum::<f32>() / d.len().max(1) as f32;
    d.sort_by(f32::total_cmp);
    let p99 = d[((d.len() as f32 * 0.99) as usize).min(d.len().saturating_sub(1))];
    (mean, p99)
}

fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// `picture` brought to `w` by `h` when it is not that already (a pixel
/// of rounding between the two paths' fits).
fn sized(picture: Thumb, w: u32, h: u32) -> Vec<u8> {
    if (picture.width, picture.height) == (w, h) {
        return picture.rgb;
    }
    let img = image::RgbImage::from_raw(picture.width, picture.height, picture.rgb).unwrap();
    image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3).into_raw()
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn sample_raws() -> Vec<PathBuf> {
    let Some(dir) = std::env::var_os("GREYCARD_SAMPLES") else {
        return Vec::new();
    };
    let mut raws: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| greycard_core::decode::is_raw_path(p))
        .collect();
    raws.sort();
    if let Ok(only) = std::env::var("GREYCARD_SAMPLE_FILES") {
        raws.retain(|p| {
            only.split(',').any(|n| {
                p.file_name()
                    .is_some_and(|f| f.to_string_lossy() == n.trim())
            })
        });
    }
    raws
}

/// The reduced develop's pictures against the full develop's at 2048
/// and at 360, over seeded random edits.
#[test]
#[ignore = "needs GREYCARD_SAMPLES; minutes"]
fn the_reduced_develop_is_the_full_one_within_the_limit() {
    let raws = sample_raws();
    if raws.is_empty() {
        eprintln!("GREYCARD_SAMPLES names no folder; nothing checked");
        return;
    }
    let seed = env_u64("GREYCARD_REDUCED_SEED", 0x6772_6579);
    let cases = env_u64("GREYCARD_REDUCED_CASES", 18);
    let measure = std::env::var("GREYCARD_REDUCED_MEASURE").is_ok();
    let honest = std::env::var("GREYCARD_REDUCED_HONEST").is_ok();
    let csv = std::env::var_os("GREYCARD_REDUCED_CSV").map(PathBuf::from);
    let looks: Vec<String> = greycard_edit::look::list()
        .into_iter()
        .filter(|e| e.variant.is_none())
        .map(|e| e.name)
        .collect();
    let lenses = greycard_lens::Store::user()
        .ok()
        .and_then(|s| s.load())
        .map(|(db, _)| db);
    let mut rows = vec![
        "file,case,size,mean,p99,texture,clarity,dehaze,angle,crop,mask,vignette,look,grain,\
         ref_w,ref_h,ours_w,ours_h,bin,reduced_ms,full_ms"
            .to_string(),
    ];
    let mut worst = (0.0f32, 0.0f32);
    let mut failures = Vec::new();
    let mut means_2048 = Vec::new();
    for path in &raws {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let (input, _) = match timed_open(path) {
            Ok(opened) => opened,
            Err(e) => {
                eprintln!("{name}: not opened: {e:#}");
                continue;
            }
        };
        for k in 0..cases {
            let edit = drawn(seed, k, &looks);
            let mut reference_edit = edit.clone();
            if honest {
                reference_edit.sharpen.enabled = true;
                reference_edit.noise.enabled = true;
                reference_edit.noise.profiled = true;
                reference_edit.noise.strength = 0.5;
            }
            // The reference: the full develop on the CPU, finished at
            // each size as an export is, with no output sharpening.
            let started = Instant::now();
            let mut base = None;
            let deliver: Deliver = Arc::new(|_| {});
            let mut ai = Ai::new();
            let (outcome, image) = develop_job(
                &input,
                &reference_edit,
                0,
                1,
                &mut base,
                &mut None,
                &mut ai,
                None,
                lenses.as_ref(),
                &mut None,
                &deliver,
            );
            let Some(image) = image else {
                let why = match outcome {
                    Outcome::Failed { message, .. } => message,
                    _ => "no picture".into(),
                };
                eprintln!("{name} case {k}: the full develop made no picture: {why}");
                continue;
            };
            let full_ms = started.elapsed().as_secs_f64() * 1e3;
            let settings = |edge: u32| crate::export::Settings {
                format: crate::export::Format::Jpeg,
                long_edge: Some(edge),
                space: crate::export::Space::Srgb,
                sharpen: crate::export::Sharpen::Off,
                watermark: None,
                ..crate::export::Settings::default()
            };
            let (rendered, left_out) = finish_export(
                image.clone(),
                &edit,
                base.as_ref(),
                &mut ai,
                &settings(2048),
            );
            assert!(left_out.is_empty(), "{name}: {left_out:?}");
            let crate::export::Pixels::Eight(rgb) = rendered.pixels else {
                unreachable!("a JPEG's pixels are eight bits")
            };
            let big = Thumb {
                width: rendered.width,
                height: rendered.height,
                rgb,
            };
            // The 360 as both develops' kept pictures make it: shrunk
            // from the 2048, so what is measured is the develop and not
            // a second resample.
            let reference = [
                (2048u32, crate::edited::shrunk(&big, 2048)),
                (360, crate::edited::shrunk(&big, 360)),
            ];
            // The reduced develop and the pictures made from it.
            let started = Instant::now();
            let developed = match crate::reduced::develop(path, &edit, 0, lenses.as_ref()) {
                Ok(d) => d,
                Err(why) => {
                    eprintln!("{name} case {k}: not reduced: {why}");
                    continue;
                }
            };
            let finish = crate::edited::Finish {
                edit: edit.clone(),
                turn: 0,
                source: (developed.image.width as u32, developed.image.height as u32),
                full: developed.full,
                rasters: Default::default(),
                clip_level: developed.clip_level,
                guide: Some(developed.guide.clone()),
                kind: developed.kind,
                white: developed.white.clone(),
                detail: developed.detail.clone(),
            };
            let made = crate::edited::make(&developed.image, &finish);
            let reduced_ms = started.elapsed().as_secs_f64() * 1e3;
            let describe = format!(
                "{:.2},{:.2},{:.2},{:.1},{},{},{:.2},{},{:.2}",
                edit.detail.texture,
                edit.detail.clarity,
                edit.detail.dehaze,
                edit.geometry.angle,
                edit.geometry.crop.is_some() as u8,
                edit.adjustments.len(),
                edit.vignette.amount,
                matches!(edit.look_lut.lut, greycard_edit::look::LutChoice::Named(_)) as u8,
                if edit.grain.is_off() {
                    0.0
                } else {
                    edit.grain.amount
                },
            );
            for (edge, reference) in reference {
                let ours = if edge == 2048 {
                    made.big.clone()
                } else {
                    made.thumbs
                        .iter()
                        .find(|(s, _)| *s == edge)
                        .map(|(_, t)| t.clone())
                        .expect("a 360 thumbnail")
                };
                let (ow, oh) = (ours.width, ours.height);
                let ours = sized(ours, reference.width, reference.height);
                let (mean, p99) = delta_e(&reference.rgb, &ours);
                worst = (worst.0.max(mean), worst.1.max(p99));
                if edge == 2048 {
                    means_2048.push(mean);
                }
                let line = format!(
                    "{name},{k},{edge},{mean:.3},{p99:.3},{describe},{},{},{ow},{oh},{},\
                     {reduced_ms:.0},{full_ms:.0}",
                    reference.width, reference.height, developed.bin
                );
                println!("{line}");
                rows.push(line);
                let (limit_mean, limit_p99) = if edge == 2048 { LIMIT_2048 } else { LIMIT_360 };
                if !measure && !honest && (mean > limit_mean || p99 > limit_p99) {
                    failures.push(format!(
                        "{name} case {k} at {edge}: mean {mean:.3} p99 {p99:.3} ({describe})"
                    ));
                }
            }
        }
    }
    means_2048.sort_by(f32::total_cmp);
    let median = means_2048.get(means_2048.len() / 2).copied().unwrap_or(0.0);
    println!(
        "worst: mean {:.3}, p99 {:.3}; the median of the means at 2048 {median:.3} over {} cases",
        worst.0,
        worst.1,
        means_2048.len()
    );
    if !measure && !honest && median > LIMIT_MEDIAN_2048 {
        failures.push(format!(
            "the median of the means at 2048: {median:.3} past {LIMIT_MEDIAN_2048}"
        ));
    }
    if let Some(csv) = csv {
        std::fs::write(&csv, rows.join("\n") + "\n").unwrap();
    }
    assert!(
        failures.is_empty(),
        "past the limit:\n{}",
        failures.join("\n")
    );
}

/// Not a test: writes a sidecar with a brightening (+1.5 EV) beside
/// every raw under `GREYCARD_EDITED_FOLDER`, for a cold folder of edited
/// frames to open in the editor and watch the queue fill.
#[test]
#[ignore = "a helper for the folder measurement; needs GREYCARD_EDITED_FOLDER"]
fn write_edited_folder() {
    let Some(dir) = std::env::var_os("GREYCARD_EDITED_FOLDER") else {
        return;
    };
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if !greycard_core::decode::is_raw_path(&path) {
            continue;
        }
        let mut edit = Edit::default();
        edit.light.exposure = 1.5;
        let mut sidecar = greycard_edit::Sidecar {
            line: greycard_edit::Line::of(edit),
            ..greycard_edit::Sidecar::default()
        };
        sidecar.save(&path).unwrap();
        n += 1;
    }
    println!(
        "{n} sidecars written under {}",
        std::path::Path::new(&dir).display()
    );
}

/// The reduced develop timed per frame on the samples: the decode, the
/// develop to the binned picture, the lens and the detail, the finish
/// to the pictures, and the keep into a scratch cache.
#[test]
#[ignore = "needs GREYCARD_SAMPLES; a measurement"]
fn the_reduced_develop_of_the_samples_timed() {
    let raws = sample_raws();
    if raws.is_empty() {
        eprintln!("GREYCARD_SAMPLES names no folder; nothing measured");
        return;
    }
    let lenses = greycard_lens::Store::user()
        .ok()
        .and_then(|s| s.load())
        .map(|(db, _)| db);
    let scratch = tests::thumb_scratch("reduced-timed");
    let runs = env_u64("GREYCARD_REDUCED_RUNS", 3);
    for path in &raws {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut edit = Edit::default();
        edit.light.exposure = 1.0;
        edit.sharpen.enabled = false;
        for run in 0..runs {
            let started = Instant::now();
            let developed = match crate::reduced::develop(path, &edit, 0, lenses.as_ref()) {
                Ok(d) => d,
                Err(why) => {
                    eprintln!("{name}: not reduced: {why}");
                    break;
                }
            };
            let finish = crate::edited::Finish {
                edit: edit.clone(),
                turn: 0,
                source: (developed.image.width as u32, developed.image.height as u32),
                full: developed.full,
                rasters: Default::default(),
                clip_level: developed.clip_level,
                guide: Some(developed.guide.clone()),
                kind: developed.kind,
                white: developed.white.clone(),
                detail: developed.detail.clone(),
            };
            let before_finish = started.elapsed().as_secs_f64();
            let made = crate::edited::make(&developed.image, &finish);
            let finished = started.elapsed().as_secs_f64();
            let cache: ThumbCache = Arc::new(Mutex::new(Some(
                Thumbs::at(
                    scratch.join("thumbs"),
                    greycard_library::thumbs::DEFAULT_CAP,
                )
                .with_previews(2048, greycard_library::thumbs::DEFAULT_PREVIEW_CAP),
            )));
            let edited = crate::edited::Edited::new(cache);
            let hash = greycard_library::hash_file(path).unwrap();
            let bytes = edited
                .store(
                    &crate::versions::Key::file(path),
                    &hash,
                    run + 1,
                    None,
                    &made,
                )
                .unwrap();
            let total = started.elapsed().as_secs_f64();
            println!(
                "{name} run {run}: {}x{} by {} to {}x{}: decode {:.0} ms, develop {:.0} ms, \
                 lens+detail {:.0} ms, finish {:.0} ms, keep {:.0} ms; total {:.0} ms, {} KB",
                developed.full.0,
                developed.full.1,
                developed.bin,
                developed.image.width,
                developed.image.height,
                developed.decode * 1e3,
                developed.develop * 1e3,
                developed.rest * 1e3,
                (finished - before_finish) * 1e3,
                (total - finished) * 1e3,
                total * 1e3,
                bytes / 1024
            );
        }
    }
    crate::testing::remove_dir_retry(&scratch);
}

/// A mask's Detail measured on the sample raws (`GREYCARD_SAMPLES`,
/// `GREYCARD_SAMPLE_FILES`): the develop's time with no mask asking,
/// with a mask asking for the texture band, and for all three planes,
/// the planes' own share; and how far a whole-frame mask's Texture at
/// -0.5, put on after the sharpen, lands from the global's at -0.5
/// put on before it, the sharpen at its default, as Oklab ΔE times 100
/// over the frame. Built optimized (`--release`) for the times.
#[test]
#[ignore = "needs GREYCARD_SAMPLES; a measurement"]
fn a_masks_detail_measured() {
    let raws = sample_raws();
    let lenses = None;
    let whole = |detail: greycard_edit::Detail| {
        let mut a = greycard_edit::Adjustment {
            id: 1,
            name: "whole".into(),
            ..Default::default()
        };
        a.mask.components.push(Component {
            shape: Shape::Linear {
                from: [0.5, 10.0],
                to: [0.5, 11.0],
            },
            ..Default::default()
        });
        a.look.detail = detail;
        a
    };
    let skin = greycard_edit::Detail {
        texture: -0.5,
        ..Default::default()
    };
    let all = greycard_edit::Detail {
        texture: -0.5,
        clarity: 0.3,
        dehaze: 0.3,
        ..Default::default()
    };
    let settings = crate::export::Settings {
        format: crate::export::Format::Jpeg,
        sharpen: crate::export::Sharpen::Off,
        metadata: crate::export::Metadata::None,
        ..Default::default()
    };
    for path in &raws {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut plain = Edit::default();
        plain.sharpen.enabled = true;
        plain.noise.enabled = false;
        let mut global = plain.clone();
        global.detail = skin;
        let mut masked = plain.clone();
        masked.adjustments.push(whole(skin));
        let mut every = plain.clone();
        every.adjustments.push(whole(all));
        let timed = |edit: &Edit| {
            // Twice, the second timed: the first pays for the decode
            // cache and the pools.
            let _ = FrameDevelop::develop(path, edit, lenses);
            let started = Instant::now();
            let d = FrameDevelop::develop(path, edit, lenses).expect("developed");
            (d, started.elapsed().as_secs_f64())
        };
        let (_, t_plain) = timed(&plain);
        let (mut g, t_global) = timed(&global);
        let (mut m, t_masked) = timed(&masked);
        let (_, t_every) = timed(&every);
        let size = (g.image.width, g.image.height);
        let started = Instant::now();
        let _ = crate::detail::DetailPlanes::make(&g.image, None, 1.0, every.detail_wants());
        let t_planes = started.elapsed().as_secs_f64();
        let eight = |r: crate::export::Rendered| match r.pixels {
            crate::export::Pixels::Eight(p) => p,
            crate::export::Pixels::Sixteen(_) => unreachable!(),
        };
        let a = eight(g.finish(&global, &settings));
        let b = eight(m.finish(&masked, &settings));
        let (mean, p99) = delta_e(&a, &b);
        eprintln!(
            "{name} {}x{} ({:.1} MP): develop {t_plain:.2} s plain, {t_global:.2} s global \
             Texture, {t_masked:.2} s a mask's Texture, {t_every:.2} s a mask's three; the three \
             planes alone {t_planes:.2} s; the order against the sharpen ΔE mean {mean:.3}, \
             p99 {p99:.3}",
            size.0,
            size.1,
            (size.0 * size.1) as f64 / 1e6,
        );
    }
}

/// The develop's latency on the GPU's path with a mask asking for its
/// Detail, on the sample raws: a develop that makes the patched picture
/// again (a white balance moved) with no mask asking and with a mask
/// asking for all three planes, which are made on a thread of their
/// own beside the GPU's work; and a move of the global's Texture with
/// the mask asking, which keeps the planes. Each the best of three.
/// With `GREYCARD_PLANES_CPU=1`, on the CPU's path instead.
#[test]
#[ignore = "needs GREYCARD_SAMPLES and a GPU; a measurement"]
fn a_masks_detail_on_the_gpu_path_timed() {
    let raws = sample_raws();
    let on_cpu = std::env::var_os("GREYCARD_PLANES_CPU").is_some();
    let all = greycard_edit::Detail {
        texture: -0.5,
        clarity: 0.3,
        dehaze: 0.3,
        ..Default::default()
    };
    for path in &raws {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let (input, _) = timed_open(path).expect("opened");
        let mut gpu = if on_cpu {
            None
        } else {
            Some(greycard_gpu::Context::own().expect("a GPU"))
        };
        let mut base = None;
        let mut ai = Ai::new();
        let deliver: Deliver = Arc::new(|_| {});
        let mut run = |edit: &Edit, base: &mut Option<Base>| {
            let started = Instant::now();
            let (outcome, _) = develop_job(
                &input, edit, 0, 1, base, &mut None, &mut ai, None, None, &mut gpu, &deliver,
            );
            assert!(matches!(outcome, Outcome::Developed { .. }));
            started.elapsed().as_secs_f64()
        };
        let mut plain = Edit::default();
        plain.sharpen.enabled = true;
        plain.noise.enabled = false;
        let mut masked = plain.clone();
        let mut a = greycard_edit::Adjustment {
            id: 1,
            ..Default::default()
        };
        a.mask.components.push(Component::default());
        a.look.detail = all;
        masked.adjustments.push(a);
        // The white moved each time, so the patched picture is new.
        let mut moved = |edit: &Edit, base: &mut Option<Base>| {
            let mut best = f64::INFINITY;
            for k in 0..4 {
                let mut e = edit.clone();
                e.white_balance = greycard_edit::WhiteBalance::Custom {
                    temperature: 5000.0 + 100.0 * k as f64,
                    tint: 0.0,
                };
                let t = run(&e, base);
                if k > 0 {
                    best = best.min(t);
                }
            }
            best
        };
        let t_plain = moved(&plain, &mut base);
        let t_masked = moved(&masked, &mut base);
        // The global's Texture moved under a mask asking for the haze:
        // the patched picture is the same, and the planes are kept.
        let mut global = masked.clone();
        let mut best = f64::INFINITY;
        for k in 0..4 {
            global.detail.texture = 0.1 * (k + 1) as f32;
            let t = run(&global, &mut base);
            if k > 0 {
                best = best.min(t);
            }
        }
        let size = base
            .as_ref()
            .map(|b| (b.image.width, b.image.height))
            .unwrap_or_default();
        eprintln!(
            "{name} ({:.1} MP, {}): a white moved {t_plain:.2} s with no mask asking, \
             {t_masked:.2} s with a mask asking for all three planes; the global's \
             Texture moved with the mask asking {best:.2} s",
            (size.0 * size.1) as f64 / 1e6,
            if on_cpu { "CPU" } else { "GPU" },
        );
    }
}
