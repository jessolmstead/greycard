//! The generative remove against the reference pipeline: set
//! `GREYCARD_MODELS` to a store root holding `rorem-mixed-fp16` and
//! `GREYCARD_REMOVE_PARITY` to the folder the reference was made in
//! (`photos/crop{N}.png`, `photos/mask{N}.png`,
//! `runs/noise_{N}_1234.f32`, the seed's noise as raw f32, and
//! `runs/latents_ref_torch32_{N}.npy`, the torch fp32 run's final
//! latents, for N of 512 and 1024), and run with `--ignored
//! --test-threads=1`: the two at once would not fit a card. Needs
//! WebGPU.

use std::path::Path;

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

/// At 1024, with the reference's own noise: under the hole the
/// removal is the torch fp32 run's, decoded by the same decoder, to
/// above 35 dB, and outside it the picture.
#[test]
#[ignore]
fn the_removal_at_1024_matches_the_reference() {
    parity(1024, 35.0);
}

/// The same at 512, the other size the graphs are exported at, to
/// above 33 dB: fp16 drifts further from fp32 at 512 on any provider
/// (CUDA's fp16 gives 38.7 dB here against 44.8 at 1024), and WebGPU's
/// further still, 34.3 dB, as the `rorem` branch's own example
/// measured before this engine. The difference map is glints and grain
/// with no structure, and a slight darkening.
#[test]
#[ignore]
fn the_removal_at_512_matches_the_reference() {
    parity(512, 33.0);
}

fn parity(n: usize, floor: f64) {
    let (Some(models), Some(dir)) = (
        std::env::var_os("GREYCARD_MODELS"),
        std::env::var_os("GREYCARD_REMOVE_PARITY"),
    ) else {
        eprintln!("skipped: GREYCARD_MODELS and GREYCARD_REMOVE_PARITY are not both set");
        return;
    };
    let dir = Path::new(&dir);
    let store = Store::at(models);
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
    let hole: Vec<f32> = inside.iter().map(|&m| f32::from(u8::from(m))).collect();
    let noise = read_f32(&dir.join(format!("runs/noise_{n}_1234.f32")));
    let reference = read_npy_f32(&dir.join(format!("runs/latents_ref_torch32_{n}.npy")));
    assert_eq!(reference.len(), 4 * (n / 8) * (n / 8));

    let mut remove = Remove::load(&store, &[Provider::WebGpu]).expect("load");
    let out = remove
        .remove_with_noise(&image, &hole, &noise)
        .expect("remove");
    let t = remove.times();
    println!(
        "{:?} at {}: loads {:.2}s, encode {:.2}s, {} UNet calls {:.2}s, decode {:.2}s, total {:.2}s",
        remove.memory(),
        t.size,
        t.loads,
        t.encode,
        t.unet_calls,
        t.steps,
        t.decode,
        t.total
    );
    let want = remove.decode(&reference, n).expect("decode");
    let hole_psnr = psnr(&out, &want, &inside);
    let outside: Vec<bool> = inside.iter().map(|m| !m).collect();
    let drift = psnr(&want, &image, &outside);
    println!(
        "hole PSNR {hole_psnr:.2} dB against the reference's decode; its outside {drift:.2} dB from the picture"
    );
    if let Ok(png) = image::open(dir.join(format!("runs/ref_torch32_{n}.png"))) {
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
