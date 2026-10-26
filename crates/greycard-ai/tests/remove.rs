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
