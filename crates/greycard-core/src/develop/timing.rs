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
        println!("{name:>14} run {i}: {t:8.4} s  (load {at})  bits {bits:016x}");
        times.push(t);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "{name:>14}: min {:.4} s  median {:.4} s  over {runs}",
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
    let model = prepared.noise_model().expect("the noise was measured");
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
    if wants("bilinear") {
        time("bilinear", runs, || {
            let t = Instant::now();
            let out = demosaic_bilinear(&prepared.samples, w, h, &pattern);
            (t.elapsed(), out)
        });
    }
    if wants("nlm") {
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
    if wants("rcd") {
        time("rcd", runs, || {
            let t = Instant::now();
            let out = rcd::demosaic_rcd(&prepared.samples, w, h, &pattern).unwrap();
            (t.elapsed(), out)
        });
    }
    if wants("highlights") || wants("segments") {
        let clips = prepared.gains.map(|g| g * highlights::CLIP_MAGIC);
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
    if wants("ca") {
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
    if wants("local_contrast") {
        let options = local_contrast::LocalContrastOptions {
            texture: 0.5,
            clarity: 0.5,
        };
        time("local_contrast", runs, || {
            let mut im = image.clone();
            let t = Instant::now();
            local_contrast::local_contrast(&mut im, &options, prepared.ceiling);
            (t.elapsed(), im.data)
        });
    }
}
