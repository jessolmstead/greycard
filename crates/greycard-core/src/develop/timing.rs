//! Each develop op timed alone on a real frame: a measurement, not a
//! test, ignored unless asked for.
//!
//! ```text
//! GREYCARD_RAW=path/to/frame.CR3 GREYCARD_OPS=nlm,dehaze GREYCARD_RUNS=5 \
//!     cargo test --release -p greycard-core --lib -- --ignored --nocapture ops_alone
//! ```
//!
//! The frame is decoded and taken through [`prepare`] and the demosaic
//! once; each op then runs `GREYCARD_RUNS` times on a fresh copy of its
//! input, and the copy is not timed. Each run prints its time, the
//! one-minute load beside it and a digest of the output's bits, so two
//! builds can be seen to agree to the bit; then the least and the
//! median. Pin the thread count with `RAYON_NUM_THREADS` and the cores
//! with `taskset`.

use super::*;
use std::time::{Duration, Instant};

fn load() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_owned))
        .unwrap_or_else(|| "?".into())
}

/// The bits of an op's output folded into one number.
fn digest(values: &[f32]) -> u64 {
    values.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, v| {
        (h ^ u64::from(v.to_bits())).wrapping_mul(0x0100_0000_01b3)
    })
}

fn time(name: &str, runs: usize, mut run: impl FnMut() -> (Duration, Vec<f32>)) {
    let mut times = Vec::with_capacity(runs);
    for i in 0..runs {
        let at = load();
        let (t, out) = run();
        let t = t.as_secs_f64();
        let bits = digest(&out);
        println!("{name:>20} run {i}: {t:8.4} s  (load {at})  bits {bits:016x}");
        times.push(t);
    }
    if times.is_empty() {
        return;
    }
    times.sort_by(f64::total_cmp);
    println!(
        "{name:>20}: min {:.4} s  median {:.4} s  over {runs}",
        times[0],
        times[runs / 2]
    );
}

#[test]
#[ignore = "a measurement, not a test"]
fn ops_alone() {
    let Ok(path) = std::env::var("GREYCARD_RAW") else {
        panic!("GREYCARD_RAW names the frame to time on");
    };
    let ops = std::env::var("GREYCARD_OPS").unwrap_or_else(|_| "all".into());
    let runs: usize = std::env::var("GREYCARD_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let wants = |op: &str| ops == "all" || ops.split(',').any(|o| o == op);

    let frame = crate::decode::decode_path(&path).expect("the frame decodes");
    let settings = DevelopSettings::default();
    let prepared = prepare(&frame, &settings, true).expect("the frame prepares");
    let (w, h) = (prepared.width, prepared.height);
    let pattern = prepared.pattern.clone().expect("a mosaic");
    let (camera, _, _) = demosaic_prepared(&prepared, &settings).expect("it demosaics");
    let model = prepared.noise_model();
    let mut image = WorkingImage::from_data(w, h, camera.clone()).unwrap();
    apply_matrix(&mut image.data, prepared.white_balance.matrix_f32());
    println!("frame {w}x{h}, {} threads", rayon::current_num_threads());

    if wants("normalize") {
        time("normalize", runs, || {
            let t = Instant::now();
            let out = normalize_levels(&frame);
            (t.elapsed(), out)
        });
    }
    if wants("gains") {
        time("gains", runs, || {
            let mut s = prepared.samples.clone();
            let t = Instant::now();
            apply_gains_cfa(&mut s, w, &pattern, prepared.gains);
            (t.elapsed(), s)
        });
    }
    // The noise estimate and the hot pixel repair read the balanced
    // samples here, not the levels before the gains the develop gives
    // them: the same reads, and a time, not an answer.
    if wants("noise") {
        time("noise", runs, || {
            let t = Instant::now();
            let estimate = noise::estimate_noise(&prepared.samples, w, h, &pattern).unwrap();
            (t.elapsed(), [estimate.model.a, estimate.model.b].concat())
        });
    }
    if let Some(estimate) = prepared.noise.filter(|_| wants("hotpixels")) {
        time("hotpixels", runs, || {
            let mut s = prepared.samples.clone();
            let t = Instant::now();
            hotpixels::fix_hot_pixels(
                &mut s,
                w,
                h,
                &pattern,
                &estimate.model.after_gains(prepared.gains),
                &hotpixels::HotPixelOptions::default(),
            )
            .unwrap();
            (t.elapsed(), s)
        });
    }
    if wants("bilinear") {
        time("bilinear", runs, || {
            let t = Instant::now();
            let out = demosaic_bilinear(&prepared.samples, w, h, &pattern);
            (t.elapsed(), out)
        });
    }
    if let Some(model) = model.filter(|_| wants("nlm")) {
        time("nlm", runs, || {
            let mut rgb = camera.clone();
            let t = Instant::now();
            nlm::denoise_nlm(&mut rgb, w, h, &model, 1.0, &nlm::NlmOptions::default());
            (t.elapsed(), rgb)
        });
    }
    if wants("dehaze") {
        let options = dehaze::DehazeOptions { amount: 0.5 };
        time("dehaze", runs, || {
            let mut im = image.clone();
            let t = Instant::now();
            dehaze::dehaze(&mut im, &options);
            (t.elapsed(), im.data)
        });
    }
    // The halves the GPU path runs on the CPU: the reduced copy with
    // what it reads that the amount does not change (here from the
    // picture; the GPU path divides the sums it read back), and the
    // fit at an amount from a kept copy, which is a Dehaze move's.
    if wants("dehaze_reduce") {
        time("dehaze_reduce", runs, || {
            let t = Instant::now();
            let reduced = dehaze::Reduced::new(&image);
            (t.elapsed(), reduced.means().to_vec())
        });
    }
    if wants("dehaze_fit") {
        let options = dehaze::DehazeOptions { amount: 0.5 };
        let reduced = dehaze::Reduced::new(&image);
        time("dehaze_fit", runs, || {
            let t = Instant::now();
            let model = reduced.model(&options).expect("a strength");
            (t.elapsed(), model.slope().to_vec())
        });
    }
    if wants("dehaze_map") {
        let options = dehaze::DehazeOptions { amount: 0.5 };
        time("dehaze_map", runs, || {
            let mut im = image.clone();
            let t = Instant::now();
            let (_, map) = dehaze::dehaze_with_map(&mut im, &options);
            let e = t.elapsed();
            im.data.extend_from_slice(&map);
            (e, im.data)
        });
    }
    if wants("lens") {
        let correction = lens::LensCorrection {
            distortion: Some(lens::Distortion::Poly5 {
                k1: -0.02,
                k2: 0.004,
            }),
            chromatic_aberration: Some(lens::ChromaticAberration {
                red: [1.0003, 0.0, 0.0],
                blue: [0.9997, 0.0, 0.0],
            }),
            vignetting: Some(lens::Vignetting {
                k: [-0.3, 0.05, -0.01],
            }),
            radius_scale: 1.0,
            vignetting_scale: 1.0,
            scale: lens::Scale::Auto,
        };
        time("lens", runs, || {
            let t = Instant::now();
            let (out, _) = lens::correct(&image, &correction);
            (t.elapsed(), out.data)
        });
    }
    if wants("vignette") {
        // The vignetting alone: the path that does not resample.
        let correction = lens::LensCorrection {
            distortion: None,
            chromatic_aberration: None,
            vignetting: Some(lens::Vignetting {
                k: [-0.3, 0.05, -0.01],
            }),
            radius_scale: 1.0,
            vignetting_scale: 1.0,
            scale: lens::Scale::Fixed(1.0),
        };
        time("vignette", runs, || {
            let t = Instant::now();
            let (out, _) = lens::correct(&image, &correction);
            (t.elapsed(), out.data)
        });
    }
    if wants("blur") {
        let plane: Vec<f32> = image.data.as_chunks::<3>().0.iter().map(|p| p[1]).collect();
        time("blur", runs, || {
            let mut p = plane.clone();
            let t = Instant::now();
            dual::gaussian_blur(&mut p, w, h, sharpen::BLEND_BLUR_SIGMA);
            (t.elapsed(), p)
        });
    }
    if wants("sharpen") {
        let options = sharpen::SharpenOptions::default();
        time("sharpen", runs, || {
            let mut im = image.clone();
            let t = Instant::now();
            sharpen::sharpen(&mut im, &options, prepared.sharpen_radius, prepared.ceiling);
            (t.elapsed(), im.data)
        });
    }
    if wants("dual") {
        let contrast = dual_contrast(&settings, prepared.noise.as_ref(), prepared.gains);
        time("dual", runs, || {
            let t = Instant::now();
            let (out, _) = demosaic_cfa_with(
                &prepared.samples,
                w,
                h,
                &pattern,
                DemosaicMethod::AmazeVng4,
                contrast,
            )
            .unwrap();
            (t.elapsed(), out)
        });
    }
    if pattern.is_xtrans() {
        for (name, passes) in [
            ("markesteijn1", xtrans::Passes::One),
            ("markesteijn3", xtrans::Passes::Three),
        ] {
            if wants(name) {
                time(name, runs, || {
                    let t = Instant::now();
                    let out =
                        xtrans::demosaic_markesteijn(&prepared.samples, w, h, &pattern, passes)
                            .unwrap();
                    (t.elapsed(), out)
                });
            }
        }
    }
    // RCD is Bayer only; on an X-Trans frame it is left out of `all`,
    // and out when named.
    if wants("rcd") && rcd::is_bayer(&pattern) {
        time("rcd", runs, || {
            let t = Instant::now();
            let out = rcd::demosaic_rcd(&prepared.samples, w, h, &pattern).unwrap();
            (t.elapsed(), out)
        });
    }
    if wants("highlights") || wants("segments") {
        // The clips the develop rebuilds at, the headroom's included.
        let clips = prepared
            .gains
            .map(|g| g * prepared.headroom * highlights::CLIP_MAGIC);
        if wants("highlights") {
            time("highlights", runs, || {
                let t = Instant::now();
                let (out, _) =
                    highlights::inpaint_opposed(&prepared.samples, w, h, &pattern, clips).unwrap();
                (t.elapsed(), out)
            });
        }
        if wants("segments") {
            let (opposed, _) =
                highlights::inpaint_opposed(&prepared.samples, w, h, &pattern, clips).unwrap();
            time("segments", runs, || {
                let t = Instant::now();
                let (out, _) = segments::inpaint_segments(
                    &prepared.samples,
                    &opposed,
                    w,
                    h,
                    &pattern,
                    clips,
                    &segments::SegmentOptions::default(),
                )
                .unwrap();
                (t.elapsed(), out)
            });
        }
    }
    if wants("ca") && ca::corrects(&pattern) {
        time("ca", runs, || {
            let t = Instant::now();
            let (out, _) =
                ca::correct_ca(&prepared.samples, w, h, &pattern, &ca::CaOptions::default())
                    .unwrap();
            (t.elapsed(), out)
        });
    }
    if wants("defringe") {
        let options = defringe::DefringeOptions::default();
        time("defringe", runs, || {
            let mut im = image.clone();
            let t = Instant::now();
            defringe::defringe(&mut im, &options);
            (t.elapsed(), im.data)
        });
    }
    // Clarity alone and with Texture beside it, each as its radius
    // chooses and with the exact filter forced, which is the op as it
    // was before the coarse grid, to the bit; Structure alone, with
    // Clarity, and with both others.
    for (name, texture, clarity, structure, step) in [
        ("clarity", 0.0, 0.5, 0.0, None),
        ("clarity_exact", 0.0, 0.5, 0.0, Some(1)),
        ("local_contrast", 0.5, 0.5, 0.0, None),
        ("local_contrast_exact", 0.5, 0.5, 0.0, Some(1)),
        ("structure", 0.0, 0.0, 0.5, None),
        ("clarity_structure", 0.0, 0.5, 0.5, None),
        ("local_contrast_all", 0.5, 0.5, 0.5, None),
    ] {
        if !wants(name) {
            continue;
        }
        let options = local_contrast::LocalContrastOptions {
            texture,
            clarity,
            structure,
        };
        time(name, runs, || {
            let mut im = image.clone();
            let t = Instant::now();
            match step {
                None => {
                    local_contrast::local_contrast(&mut im, &options, prepared.ceiling);
                }
                Some(step) => {
                    local_contrast::local_contrast_with_step(
                        &mut im,
                        &options,
                        prepared.ceiling,
                        step,
                    );
                }
            }
            (t.elapsed(), im.data)
        });
    }
}

/// The dual demosaic's time split, and the RCD dual against the AMaZE
/// dual at the fit view's scale, on each frame `GREYCARD_RAWS` names
/// (comma separated): the measure a quick demosaic on the GPU stands
/// on, the RCD dual standing in for the AMaZE dual until the latter
/// lands.
///
/// ```text
/// GREYCARD_RAWS=a.CR3,b.NEF GREYCARD_RUNS=3 RAYON_NUM_THREADS=8 \
///     cargo test --release -p greycard-core --lib -- --ignored --nocapture dual_split
/// ```
///
/// The split: AMaZE alone, VNG4 alone, the blend mask alone (the
/// lightness, the contrast's sigmoid, and its blur), then the whole
/// dual, whose blend is the rest. The comparison: both duals box
/// reduced to a long edge of 2000, as the fit view shows them, and
/// compared in camera RGB clamped to 0..1, both linear and through a
/// 1/2.2 power as a display sees it: PSNR, the largest difference, and
/// the 99.9th percentile of the differences.
#[test]
#[ignore = "a measurement, not a test"]
fn dual_split() {
    let Ok(paths) = std::env::var("GREYCARD_RAWS") else {
        panic!("GREYCARD_RAWS names the frames to measure");
    };
    let runs: usize = std::env::var("GREYCARD_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    for path in paths.split(',') {
        let frame = crate::decode::decode_path(path).expect("the frame decodes");
        let iso = frame.shot.iso;
        let settings = DevelopSettings::default();
        let prepared = prepare(&frame, &settings, true).expect("the frame prepares");
        let (w, h) = (prepared.width, prepared.height);
        let pattern = prepared.pattern.clone().expect("a mosaic");
        let contrast = dual_contrast(&settings, prepared.noise.as_ref(), prepared.gains);
        let s = &prepared.samples;
        println!(
            "frame {w}x{h} ({:.1} MP), ISO {iso:?}, {} threads",
            (w * h) as f64 / 1e6,
            rayon::current_num_threads()
        );
        let mut amaze_out = Vec::new();
        time("amaze", runs, || {
            let t = Instant::now();
            amaze_out = amaze::demosaic_amaze(s, w, h, &pattern).unwrap();
            (t.elapsed(), Vec::new())
        });
        time("vng4", runs, || {
            let t = Instant::now();
            let out = vng4::demosaic_vng4(s, w, h, &pattern).unwrap();
            (t.elapsed(), out)
        });
        time("rcd", runs, || {
            let t = Instant::now();
            let out = rcd::demosaic_rcd(s, w, h, &pattern).unwrap();
            (t.elapsed(), out)
        });
        let (threshold, _) = dual::threshold_for(contrast, &amaze_out, w, h);
        time("mask", runs, || {
            let t = Instant::now();
            let l = dual::lightness(&amaze_out, w, h);
            let mask = dual::blend_mask(&l, w, h, &threshold);
            (t.elapsed(), mask)
        });
        drop(amaze_out);
        let mut amaze_dual = Vec::new();
        time("amaze dual", runs, || {
            let t = Instant::now();
            amaze_dual = dual::demosaic_dual(s, w, h, &pattern, DemosaicMethod::Amaze, contrast)
                .unwrap()
                .0;
            (t.elapsed(), Vec::new())
        });
        let mut rcd_dual = Vec::new();
        time("rcd dual", runs, || {
            let t = Instant::now();
            rcd_dual = dual::demosaic_dual(s, w, h, &pattern, DemosaicMethod::Rcd, contrast)
                .unwrap()
                .0;
            (t.elapsed(), Vec::new())
        });
        let (fa, fw, fh) = fit_view(&amaze_dual, w, h, 2000);
        let (fr, _, _) = fit_view(&rcd_dual, w, h, 2000);
        for (name, curve) in [
            ("linear", (|v: f32| v) as fn(f32) -> f32),
            ("gamma 2.2", |v: f32| v.powf(1.0 / 2.2)),
        ] {
            let mut d: Vec<f32> = fa
                .iter()
                .zip(&fr)
                .map(|(a, b)| (curve(a.clamp(0.0, 1.0)) - curve(b.clamp(0.0, 1.0))).abs())
                .collect();
            let mse = d.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / d.len() as f64;
            d.sort_by(f32::total_cmp);
            println!(
                "fit view {fw}x{fh}, {name}: PSNR {:.1} dB, max {:.5}, p99.9 {:.5}",
                -10.0 * mse.log10(),
                d[d.len() - 1],
                d[d.len() * 999 / 1000]
            );
        }
        // The same at 1:1, for scale.
        let mut d: Vec<f32> = amaze_dual
            .iter()
            .zip(&rcd_dual)
            .map(|(a, b)| (a.clamp(0.0, 1.0) - b.clamp(0.0, 1.0)).abs())
            .collect();
        let mse = d.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / d.len() as f64;
        d.sort_by(f32::total_cmp);
        println!(
            "1:1, linear: PSNR {:.1} dB, max {:.5}, p99.9 {:.5}",
            -10.0 * mse.log10(),
            d[d.len() - 1],
            d[d.len() * 999 / 1000]
        );
    }
}

/// Interleaved RGB box reduced by the whole factor that brings its
/// long edge to `long` or under, as the fit view's mipmaps do.
fn fit_view(rgb: &[f32], w: usize, h: usize, long: usize) -> (Vec<f32>, usize, usize) {
    let f = w.max(h).div_ceil(long);
    let (ow, oh) = (w / f, h / f);
    let mut out = vec![0.0f32; ow * oh * 3];
    for y in 0..oh {
        for x in 0..ow {
            for c in 0..3 {
                let mut sum = 0.0f64;
                for yy in y * f..y * f + f {
                    for xx in x * f..x * f + f {
                        sum += rgb[(yy * w + xx) * 3 + c] as f64;
                    }
                }
                out[(y * ow + x) * 3 + c] = (sum / (f * f) as f64) as f32;
            }
        }
    }
    (out, ow, oh)
}
