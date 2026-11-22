//! The generative remove against the reference pipeline: set
//! `GREYCARD_MODELS` to a store root holding `rorem-mixed-fp16` and
//! `GREYCARD_REMOVE_PARITY` to the folder the reference was made in
//! (`photos/crop{N}.png`, `photos/mask{N}.png`,
//! `runs/noise_{N}_1234.f32`, the seed's noise as raw f32, and
//! `runs/latents_ref_torch32_{N}.npy`, the torch fp32 run's final
//! latents, for N of 512 and 1024), and run with `--ignored
//! --test-threads=1`: two at once would not fit a card. Needs
//! WebGPU. The 4-step tests want the store to hold the 4-step UNet
//! too ([`greycard_ai::REMOVE_FAST`]) and the folder
//! `runs/noise_lcm_{N}_1234.f32`, the noise of the three calls, and
//! `runs/latents_ref_lcm_torch32_{N}.npy`, diffusers' 4-step pipeline
//! in torch fp32 on that noise.

use std::path::Path;

use greycard_ai::remove::{Steps, Vae, composite};
use greycard_ai::{Provider, Remove, Rgbf, Store};

fn read_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

/// A version 1 `.npy` of little-endian f32: the data after its header.
fn read_npy_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(&bytes[..6], b"\x93NUMPY");
    let header = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    let text = String::from_utf8_lossy(&bytes[10..10 + header]);
    assert!(text.contains("'<f4'"), "{text}");
    bytes[10 + header..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

/// PSNR over the pixels where `inside` is true, peak 1.
fn psnr(a: &Rgbf, b: &Rgbf, inside: &[bool]) -> f64 {
    let (mut sum, mut n) = (0.0f64, 0usize);
    for (i, _) in inside.iter().enumerate().filter(|(_, m)| **m) {
        for c in 0..3 {
            let d = f64::from(a.data[i * 3 + c] - b.data[i * 3 + c]);
            sum += d * d;
        }
        n += 3;
    }
    10.0 * (1.0 / (sum / n as f64)).log10()
}

/// At 1024, with the reference's own noise and the decoder whole: under
/// the hole the removal is the torch fp32 run's, decoded by the same
/// decoder, to above 35 dB, and outside it the picture. The removal is
/// matched to the picture's level round the hole and the reference's
/// decode is not: 38.58 dB, against 38.6 before the match.
#[test]
#[ignore]
fn the_removal_at_1024_matches_the_reference() {
    parity(1024, 35.0, Vae::Whole, Steps::Euler25);
}

/// At 1024 with the decoder in 512 tiles: against the reference, as
/// the whole decoder is held, and against the whole decoder's own
/// answer, in the hole and outside it, where a seam would show. The
/// tiles measured 38.22 dB against the reference (the removal matched
/// to the picture's level, the reference's decode not; 38.3 before the
/// match), and 47.11 dB in the hole and 45.05 outside against the whole
/// decoder, both unmatched decodes.
#[test]
#[ignore]
fn the_removal_at_1024_in_tiles_matches_the_whole() {
    let Some(case) = Case::load(1024, Steps::Euler25) else {
        return;
    };
    let mut remove = Remove::load(&case.store, &[Provider::WebGpu]).expect("load");
    remove.set_steps(Steps::Euler25).expect("25 steps");
    remove.set_vae(Vae::Whole);
    let whole = remove
        .decoded_with_noise(&case.image, &case.hole, &case.noise)
        .expect("whole");
    report(&remove);
    remove.set_vae(Vae::Tiled);
    let tiled = remove
        .decoded_with_noise(&case.image, &case.hole, &case.noise)
        .expect("tiled");
    report(&remove);
    assert!(remove.times().tiled);
    let out = composite(&case.image, &case.hole, &tiled).expect("an honest answer");
    remove.set_vae(Vae::Whole);
    let want = remove.decode(&case.reference, 1024).expect("decode");
    let outside: Vec<bool> = case.inside.iter().map(|m| !m).collect();
    let (against_ref, hole, rest) = (
        psnr(&out, &want, &case.inside),
        psnr(&tiled, &whole, &case.inside),
        psnr(&tiled, &whole, &outside),
    );
    println!(
        "tiled: hole PSNR {against_ref:.2} dB against the reference's decode; against the whole decoder {hole:.2} dB in the hole, {rest:.2} dB outside"
    );
    assert!(against_ref > 35.0, "{against_ref:.2} dB");
    assert!(hole > 42.0, "{hole:.2} dB");
    assert!(rest > 42.0, "{rest:.2} dB");
}

/// The same at 512, the other size the graphs are exported at, to
/// above 33 dB: fp16 drifts further from fp32 at 512 on any provider
/// (CUDA's fp16 gives 38.7 dB here against 44.8 at 1024), and WebGPU's
/// further still, 34.3 dB, as the `rorem` branch's own example
/// measured before this engine; 33.96 dB here with the removal matched
/// to the picture's level and the reference's decode not. The
/// difference map is glints and grain with no structure, and a slight
/// darkening.
#[test]
#[ignore]
fn the_removal_at_512_matches_the_reference() {
    parity(512, 33.0, Vae::Whole, Steps::Euler25);
}

/// The 4-step UNet and sampler against diffusers' pipeline at 1024,
/// held to the 25-step floor: measured 37.4 dB with the authors' LoRA
/// and 38.2 with ours, against 38.6 for 25 steps.
#[test]
#[ignore]
fn the_4_step_removal_at_1024_matches_the_reference() {
    parity(1024, 35.0, Vae::Whole, Steps::Lcm4);
}

/// The same at 512, held to the 25-step floor there.
#[test]
#[ignore]
fn the_4_step_removal_at_512_matches_the_reference() {
    parity(512, 33.0, Vae::Whole, Steps::Lcm4);
}

/// A large, smooth, strongly colored object filling a window's bottom
/// right corner and running off two of its edges, under a hole drawn
/// well past it: removed, not painted back. The sampler starts at
/// strength 0.99, where a tenth of the start's signal is left, and
/// from the window as it is that tenth was the object; the model, which
/// was trained on starts with the object already gone, painted a muddy
/// orange back in. On the CPU the hole's mean was 0.047 from its ring's
/// in Oklab a, b from that start (seed 0) and 0.003 to 0.005 from the
/// prefilled one (seeds 0 to 2), the object 0.140. Needs only
/// `GREYCARD_MODELS`; on WebGPU, or on the CPU with `GREYCARD_REMOVE_CPU`
/// set (about 80 s a seed on 16 cores).
#[test]
#[ignore]
fn a_large_orange_object_is_removed() {
    let Some(mut remove) = remove_on_hand() else {
        return;
    };
    let (image, hole) = orange_in_the_corner();
    let ring = ring_round(&hole, image.width, image.height, 12);
    let inside: Vec<bool> = hole.iter().map(|&m| m > 0.0).collect();
    let before = left(&image, &inside, &ring);
    println!("before: {before:?}");
    for seed in 0..3 {
        let out = remove.remove(&image, &hole, seed).expect("remove");
        let after = left(&out, &inside, &ring);
        println!("seed {seed}: {after:?}");
        assert!(
            after.ab < 0.03 && after.orange < 0.05,
            "seed {seed}: {after:?}"
        );
    }
}

/// The same object running on past a crop's edge, the hole drawn only
/// as far as the crop shows: what the crop hides is given to the model
/// as hole too, as the engine gives it (`greycard_engine::shown`), and
/// the object is removed from what the user sees. Shown the strip, the
/// model carried the object back in from it: on the CPU the hole's
/// mean was 0.129 from its ring's in Oklab a, b and 56% of it orange
/// (seed 0, printed as `shown the strip`), against 0.002 to 0.004 and
/// none with the strip given as hole (seeds 0 to 2).
#[test]
#[ignore]
fn an_object_running_on_past_the_crop_is_removed() {
    let Some(mut remove) = remove_on_hand() else {
        return;
    };
    let (image, whole) = orange_in_the_corner();
    let (w, h) = (image.width, image.height);
    // The crop's edge at x 470: the strip right of it hidden, the hole
    // cut there, the object running on under the strip.
    let hidden: Vec<bool> = (0..w * h).map(|i| i % w >= 470).collect();
    let hole: Vec<f32> = whole
        .iter()
        .zip(&hidden)
        .map(|(&m, &out)| if out { 0.0 } else { m })
        .collect();
    let given: Vec<f32> = hole
        .iter()
        .zip(&hidden)
        .map(|(&m, &out)| if out { 1.0 } else { m })
        .collect();
    let inside: Vec<bool> = hole.iter().map(|&m| m > 0.0).collect();
    let ring: Vec<bool> = ring_round(&hole, w, h, 12)
        .into_iter()
        .zip(&hidden)
        .map(|(r, &out)| r && !out)
        .collect();
    let shown_the_strip = remove.remove(&image, &hole, 0).expect("remove");
    println!(
        "shown the strip: {:?}",
        left(&shown_the_strip, &inside, &ring)
    );
    for seed in 0..3 {
        let out = remove.remove(&image, &given, seed).expect("remove");
        let after = left(&out, &inside, &ring);
        println!("seed {seed}: {after:?}");
        assert!(
            after.ab < 0.03 && after.orange < 0.05,
            "seed {seed}: {after:?}"
        );
    }
}

/// The remove from `GREYCARD_MODELS`, on WebGPU or, with
/// `GREYCARD_REMOVE_CPU` set, the CPU, at 25 steps; `None`, said,
/// without the store.
fn remove_on_hand() -> Option<Remove> {
    let Some(models) = std::env::var_os("GREYCARD_MODELS") else {
        eprintln!("skipped: GREYCARD_MODELS is not set");
        return None;
    };
    let store = Store::at(models);
    let mut remove = if std::env::var_os("GREYCARD_REMOVE_CPU").is_some() {
        Remove::load_on_cpu(&store)
    } else {
        Remove::load(&store, &[Provider::WebGpu])
    }
    .expect("load");
    remove.set_steps(Steps::Euler25).expect("25 steps");
    Some(remove)
}

/// How much of an object's orange is left under a hole: the distance
/// of the hole's mean Oklab a, b from the ring's round it, and the
/// share of the hole's pixels of an orange hue (40 to 80 degrees) and
/// a chroma past 0.08.
#[derive(Debug)]
struct Left {
    ab: f64,
    orange: f64,
}

fn left(image: &Rgbf, inside: &[bool], ring: &[bool]) -> Left {
    let lab = |i: usize| oklab(&image.data[i * 3..i * 3 + 3]);
    let mean = |set: &[bool]| {
        let (mut a, mut b, mut n) = (0.0, 0.0, 0.0);
        for (i, _) in set.iter().enumerate().filter(|(_, m)| **m) {
            let [_, la, lb] = lab(i);
            (a, b, n) = (a + la, b + lb, n + 1.0);
        }
        (a / n, b / n)
    };
    let ((ha, hb), (ra, rb)) = (mean(inside), mean(ring));
    let (mut orange, mut n) = (0.0, 0.0);
    for (i, _) in inside.iter().enumerate().filter(|(_, m)| **m) {
        let [_, a, b] = lab(i);
        let hue = b.atan2(a).to_degrees();
        orange += f64::from(u8::from((40.0..80.0).contains(&hue) && a.hypot(b) > 0.08));
        n += 1.0;
    }
    Left {
        ab: (ha - ra).hypot(hb - rb),
        orange: orange / n,
    }
}

/// Oklab (Ottosson, 2020) of an sRGB-encoded pixel.
fn oklab(rgb: &[f32]) -> [f64; 3] {
    let lin = |v: f32| {
        let v = f64::from(v.clamp(0.0, 1.0));
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    [
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    ]
}

/// The pixels within `r` (a square's reach) of the hole and not in it.
fn ring_round(hole: &[f32], w: usize, h: usize, r: usize) -> Vec<bool> {
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            hole[i] <= 0.0
                && (y.saturating_sub(r)..(y + r + 1).min(h)).any(|yy| {
                    (x.saturating_sub(r)..(x + r + 1).min(w)).any(|xx| hole[yy * w + xx] > 0.0)
                })
        })
        .collect()
}

/// A 512 × 384 window made in code: a pale sky, a dark band of rock,
/// and blue-green water with ripples and glints below, and a soft,
/// out-of-focus orange shape filling the bottom right, off the right
/// and bottom edges, nearly half the window's width; the hole, an
/// ellipse 32 pixels past where the shape fades out.
fn orange_in_the_corner() -> (Rgbf, Vec<f32>) {
    let (w, h) = (512usize, 384usize);
    let hash = |x: usize, y: usize| {
        let mut z = (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (y as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z ^= z >> 31;
        z = z.wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z >> 40) as f32 / (1u64 << 24) as f32) - 0.5
    };
    let (cx, cy, a, b) = (470.0f32, 360.0f32, 200.0f32, 130.0f32);
    let reach = |x: usize, y: usize| {
        let (dx, dy) = ((x as f32 - cx) / a, (y as f32 - cy) / b);
        (dx * dx + dy * dy).sqrt()
    };
    let mut data = Vec::with_capacity(w * h * 3);
    let mut hole = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (xf, yf) = (x as f32, y as f32);
            let grain = 0.02 * hash(x, y);
            let back = if yf < 70.0 {
                let t = yf / 70.0;
                [0.75 - 0.1 * t, 0.82 - 0.08 * t, 0.9 - 0.05 * t]
            } else if yf < 150.0 + 12.0 * (xf * 0.031).sin() {
                let v = 0.22 + 0.06 * (xf * 0.11 + yf * 0.07).sin() * (yf * 0.05).cos();
                [v + 0.02, v, v - 0.02]
            } else {
                let ripple = 0.05 * (yf * 0.35 + 3.0 * (xf * 0.02).sin()).sin();
                let glint = if hash(x / 3, y / 2) > 0.47 { 0.3 } else { 0.0 };
                [
                    0.18 + ripple + glint,
                    0.38 + ripple + glint,
                    0.42 + ripple + glint,
                ]
            };
            // Out of focus: full inside, fading over a fifth of the
            // radius, with a soft shading top to bottom.
            let r = reach(x, y);
            let alpha = ((1.15 - r) / 0.3).clamp(0.0, 1.0);
            let alpha = alpha * alpha * (3.0 - 2.0 * alpha);
            let shade = 0.88 + 0.12 * ((yf - (cy - b)) / (2.0 * b)).clamp(0.0, 1.0);
            let orange = [0.95 * shade, 0.42 * shade, 0.06 * shade];
            for c in 0..3 {
                let v = back[c] * (1.0 - alpha) + orange[c] * alpha + grain;
                data.push(v.clamp(0.0, 1.0));
            }
            let (hx, hy) = ((xf - cx) / (1.15 * a + 32.0), (yf - cy) / (1.15 * b + 32.0));
            hole.push(f32::from(u8::from(hx * hx + hy * hy <= 1.0)));
        }
    }
    (Rgbf::new(w, h, data), hole)
}

/// One reference case: the store, the crop and its hole, the noise,
/// and the torch fp32 run's latents; `None`, said, without the two
/// variables.
struct Case {
    store: Store,
    dir: std::path::PathBuf,
    image: Rgbf,
    inside: Vec<bool>,
    hole: Vec<f32>,
    noise: Vec<f32>,
    reference: Vec<f32>,
}

impl Case {
    fn load(n: usize, steps: Steps) -> Option<Self> {
        let (Some(models), Some(dir)) = (
            std::env::var_os("GREYCARD_MODELS"),
            std::env::var_os("GREYCARD_REMOVE_PARITY"),
        ) else {
            eprintln!("skipped: GREYCARD_MODELS and GREYCARD_REMOVE_PARITY are not both set");
            return None;
        };
        let dir = Path::new(&dir).to_path_buf();
        let rgb = image::open(dir.join(format!("photos/crop{n}.png")))
            .expect("the crop")
            .to_rgb8();
        let mask = image::open(dir.join(format!("photos/mask{n}.png")))
            .expect("the mask")
            .to_luma8();
        let image = Rgbf::new(
            n,
            n,
            rgb.as_raw().iter().map(|&v| v as f32 / 255.0).collect(),
        );
        let inside: Vec<bool> = mask.as_raw().iter().map(|&v| v > 127).collect();
        let hole = inside.iter().map(|&m| f32::from(u8::from(m))).collect();
        let lcm = if steps == Steps::Lcm4 { "_lcm" } else { "" };
        let noise = dir.join(format!("runs/noise{lcm}_{n}_1234.f32"));
        if !noise.is_file() {
            eprintln!("skipped: no {}", noise.display());
            return None;
        }
        let noise = read_f32(&noise);
        let reference = read_npy_f32(&dir.join(format!("runs/latents_ref{lcm}_torch32_{n}.npy")));
        assert_eq!(noise.len(), steps.noises() * 4 * (n / 8) * (n / 8));
        assert_eq!(reference.len(), 4 * (n / 8) * (n / 8));
        Some(Self {
            store: Store::at(models),
            dir,
            image,
            inside,
            hole,
            noise,
            reference,
        })
    }
}

fn report(remove: &Remove) {
    let t = remove.times();
    println!(
        "{:?} at {}, decoder {}: loads {:.2}s, encode {:.2}s, {} UNet calls {:.2}s, decode {:.2}s, total {:.2}s; outside the hole {:?}",
        remove.memory(),
        t.size,
        if t.tiled { "in tiles" } else { "whole" },
        t.loads,
        t.encode,
        t.unet_calls,
        t.steps,
        t.decode,
        t.total,
        t.drift
    );
}

fn parity(n: usize, floor: f64, vae: Vae, steps: Steps) {
    let Some(Case {
        store,
        dir,
        image,
        inside,
        hole,
        noise,
        reference,
    }) = Case::load(n, steps)
    else {
        return;
    };
    let mut remove = Remove::load(&store, &[Provider::WebGpu]).expect("load");
    remove.set_steps(steps).expect("the steps' UNet");
    remove.set_vae(vae);
    let out = remove
        .remove_with_noise(&image, &hole, &noise)
        .expect("remove");
    report(&remove);
    let t = remove.times();
    // An honest answer follows the picture outside the hole.
    assert!(
        t.drift.correlation.is_some_and(|r| r > 0.9),
        "{:?}",
        t.drift
    );
    let want = remove.decode(&reference, n).expect("decode");
    let hole_psnr = psnr(&out, &want, &inside);
    let outside: Vec<bool> = inside.iter().map(|m| !m).collect();
    let drift = psnr(&want, &image, &outside);
    println!(
        "hole PSNR {hole_psnr:.2} dB against the reference's decode; its outside {drift:.2} dB from the picture"
    );
    let lcm = if steps == Steps::Lcm4 { "_lcm" } else { "" };
    if let Ok(png) = image::open(dir.join(format!("runs/ref{lcm}_torch32_{n}.png"))) {
        let png = Rgbf::new(
            n,
            n,
            png.to_rgb8()
                .as_raw()
                .iter()
                .map(|&v| v as f32 / 255.0)
                .collect(),
        );
        println!(
            "hole PSNR {:.2} dB against the reference's own PNG",
            psnr(&out, &png, &inside)
        );
    }
    assert!(hole_psnr > floor, "{hole_psnr:.2} dB");
    for (i, _) in outside.iter().enumerate().filter(|(_, m)| **m) {
        assert_eq!(out.data[i * 3], image.data[i * 3]);
    }
}
