//! greycard-bench: mosaic reference images, demosaic them with each of the
//! engine's algorithms, score the result.
//!
//! Every demosaic change has to move these numbers. Published tables for
//! the Kodak and McMaster sets exist for most algorithms, so a port or a
//! from-scratch implementation is checked against the literature, not
//! against our eyes.

mod metrics;
mod mosaic;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::Parser;
use greycard_core::develop::ca::{CaOptions, correct_ca};
use greycard_core::develop::demosaic_cfa_with;
use greycard_core::develop::denoise::{
    DEFAULT_CHROMA, DEFAULT_HYBRID_FROM, DenoiseMethod, DenoiseOptions, Strength, denoise_profiled,
};
use greycard_core::develop::dual::DualContrast;
use greycard_core::develop::highlights::{CLIP_MAGIC, inpaint_opposed};
use greycard_core::develop::nlm::{DEFAULT_CENTER_WEIGHT, DEFAULT_SEARCH, NlmOptions};
use greycard_core::develop::noise::{MID_GREY, NoiseModel, estimate_noise};
use greycard_core::develop::segments::{SegmentOptions, inpaint_segments};
use greycard_core::raw::CfaPattern;
use greycard_core::{DemosaicMethod, HighlightMode};
use rayon::prelude::*;

use metrics::Scores;
use mosaic::{bayer_pattern, mosaic, srgb_decode, srgb_encode};

#[derive(Parser)]
#[command(
    name = "greycard-bench",
    version,
    about = "Score demosaic algorithms against reference images"
)]
struct Cli {
    /// Image files or directories of them; each directory is one data set
    inputs: Vec<PathBuf>,
    /// Algorithms to run, by name, or "all"
    #[arg(long, default_value = "all")]
    method: String,
    /// Bayer pattern to mosaic with
    #[arg(long, default_value = "RGGB")]
    pattern: String,
    /// Mosaic the sRGB values as they are (the literature's convention), or
    /// linearize first, demosaic, and re-encode: what the engine really sees
    #[arg(long)]
    linear: bool,
    /// Pixels to leave out on every side when scoring
    #[arg(long, default_value_t = 8)]
    border: usize,
    /// Print every image, not only the per-set means
    #[arg(long)]
    per_image: bool,
    /// Comma-separated output instead of a table
    #[arg(long)]
    csv: bool,
    /// Write each demosaiced image, and its amplified error, here
    #[arg(long)]
    dump: Option<PathBuf>,
    /// Overexpose by this many stops before mosaicking, so each channel
    /// clips at its white balance gain, then bring the result back down:
    /// scores highlight handling against the unclipped reference
    #[arg(long, default_value_t = 0.0)]
    overexpose: f64,
    /// Highlight handling when overexposing: opposed or clip
    #[arg(long, default_value = "opposed")]
    highlights: HighlightMode,
    /// Add lateral chromatic aberration before mosaicking: red displaced
    /// outward by this many pixels at the corners, blue inward by 3/4 of it
    #[arg(long, default_value_t = 0.0)]
    aberrate: f64,
    /// Leave the aberration uncorrected, to see what it costs
    #[arg(long)]
    no_ca: bool,
    /// Add sensor-like noise to the mosaic: the standard deviation at mid
    /// grey (0.18), scaling with the square root of the signal like shot
    /// noise, plus a tenth of it everywhere as read noise
    #[arg(long, default_value_t = 0.0)]
    noise: f64,
    /// Contrast threshold of the dual demosaics: "tiles" (RawTherapee's
    /// flattest-tile search), "noise" (from a noise model estimated on the
    /// mosaic), or a number in RawTherapee's percent
    #[arg(long, default_value = "tiles")]
    dual_contrast: String,
    /// Denoise after the demosaic, from a noise model measured on the
    /// (noisy) mosaic
    #[arg(long)]
    denoise: bool,
    /// How hard to denoise: a multiplier on the wavelet thresholds, or auto
    /// to follow the measured noise
    #[arg(long, default_value = "auto")]
    denoise_strength: Strength,
    /// Further multiplier on the chrominance thresholds
    #[arg(long, default_value_t = DEFAULT_CHROMA)]
    denoise_chroma: f32,
    /// Denoiser: hybrid, nlm or wavelets
    #[arg(long, default_value = "hybrid")]
    denoise_method: DenoiseMethod,
    /// Non-local means patch radius (auto from the noise if absent)
    #[arg(long)]
    nlm_patch: Option<usize>,
    /// Non-local means search radius
    #[arg(long, default_value_t = DEFAULT_SEARCH)]
    nlm_search: usize,
    /// Non-local means scattering, 0 to 1 (auto if absent)
    #[arg(long)]
    nlm_scatter: Option<f32>,
    /// Non-local means central pixel weight
    #[arg(long, default_value_t = DEFAULT_CENTER_WEIGHT)]
    nlm_center: f32,
    /// Hybrid: first wavelet scale shrunk after the means
    #[arg(long, default_value_t = DEFAULT_HYBRID_FROM)]
    hybrid_from: usize,
    /// Demosaic and denoise with the learned model at this path instead
    /// of --method and --denoise; reported as method "ai"
    #[arg(long, value_name = "MODEL.onnx")]
    ai_model: Option<PathBuf>,
    /// Which execution provider the learned model may use: "auto" (the
    /// fastest available), "webgpu" or "cpu"; to check one against the other
    #[arg(long, default_value = "auto")]
    provider: String,

    /// Tile the learned model runs on, in mosaic pixels; smaller tiles
    /// need less GPU memory and cost a little margin overhead
    #[arg(long, default_value_t = greycard_ai::denoise::DEFAULT_TILE)]
    tile: usize,
    /// Mosaic in a camera's space rather than sRGB's: the reference goes
    /// through a Canon R6 Mark II's color matrix and white balance
    /// before the mosaic and back after the demosaic, so the algorithms
    /// see the desaturated channels a sensor gives, as the engine does.
    /// Needs --linear
    #[arg(long, requires = "linear")]
    camera_space: bool,
    /// Give the denoisers the noise model that was put on (--noise)
    /// rather than the one estimated from the mosaic: what the estimator
    /// costs them
    #[arg(long, requires = "noise")]
    true_model: bool,
}

/// The white balance gains the overexposure test simulates: a daylight
/// Canon's, so red and blue clip well above green.
const TEST_GAINS: [f32; 3] = [1.9, 1.0, 1.8];

struct Reference {
    set: String,
    name: String,
    width: usize,
    height: usize,
    srgb: Vec<u8>,
}

struct Run {
    set: String,
    name: String,
    method: DemosaicMethod,
    /// What to call the method in the report: its name, or "ai".
    label: &'static str,
    scores: Scores,
    /// Demosaic time, seconds per megapixel.
    seconds_per_mp: f64,
    /// Estimated over true noise sigma at mid grey, when noise was added
    /// and a model estimated.
    sigma_ratio: Option<[f64; 3]>,
    /// The estimated noise sigma at mid grey, when a model was estimated.
    sigma_estimate: Option<[f64; 3]>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.inputs.is_empty() {
        bail!("nothing to score: pass image files or directories");
    }
    let pattern = bayer_pattern(&cli.pattern)
        .with_context(|| format!("{:?} is not a 2x2 Bayer pattern", cli.pattern))?;
    let methods: Vec<DemosaicMethod> = if cli.ai_model.is_some() {
        // The network stands in for every method; one placeholder run.
        vec![DemosaicMethod::default()]
    } else if cli.method == "all" {
        DemosaicMethod::ALL.to_vec()
    } else {
        cli.method
            .split(',')
            .map(|m| m.trim().parse().map_err(|e| anyhow::anyhow!("{e}")))
            .collect::<Result<_>>()?
    };

    let references = load_references(&cli.inputs)?;
    if references.is_empty() {
        bail!("no images found");
    }
    if let Some(dir) = &cli.dump {
        std::fs::create_dir_all(dir)?;
    }

    let denoiser = match &cli.ai_model {
        Some(path) => {
            let providers = match cli.provider.as_str() {
                "auto" => greycard_ai::Provider::available(),
                "webgpu" => vec![greycard_ai::Provider::WebGpu],
                "cpu" => vec![greycard_ai::Provider::Cpu],
                other => bail!("unknown provider {other}; auto, webgpu or cpu"),
            };
            let d = greycard_ai::Denoiser::load(path, &providers)
                .with_context(|| format!("loading {}", path.display()))?;
            eprintln!(
                "learned denoiser {} on {}",
                d.version(),
                d.provider().name()
            );
            Some(Mutex::new(d))
        }
        None => None,
    };
    let mut runs: Vec<Run> = references
        .par_iter()
        .flat_map_iter(|r| methods.iter().map(move |m| (r, *m)))
        .map(|(reference, method)| run_one(reference, method, &pattern, &cli, denoiser.as_ref()))
        .collect::<Result<_>>()?;
    runs.sort_by(|a, b| {
        (&a.set, &a.name, a.method.name()).cmp(&(&b.set, &b.name, b.method.name()))
    });

    report(&runs, &methods, &cli);
    Ok(())
}

fn load_references(inputs: &[PathBuf]) -> Result<Vec<Reference>> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for input in inputs {
        if input.is_dir() {
            let set = input
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "set".into());
            let mut entries: Vec<PathBuf> = std::fs::read_dir(input)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| is_image(p))
                .collect();
            entries.sort();
            files.extend(entries.into_iter().map(|p| (set.clone(), p)));
        } else {
            files.push(("files".into(), input.clone()));
        }
    }
    files
        .par_iter()
        .map(|(set, path)| {
            let img = image::open(path)
                .with_context(|| format!("reading {}", path.display()))?
                .to_rgb8();
            Ok(Reference {
                set: set.clone(),
                name: path
                    .file_stem()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                width: img.width() as usize,
                height: img.height() as usize,
                srgb: img.into_raw(),
            })
        })
        .collect()
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "tif" | "tiff" | "bmp" | "ppm" | "jpg" | "jpeg")
    )
}

fn run_one(
    reference: &Reference,
    method: DemosaicMethod,
    pattern: &CfaPattern,
    cli: &Cli,
    denoiser: Option<&Mutex<greycard_ai::Denoiser>>,
) -> Result<Run> {
    let (w, h) = (reference.width, reference.height);
    let to_f32 = |v: &u8| *v as f32 / 255.0;
    let rgb: Vec<f32> = if cli.linear {
        reference
            .srgb
            .iter()
            .map(|v| srgb_decode(to_f32(v)))
            .collect()
    } else {
        reference.srgb.iter().map(to_f32).collect()
    };
    let rgb = if cli.camera_space {
        to_camera(&rgb)
    } else {
        rgb
    };
    let rgb = if cli.aberrate > 0.0 {
        aberrate(&rgb, w, h, cli.aberrate as f32)
    } else {
        rgb
    };
    let mut cfa = mosaic(&rgb, w, h, pattern);
    if cli.aberrate > 0.0 && !cli.no_ca {
        cfa = correct_ca(&cfa, w, h, pattern, &CaOptions::default())?.0;
    }

    // Simulate a sensor that saw `boost` times the light: after white
    // balance, channel c saturates at its gain.
    let boost = 2f32.powf(cli.overexpose as f32);
    let mut ceiling = 1.0f32;
    if cli.overexpose > 0.0 {
        for (i, v) in cfa.iter_mut().enumerate() {
            let c = pattern
                .color_at(i / w, i % w)
                .rgb_index()
                .expect("Bayer patterns are RGB");
            *v = (*v * boost).min(TEST_GAINS[c]);
        }
        match cli.highlights {
            HighlightMode::InpaintOpposed | HighlightMode::Segments => {
                let clips = TEST_GAINS.map(|g| g * CLIP_MAGIC);
                let opposed = inpaint_opposed(&cfa, w, h, pattern, clips)?.0;
                cfa = if cli.highlights == HighlightMode::Segments {
                    inpaint_segments(
                        &cfa,
                        &opposed,
                        w,
                        h,
                        pattern,
                        clips,
                        &SegmentOptions::default(),
                    )?
                    .0
                } else {
                    opposed
                };
                ceiling = TEST_GAINS.iter().copied().fold(1.0, f32::max);
            }
            HighlightMode::Clip => {}
            _ => bail!("unsupported highlight mode"),
        }
        for v in cfa.iter_mut() {
            *v = v.min(ceiling);
        }
    }

    if cli.noise > 0.0 {
        add_noise(&mut cfa, cli.noise as f32, &reference.name);
    }
    let mut sigma_ratio = None;
    let mut sigma_estimate = None;
    let model = if cli.denoise || denoiser.is_some() {
        let estimate = estimate_noise(&cfa, w, h, pattern)?;
        sigma_estimate = Some(std::array::from_fn(|c| {
            estimate.model.sigma(c, MID_GREY) as f64
        }));
        if cli.noise > 0.0 {
            let truth = true_model(cli.noise as f32);
            sigma_ratio = Some(std::array::from_fn(|c| {
                (estimate.model.sigma(c, MID_GREY) / truth.sigma(c, MID_GREY)) as f64
            }));
        }
        if cli.true_model {
            Some(true_model(cli.noise as f32))
        } else {
            Some(estimate.model)
        }
    } else {
        None
    };

    let contrast = match cli.dual_contrast.as_str() {
        "noise" => DualContrast::Noise(estimate_noise(&cfa, w, h, pattern)?.model),
        other => dual_contrast(other)?,
    };
    let start = Instant::now();
    let (mut out, label) = match denoiser {
        Some(denoiser) => {
            let model = model.as_ref().expect("measured for the learned denoiser");
            let rgb = denoiser
                .lock()
                .expect("denoiser lock")
                .run(
                    &cfa,
                    w,
                    h,
                    pattern,
                    model,
                    greycard_ai::denoise::Tiling {
                        tile: cli.tile,
                        ..Default::default()
                    },
                )
                .with_context(|| format!("learned denoiser on {}", reference.name))?;
            (rgb, "ai")
        }
        None => (
            demosaic_cfa_with(&cfa, w, h, pattern, method, contrast)
                .with_context(|| format!("{} on {}", method.name(), reference.name))?
                .0,
            method.name(),
        ),
    };
    let seconds_per_mp = start.elapsed().as_secs_f64() / ((w * h) as f64 / 1e6);
    if let (Some(model), None) = (&model, denoiser) {
        denoise_profiled(
            &mut out,
            w,
            h,
            model,
            &DenoiseOptions {
                method: cli.denoise_method,
                strength: cli.denoise_strength,
                chroma: cli.denoise_chroma,
                nlm: NlmOptions {
                    patch_radius: cli.nlm_patch,
                    search_radius: cli.nlm_search,
                    scattering: cli.nlm_scatter,
                    center_weight: cli.nlm_center,
                },
                hybrid_from: cli.hybrid_from,
            },
        )?;
    }
    if cli.overexpose > 0.0 {
        for v in out.iter_mut() {
            *v /= boost;
        }
    }
    if cli.camera_space {
        out = from_camera(&out);
    }

    let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let out_srgb: Vec<u8> = if cli.linear {
        out.iter().map(|v| to_u8(srgb_encode(*v))).collect()
    } else {
        out.iter().map(|v| to_u8(*v)).collect()
    };

    let scores = metrics::score(&reference.srgb, &out_srgb, w, h, cli.border);

    if let Some(dir) = &cli.dump {
        let stem = format!("{}-{}-{}", reference.set, reference.name, method.name());
        save_rgb(&dir.join(format!("{stem}.png")), &out_srgb, w, h)?;
        let error: Vec<u8> = reference
            .srgb
            .iter()
            .zip(&out_srgb)
            .map(|(a, b)| ((*a as i32 - *b as i32).abs() * 8).min(255) as u8)
            .collect();
        save_rgb(&dir.join(format!("{stem}.error.png")), &error, w, h)?;
    }

    Ok(Run {
        set: reference.set.clone(),
        name: reference.name.clone(),
        method,
        label,
        scores,
        seconds_per_mp,
        sigma_ratio,
        sigma_estimate,
    })
}

/// A Canon EOS R6 Mark II's D65 `ColorMatrix` (XYZ to camera, rows), as
/// its files declare it.
const CAMERA_FROM_XYZ: [[f32; 3]; 3] = [
    [0.9539, -0.2795, -0.1224],
    [-0.4175, 1.1998, 0.2458],
    [-0.0465, 0.1755, 0.6048],
];

/// Linear sRGB to XYZ, D65.
const XYZ_FROM_SRGB: [[f32; 3]; 3] = [
    [0.4124, 0.3576, 0.1805],
    [0.2126, 0.7152, 0.0722],
    [0.0193, 0.1192, 0.9505],
];

fn mat_mul(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn mat_inverse(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let c = |r: usize, s: usize| -> f32 {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (s1, s2) = ((s + 1) % 3, (s + 2) % 3);
        m[r1][s1] * m[r2][s2] - m[r1][s2] * m[r2][s1]
    };
    let det = (0..3).map(|k| m[0][k] * c(0, k)).sum::<f32>();
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = c(j, i) / det;
        }
    }
    out
}

fn apply(m: &[[f32; 3]; 3], [r, g, b]: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * r + m[i][1] * g + m[i][2] * b)
}

/// Linear sRGB to the camera's white-balanced space: the matrix, then
/// the gains that put sRGB white at 1, 1, 1, then a common scale so no
/// sRGB color goes above 1 (the sensor's clip), which `from_camera`
/// undoes. Nothing goes below 0: a sensor cannot see it.
fn camera_matrices() -> ([[f32; 3]; 3], [[f32; 3]; 3]) {
    let k0 = mat_mul(&CAMERA_FROM_XYZ, &XYZ_FROM_SRGB);
    let white = apply(&k0, [1.0; 3]);
    let mut k = k0;
    for (row, w) in k.iter_mut().zip(white) {
        for v in row.iter_mut() {
            *v /= w;
        }
    }
    let peak = k
        .iter()
        .map(|row| row.iter().map(|v| v.max(0.0)).sum::<f32>())
        .fold(1.0, f32::max);
    for row in k.iter_mut() {
        for v in row.iter_mut() {
            *v /= peak;
        }
    }
    let inverse = mat_inverse(&k);
    (k, inverse)
}

fn to_camera(rgb: &[f32]) -> Vec<f32> {
    let (k, _) = camera_matrices();
    rgb.as_chunks::<3>()
        .0
        .iter()
        .flat_map(|px| apply(&k, *px).map(|v| v.max(0.0)))
        .collect()
}

fn from_camera(rgb: &[f32]) -> Vec<f32> {
    let (_, inverse) = camera_matrices();
    rgb.as_chunks::<3>()
        .0
        .iter()
        .flat_map(|px| apply(&inverse, *px))
        .collect()
}

/// The model [`add_noise`] follows: variance `sigma_mid^2 / 0.18` per unit
/// of level, plus `(0.1 sigma_mid)^2`.
fn true_model(sigma_mid: f32) -> NoiseModel {
    NoiseModel {
        a: [sigma_mid * sigma_mid / MID_GREY; 3],
        b: [0.01 * sigma_mid * sigma_mid; 3],
    }
}

/// Magnify red about the center and shrink blue, so red lands `pixels`
/// outward at the corners and blue three quarters of that inward.
fn aberrate(rgb: &[f32], w: usize, h: usize, pixels: f32) -> Vec<f32> {
    let (cx, cy) = ((w - 1) as f32 / 2.0, (h - 1) as f32 / 2.0);
    let corner = (cx * cx + cy * cy).sqrt();
    let scale = [1.0 + pixels / corner, 1.0, 1.0 - 0.75 * pixels / corner];
    let sample = |c: usize, x: f32, y: f32| {
        let x = x.clamp(0.0, (w - 1) as f32);
        let y = y.clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |xx: usize, yy: usize| rgb[(yy * w + xx) * 3 + c];
        (1.0 - fy) * ((1.0 - fx) * p(x0, y0) + fx * p(x1, y0))
            + fy * ((1.0 - fx) * p(x0, y1) + fx * p(x1, y1))
    };
    let mut out = vec![0.0f32; rgb.len()];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let sx = cx + (x as f32 - cx) / scale[c];
                let sy = cy + (y as f32 - cy) / scale[c];
                out[(y * w + x) * 3 + c] = sample(c, sx, sy);
            }
        }
    }
    out
}

fn dual_contrast(arg: &str) -> Result<DualContrast> {
    if arg == "tiles" || arg == "auto" {
        return Ok(DualContrast::Tiles);
    }
    let percent: f32 = arg
        .parse()
        .with_context(|| format!("--dual-contrast {arg:?}: want a number or auto"))?;
    Ok(DualContrast::Fixed(percent / 100.0))
}

/// Gaussian noise with a signal-dependent standard deviation, from a
/// generator seeded by the image name so runs repeat.
fn add_noise(cfa: &mut [f32], sigma_mid: f32, name: &str) {
    let mut seed = name.bytes().fold(0x9E37_79B9_7F4A_7C15u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01B3)
    });
    let mut uniform = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        ((seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    for v in cfa.iter_mut() {
        let (u1, u2) = (uniform(), uniform());
        let gaussian = ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32;
        let shot = sigma_mid * (v.max(0.0) / MID_GREY).sqrt();
        let read = 0.1 * sigma_mid;
        *v += gaussian * (shot * shot + read * read).sqrt();
    }
}

fn save_rgb(path: &Path, data: &[u8], w: usize, h: usize) -> Result<()> {
    image::RgbImage::from_raw(w as u32, h as u32, data.to_vec())
        .context("image size mismatch")?
        .save(path)
        .with_context(|| format!("writing {}", path.display()))
}

fn report(runs: &[Run], methods: &[DemosaicMethod], cli: &Cli) {
    let domain = if cli.linear { "linear" } else { "sRGB" };
    let header = [
        "set", "image", "method", "PSNR R", "PSNR G", "PSNR B", "CPSNR", "zipper%", "dE", "dab",
        "coarse", "ms/MP",
    ];
    let row = |set: &str, image: &str, method: &str, s: &Scores, secs: f64| -> Vec<String> {
        vec![
            set.into(),
            image.into(),
            method.into(),
            format!("{:.2}", s.psnr[0]),
            format!("{:.2}", s.psnr[1]),
            format!("{:.2}", s.psnr[2]),
            format!("{:.2}", s.cpsnr),
            format!("{:.2}", s.zipper_percent),
            format!("{:.3}", s.delta_e),
            format!("{:.3}", s.delta_ab),
            format!("{:.3}", s.coarse),
            format!("{:.1}", secs * 1e3),
        ]
    };

    let mut rows: Vec<Vec<String>> = Vec::new();
    if cli.per_image {
        for r in runs {
            rows.push(row(&r.set, &r.name, r.label, &r.scores, r.seconds_per_mp));
        }
    }
    let mut sets: Vec<&str> = runs.iter().map(|r| r.set.as_str()).collect();
    sets.dedup();
    for set in sets {
        for method in methods {
            let these: Vec<&Run> = runs
                .iter()
                .filter(|r| r.set == set && r.method == *method)
                .collect();
            let scores: Vec<Scores> = these.iter().map(|r| r.scores).collect();
            let secs =
                these.iter().map(|r| r.seconds_per_mp).sum::<f64>() / these.len().max(1) as f64;
            let label = these.first().map(|r| r.label).unwrap_or(method.name());
            rows.push(row(
                set,
                &format!("mean of {}", these.len()),
                label,
                &Scores::mean(&scores),
                secs,
            ));
        }
    }

    if cli.csv {
        println!("{}", header.join(","));
        for r in &rows {
            println!("{}", r.join(","));
        }
        return;
    }
    let aberrated = if cli.aberrate > 0.0 {
        format!(
            "{} px lateral CA at the corners{}",
            cli.aberrate,
            if cli.no_ca {
                " uncorrected"
            } else {
                " corrected"
            }
        )
    } else {
        String::new()
    };
    let overexposed = if cli.overexpose > 0.0 {
        format!(
            ", +{} EV with gains {:?} and highlights {}",
            cli.overexpose,
            TEST_GAINS,
            cli.highlights.name()
        )
    } else {
        String::new()
    };
    println!(
        "pattern {}, {} domain, border {} px, zipper threshold {}{}",
        cli.pattern,
        domain,
        cli.border,
        metrics::ZIPPER_THRESHOLD,
        overexposed
    );
    if !aberrated.is_empty() {
        println!("{aberrated}");
    }
    if cli.noise > 0.0 {
        println!(
            "noise {} at mid grey, dual contrast {}{}",
            cli.noise,
            cli.dual_contrast,
            if cli.denoise {
                format!(
                    ", denoised by {} at strength {:?} chroma {}",
                    cli.denoise_method.name(),
                    cli.denoise_strength,
                    cli.denoise_chroma
                )
            } else {
                String::new()
            }
        );
    }
    let estimates: Vec<[f64; 3]> = runs.iter().filter_map(|r| r.sigma_estimate).collect();
    if !estimates.is_empty() {
        let mean: [f64; 3] = std::array::from_fn(|c| {
            estimates.iter().map(|r| r[c]).sum::<f64>() / estimates.len() as f64
        });
        println!(
            "estimated noise sigma at mid grey: R {:.4} G {:.4} B {:.4}",
            mean[0], mean[1], mean[2]
        );
    }
    let ratios: Vec<[f64; 3]> = runs.iter().filter_map(|r| r.sigma_ratio).collect();
    if !ratios.is_empty() {
        let mean: [f64; 3] =
            std::array::from_fn(|c| ratios.iter().map(|r| r[c]).sum::<f64>() / ratios.len() as f64);
        println!(
            "noise estimate over truth at mid grey: R {:.3} G {:.3} B {:.3}",
            mean[0], mean[1], mean[2]
        );
    }
    let widths: Vec<usize> = (0..header.len())
        .map(|i| {
            rows.iter()
                .map(|r| r[i].len())
                .chain([header[i].len()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: &[String]| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i < 3 {
                    format!("{:<w$}", c, w = widths[i])
                } else {
                    format!("{:>w$}", c, w = widths[i])
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
    };
    println!("{}", line(&header.map(String::from)));
    for r in &rows {
        println!("{}", line(r));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_space_round_trips_and_is_desaturated() {
        let (k, inverse) = camera_matrices();
        let identity = mat_mul(&k, &inverse);
        for (i, row) in identity.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-5, "{identity:?}");
            }
        }
        // White stays neutral, at or below the clip.
        let white = apply(&k, [1.0; 3]);
        assert!((white[0] - white[1]).abs() < 1e-5 && (white[1] - white[2]).abs() < 1e-5);
        assert!(white[0] <= 1.0 + 1e-6);
        // A pure sRGB red is far less pure in camera space.
        let red = apply(&k, [1.0, 0.0, 0.0]);
        assert!(red[1] > 0.1 * red[0], "{red:?}");
        let back = apply(&inverse, apply(&k, [0.2, 0.5, 0.7]));
        assert!((back[0] - 0.2).abs() < 1e-5 && (back[2] - 0.7).abs() < 1e-5);
    }
}
