//! greycard: decode a RAW, white balance and color-transform it correctly,
//! write the result somewhere every editor can read.

use std::path::PathBuf;

use anyhow::{Context, Result};
mod log;

use clap::{Parser, Subcommand};
use greycard_core::color::{CAT, WORKING_SPACE, as_shot_temp_tint, profile_from_frame};
use greycard_core::decode::RawlerDecoder;
use greycard_core::develop::defringe;
use greycard_core::develop::dehaze::{self, DehazeOptions};
use greycard_core::develop::denoise::{
    DEFAULT_CHROMA, DEFAULT_HYBRID_FROM, DenoiseMethod, DenoiseOptions, Strength,
};
use greycard_core::develop::dual::{DualContrast, ThresholdSource};
use greycard_core::develop::hotpixels::{DEFAULT_RATIO, DEFAULT_SIGMAS, HotPixelOptions};
use greycard_core::develop::local_contrast::{LocalContrastOptions, local_contrast};
use greycard_core::develop::nlm::{DEFAULT_CENTER_WEIGHT, DEFAULT_SEARCH, NlmOptions};
use greycard_core::develop::noise::MID_GREY;
use greycard_core::develop::segments::SegmentOptions;
use greycard_core::develop::sharpen;
use greycard_core::develop::{finish, prepare};
use greycard_core::dng::{Compression, LinearDngOptions, write_linear_dng};
use greycard_core::output::{OnExists, Resolved};
use greycard_core::raw::{
    Orientation, SensorLayout, Shot, aperture_text, camera_name, focal_text, shutter_text,
};
use greycard_core::register::{self, Model, Options};
use greycard_core::stack::{self, Reference};
use greycard_core::{
    CalibrationIlluminant, CameraImage, DemosaicMethod, DevelopSettings, HighlightMode, RawFrame,
    RgbSpace, TempTint, WhitePoint, WorkingImage, decode, demosaic, develop,
};

/// A policy named on the command line.
fn on_exists_named(name: &str) -> Result<OnExists, String> {
    OnExists::from_name(name)
        .ok_or_else(|| format!("want increment, overwrite or skip, not {name}"))
}

#[derive(Parser)]
#[command(
    name = "greycard",
    version,
    about = "RAW pre-processor: decode, white balance and camera color, done right"
)]
struct Cli {
    /// Say more: once, what greycard's own crates note along the way;
    /// twice, their debug lines. RUST_LOG, when set, decides instead
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
// Develop carries every setting; the size gap to Info is of no consequence.
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Print what the file says about itself and what the engine makes of it
    Info { file: PathBuf },
    /// The lens database: where it is and what it has; with a file,
    /// what it finds for that file
    Lenses {
        file: Option<PathBuf>,
        /// Fetch the database (about 450 KB, CC BY-SA 3.0) into the
        /// user's cache first
        #[arg(long)]
        fetch: bool,
        /// Look a lens up by name instead of from a file (with
        /// --camera for the body)
        #[arg(long, value_name = "NAME")]
        lens: Option<String>,
        /// The body to look a lens up for, as make and model
        #[arg(long, num_args = 2, value_names = ["MAKE", "MODEL"])]
        camera: Option<Vec<String>>,
    },
    /// The learned models: where they are kept, what each is for, and
    /// what it costs to fetch one
    Models {
        /// Fetch a denoiser tier (fast, balanced, best), a model by
        /// its id, or `all`, into the user's model store. Each
        /// model's license is printed before it is fetched, and the
        /// published hash is checked when it arrives
        #[arg(long, value_name = "TIER|ID|all")]
        fetch: Option<String>,
    },
    /// The presets: list them; import Lightroom (.xmp) or greycard
    /// (.gcp) preset files; lay one over files' edits
    Presets {
        /// Files whose sidecars take the preset named by --apply
        files: Vec<PathBuf>,
        /// Preset files to bring into the store, Lightroom's .xmp or
        /// this engine's .gcp
        #[arg(long, value_name = "FILE", num_args = 1..)]
        import: Vec<PathBuf>,
        /// Lay this preset (by name, or a preset file's path) over
        /// each file's edit, as a step in its history
        #[arg(long, value_name = "NAME")]
        apply: Option<String>,
        /// A store other than the user's
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Fit the transform that puts one frame on top of another:
    /// pyramidal Lucas-Kanade on the two frames' luminance, for a
    /// stack's alignment. Prints the transform, what it moves the
    /// frame by, and the residual that says whether to believe it
    Register {
        /// The frame everything else is put onto
        reference: PathBuf,
        /// The frame whose transform onto the reference is wanted
        moving: PathBuf,
        /// What to fit: translation, similarity (shift, rotation and
        /// one scale) or affine
        #[arg(long, default_value = "similarity")]
        model: Model,
        /// Stop at this pyramid level rather than at full resolution:
        /// 1 is half size, 2 a quarter. Halves the time and costs a
        /// fraction of a pixel
        #[arg(long, default_value_t = 0)]
        finest_level: usize,
    },
    /// Merge a focus stack: register the frames onto one another, take
    /// each band of the picture from whichever frame holds it
    /// sharpest, and write the frame that is sharp everywhere beside
    /// the sources. Prints what each frame contributed and what was
    /// left out.
    ///
    /// Every frame is held in memory at once: about 0.6 s and a
    /// gigabyte a frame at 24 megapixels, and ten 45-megapixel frames
    /// peak at 9.3 GB
    Stack {
        /// The frames, in focus order. RAW files of one camera and one
        /// size
        files: Vec<PathBuf>,
        /// Where the merge goes: a linear DNG (.dng) for any editor to
        /// finish, or a 16-bit linear Rec.2020 TIFF (.tif). Without
        /// one, a DNG beside the first frame and named after it — the
        /// name comes from the first frame, the color tags and the
        /// EXIF from the reference
        #[arg(short, long, value_name = "OUT.dng")]
        output: Option<PathBuf>,
        /// The frame the others are put onto: `middle` (the one
        /// nearest in magnification to all the rest, since focus
        /// breathing runs one way through a stack), `sharpest`, or a
        /// frame's number counting from 0
        #[arg(long, default_value = "middle", value_parser = stack_reference)]
        reference: Reference,
        /// How far each band is taken from the frame with the most to
        /// say in it rather than averaged: 0 is a weighted average,
        /// higher keeps more detail and less of the stack's noise
        /// averaging
        #[arg(long, default_value_t = stack::Options::default().selectivity)]
        selectivity: f32,
        /// The residual a link between two neighboring frames may
        /// have and still be believed. Frames of a focus stack differ
        /// by their defocus, so this is not the registration's own
        /// threshold. The default is measured on synthetic frames and
        /// on two real scenes, not on a real focus stack
        #[arg(long, default_value_t = stack::Options::default().max_residual)]
        max_residual: f32,
        /// Store the DNG samples uncompressed instead of lossless JPEG
        #[arg(long)]
        uncompressed: bool,
        /// What to do when the output file is there already
        #[arg(long, value_name = "WHAT", default_value = "increment",
              value_parser = on_exists_named)]
        on_exists: OnExists,
    },
    /// Develop a RAW: linear DNG for other editors, or linear Rec.2020 here
    Develop {
        file: PathBuf,
        /// Linear DNG: demosaiced camera-space samples with the camera's
        /// color tags and EXIF, for any RAW editor to finish
        #[arg(long)]
        dng: Option<PathBuf>,
        /// Store the DNG samples uncompressed instead of lossless JPEG
        #[arg(long, requires = "dng")]
        uncompressed: bool,
        /// 16-bit linear Rec.2020 TIFF, the source's EXIF carried, no profile
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// 8-bit sRGB PNG for looking at, exposure applied, highlights clipped
        #[arg(long)]
        preview: Option<PathBuf>,
        /// What to do when an output file is there already: write
        /// beside it under the first free " (2)" name, write over it,
        /// or leave it and write nothing
        #[arg(long, value_name = "WHAT", default_value = "increment",
              value_parser = on_exists_named)]
        on_exists: OnExists,
        /// Scene white as a color temperature in Kelvin (default: as shot)
        #[arg(long)]
        temp: Option<f64>,
        /// Tint as Duv, positive toward green; only used with --temp
        #[arg(long, default_value_t = 0.0)]
        tint: f64,
        /// Exposure adjustment in stops, preview only
        #[arg(long, default_value_t = 0.0)]
        exposure: f64,
        /// Demosaic algorithm: amaze-vng4, rcd-vng4, amaze, rcd, vng4 or bilinear
        #[arg(long, default_value = "amaze-vng4")]
        demosaic: DemosaicMethod,
        /// Camera profile: a DCP in the profile directory by name, a path to
        /// one, or "embedded" for the file's own calibrations
        #[arg(long, default_value = "embedded")]
        camera_profile: String,
        /// Clipped highlights: opposed (reconstruct), segments (reconstruct,
        /// then refine each region) or clip
        #[arg(long, default_value = "opposed")]
        highlights: HighlightMode,
        /// Segments: radius in blocks that joins nearby clipped areas, 0 to 8
        #[arg(long, default_value_t = SegmentOptions::default().combine)]
        combine: usize,
        /// Segments: how poor a reference spot may be and still count, 0 to 1
        #[arg(long, default_value_t = SegmentOptions::default().candidating)]
        candidating: f32,
        /// Leave lateral chromatic aberration uncorrected
        #[arg(long)]
        no_ca: bool,
        /// Repair hot and dead photosites on the mosaic. Off by default and
        /// not safe on: anything at most two photosites across looks hot to
        /// it, catchlights and stars included
        #[arg(long)]
        hot_pixels: bool,
        /// How far out a photosite must be, in standard deviations of the
        /// measured noise beyond its brightest or darkest same-color neighbor
        #[arg(long, default_value_t = DEFAULT_SIGMAS)]
        hot_sigmas: f32,
        /// The factor a hot photosite must exceed its brightest same-color
        /// neighbor by
        #[arg(long, default_value_t = DEFAULT_RATIO)]
        hot_ratio: f32,
        /// Capture sharpening: Richardson-Lucy deconvolution of the
        /// luminance, blended in by local contrast
        #[arg(long)]
        sharpen: bool,
        /// The point spread's width in pixels, or auto from the mosaic
        #[arg(long, default_value = "auto")]
        sharpen_radius: AutoF32,
        #[arg(long, default_value_t = sharpen::DEFAULT_ITERATIONS)]
        sharpen_iterations: usize,
        /// Local contrast below which a pixel is left alone, in percent,
        /// or auto from the picture's flattest patch
        #[arg(long, default_value = "auto")]
        sharpen_contrast: AutoF32,
        /// Local contrast at the fine scale, a few pixels: -1 to 1 (the
        /// panel's -100 to 100), negative smoothing instead
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        texture: f32,
        /// Local contrast at the coarse scale, a fortieth of the long
        /// edge, weighted to the mid-tones: -1 to 1, negative softening
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        clarity: f32,
        /// Haze removal by the dark channel prior, -100 to 100 as the
        /// panel has it; negative puts haze in
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        dehaze: f32,
        /// Write the transmission the dehaze applies as an 8-bit grey
        /// PNG, black for none through white for all, at the
        /// developed picture's size; wants --dehaze
        #[arg(long, requires = "dehaze")]
        dehaze_map: Option<PathBuf>,
        /// Contrast threshold of the dual demosaics (rcd-vng4, amaze-vng4):
        /// "auto" (from the frame's measured noise), "tiles" (RawTherapee's
        /// flattest-tile search), or a number in RawTherapee's percent
        #[arg(long, default_value = "auto")]
        dual_contrast: String,
        /// Profiled noise reduction, from noise measured on the frame
        #[arg(long)]
        denoise: bool,
        /// How hard to denoise: a multiplier on the wavelet thresholds, or
        /// auto to follow the measured noise
        #[arg(long, default_value = "auto")]
        denoise_strength: Strength,
        /// Further multiplier on the chrominance thresholds
        #[arg(long, default_value_t = DEFAULT_CHROMA)]
        denoise_chroma: f32,
        /// Denoiser: hybrid (non-local means, then wavelets from --hybrid-from), nlm, or wavelets
        #[arg(long, default_value = "hybrid")]
        denoise_method: DenoiseMethod,
        /// Non-local means patch radius, or auto from the noise
        #[arg(long, default_value = "auto")]
        nlm_patch: AutoUsize,
        /// Non-local means search radius
        #[arg(long, default_value_t = DEFAULT_SEARCH)]
        nlm_search: usize,
        /// Non-local means scattering of the search, 0 to 1, or auto
        #[arg(long, default_value = "auto")]
        nlm_scatter: AutoF32,
        /// Non-local means central pixel weight
        #[arg(long, default_value_t = DEFAULT_CENTER_WEIGHT)]
        nlm_center: f32,
        /// Hybrid: the first wavelet scale shrunk after the means
        #[arg(long, default_value_t = DEFAULT_HYBRID_FROM)]
        hybrid_from: usize,
        /// Demosaic and denoise with the learned model instead of the
        /// engine's own demosaic and profiled denoise: a tier from the
        /// model store (fast, balanced, best; see `greycard models
        /// --fetch`) or a path to a model.
        /// Always run here at full blend; the editor's own sidecar
        /// starts a fresh file at a blend that follows its ISO (lower
        /// at base ISO, full from ISO 3200 up) and remembers whatever
        /// the NOISE panel's slider is set to after that
        #[arg(long, value_name = "TIER|MODEL.onnx")]
        ai_denoise: Option<String>,
        /// Correct the lens from its profile in the lens database
        /// (lensfun's own copy on the system, or `greycard lenses
        /// --fetch`): distortion, chromatic aberration and vignetting
        #[arg(long)]
        lens: bool,
        /// A distortion set by hand, poly3's k1 (negative for a
        /// picture that bows out), on top of the profile's
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        lens_distortion: f32,
        /// A chromatic aberration set by hand: the red's radius scaled
        /// beyond the green's, as a fraction, on top of the profile's
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        lens_ca_red: f32,
        /// The same for the blue
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        lens_ca_blue: f32,
        /// The magnification after the lens correction, in place of
        /// the smallest that leaves no edge empty
        #[arg(long)]
        lens_scale: Option<f32>,
        /// Neutralize axial chromatic aberration and purple fringing
        /// after the lens correction: where a pixel's chroma departs
        /// from its neighborhood's in a purple or a green direction,
        /// it takes the neighborhood's instead (--defringe-all-hues
        /// for every direction). A different fault from the lateral
        /// aberration the profile and --lens-ca-red correct
        #[arg(long)]
        defringe: bool,
        /// How far the local mean chroma is taken over, pixels
        #[arg(long, default_value_t = defringe::DEFAULT_RADIUS)]
        defringe_radius: f32,
        /// How far past the frame's mean chroma deviation a pixel must
        /// sit to count as fringing, RawTherapee's scale (0 to 100)
        #[arg(long, default_value_t = defringe::DEFAULT_THRESHOLD)]
        defringe_threshold: f32,
        /// Defringe every hue, not just the purple and green windows:
        /// the pass as it was before the windows, for a fringe in some
        /// other color
        #[arg(long)]
        defringe_all_hues: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    log::start(cli.verbose);
    match cli.command {
        Command::Info { file } => {
            if greycard_core::picture::is_picture_path(&file) {
                info_picture(&file)
            } else {
                info(&file)
            }
        }
        Command::Lenses {
            file,
            fetch,
            lens,
            camera,
        } => lenses(file.as_deref(), fetch, lens.as_deref(), camera.as_deref()),
        Command::Models { fetch } => models(fetch.as_deref()),
        Command::Register {
            reference,
            moving,
            model,
            finest_level,
        } => register(&reference, &moving, model, finest_level),
        Command::Stack {
            files,
            output,
            reference,
            selectivity,
            max_residual,
            uncompressed,
            on_exists,
        } => stack_files(
            &files,
            output.as_deref(),
            reference,
            selectivity,
            max_residual,
            uncompressed,
            on_exists,
        ),
        Command::Presets {
            files,
            import,
            apply,
            dir,
        } => presets(&files, &import, apply.as_deref(), dir),
        Command::Develop {
            file,
            dng,
            uncompressed,
            output,
            preview,
            on_exists,
            temp,
            tint,
            exposure,
            demosaic,
            camera_profile,
            highlights,
            combine,
            candidating,
            no_ca,
            hot_pixels,
            hot_sigmas,
            hot_ratio,
            sharpen,
            sharpen_radius,
            sharpen_iterations,
            sharpen_contrast,
            texture,
            clarity,
            dehaze,
            dehaze_map,
            dual_contrast,
            denoise,
            denoise_strength,
            denoise_chroma,
            denoise_method,
            nlm_patch,
            nlm_search,
            nlm_scatter,
            nlm_center,
            hybrid_from,
            ai_denoise,
            lens,
            lens_distortion,
            lens_ca_red,
            lens_ca_blue,
            lens_scale,
            defringe,
            defringe_radius,
            defringe_threshold,
            defringe_all_hues,
        } => {
            if dng.is_none() && output.is_none() && preview.is_none() {
                anyhow::bail!("nothing to write: pass --dng, --output and/or --preview");
            }
            let white_point = match temp {
                Some(cct) => WhitePoint::TempTint(TempTint { cct, duv: tint }),
                None => WhitePoint::AsShot,
            };
            let outputs = Outputs {
                dng: dng.as_deref(),
                compression: if uncompressed {
                    Compression::Uncompressed
                } else {
                    Compression::Lossless
                },
                tiff: output.as_deref(),
                preview: preview.as_deref(),
                dehaze_map: dehaze_map.as_deref(),
                on_exists,
            };
            // The Detail section through the edit crate's own type,
            // so a flag past the end is held to it the way the
            // panel's slider is.
            let detail_edit = greycard_edit::Detail {
                enabled: true,
                texture,
                clarity,
                dehaze: dehaze / 100.0,
            };
            let settings = DevelopSettings {
                white_point,
                profile: chosen_profile(&camera_profile)?,
                demosaic,
                highlights,
                segments: SegmentOptions {
                    combine,
                    candidating,
                },
                chromatic_aberration: if no_ca {
                    None
                } else {
                    Some(Default::default())
                },
                hot_pixels: hot_pixels.then_some(HotPixelOptions {
                    sigmas: hot_sigmas,
                    ratio: hot_ratio,
                }),
                sharpen: sharpen.then_some(sharpen::SharpenOptions {
                    radius: match sharpen_radius.0 {
                        Some(r) => sharpen::Radius::Fixed(r),
                        None => sharpen::Radius::Auto,
                    },
                    iterations: sharpen_iterations,
                    contrast: match sharpen_contrast.0 {
                        Some(c) => sharpen::Threshold::Fixed(c / 100.0),
                        None => sharpen::Threshold::Auto,
                    },
                    stop_early: true,
                }),
                dehaze: detail_edit.dehaze_options(),
                dual_contrast: if dual_contrast == "auto" {
                    DualContrast::Auto
                } else if dual_contrast == "tiles" {
                    DualContrast::Tiles
                } else {
                    let percent: f32 = dual_contrast.parse().with_context(|| {
                        format!("--dual-contrast {dual_contrast:?}: want a number or auto")
                    })?;
                    DualContrast::Fixed(percent / 100.0)
                },
                denoise: denoise.then_some(DenoiseOptions {
                    strength: denoise_strength,
                    chroma: denoise_chroma,
                    method: denoise_method,
                    nlm: NlmOptions {
                        patch_radius: nlm_patch.0,
                        search_radius: nlm_search,
                        scattering: nlm_scatter.0,
                        center_weight: nlm_center,
                    },
                    hybrid_from,
                }),
                // The file's own tag: the CLI develops a frame, not a
                // sidecar, so there are no quarter turns to put on it.
                orientation: None,
            };
            let ai_denoise = ai_denoise.as_deref().map(denoiser_for).transpose()?;
            let detail = detail_edit.options();
            let lens = greycard_edit_lens(
                lens,
                lens_distortion,
                (lens_ca_red, lens_ca_blue),
                lens_scale,
                (defringe, defringe_radius, defringe_threshold),
                defringe_all_hues,
            );
            run_develop(
                &file,
                &outputs,
                &settings,
                exposure,
                ai_denoise.as_ref(),
                &lens,
                detail,
            )
        }
    }
}

/// The lens flags as the edit crate would hold them, so the CLI and
/// the editor correct alike.
fn greycard_edit_lens(
    profile: bool,
    manual: f32,
    (ca_red, ca_blue): (f32, f32),
    scale: Option<f32>,
    (defringe, defringe_radius, defringe_threshold): (bool, f32, f32),
    all_hues: bool,
) -> greycard_edit::Lens {
    let mut lens = greycard_edit::Lens {
        profile,
        manual,
        ca_red,
        ca_blue,
        auto_scale: scale.is_none(),
        scale: scale.unwrap_or(1.0),
        defringe,
        defringe_radius,
        defringe_threshold,
        ..Default::default()
    };
    // The defaults are the two measured hue windows. One window over
    // the whole circle, with the other shut, is the pass as it was
    // before the windows: every hue at full strength.
    if all_hues {
        lens.defringe_purple_width = defringe::FULL_CIRCLE;
        lens.defringe_green_amount = 0.0;
    }
    lens
}

/// The denoiser tiers, for a message to name.
fn denoiser_tiers() -> String {
    greycard_ai::DENOISERS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What to say when a tier is named but its model was never fetched:
/// the way to get it is `greycard models`, not the editor's panel,
/// since the machine asking may have no window at all.
fn missing_tier_complaint(store: &greycard_ai::Store, name: &str) -> String {
    let model = greycard_ai::denoiser(name).expect("a tier");
    format!(
        "{} is not in {}; run `greycard models --fetch {name}` to get it ({} MB, {})",
        model.name,
        store.root().display(),
        megabytes(model.bytes()),
        model.license.name
    )
}

/// The model for `--ai-denoise`: a registered tier from the model
/// store, whose answers the disk cache keeps, or a path.
fn denoiser_for(name: &str) -> Result<(PathBuf, Option<&'static greycard_ai::Model>)> {
    let Some(model) = greycard_ai::denoiser(name) else {
        // Not a tier, so a path; a mistyped tier would otherwise go
        // to the runtime and come back as "File at `turbo` does not
        // exist", once per execution provider.
        let path = PathBuf::from(name);
        anyhow::ensure!(
            path.is_file(),
            "want a tier ({}) or the path of a model file, not {name}",
            denoiser_tiers()
        );
        return Ok((path, None));
    };
    let store = greycard_ai::Store::user().context("finding the model store")?;
    anyhow::ensure!(
        store.have(model),
        "{}",
        missing_tier_complaint(&store, name)
    );
    Ok((store.path(model, &model.files[0]), Some(model)))
}

/// The engine's develop with the learned denoiser in place of the
/// demosaic and the profiled denoise.
fn develop_learned(
    frame: &RawFrame,
    settings: &DevelopSettings,
    (path, model): &(PathBuf, Option<&'static greycard_ai::Model>),
) -> Result<greycard_core::develop::Developed> {
    let start = std::time::Instant::now();
    let providers = greycard_ai::Provider::available();
    // A registered tier goes through the store, so a provider's
    // answer for it is read from and written to the record
    // (`runtime::Remembered`); a bare path (an arbitrary model file
    // named on the command line) has no store entry to key that on.
    let mut denoiser = match *model {
        Some(model) => {
            let store = greycard_ai::Store::user().context("finding the model store")?;
            greycard_ai::Denoiser::from_store(&store, model, &providers)
        }
        None => greycard_ai::Denoiser::load(path, &providers),
    }
    .with_context(|| format!("loading {}", path.display()))?;
    tracing::info!(
        "learned denoiser {} on {} ({:.1} s to load)",
        denoiser.version(),
        denoiser.provider().name(),
        start.elapsed().as_secs_f32()
    );
    let prepared = prepare(frame, settings, true)?;
    let pattern = prepared
        .pattern
        .as_ref()
        .context("the learned denoiser needs a Bayer mosaic")?;
    let noise = prepared
        .noise_model()
        .context("the learned denoiser needs the frame's noise model")?;
    let tiling = greycard_ai::denoise::Tiling::default();
    let cache = model.and_then(|m| match greycard_ai::DenoiseCache::user() {
        Ok(cache) => Some((cache, greycard_ai::DenoiseCache::key(m, tiling, &prepared))),
        Err(e) => {
            tracing::warn!("no cache for the denoiser's answer: {e}");
            None
        }
    });
    let start = std::time::Instant::now();
    let rgb = match cache.as_ref().and_then(|(c, k)| c.read(*k, &prepared)) {
        Some(rgb) => {
            tracing::info!(
                "learned denoiser's answer from the cache in {:.1} s",
                start.elapsed().as_secs_f32()
            );
            rgb
        }
        None => {
            let rgb = denoiser.run(
                &prepared.samples,
                prepared.width,
                prepared.height,
                pattern,
                &noise,
                tiling,
            )?;
            tracing::info!(
                "learned denoiser ran in {:.1} s",
                start.elapsed().as_secs_f32()
            );
            if let Some((c, k)) = &cache {
                match c.write(*k, frame, &prepared, &rgb) {
                    Ok(()) => {
                        tracing::info!("the denoiser's answer kept in {}", c.path(*k).display())
                    }
                    Err(e) => tracing::warn!("not kept: {e}"),
                }
            }
            rgb
        }
    };
    Ok(finish(prepared, rgb, None, None, settings)?)
}

/// A file's luminance, for [`register`]: a picture as it stands, a raw
/// developed the short way.
///
/// White balance and the camera matrix still run — they are what makes
/// a working image, and `develop` does not offer to skip them. What is
/// skipped is everything the fit cannot use: the good demosaic
/// (bilinear puts the luminance where it belongs, which is all a fit
/// on the luminance asks of it), the chromatic aberration correction,
/// and highlight reconstruction.
///
/// Highlights are clipped rather than reconstructed on purpose.
/// Reconstruction invents detail above the clip from whichever channel
/// is still reading, and two frames of a bracket clip in different
/// places, so it would invent differently in each and put a difference
/// where the scene has none. Clipped, both frames go flat over the
/// same scene luminance: no gradient there, which costs the fit
/// nothing it had, and no disagreement either.
///
/// Hot photosite repair is on, which nothing else in this command
/// defaults to: a stuck photosite is a bright speck in every frame at
/// the same sensor position, which is to say a feature that does not
/// move, and the fit would rather not be shown one.
fn luminance_of(file: &std::path::Path) -> Result<(Vec<f32>, usize, usize)> {
    let image = if greycard_core::picture::is_picture_path(file) {
        greycard_core::picture::decode_picture_path(file)
            .with_context(|| format!("reading {}", file.display()))?
            .image
    } else {
        let frame = load(file)?;
        let settings = DevelopSettings {
            demosaic: DemosaicMethod::Bilinear,
            highlights: HighlightMode::Clip,
            chromatic_aberration: None,
            hot_pixels: Some(HotPixelOptions {
                sigmas: DEFAULT_SIGMAS,
                ratio: DEFAULT_RATIO,
            }),
            ..Default::default()
        };
        develop(&frame, &settings)
            .with_context(|| format!("developing {}", file.display()))?
            .image
    };
    let (width, height) = (image.width, image.height);
    Ok((register::luminance(&image), width, height))
}

/// Fit one frame onto another and say what came of it.
fn register(
    reference: &std::path::Path,
    moving: &std::path::Path,
    model: Model,
    finest_level: usize,
) -> Result<()> {
    let (a, w, h) = luminance_of(reference)?;
    let (b, mw, mh) = luminance_of(moving)?;
    if (w, h) != (mw, mh) {
        anyhow::bail!("the frames are {w}x{h} and {mw}x{mh}: registration wants one size");
    }
    let options = Options {
        model,
        finest_level,
        ..Options::default()
    };
    let started = std::time::Instant::now();
    let fit = register::fit(&a, &b, w, h, &options)?;
    let took = started.elapsed();

    println!("reference     {} ({w}x{h})", reference.display());
    println!("moving        {}", moving.display());
    println!(
        "pyramid       {} levels down to {}x{}{}, {model}, {} iterations in {:.0} ms",
        fit.levels,
        fit.coarsest.0,
        fit.coarsest.1,
        match fit.finest {
            0 => String::new(),
            n => format!(", stopping at level {n}"),
        },
        fit.iterations,
        took.as_secs_f64() * 1000.0
    );
    let m = fit.transform.m;
    println!(
        "transform     [{:>12.7} {:>12.7} {:>10.4}]",
        m[0], m[1], m[2]
    );
    println!(
        "              [{:>12.7} {:>12.7} {:>10.4}]",
        m[3], m[4], m[5]
    );
    // Which way round the shift reads: the transform takes reference
    // coordinates to moving ones, so this is where the middle of the
    // reference frame is to be found in the moving frame.
    let (dx, dy) = fit
        .transform
        .displacement((w - 1) as f64 / 2.0, (h - 1) as f64 / 2.0);
    println!(
        "shift         the reference's middle is at {dx:+.3}, {dy:+.3} px in the moving frame"
    );
    println!(
        "              scale {:.6}, rotation {:.4} degrees{}",
        fit.transform.scale(),
        fit.transform.rotation().to_degrees(),
        match model {
            // A scale and an angle are the whole of a similarity's
            // linear part. An affine's also has a shear and a second
            // scale, which these two numbers cannot hold: they are the
            // square root of the area factor and the angle the x axis
            // turned through. The matrix above is the answer.
            Model::Affine => " (an affine's linear part does not reduce to these; see the matrix)",
            _ => "",
        }
    );
    println!(
        "residual      {:.4} over {:.1}% of the frame{}",
        fit.residual,
        fit.overlap * 100.0,
        if fit.converged {
            ""
        } else {
            " (ran out of iterations)"
        }
    );
    // The engine's own threshold, so this agrees with what a caller
    // asking `Fit::aligned` would be told. Between aligned and plainly
    // hopeless there is a band worth naming: the fit found the
    // picture, but something in the frame did not hold still.
    let verdict = if fit.aligned() {
        "aligned"
    } else if fit.residual < 0.2 {
        "close, but something moved or changed: check before merging"
    } else {
        "no alignment found: do not use this transform"
    };
    println!("verdict       {verdict}");
    Ok(())
}

fn load(file: &std::path::Path) -> Result<RawFrame> {
    decode::decode_path(file).with_context(|| format!("decoding {}", file.display()))
}

fn info(file: &std::path::Path) -> Result<()> {
    let frame = load(file)?;
    println!("camera        {}", camera_name(&frame.make, &frame.model));
    print_exposure(&frame.shot);
    println!(
        "sensor        {}x{} x{} samples",
        frame.width, frame.height, frame.channels
    );
    match &frame.layout {
        SensorLayout::Cfa(p) => {
            println!("layout        CFA {}x{} {:?}", p.width, p.height, p.colors)
        }
        other => println!("layout        {other:?}"),
    }
    let b = &frame.levels.black;
    let w = &frame.levels.white;
    println!(
        "black         {}x{}x{} {:?}",
        b.width, b.height, b.cpp, b.values
    );
    println!(
        "white         {}x{}x{} {:?}",
        w.width, w.height, w.cpp, w.values
    );
    let check = frame.white_check();
    println!(
        "brightest     {:.0}; {} of {} samples above the declared white ({:.3}%)",
        check.brightest,
        check.above,
        check.total,
        check.fraction_above() * 100.0
    );
    match frame.crop {
        Some(c) => println!(
            "crop          {}x{} at ({}, {})",
            c.width, c.height, c.x, c.y
        ),
        None => println!("crop          none"),
    }
    println!("orientation   {:?}", frame.orientation);
    let shot = &frame.shot;
    println!(
        "lens          {}{}{}{}",
        shot.lens_model.as_deref().unwrap_or("not recorded"),
        shot.focal_length
            .map_or(String::new(), |f| format!(", {}", focal_text(f))),
        shot.f_number
            .map_or(String::new(), |f| format!(", {}", aperture_text(f))),
        shot.focus_distance
            .map_or(String::new(), |d| format!(", {d:.1} m")),
    );
    match frame.as_shot_coefficients {
        Some(c) => println!("as-shot gains R {:.4}  G {:.4}  B {:.4}", c[0], c[1], c[2]),
        None => println!("as-shot gains none"),
    }
    for c in &frame.calibrations {
        let name = CalibrationIlluminant::from_exif_code(c.illuminant)
            .map(|i| format!("{i:?}"))
            .unwrap_or_else(|| format!("code {}", c.illuminant));
        println!("calibration   {name}: {:?}", c.color_matrix);
    }
    match profile_from_frame(&frame) {
        Ok(profile) => {
            println!(
                "profile       {:?}{}",
                profile.primary.illuminant,
                profile
                    .secondary
                    .as_ref()
                    .map(|s| format!(" + {:?}", s.illuminant))
                    .unwrap_or_default()
            );
            match as_shot_temp_tint(&frame, &profile) {
                Ok(tt) => println!("as-shot white {:.0} K, Duv {:+.4}", tt.cct, tt.duv),
                Err(e) => println!("as-shot white unavailable ({e})"),
            }
        }
        Err(e) => println!("profile       unavailable ({e})"),
    }
    Ok(())
}

/// The `lenses` command.
fn lenses(
    file: Option<&std::path::Path>,
    fetch: bool,
    lens_name: Option<&str>,
    camera: Option<&[String]>,
) -> Result<()> {
    let store = greycard_lens::Store::user().context("finding the lens database")?;
    if fetch {
        let start = std::time::Instant::now();
        store.fetch(|_| {}).context("fetching the lens database")?;
        println!(
            "fetched into {} in {:.1} s",
            store.dir().display(),
            start.elapsed().as_secs_f32()
        );
    }
    let Some((db, from)) = store.load() else {
        println!(
            "no lens database in {} or lensfun's own places; run with --fetch",
            store.dir().display()
        );
        return Ok(());
    };
    println!(
        "database      {} ({} lenses, {} cameras, {} mounts)",
        from.display(),
        db.lenses.len(),
        db.cameras.len(),
        db.mounts.len()
    );
    let frame = match (file, lens_name) {
        (Some(file), _) => load(file)?,
        (None, Some(name)) => {
            // A frame that is only its tags.
            let mut frame = greycard_core::RawFrame {
                make: String::new(),
                model: String::new(),
                width: 6000,
                height: 4000,
                channels: 1,
                layout: SensorLayout::Cfa(greycard_core::raw::CfaPattern::rggb()),
                samples: greycard_core::raw::Samples::U16(Vec::new()),
                levels: greycard_core::raw::Levels {
                    black: greycard_core::raw::LevelPattern::uniform(0.0, 1),
                    white: greycard_core::raw::LevelPattern::uniform(1.0, 1),
                },
                as_shot_coefficients: None,
                calibrations: Vec::new(),
                crop: None,
                orientation: Orientation::Normal,
                shot: greycard_core::raw::Shot {
                    lens_model: Some(name.to_string()),
                    ..Default::default()
                },
            };
            if let Some([make, model]) = camera {
                frame.make = make.clone();
                frame.model = model.clone();
            }
            frame
        }
        (None, None) => return Ok(()),
    };
    let shot = &frame.shot;
    println!("camera        {}", camera_name(&frame.make, &frame.model));
    println!(
        "lens          {}{}{}",
        shot.lens_model.as_deref().unwrap_or("not recorded"),
        shot.focal_length
            .map_or(String::new(), |f| format!(", {}", focal_text(f))),
        shot.f_number
            .map_or(String::new(), |f| format!(", {}", aperture_text(f))),
    );
    let l = db.lookup(&frame);
    match l.camera {
        Some(c) => println!(
            "body          {} (crop {}, {})",
            c.models[0], c.crop_factor, c.mount
        ),
        None => println!("body          not in the database"),
    }
    match l.lens {
        Some(lens) => {
            println!(
                "profile       {} (crop {}, {}; {} distortion, {} CA, {} vignetting entries)",
                lens.name(),
                lens.crop_factor,
                lens.mounts.join("/"),
                lens.distortion.len(),
                lens.tca.len(),
                lens.vignetting.len()
            );
            let p = greycard_lens::Profile {
                lens,
                camera: l.camera,
            };
            let c = p.correction(frame.width, frame.height, shot, greycard_lens::Wanted::ALL);
            println!("correction    {}", correction_words(&c));
            if let Some(line) = coverage_words(&p, &c) {
                println!("covers        {line}");
            }
        }
        None => println!("profile       none"),
    }
    Ok(())
}

/// A model's size in whole megabytes, as the editor's sheet says it.
fn megabytes(bytes: u64) -> u64 {
    (bytes as f64 / 1e6).round() as u64
}

/// The models `what` names: a denoiser tier, a model's id, or `all`.
fn models_named(what: &str) -> Result<Vec<&'static greycard_ai::Model>> {
    if what == "all" {
        return Ok(greycard_ai::MODELS.iter().collect());
    }
    if let Some(model) = greycard_ai::denoiser(what).or_else(|| greycard_ai::model(what)) {
        return Ok(vec![model]);
    }
    let ids: Vec<&str> = greycard_ai::MODELS.iter().map(|m| m.id).collect();
    anyhow::bail!(
        "want all, a denoiser tier ({}) or a model id ({}), not {what}",
        denoiser_tiers(),
        ids.join(", ")
    )
}

/// One model's two lines in the listing: the id and what it is for,
/// then its size, its license and whether the store has it.
fn model_lines(
    model: &greycard_ai::Model,
    store: &greycard_ai::Store,
    width: usize,
) -> [String; 2] {
    let tier = greycard_ai::tier_of(model)
        .map(|t| format!("tier {t}, "))
        .unwrap_or_default();
    [
        format!("{:width$}  {}", model.id, model.purpose),
        format!(
            "{:width$}  {tier}{} MB, {}, {}",
            "",
            megabytes(model.bytes()),
            model.license.name,
            if store.have(model) {
                "in the store"
            } else {
                "not fetched"
            }
        ),
    ]
}

/// The `models` command: the registry, and fetching from it.
fn models(fetch: Option<&str>) -> Result<()> {
    let store = greycard_ai::Store::user().context("finding the model store")?;
    println!("store         {}", store.root().display());
    let Some(what) = fetch else {
        let width = greycard_ai::MODELS
            .iter()
            .map(|m| m.id.len())
            .max()
            .unwrap_or(0);
        for model in greycard_ai::MODELS {
            println!();
            for line in model_lines(model, &store, width) {
                println!("{line}");
            }
        }
        if greycard_ai::MODELS.iter().any(|m| !store.have(m)) {
            println!();
            println!("fetch one with `greycard models --fetch <tier|id|all>`; nothing is bundled");
        }
        return Ok(());
    };
    for model in models_named(what)? {
        println!();
        println!("{}", model.id);
        println!("  for         {}", model.purpose);
        println!(
            "  license     {} <{}>",
            model.license.name, model.license.url
        );
        println!("  source      {}", model.source);
        println!(
            "  size        {} MB in {} file{}",
            megabytes(model.bytes()),
            model.files.len(),
            if model.files.len() == 1 { "" } else { "s" }
        );
        if store.have(model) {
            println!("  already in  {}", store.dir(model).display());
            continue;
        }
        let start = std::time::Instant::now();
        let mut reporter = FetchReport::new();
        let fetched = store.fetch(model, |p| reporter.report(p));
        // Close the rewritten line before anything else is printed,
        // so a failure does not land on the end of it.
        reporter.done();
        fetched.with_context(|| format!("fetching {}", model.name))?;
        println!(
            "  fetched     into {} in {:.1} s",
            store.dir(model).display(),
            start.elapsed().as_secs_f32()
        );
    }
    Ok(())
}

/// The download's progress on stderr: a line rewritten in place at a
/// terminal, a line a file otherwise, and never faster than the eye.
struct FetchReport {
    terminal: bool,
    last: std::time::Instant,
    open: bool,
}

impl FetchReport {
    fn new() -> Self {
        use std::io::IsTerminal;
        Self {
            terminal: std::io::stderr().is_terminal(),
            last: std::time::Instant::now(),
            open: false,
        }
    }

    fn report(&mut self, p: greycard_ai::Progress) {
        use std::io::Write;
        let first = p.done == 0;
        let last = p.total > 0 && p.done == p.total;
        if !self.terminal {
            if first {
                eprintln!("  fetching    {} ({} MB)", p.file, megabytes(p.total));
            }
            return;
        }
        if !(first || last || self.last.elapsed().as_millis() > 200) {
            return;
        }
        self.last = std::time::Instant::now();
        let percent = if p.total > 0 {
            format!(" {:3.0}%", 100.0 * p.done as f64 / p.total as f64)
        } else {
            String::new()
        };
        eprint!(
            "\r  fetching    {} {} of {} MB{percent}",
            p.file,
            megabytes(p.done),
            megabytes(p.total)
        );
        let _ = std::io::stderr().flush();
        self.open = true;
    }

    /// Close the line the reports were written over.
    fn done(&mut self) {
        if self.open {
            eprintln!();
            self.open = false;
        }
    }
}

/// The lens corrections on the developed picture, from the database's
/// profile when one is asked for and found, and the manual ones.
/// The presets subcommand: what the store holds, and imports and
/// applications asked for.
fn presets(
    files: &[PathBuf],
    import: &[PathBuf],
    apply: Option<&str>,
    dir: Option<PathBuf>,
) -> Result<()> {
    use greycard_edit::Sidecar;
    use greycard_edit::preset::Store;
    let store = match dir {
        Some(dir) => Store::at(dir),
        None => {
            let store = Store::user().context("finding the configuration directory")?;
            // As the editor does on its way up: the shipped presets,
            // once. Only the user's own store — a `--dir` is somewhere
            // the caller named, a staging directory or a copy, and
            // dropping three presets into it is not what was asked.
            store.seed();
            store
        }
    };
    for path in import {
        let greycard_edit::lightroom::Imported { preset, unmapped } =
            greycard_edit::preset::import(path)
                .with_context(|| format!("reading {} as a preset", path.display()))?;
        let written = store
            .save(&preset)
            .with_context(|| format!("saving {}", preset.name))?;
        println!(
            "imported      {} ({}) into {}",
            preset.name,
            section_names(&preset),
            written.display()
        );
        if !unmapped.is_empty() {
            println!("              no place here for {}", unmapped.join(", "));
        }
    }
    if let Some(name) = apply {
        let entry = store
            .find(name)
            .with_context(|| format!("no preset called {name} in {}", store.dir().display()))?;
        if files.is_empty() {
            anyhow::bail!("--apply wants files to lay {} over", entry.preset.name);
        }
        for file in files {
            let mut sidecar = match Sidecar::load(file)
                .with_context(|| format!("reading the sidecar of {}", file.display()))?
            {
                Some(s) => s,
                None if greycard_core::picture::is_picture_path(file) => Sidecar {
                    current: greycard_edit::Edit::for_picture(),
                    ..Sidecar::default()
                },
                // A raw's learned-denoiser blend starts from its ISO,
                // probed without a full decode; a probe that fails
                // says so and leaves it at the old constant.
                None => {
                    let iso = match greycard_core::decode::probe_path(file) {
                        Ok(p) => p.iso,
                        Err(e) => {
                            tracing::warn!(
                                "no iso from {}, the learned blend keeps its default: {e}",
                                file.display()
                            );
                            None
                        }
                    };
                    let mut edit = greycard_edit::Edit::default();
                    edit.noise.learned_strength = greycard_edit::Noise::blend_for_iso(iso);
                    Sidecar {
                        current: edit,
                        ..Sidecar::default()
                    }
                }
            };
            // A sidecar from before schema 4 does not know which
            // way up an `Original` crop goes until the frame says;
            // the file is here, so ask it before writing anything
            // back (see `Edit::migrate_with_frame`).
            if sidecar.needs_frame() {
                match greycard_core::decode::stance_path(file)
                    .ok()
                    .and_then(|s| s.shown_size(sidecar.turn))
                {
                    Some((w, h)) => sidecar.migrate_with_frame(h > w),
                    None => tracing::warn!(
                        "{}: will not say which way up it is, so its Original crop \
                         is left for the editor to bring up to date",
                        file.display()
                    ),
                }
            }
            let edit = entry.preset.applied(&sidecar.current);
            if sidecar.record(edit) {
                // Back where it was found: the CLI has no setting.
                sidecar
                    .save_in(file, greycard_edit::Placement::of(file))
                    .with_context(|| format!("writing the sidecar of {}", file.display()))?;
                println!("applied       {} to {}", entry.preset.name, file.display());
            } else {
                println!(
                    "unchanged     {} has {} already",
                    file.display(),
                    entry.preset.name
                );
            }
        }
    } else if !files.is_empty() {
        anyhow::bail!("files are for --apply; say which preset");
    }
    if import.is_empty() && apply.is_none() {
        let entries = store.list();
        println!("store         {}", store.dir().display());
        if entries.is_empty() {
            println!("no presets; save one from the editor, or --import a file");
        }
        for e in &entries {
            println!(
                "{:<13} {} ({})",
                e.preset.name,
                section_names(&e.preset),
                e.path.display()
            );
        }
    }
    Ok(())
}

/// A preset's sections, in a row.
fn section_names(preset: &greycard_edit::Preset) -> String {
    let names: Vec<&str> = preset.sections.iter().map(|s| s.title()).collect();
    if names.is_empty() {
        "nothing".into()
    } else {
        names.join(", ")
    }
}

/// What a profile would do to this picture, in words: the engine's own
/// struct printed with `{:?}` is not for anybody to read.
fn correction_words(c: &greycard_core::develop::lens::LensCorrection) -> String {
    use greycard_core::develop::lens::{Distortion, Scale};
    let distortion = match c.distortion {
        Some(Distortion::Poly3 { .. }) => "distortion (poly3)",
        Some(Distortion::Poly5 { .. }) => "distortion (poly5)",
        Some(Distortion::Ptlens { .. }) => "distortion (ptlens)",
        Some(Distortion::Acm { .. }) => "distortion (acm)",
        None => "no distortion",
    };
    let ca = match c.chromatic_aberration {
        Some(_) => "chromatic aberration",
        None => "no chromatic aberration",
    };
    let vignetting = match c.vignetting {
        Some(_) => "vignetting",
        None => "no vignetting",
    };
    let magnification = match c.scale {
        Scale::Auto => "auto".to_string(),
        Scale::Fixed(s) => format!("{s:.4}"),
    };
    format!(
        "{distortion}, {ca}, {vignetting}; radius scale {:.4}, vignetting scale {:.4}, magnification {magnification}",
        c.radius_scale, c.vignetting_scale
    )
}

/// What a profile measured on a smaller sensor leaves out of this
/// picture, in words; nothing when it covers it.
fn coverage_words(
    p: &greycard_lens::Profile,
    c: &greycard_core::develop::lens::LensCorrection,
) -> Option<String> {
    if !p.measured_on_smaller_sensor() {
        return None;
    }
    Some(match c.vignetting_edge() {
        Some(edge) => format!(
            "the profile was measured on a smaller sensor; the corners reach r={:.2}, and the vignetting past r=1 ({:.0}% of the way to the corner) is held at its edge",
            p.reach(),
            edge * 100.0
        ),
        None => format!(
            "the profile was measured on a smaller sensor; the corners reach r={:.2}",
            p.reach()
        ),
    })
}

/// The camera profile a run asks for: nothing for the file's own, a
/// path to a DCP, or a name in the profile directory. Unlike a develop
/// from a sidecar, which falls back to the file's own calibrations and
/// says so in the log, a profile asked for outright and not found is an
/// error: the picture would not be the one that was asked for.
fn chosen_profile(name: &str) -> Result<Option<std::sync::Arc<greycard_core::Profile>>> {
    use greycard_edit::camera::{self, ProfileChoice};
    let ProfileChoice::Named(name) = ProfileChoice::from_name(name) else {
        return Ok(None);
    };
    let path = if name.contains(['/', '\\']) || name.to_lowercase().ends_with(".dcp") {
        PathBuf::from(&name)
    } else {
        camera::path_for(&name)
            .with_context(|| format!("finding the profile directory for {name:?}"))?
    };
    let dcp = greycard_core::Dcp::load(&path)
        .with_context(|| format!("reading {} as a camera profile", path.display()))?;
    let profile = dcp
        .profile()
        .with_context(|| format!("using {} as a camera profile", path.display()))?;
    Ok(Some(std::sync::Arc::new(profile)))
}

/// The lens database a run wants, read before anything long happens,
/// so a machine without one hears about it at once and not after a
/// develop it is going to throw away.
fn lens_database(lens: &greycard_edit::Lens) -> Result<Option<greycard_lens::Database>> {
    if !lens.wants_profile() {
        return Ok(None);
    }
    let store = greycard_lens::Store::user().context("finding the lens database")?;
    let (db, from) = store.load().with_context(|| {
        format!(
            "no lens database in {} or lensfun's own places; run `greycard lenses --fetch` to get it",
            store.dir().display()
        )
    })?;
    tracing::info!(
        "lens database from {} ({} lenses, {} cameras)",
        from.display(),
        db.lenses.len(),
        db.cameras.len()
    );
    Ok(Some(db))
}

fn correct_lens(
    image: &mut WorkingImage,
    (make, model, shot): (&str, &str, &greycard_core::raw::Shot),
    lens: &greycard_edit::Lens,
    db: Option<&greycard_lens::Database>,
) -> Result<()> {
    let profile = db.and_then(|db| {
        let l = db.lookup_shot(make, model, shot);
        match (l.lens, l.camera) {
            (Some(lens), camera) => {
                tracing::info!("lens profile {}", lens.name());
                let p = greycard_lens::Profile { lens, camera };
                if p.measured_on_smaller_sensor() {
                    tracing::info!(
                        "{}: measured on a smaller sensor, the corners reach r={:.2}; the vignetting is held at its edge past r=1",
                        lens.name(),
                        p.reach()
                    );
                }
                if camera.is_none() {
                    tracing::warn!(
                        "{}: body not in the lens database, the lens's own format assumed",
                        lens.name()
                    );
                }
                Some(greycard_lens::Profile { lens, camera })
            }
            (None, _) => {
                tracing::warn!(
                    "no lens profile for {}",
                    shot.lens_model
                        .as_deref()
                        .unwrap_or("a file that names no lens")
                );
                None
            }
        }
    });
    let correction =
        profile.map(|p| p.correction(image.width, image.height, shot, greycard_lens::Wanted::ALL));
    for c in lens.corrections(correction) {
        let start = std::time::Instant::now();
        let (corrected, stats) = greycard_core::develop::lens::correct(image, &c);
        *image = corrected;
        tracing::info!(
            "lens corrected in {:.2} s: scale {:.4}{}{}{}{}",
            start.elapsed().as_secs_f32(),
            stats.scale,
            if c.distortion.is_some() {
                ", distortion"
            } else {
                ""
            },
            if c.chromatic_aberration.is_some() {
                ", chromatic aberration"
            } else {
                ""
            },
            match stats.corner_gain {
                Some(g) => format!(", vignetting (corner gain {g:.3})"),
                None => String::new(),
            },
            if stats.resampled { "" } else { ", no resample" },
        );
    }
    // After the geometry, so the resample does not smear a chroma the
    // defringe has just neutralized, and before the sharpen.
    if let Some(options) = lens.defringe() {
        let start = std::time::Instant::now();
        let stats = defringe::defringe(image, &options);
        tracing::info!(
            "defringed in {:.2} s: radius {:.1}, threshold {:.0}, {:.3}% of the frame replaced, of the {:.3}% over the threshold; mean Oklab chroma there {:.4} to {:.4}",
            start.elapsed().as_secs_f32(),
            stats.radius,
            options.threshold,
            stats.fraction * 100.0,
            stats.over_threshold * 100.0,
            stats.chroma_before,
            stats.chroma_after,
        );
    }
    Ok(())
}

/// Everything after the engine's develop, in the order the editor's
/// worker runs it: the lens correction, the defringe that follows it,
/// the local contrast, the dehaze, then the capture sharpening.
/// Hands back what the sharpen did; the local contrast and the
/// dehaze log what they did, and the dehaze's transmission is
/// written to `dehaze_map` when a path is given.
///
/// The sharpen is last because the correction resamples. Sharpening
/// first would leave the resample to blur what had just been
/// sharpened, and the point spread the deconvolution assumes, measured
/// on the mosaic, would no longer be the picture's.
/// [`DevelopSettings::sharpen`] runs it inside `finish`, before any
/// geometry, so the CLI keeps it out of the engine's settings and runs
/// it here instead, where the worker has it (notes §21, §74).
///
/// `db` is the lens database [`lens_database`] read before the decode,
/// or none when no profile is asked for.
#[allow(clippy::too_many_arguments)]
fn correct_and_sharpen(
    image: &mut WorkingImage,
    identity: (&str, &str, &greycard_core::raw::Shot),
    lens: &greycard_edit::Lens,
    db: Option<&greycard_lens::Database>,
    detail: Option<LocalContrastOptions>,
    dehazing: Option<DehazeOptions>,
    dehaze_map: Option<&std::path::Path>,
    sharpening: Option<sharpen::SharpenOptions>,
    measured_radius: Option<f32>,
    clip_level: f32,
) -> Result<Option<sharpen::SharpenStats>> {
    correct_lens(image, identity, lens, db)?;
    // Before the sharpen, as the worker has it: the sharpen's blend
    // mask reads the picture's local contrast, and should read it
    // with the detail put in.
    if let Some(options) = detail {
        let start = std::time::Instant::now();
        let stats = local_contrast(image, &options, clip_level);
        tracing::info!(
            "local contrast in {:.2} s: texture {:+.2} at {} px, clarity {:+.2} at {} px",
            start.elapsed().as_secs_f32(),
            options.texture,
            stats.texture_radius,
            options.clarity,
            stats.clarity_radius,
        );
    }
    // After the local contrast and before the sharpen, whose contrast
    // threshold is measured on the picture it gets.
    if let Some(options) = dehazing {
        let start = std::time::Instant::now();
        let d = match dehaze_map {
            Some(path) => {
                let (d, map) = dehaze::dehaze_with_map(image, &options);
                write_transmission(&map, image.width, image.height, path)?;
                d
            }
            None => dehaze::dehaze(image, &options),
        };
        tracing::info!(
            "dehazed {:+.0} (strength {:.2}) in {:.2} s: airlight R {:.3} G {:.3} B {:.3}, map at 1/{}, transmission mean {:.2} least {:.2}",
            d.amount * 100.0,
            d.strength,
            start.elapsed().as_secs_f32(),
            d.airlight[0],
            d.airlight[1],
            d.airlight[2],
            d.factor,
            d.transmission_mean,
            d.transmission_min,
        );
    }
    Ok(sharpening.map(|options| sharpen::sharpen(image, &options, measured_radius, clip_level)))
}

/// The dehaze's transmission as an 8-bit grey PNG: 0 to 1 onto
/// black to white, so the floor shows as dark grey and a negative
/// amount, whose transmission is over one, as white throughout.
fn write_transmission(
    map: &[f32],
    width: usize,
    height: usize,
    path: &std::path::Path,
) -> Result<()> {
    let data: Vec<u8> = map
        .iter()
        .map(|&t| (t.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let grey = image::GrayImage::from_raw(width as u32, height as u32, data)
        .context("the transmission map's size")?;
    grey.save(path)
        .with_context(|| format!("writing {}", path.display()))?;
    eprintln!("wrote the transmission map to {}", path.display());
    Ok(())
}

struct Outputs<'a> {
    dng: Option<&'a std::path::Path>,
    compression: Compression,
    tiff: Option<&'a std::path::Path>,
    preview: Option<&'a std::path::Path>,
    /// The dehaze's transmission map, a grey PNG.
    dehaze_map: Option<&'a std::path::Path>,
    on_exists: OnExists,
}

impl Outputs<'_> {
    /// Where a wanted output goes with the policy applied, warning
    /// what it made of a file being there already; none when the
    /// output was not asked for, or when the policy leaves what is
    /// there.
    fn target(&self, wanted: Option<&std::path::Path>) -> Option<PathBuf> {
        let resolved = self.on_exists.resolve(wanted?);
        match &resolved {
            Resolved::Free(_) => {}
            Resolved::Renamed { path, asked } => tracing::warn!(
                "{} is there already: writing {} instead",
                asked.display(),
                path.display()
            ),
            Resolved::Replaced(path) => {
                tracing::warn!("{} is there already: writing over it", path.display())
            }
            Resolved::Skipped(path) => {
                tracing::warn!("skipped {}: it is there already", path.display())
            }
        }
        resolved.path().map(std::path::Path::to_path_buf)
    }

    /// Every output asked for, with the policy applied. None at all
    /// left to write is not a failure, but it is worth a warning.
    fn targets(&self) -> Targets {
        let targets = Targets {
            dng: self.target(self.dng),
            tiff: self.target(self.tiff),
            preview: self.target(self.preview),
            dehaze_map: self.target(self.dehaze_map),
        };
        if targets.nothing() {
            tracing::warn!("nothing left to write");
        }
        targets
    }
}

/// The paths the outputs actually go to.
struct Targets {
    dng: Option<PathBuf>,
    tiff: Option<PathBuf>,
    preview: Option<PathBuf>,
    dehaze_map: Option<PathBuf>,
}

impl Targets {
    fn nothing(&self) -> bool {
        self.dng.is_none()
            && self.tiff.is_none()
            && self.preview.is_none()
            && self.dehaze_map.is_none()
    }
}

fn run_develop(
    file: &std::path::Path,
    outputs: &Outputs<'_>,
    settings: &DevelopSettings,
    exposure: f64,
    ai_denoise: Option<&(PathBuf, Option<&'static greycard_ai::Model>)>,
    lens: &greycard_edit::Lens,
    detail: Option<LocalContrastOptions>,
) -> Result<()> {
    if greycard_core::picture::is_picture_path(file) {
        if ai_denoise.is_some() {
            tracing::warn!(
                "--ai-denoise needs a Bayer mosaic; {} is a picture already rendered, so it is ignored",
                file.display()
            );
        }
        return run_develop_picture(
            file,
            outputs,
            lens,
            detail,
            settings.dehaze,
            settings.sharpen,
            exposure,
        );
    }
    // What is already there decides the names before the developing,
    // so a run that would write nothing does nothing.
    let targets = outputs.targets();
    if targets.nothing() {
        return Ok(());
    }
    let db = lens_database(lens)?;
    let (frame, metadata) = RawlerDecoder
        .decode_path_with_metadata(file)
        .with_context(|| format!("decoding {}", file.display()))?;
    // The dehaze and the sharpen are not the engine's to run here:
    // they go after the lens correction, as `correct_and_sharpen`
    // says and as the editor's worker has it.
    let dehazing = settings.dehaze;
    let sharpening = settings.sharpen;
    let settings = &DevelopSettings {
        sharpen: None,
        dehaze: None,
        ..settings.clone()
    };
    let mut developed = match ai_denoise {
        Some(model) => develop_learned(&frame, settings, model),
        None => develop(&frame, settings).map_err(anyhow::Error::from),
    }
    .with_context(|| format!("developing {}", file.display()))?;
    developed.sharpen = correct_and_sharpen(
        &mut developed.image,
        (&frame.make, &frame.model, &frame.shot),
        lens,
        db.as_ref(),
        detail,
        dehazing,
        targets.dehaze_map.as_deref(),
        sharpening,
        developed.sharpen_radius,
        developed.clip_level,
    )?;
    let wb = &developed.white_balance;
    let g = wb.coefficients_f32();
    tracing::info!(
        "white {:.0} K, Duv {:+.4}; gains R {:.4} G {:.4} B {:.4}; {}x{}",
        wb.temp_tint.cct,
        wb.temp_tint.duv,
        g[0],
        g[1],
        g[2],
        developed.image.width,
        developed.image.height
    );
    if let Some(ca) = &developed.chromatic_aberration {
        if ca.corrected {
            tracing::info!(
                "chromatic aberration up to {:.2} px red, {:.2} px blue; order-{} fit from {} tiles",
                ca.max_shift[0],
                ca.max_shift[1],
                ca.order,
                ca.blocks
            );
        } else {
            tracing::warn!(
                "chromatic aberration not corrected ({} usable tiles)",
                ca.blocks
            );
        }
    }
    if let Some(d) = &developed.dual {
        tracing::info!(
            "dual demosaic contrast threshold {:.0}{}; {:.1}% of the frame from VNG4",
            d.threshold * 100.0,
            match d.source {
                ThresholdSource::Noise => " at mid grey, from the noise model",
                ThresholdSource::Tiles => " from the flattest tile",
                ThresholdSource::Fixed => "",
            },
            d.flat_fraction * 100.0
        );
    }
    if let Some(n) = &developed.noise {
        let m = &n.model;
        tracing::info!(
            "noise at mid grey: sigma R {:.5} G {:.5} B {:.5} (a {:.2e} {:.2e} {:.2e}, b {:.2e} {:.2e} {:.2e}) from {:?} blocks",
            m.sigma(0, MID_GREY),
            m.sigma(1, MID_GREY),
            m.sigma(2, MID_GREY),
            m.a[0],
            m.a[1],
            m.a[2],
            m.b[0],
            m.b[1],
            m.b[2],
            n.blocks
        );
    }
    if let Some(h) = &developed.hot_pixels {
        tracing::info!(
            "hot pixels: {} hot and {} dead photosites replaced",
            h.hot,
            h.dead
        );
    }
    if let Some(s) = &developed.sharpen {
        tracing::info!(
            "sharpened: radius {:.2} (the mosaic says {}), {} iterations, contrast threshold {:.0}%, {:.0}% of the picture, {:.2}% clipped",
            s.radius,
            developed
                .sharpen_radius
                .map(|r| format!("{r:.2}"))
                .unwrap_or_else(|| "nothing".into()),
            s.iterations,
            s.threshold * 100.0,
            s.blend_mean * 100.0,
            s.clipped * 100.0
        );
    }
    if let Some(d) = &developed.denoise {
        if let Some(n) = &d.nlm {
            tracing::info!(
                "denoised by non-local means at strength {:.2}: patch radius {}, center weight {:.2}, {} offsets scattered {:.2} up to {} px",
                n.strength,
                n.patch_radius,
                n.center_weight,
                n.patches,
                n.scattering,
                n.max_shift
            );
        }
        if d.scales > 0 {
            tracing::info!(
                "denoised over {} wavelet scales at strength {:.2}",
                d.scales,
                d.strength
            );
        }
    }
    if let Some(h) = &developed.highlights {
        tracing::info!(
            "highlights {} clipped photosites; chrominance R {:+.4} G {:+.4} B {:+.4} from {:?} votes",
            h.clipped,
            h.chrominance[0],
            h.chrominance[1],
            h.chrominance[2],
            h.votes
        );
    }
    if let Some(s) = &developed.segments {
        tracing::info!(
            "segments {:?} per channel, {:?} with a candidate; {} photosites refined",
            s.segments,
            s.candidates,
            s.replaced
        );
    }

    if let Some(path) = &targets.dng {
        let demosaiced = demosaic(&frame, settings)
            .with_context(|| format!("demosaicing {}", file.display()))?;
        // The embedded preview is what a browser or a culling tool shows
        // before it decodes anything, so it gets the same rendering as
        // --preview, at as-shot exposure, and at reduced size since it is
        // stored as an 8-bit JPEG.
        let preview = srgb_preview(&developed.image, 0.0)?;
        let preview = image::imageops::resize(
            &preview,
            preview.width().min(1024),
            (preview.height() * preview.width().min(1024) / preview.width()).max(1),
            image::imageops::FilterType::Triangle,
        );
        // The developed image is in display orientation; the DNG stores
        // everything in sensor orientation and says how to turn it.
        let preview = unorient(preview, frame.orientation);
        let options = LinearDngOptions {
            compression: outputs.compression,
            metadata: Some(&metadata),
            preview: Some(&preview),
            software: None,
            headroom: None,
        };
        let file =
            std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut writer = std::io::BufWriter::new(file);
        write_linear_dng(
            &mut writer,
            &frame,
            &demosaiced.image,
            demosaiced.white_balance.coefficients_f32(),
            &options,
        )
        .with_context(|| format!("writing {}", path.display()))?;
        std::io::Write::flush(&mut writer)?;
        eprintln!("wrote {}", path.display());
    }
    // The developed picture is in sensor orientation; the EXIF says so
    // for the TIFF and the preview alike, as the DNG's does.
    let source_name = file.file_name().map(|n| n.to_string_lossy().into_owned());
    let provenance = |width: u32, height: u32, srgb: bool| greycard_core::exif::Provenance {
        metadata: &metadata,
        software: SOFTWARE,
        width,
        height,
        srgb,
        written: Some(std::time::SystemTime::now()),
        source_name: source_name.as_deref(),
        output: None,
        edit: None,
    };
    if let Some(path) = &targets.tiff {
        let (w, h) = (developed.image.width as u32, developed.image.height as u32);
        write_linear_tiff(&developed.image, path, &provenance(w, h, false))?;
        eprintln!("wrote {}", path.display());
    }
    if let Some(path) = &targets.preview {
        let preview = srgb_preview(&developed.image, exposure)?;
        save_preview(
            &preview,
            path,
            &provenance(preview.width(), preview.height(), true),
        )?;
        eprintln!("wrote {}", path.display());
    }
    Ok(())
}

/// The `Software` tag on what the CLI writes.
const SOFTWARE: &str = concat!("greycard ", env!("CARGO_PKG_VERSION"));

/// What a picture that is not a raw says about itself.
fn info_picture(file: &std::path::Path) -> Result<()> {
    let p = greycard_core::picture::decode_picture_path(file)
        .with_context(|| format!("decoding {}", file.display()))?;
    println!("picture       {}-bit, taken as {}", p.bits, p.space);
    println!("camera        {}", camera_name(&p.make, &p.model));
    print_exposure(&p.shot);
    println!(
        "size          {}x{}, upright",
        p.image.width, p.image.height
    );
    match &p.shot.lens_model {
        Some(l) => println!("lens          {l}"),
        None => println!("lens          not recorded"),
    }
    Ok(())
}

/// What the camera was set to, one tag a line, said the way the
/// editor's panel says it.
fn print_exposure(shot: &Shot) {
    match shot.iso {
        Some(iso) => println!("iso           {iso}"),
        None => println!("iso           not recorded"),
    }
    match shot.exposure_time.and_then(shutter_text) {
        Some(t) => println!("shutter       {t}"),
        None => println!("shutter       not recorded"),
    }
}

/// A picture that is not a raw: nothing to develop, so only the lens
/// correction, the dehaze, the capture sharpening and the outputs apply; a DNG is
/// refused, since there is no camera space to write. `exposure`
/// reaches `--preview` the way it does on the raw path; `--ai-denoise`
/// is the caller's to reject before this is called, since the learned
/// denoiser wants a Bayer mosaic a picture does not have.
///
/// The sharpen runs where [`correct_and_sharpen`] puts it for a raw:
/// after the lens correction, on the same reasoning (notes §88). A
/// picture has no mosaic to measure a point spread from, so the
/// radius is always what the options themselves say (`Auto` falling
/// back to [`sharpen::DEFAULT_RADIUS`]); the clip level is the
/// picture's own, the same [`greycard_core::picture::Picture::clip_level`]
/// the editor's worker reads off its `Picture` for this path.
fn run_develop_picture(
    file: &std::path::Path,
    outputs: &Outputs<'_>,
    lens: &greycard_edit::Lens,
    detail: Option<LocalContrastOptions>,
    dehazing: Option<DehazeOptions>,
    sharpening: Option<sharpen::SharpenOptions>,
    exposure: f64,
) -> Result<()> {
    anyhow::ensure!(
        outputs.dng.is_none(),
        "a linear DNG wants a raw; {} is a picture already rendered",
        file.display()
    );
    let targets = outputs.targets();
    if targets.nothing() {
        return Ok(());
    }
    let db = lens_database(lens)?;
    let mut p = greycard_core::picture::decode_picture_path(file)
        .with_context(|| format!("decoding {}", file.display()))?;
    tracing::info!(
        "{}-bit picture taken as {}; {}x{}",
        p.bits,
        p.space,
        p.image.width,
        p.image.height
    );
    let clip_level = p.clip_level();
    let stats = correct_and_sharpen(
        &mut p.image,
        (&p.make, &p.model, &p.shot),
        lens,
        db.as_ref(),
        detail,
        dehazing,
        targets.dehaze_map.as_deref(),
        sharpening,
        None,
        clip_level,
    )?;
    if let Some(s) = &stats {
        tracing::info!(
            "sharpened: radius {:.2} (no mosaic to measure), {} iterations, contrast threshold {:.0}%, {:.0}% of the picture, {:.2}% clipped",
            s.radius,
            s.iterations,
            s.threshold * 100.0,
            s.blend_mean * 100.0,
            s.clipped * 100.0
        );
    }
    let source_name = file.file_name().map(|n| n.to_string_lossy().into_owned());
    let provenance = |width: u32, height: u32, srgb: bool| greycard_core::exif::Provenance {
        metadata: &p.metadata,
        software: SOFTWARE,
        width,
        height,
        srgb,
        written: Some(std::time::SystemTime::now()),
        source_name: source_name.as_deref(),
        output: None,
        edit: None,
    };
    if let Some(path) = &targets.tiff {
        let (w, h) = (p.image.width as u32, p.image.height as u32);
        write_linear_tiff(&p.image, path, &provenance(w, h, false))?;
        eprintln!("wrote {}", path.display());
    }
    if let Some(path) = &targets.preview {
        let preview = srgb_preview(&p.image, exposure)?;
        save_preview(
            &preview,
            path,
            &provenance(preview.width(), preview.height(), true),
        )?;
        eprintln!("wrote {}", path.display());
    }
    Ok(())
}

fn write_linear_tiff(
    image: &WorkingImage,
    path: &std::path::Path,
    provenance: &greycard_core::exif::Provenance<'_>,
) -> Result<()> {
    let data: Vec<u16> = image
        .data
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16)
        .collect();
    let (w, h) = (image.width as u32, image.height as u32);
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    greycard_core::exif::write_rgb16_tiff(&mut writer, w, h, &data, None, Some(provenance))
        .with_context(|| format!("writing {}", path.display()))?;
    std::io::Write::flush(&mut writer).with_context(|| format!("writing {}", path.display()))
}

/// Save an sRGB preview by its extension, with the EXIF where the
/// format has a place for it (JPEG and PNG).
/// The preview with its EXIF and XMP, as a JPEG or a PNG by the
/// path's extension.
fn save_preview(
    preview: &image::RgbImage,
    path: &std::path::Path,
    provenance: &greycard_core::exif::Provenance<'_>,
) -> Result<()> {
    use image::codecs::{jpeg::JpegEncoder, png::PngEncoder};
    use image::{ExtendedColorType, ImageEncoder};
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let (w, h) = (preview.width(), preview.height());
    let exif = greycard_core::exif::payload(provenance)?;
    let mut bytes = Vec::new();
    match ext.as_str() {
        "jpg" | "jpeg" => {
            let mut enc = JpegEncoder::new_with_quality(&mut bytes, 92);
            enc.set_exif_metadata(exif)?;
            enc.write_image(preview.as_raw(), w, h, ExtendedColorType::Rgb8)?;
        }
        "png" => {
            let mut enc = PngEncoder::new(&mut bytes);
            enc.set_exif_metadata(exif)?;
            enc.write_image(preview.as_raw(), w, h, ExtendedColorType::Rgb8)?;
        }
        _ => {
            return preview
                .save(path)
                .with_context(|| format!("writing {}", path.display()));
        }
    }
    let bytes = greycard_core::exif::with_xmp(bytes, provenance);
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Undo a display orientation, taking an image back to how the sensor saw it.
fn unorient(image: image::RgbImage, orientation: Orientation) -> image::RgbImage {
    use image::imageops::{flip_horizontal, flip_vertical, rotate90, rotate180, rotate270};
    match orientation {
        Orientation::Normal => image,
        Orientation::FlipHorizontal => flip_horizontal(&image),
        Orientation::Rotate180 => rotate180(&image),
        Orientation::FlipVertical => flip_vertical(&image),
        // Transpose and transverse are their own inverses.
        Orientation::Transpose => flip_horizontal(&rotate90(&image)),
        Orientation::Transverse => flip_vertical(&rotate90(&image)),
        Orientation::Rotate90 => rotate270(&image),
        Orientation::Rotate270 => rotate90(&image),
    }
}

/// Render the working image to 8-bit sRGB: exposure applied, highlights
/// clipped in the working space, no tone curve.
fn srgb_preview(image: &WorkingImage, exposure: f64) -> Result<image::RgbImage> {
    let to_srgb = WORKING_SPACE
        .to_space_matrix(&RgbSpace::SRGB, CAT)
        .context("working space to sRGB matrix")?
        .rows
        .map(|row| row.map(|v| v as f32));
    let scale = 2f32.powf(exposure as f32);
    let mut data = Vec::with_capacity(image.width * image.height * 3);
    for px in image.pixels() {
        // Clip in the working space so blown areas go to white rather than to
        // whichever channel the gains pushed highest.
        let [r, g, b] = px.map(|v| (v * scale).min(1.0));
        for row in to_srgb {
            let lin = row[0] * r + row[1] * g + row[2] * b;
            data.push((srgb_encode(lin.clamp(0.0, 1.0)) * 255.0).round() as u8);
        }
    }
    image::ImageBuffer::<image::Rgb<u8>, _>::from_raw(image.width as u32, image.height as u32, data)
        .context("image dimensions do not match its data")
}

fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// A number or `auto`.
#[derive(Debug, Clone, Copy)]
struct AutoUsize(Option<usize>);

impl std::str::FromStr for AutoUsize {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "auto" {
            return Ok(Self(None));
        }
        s.parse()
            .map(Some)
            .map(Self)
            .map_err(|_| format!("{s}: want auto or a number"))
    }
}

/// A number or `auto`.
#[derive(Debug, Clone, Copy)]
struct AutoF32(Option<f32>);

impl std::str::FromStr for AutoF32 {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "auto" {
            return Ok(Self(None));
        }
        s.parse()
            .map(Some)
            .map(Self)
            .map_err(|_| format!("{s}: want auto or a number"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::develop::lens;
    use greycard_core::raw::Shot;

    /// A frame with hard edges for the sharpen to bite on and a ramp
    /// under them, so a resample has something to move and a sharpen
    /// something to do.
    fn test_frame(w: usize, h: usize) -> WorkingImage {
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let block = if (x / 7 + y / 5) % 2 == 0 { 0.70 } else { 0.25 };
                let ramp = 0.10 * (x as f32 / w as f32) + 0.05 * (y as f32 / h as f32);
                let px = &mut image.data[(y * w + x) * 3..][..3];
                px[0] = block + ramp;
                px[1] = block + 0.5 * ramp;
                px[2] = 0.9 * block + ramp;
            }
        }
        image
    }

    fn max_diff(a: &WorkingImage, b: &WorkingImage) -> f32 {
        a.data
            .iter()
            .zip(&b.data)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max)
    }

    /// The CLI's order after the develop is the editor's: the lens
    /// correction first, the capture sharpening on what it made. The
    /// two do not commute when the correction moves pixels, and
    /// sharpening first is the wrong one (notes §74).
    #[test]
    fn the_sharpen_comes_after_the_lens_correction() {
        const CLIP: f32 = 10.0;
        let source = test_frame(128, 128);
        // A distortion by hand, no profile: nothing is looked up, so
        // the test needs no database.
        let edit = greycard_edit::Lens {
            profile: false,
            manual: -0.12,
            auto_scale: false,
            scale: 1.0,
            ..Default::default()
        };
        let corrections = edit.corrections(None);
        assert_eq!(corrections.len(), 1);
        assert!(corrections[0].moves(), "the test wants pixels moved");
        let options = sharpen::SharpenOptions {
            radius: sharpen::Radius::Fixed(1.0),
            iterations: sharpen::DEFAULT_ITERATIONS,
            // Everything unclipped, so the blend does not depend on
            // what the flattest patch of a synthetic frame measures.
            contrast: sharpen::Threshold::Fixed(0.0),
            stop_early: true,
        };

        let mut got = source.clone();
        let shot = Shot::default();
        let stats = correct_and_sharpen(
            &mut got,
            ("Greycard", "Test", &shot),
            &edit,
            // No profile is asked for, so no database is wanted.
            None,
            None,
            None,
            None,
            Some(options),
            None,
            CLIP,
        )
        .unwrap()
        .expect("the sharpen ran");
        assert_eq!(stats.radius, 1.0);
        assert!(stats.blend_mean > 0.9, "blend {}", stats.blend_mean);

        // sharpen(lens(x)): what the editor's worker makes.
        let mut want = lens::correct(&source, &corrections[0]).0;
        sharpen::sharpen(&mut want, &options, None, CLIP);
        // lens(sharpen(x)): what `develop`'s finish used to leave.
        let mut wrong = source.clone();
        sharpen::sharpen(&mut wrong, &options, None, CLIP);
        let wrong = lens::correct(&wrong, &corrections[0]).0;

        assert!(
            max_diff(&got, &want) < 1e-6,
            "the CLI sharpens after the lens: {}",
            max_diff(&got, &want)
        );
        assert!(
            max_diff(&got, &wrong) > 1e-3,
            "the two orders must differ, else the test pins nothing: {}",
            max_diff(&got, &wrong)
        );
    }

    /// The dehaze goes after the lens correction and before the
    /// sharpen, the editor's order: `sharpen(dehaze(lens(x)))`.
    #[test]
    fn the_dehaze_comes_after_the_lens_and_before_the_sharpen() {
        const CLIP: f32 = 10.0;
        // A veil over the test frame, so the dehaze has haze to find.
        let mut source = test_frame(128, 128);
        for v in source.data.iter_mut() {
            *v = *v * 0.5 + 0.45;
        }
        let edit = greycard_edit::Lens {
            profile: false,
            manual: -0.12,
            auto_scale: false,
            scale: 1.0,
            ..Default::default()
        };
        let corrections = edit.corrections(None);
        let options = sharpen::SharpenOptions {
            radius: sharpen::Radius::Fixed(1.0),
            iterations: sharpen::DEFAULT_ITERATIONS,
            contrast: sharpen::Threshold::Fixed(0.0),
            stop_early: true,
        };
        let dehazing = DehazeOptions { amount: 0.6 };
        let mut got = source.clone();
        let shot = Shot::default();
        correct_and_sharpen(
            &mut got,
            ("Greycard", "Test", &shot),
            &edit,
            None,
            None,
            Some(dehazing),
            None,
            Some(options),
            None,
            CLIP,
        )
        .unwrap()
        .expect("the sharpen ran");
        let mut want = lens::correct(&source, &corrections[0]).0;
        dehaze::dehaze(&mut want, &dehazing);
        sharpen::sharpen(&mut want, &options, None, CLIP);
        assert!(
            max_diff(&got, &want) < 1e-5,
            "the CLI dehazes after the lens and before the sharpen: {}",
            max_diff(&got, &want)
        );
        // And it did something: the veil is thinner than it was.
        let mut lens_only = lens::correct(&source, &corrections[0]).0;
        sharpen::sharpen(&mut lens_only, &options, None, CLIP);
        assert!(max_diff(&got, &lens_only) > 1e-2, "the dehaze did nothing");
    }

    /// And with no lens correction asked for, the sharpen still runs:
    /// moving it out of the engine's settings must not lose it.
    #[test]
    fn the_sharpen_runs_with_no_lens_correction() {
        const CLIP: f32 = 10.0;
        let source = test_frame(96, 96);
        let edit = greycard_edit::Lens {
            profile: false,
            ..Default::default()
        };
        assert!(edit.corrections(None).is_empty());
        let options = sharpen::SharpenOptions {
            radius: sharpen::Radius::Fixed(1.0),
            iterations: sharpen::DEFAULT_ITERATIONS,
            contrast: sharpen::Threshold::Fixed(0.0),
            stop_early: true,
        };
        let mut got = source.clone();
        let shot = Shot::default();
        correct_and_sharpen(
            &mut got,
            ("Greycard", "Test", &shot),
            &edit,
            None,
            None,
            None,
            None,
            Some(options),
            None,
            CLIP,
        )
        .unwrap()
        .expect("the sharpen ran");
        let mut want = source.clone();
        sharpen::sharpen(&mut want, &options, None, CLIP);
        assert_eq!(got.data, want.data);
        assert!(max_diff(&got, &source) > 1e-3, "the sharpen did nothing");
    }

    /// `run_develop_picture` used to take no develop settings at all,
    /// so `--sharpen` on a JPEG, PNG or TIFF did nothing (notes §88).
    /// This pins the fix on the picture path itself, the way the two
    /// tests above pin it on the raw path: the lens correction first,
    /// the capture sharpening on what it made, at the picture's own
    /// clip level and no measured radius, since a picture has no
    /// mosaic to measure one from.
    #[test]
    fn the_picture_path_sharpens_after_the_lens_correction() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-cli-picture-sharpen-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.png");

        // A checkerboard for the sharpen to bite on and for a
        // resample to have something to move.
        let (w, h) = (96u32, 96u32);
        let mut picture = image::RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let block = if (x / 7 + y / 5) % 2 == 0 {
                    200u8
                } else {
                    60u8
                };
                picture.put_pixel(x, y, image::Rgb([block, block, block]));
            }
        }
        picture.save(&input).expect("writing the test picture");

        let lens = greycard_edit::Lens {
            profile: false,
            manual: -0.12,
            auto_scale: false,
            scale: 1.0,
            ..Default::default()
        };
        assert!(
            !lens.wants_profile(),
            "the test must not need the lens database"
        );
        let corrections = lens.corrections(None);
        assert_eq!(corrections.len(), 1);
        assert!(corrections[0].moves(), "the test wants pixels moved");
        let options = sharpen::SharpenOptions {
            radius: sharpen::Radius::Fixed(1.0),
            iterations: sharpen::DEFAULT_ITERATIONS,
            contrast: sharpen::Threshold::Fixed(0.0),
            stop_early: true,
        };

        let sharpened = dir.join("sharpened.tiff");
        let outputs = Outputs {
            dng: None,
            compression: Compression::Lossless,
            tiff: Some(sharpened.as_path()),
            preview: None,
            dehaze_map: None,
            on_exists: OnExists::Overwrite,
        };
        run_develop_picture(&input, &outputs, &lens, None, None, Some(options), 0.0)
            .expect("develop the picture");

        // What the picture path should have made: decode, correct the
        // lens, then sharpen at the picture's own clip level with no
        // measured radius. Same decode the CLI itself runs, so the
        // color transform is not reimplemented here.
        let p = greycard_core::picture::decode_picture_path(&input).expect("decode the picture");
        let mut want = lens::correct(&p.image, &corrections[0]).0;
        sharpen::sharpen(&mut want, &options, None, p.clip_level());
        let want_u16: Vec<u16> = want
            .data
            .iter()
            .map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16)
            .collect();
        let got = image::open(&sharpened)
            .expect("reading the sharpened tiff")
            .into_rgb16();
        assert_eq!(
            got.as_raw(),
            &want_u16,
            "sharpen(lens(x)) on the picture path"
        );

        // lens(sharpen(x)): the order the picture path must not use,
        // the same guard the raw-path test above pins itself with, so
        // this test proves the order rather than only the equality.
        let mut wrong = p.image.clone();
        sharpen::sharpen(&mut wrong, &options, None, p.clip_level());
        let wrong = lens::correct(&wrong, &corrections[0]).0;
        assert!(
            max_diff(&want, &wrong) > 1e-3,
            "the two orders must differ, else the test pins nothing: {}",
            max_diff(&want, &wrong)
        );

        // With --sharpen off, the picture path must still correct the
        // lens and must differ from the sharpened output, else the
        // test above pins nothing.
        let unsharpened = dir.join("unsharpened.tiff");
        let outputs = Outputs {
            dng: None,
            compression: Compression::Lossless,
            tiff: Some(unsharpened.as_path()),
            preview: None,
            dehaze_map: None,
            on_exists: OnExists::Overwrite,
        };
        run_develop_picture(&input, &outputs, &lens, None, None, None, 0.0)
            .expect("develop the picture");
        let got_unsharpened = image::open(&unsharpened)
            .expect("reading the unsharpened tiff")
            .into_rgb16();
        assert_ne!(
            got.as_raw(),
            got_unsharpened.as_raw(),
            "--sharpen must change a picture's output"
        );
        let want_lens_only = lens::correct(&p.image, &corrections[0]).0;
        let want_lens_only_u16: Vec<u16> = want_lens_only
            .data
            .iter()
            .map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16)
            .collect();
        assert_eq!(
            got_unsharpened.as_raw(),
            &want_lens_only_u16,
            "with no --sharpen the picture path still corrects the lens"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_mistyped_denoiser_tier_names_the_tiers() {
        let complaint = denoiser_for("turbo").unwrap_err().to_string();
        assert!(complaint.contains("fast, balanced, best"), "{complaint}");
        assert!(complaint.contains("turbo"), "{complaint}");
        // The way out of a missing tier is the CLI's own, not a panel.
        let store = greycard_ai::Store::at(std::env::temp_dir().join("greycard-no-models"));
        let missing = missing_tier_complaint(&store, "fast");
        assert!(
            missing.contains("greycard models --fetch fast"),
            "{missing}"
        );
        assert!(!missing.contains("panel"), "{missing}");
        // A file that is there is taken as a model of its own.
        let file = std::env::temp_dir().join(format!("greycard-cli-{}.onnx", std::process::id()));
        std::fs::write(&file, b"stands in for a model").unwrap();
        let (path, model) = denoiser_for(file.to_str().unwrap()).unwrap();
        assert_eq!(path, file);
        assert!(model.is_none());
        std::fs::remove_file(&file).unwrap();
    }

    /// `--fetch` takes a tier, an id or `all`, and says so when it is
    /// given none of them.
    #[test]
    fn the_fetch_argument_takes_a_tier_an_id_or_all() {
        let ids = |what| {
            models_named(what)
                .unwrap()
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("all").len(), greycard_ai::MODELS.len());
        assert_eq!(ids("fast"), vec![greycard_ai::DENOISE_FAST.id]);
        assert_eq!(
            ids(greycard_ai::SUBJECT.id),
            vec![greycard_ai::SUBJECT.id],
            "a model with no tier is reachable by its id"
        );
        let complaint = models_named("everything").unwrap_err().to_string();
        assert!(complaint.contains("everything"), "{complaint}");
        assert!(complaint.contains("fast, balanced, best"), "{complaint}");
        assert!(complaint.contains(greycard_ai::SUBJECT.id), "{complaint}");
    }

    /// The listing says, for every model, what it is for, what it
    /// weighs, its license, and whether the store has it.
    #[test]
    fn the_listing_covers_every_model() {
        let store = greycard_ai::Store::at(
            std::env::temp_dir().join(format!("greycard-models-{}", std::process::id())),
        );
        let width = greycard_ai::MODELS
            .iter()
            .map(|m| m.id.len())
            .max()
            .unwrap();
        for model in greycard_ai::MODELS {
            let [first, second] = model_lines(model, &store, width);
            assert!(first.starts_with(model.id), "{first}");
            assert!(first.contains(model.purpose), "{first}");
            assert!(second.contains(model.license.name), "{second}");
            assert!(
                second.contains(&format!("{} MB", megabytes(model.bytes()))),
                "{second}"
            );
            assert!(second.contains("not fetched"), "{second}");
            match greycard_ai::tier_of(model) {
                Some(tier) => assert!(second.contains(&format!("tier {tier}")), "{second}"),
                None => assert!(!second.contains("tier "), "{second}"),
            }
        }
        assert_eq!(megabytes(greycard_ai::DENOISE_FAST.bytes()), 5);
    }
}

// ---------------------------------------------------------------------
// Focus stacking. Its own block: `greycard stack` develops every frame
// the short way, merges them in camera space, and writes the merge as
// a linear DNG beside the sources.
// ---------------------------------------------------------------------

/// `--reference`, as the command line spells it.
fn stack_reference(name: &str) -> Result<Reference, String> {
    match name.trim() {
        "middle" => Ok(Reference::Middle),
        "sharpest" => Ok(Reference::Sharpest),
        other => other.parse::<usize>().map(Reference::Index).map_err(|_| {
            format!("want middle, sharpest or a frame number counting from 0, not {other}")
        }),
    }
}

/// What a merge is written as, from the name asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StackFormat {
    /// A linear DNG: camera-space samples with the reference frame's
    /// color tags and EXIF, which is what the merge already is.
    Dng,
    /// A 16-bit linear Rec.2020 TIFF, for a consumer that wants the
    /// picture rather than a file to develop.
    Tiff,
}

fn stack_format(path: &std::path::Path) -> Result<StackFormat> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("dng") => Ok(StackFormat::Dng),
        Some("tif" | "tiff") => Ok(StackFormat::Tiff),
        _ => anyhow::bail!(
            "a merged stack is written as a linear DNG (.dng) or a 16-bit linear TIFF \
             (.tif); {} is neither",
            path.display()
        ),
    }
}

/// `<first frame> stack.dng`, beside the first frame.
fn stack_default_output(first: &std::path::Path) -> PathBuf {
    let stem = first
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "stack".into());
    let name = format!("{stem} stack.dng");
    match first.parent() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// Merge a focus stack and write it beside the sources.
///
/// The frames go through the same short develop `register` uses —
/// bilinear, hot pixel repair on, no chromatic aberration correction,
/// highlights clipped rather than reconstructed (§119) — but stop one
/// step earlier, at [`demosaic`], which leaves camera-space samples
/// with no white balance and no matrix on them. That is the space the
/// merge belongs in and the space a linear DNG stores, so the result
/// is a file every editor treats as it would the originals, which is
/// what §71 asked the output of every stack to be. A merge taken all
/// the way to the working space could not be written as one: the DNG's
/// `ColorMatrix` and `AsShotNeutral` describe samples the matrix has
/// not been applied to, and a reader would apply it a second time.
///
/// The color tags and the EXIF come from the frame the merge was
/// registered onto, since the result is in its geometry and at its
/// white balance.
#[allow(clippy::too_many_arguments)]
fn stack_files(
    files: &[PathBuf],
    output: Option<&std::path::Path>,
    reference: Reference,
    selectivity: f32,
    max_residual: f32,
    uncompressed: bool,
    on_exists: OnExists,
) -> Result<()> {
    if files.len() < 2 {
        anyhow::bail!("a stack wants at least two frames; got {}", files.len());
    }
    // What is already there decides the name before anything is
    // decoded, so a run that would write nothing does nothing.
    let wanted = match output {
        Some(path) => path.to_path_buf(),
        None => stack_default_output(&files[0]),
    };
    let format = stack_format(&wanted)?;
    let resolved = on_exists.resolve(&wanted);
    if let Some(note) = resolved.note() {
        tracing::warn!("{note}");
    }
    let Some(out_path) = resolved.path().map(std::path::Path::to_path_buf) else {
        return Ok(());
    };

    let settings = DevelopSettings {
        demosaic: DemosaicMethod::Bilinear,
        highlights: HighlightMode::Clip,
        chromatic_aberration: None,
        hot_pixels: Some(HotPixelOptions {
            sigmas: DEFAULT_SIGMAS,
            ratio: DEFAULT_RATIO,
        }),
        ..Default::default()
    };

    let developing = std::time::Instant::now();
    let mut sources = Vec::with_capacity(files.len());
    let mut images = Vec::with_capacity(files.len());
    for file in files {
        if greycard_core::picture::is_picture_path(file) {
            anyhow::bail!(
                "{} is not a RAW file: a stack is merged in camera space, before white \
                 balance and the matrix, and a developed picture is past both",
                file.display()
            );
        }
        let (frame, metadata) = RawlerDecoder
            .decode_path_with_metadata(file)
            .with_context(|| format!("decoding {}", file.display()))?;
        let demosaiced = demosaic(&frame, &settings)
            .with_context(|| format!("developing {}", file.display()))?;
        images.push(demosaiced.image);
        sources.push((frame, metadata, demosaiced.white_balance));
    }
    let developing = developing.elapsed();

    let options = stack::Options {
        reference,
        selectivity,
        max_residual,
        ..Default::default()
    };
    let merging = std::time::Instant::now();
    let (merged, report) = stack::stack_camera(&images, &options)?;
    let merging = merging.elapsed();
    drop(images);

    let name = |i: usize| {
        files[i]
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| files[i].display().to_string())
    };
    println!(
        "frames        {} at {}x{}, developed in {:.1} s",
        files.len(),
        report.width,
        report.height,
        developing.as_secs_f64()
    );
    println!(
        "reference     frame {} ({})",
        report.reference,
        name(report.reference)
    );
    println!(
        "merge         {:.1} s, {} pyramid levels, {} of {} frames used",
        merging.as_secs_f64(),
        report.levels,
        report.used(),
        files.len()
    );
    println!(
        "coverage      every frame reached {:.1}% of the picture",
        report.full_coverage_fraction() * 100.0
    );
    let (mx, my) = (
        (report.width - 1) as f64 / 2.0,
        (report.height - 1) as f64 / 2.0,
    );
    for f in &report.frames {
        print!(
            "  {:>3}  {:<24}  sharpness {:.4}",
            f.index,
            name(f.index),
            f.sharpness
        );
        match (&f.against, &f.fit) {
            (Some(against), Some(fit)) => {
                // A dropped frame has no transform onto the reference —
                // it never got one — so what is worth showing is the
                // link that was refused, and it has to say so.
                let (shown, what) = match f.dropped {
                    None => (f.transform, "the middle moves"),
                    Some(_) => (fit.transform, "the refused link moves the middle"),
                };
                let (dx, dy) = shown.displacement(mx, my);
                print!(
                    "  onto {against}, residual {:.4} over {:.1}%; \
                     {what} {dx:+.2}, {dy:+.2} px, scale {:.6}, rotation {:.4}\u{00b0}",
                    fit.residual,
                    fit.overlap * 100.0,
                    shown.scale(),
                    shown.rotation().to_degrees(),
                );
            }
            _ => print!("  the reference"),
        }
        match &f.dropped {
            Some(why) => println!("\n       DROPPED: {why}"),
            None => println!(),
        }
    }
    if report.used() < files.len() {
        println!(
            "left out      {} frame(s); the merge is of the rest",
            files.len() - report.used()
        );
    }

    let (frame, metadata, balance) = &sources[report.reference];
    let source_name = files[report.reference]
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    write_stack(
        merged,
        frame,
        metadata,
        balance,
        format,
        &out_path,
        source_name.as_deref(),
        uncompressed,
    )?;
    println!("wrote         {}", out_path.display());
    Ok(())
}

/// The merge taken into the working space: the white balance gains
/// first, then the camera matrix.
///
/// The gains are the half that is easy to forget. [`demosaic`] applies
/// them before the demosaic and divides them back out afterwards,
/// because a linear DNG stores camera-native samples and says what
/// neutralizes them in `AsShotNeutral` rather than baking it in. So
/// the merge, which is those samples, still has no white balance on
/// it, and the matrix alone gives a green picture — the matrix maps
/// *balanced* camera RGB into the working space. `develop` does gains
/// then matrix; so does this.
fn stack_to_working(
    merged: CameraImage,
    balance: &greycard_core::WhiteBalance,
) -> Result<WorkingImage> {
    let mut working = WorkingImage::from_data(merged.width, merged.height, merged.data)?;
    greycard_core::develop::apply_gains_rgb(&mut working.data, balance.coefficients_f32());
    greycard_core::develop::apply_matrix(&mut working.data, balance.matrix_f32());
    Ok(working)
}

/// A small sRGB rendering of the merge for the DNG's embedded preview,
/// in sensor orientation as the merge is, which is the orientation a
/// DNG's preview wants.
///
/// The samples are averaged down to at most `max_width` *before* the
/// color, so a 45-megapixel merge never allocates a second
/// full-resolution float image and a full-resolution 8-bit one on top
/// of it just to end up 1024 pixels wide.
fn stack_preview(
    merged: &CameraImage,
    balance: &greycard_core::WhiteBalance,
    max_width: usize,
) -> Result<image::RgbImage> {
    let step = merged.width.div_ceil(max_width.max(1)).max(1);
    let (w, h) = (merged.width.div_ceil(step), merged.height.div_ceil(step));
    let mut small = Vec::with_capacity(w * h * CameraImage::CHANNELS);
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0.0f64; CameraImage::CHANNELS];
            let mut n = 0u32;
            for sy in y * step..((y + 1) * step).min(merged.height) {
                for sx in x * step..((x + 1) * step).min(merged.width) {
                    let px = merged.pixel(sx, sy);
                    for (a, v) in sum.iter_mut().zip(px) {
                        *a += v as f64;
                    }
                    n += 1;
                }
            }
            let n = n.max(1) as f64;
            small.extend(sum.iter().map(|v| (v / n) as f32));
        }
    }
    let small = CameraImage::from_data(w, h, small)?;
    srgb_preview(&stack_to_working(small, balance)?, 0.0)
}

/// Write a merged stack, as a linear DNG or as a 16-bit linear TIFF.
///
/// Split out of [`stack_files`] so that a test can drive it without a
/// RAW file on disk, since what it does — the gains, the matrix, the
/// tags — is the part with something to get wrong.
#[allow(clippy::too_many_arguments)]
fn write_stack(
    merged: CameraImage,
    frame: &RawFrame,
    metadata: &greycard_core::decode::rawler_backend::RawMetadata,
    balance: &greycard_core::WhiteBalance,
    format: StackFormat,
    out_path: &std::path::Path,
    source_name: Option<&str>,
    uncompressed: bool,
) -> Result<()> {
    match format {
        StackFormat::Dng => {
            let preview = stack_preview(&merged, balance, 1024)?;
            let options = LinearDngOptions {
                compression: if uncompressed {
                    Compression::Uncompressed
                } else {
                    Compression::Lossless
                },
                metadata: Some(metadata),
                preview: Some(&preview),
                software: Some(SOFTWARE),
                headroom: None,
            };
            // Nothing is created until there is something to write
            // into it: a run that fails at the color must not leave an
            // empty file where the merge was asked for.
            let file = std::fs::File::create(out_path)
                .with_context(|| format!("creating {}", out_path.display()))?;
            let mut writer = std::io::BufWriter::new(file);
            write_linear_dng(
                &mut writer,
                frame,
                &merged,
                balance.coefficients_f32(),
                &options,
            )
            .with_context(|| format!("writing {}", out_path.display()))?;
            std::io::Write::flush(&mut writer)
                .with_context(|| format!("writing {}", out_path.display()))?;
        }
        StackFormat::Tiff => {
            let working = stack_to_working(merged, balance)?;
            let working = greycard_core::develop::orient(working, frame.orientation);
            let (w, h) = (working.width as u32, working.height as u32);
            let provenance = greycard_core::exif::Provenance {
                metadata,
                software: SOFTWARE,
                width: w,
                height: h,
                srgb: false,
                written: Some(std::time::SystemTime::now()),
                source_name,
                output: None,
                edit: None,
            };
            write_linear_tiff(&working, out_path, &provenance)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod stack_tests {
    use super::*;

    #[test]
    fn the_output_name_says_the_format() {
        assert_eq!(
            stack_format(std::path::Path::new("a/b.dng")).expect("a dng"),
            StackFormat::Dng
        );
        assert_eq!(
            stack_format(std::path::Path::new("a/b.TIFF")).expect("a tiff"),
            StackFormat::Tiff
        );
        assert!(stack_format(std::path::Path::new("a/b.jpg")).is_err());
        assert!(stack_format(std::path::Path::new("a/b")).is_err());
    }

    #[test]
    fn without_a_name_the_merge_goes_beside_the_first_frame() {
        assert_eq!(
            stack_default_output(std::path::Path::new("/photos/DSC_0001.NEF")),
            PathBuf::from("/photos/DSC_0001 stack.dng")
        );
        assert_eq!(
            stack_default_output(std::path::Path::new("DSC_0001.NEF")),
            PathBuf::from("DSC_0001 stack.dng")
        );
    }

    #[test]
    fn the_reference_is_named_or_numbered() {
        assert_eq!(stack_reference("middle"), Ok(Reference::Middle));
        assert_eq!(stack_reference(" sharpest "), Ok(Reference::Sharpest));
        assert_eq!(stack_reference("4"), Ok(Reference::Index(4)));
        assert!(stack_reference("last").is_err());
    }

    /// A textured RGGB frame whose camera is Rec.2020 itself, so the
    /// matrix is near enough the identity and a channel that comes
    /// back wrong has nowhere to hide. The as-shot gains are far from
    /// one in red and blue, which is what makes a merge written
    /// without them unmistakably green.
    fn textured_frame(width: usize, height: usize) -> RawFrame {
        use greycard_core::raw::{
            Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Samples,
        };
        let pattern = CfaPattern::rggb();
        // A hash lattice, smoothed: structure at a few scales and no
        // period, so the registration between two frames of it has
        // something to hold and nothing to confuse it with.
        let lattice = |ix: i64, iy: i64| {
            let mut h = (ix as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add((iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
            h ^= h >> 29;
            h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h ^= h >> 32;
            (h >> 11) as f64 / (1u64 << 53) as f64
        };
        let value = |x: f64, y: f64| {
            let mut v = 0.0;
            let mut amplitude = 0.5;
            let mut cell = 32.0;
            for _ in 0..3 {
                let (gx, gy) = (x / cell, y / cell);
                let (i, j) = (gx.floor(), gy.floor());
                let (fx, fy) = (gx - i, gy - j);
                let ease = |t: f64| t * t * (3.0 - 2.0 * t);
                let (sx, sy) = (ease(fx), ease(fy));
                let (i, j) = (i as i64, j as i64);
                let a = lattice(i, j) + sx * (lattice(i + 1, j) - lattice(i, j));
                let b = lattice(i, j + 1) + sx * (lattice(i + 1, j + 1) - lattice(i, j + 1));
                v += amplitude * (a + sy * (b - a) - 0.5);
                amplitude *= 0.7;
                cell /= 2.0;
            }
            v
        };
        // Black 100, white 1100: a thousand counts of range. The three
        // channels sit at different levels so a swap or a missing gain
        // shows up in the means.
        let (black, range) = (100.0f64, 1000.0f64);
        let mut samples = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let base = match pattern.color_at(y, x) {
                    CfaColor::Red => 0.30,
                    CfaColor::Green => 0.55,
                    CfaColor::Blue => 0.18,
                    CfaColor::Other(_) => unreachable!(),
                };
                let v = (base + 0.10 * value(x as f64, y as f64)).clamp(0.02, 0.95);
                samples.push((black + v * range).round() as u16);
            }
        }
        let matrix = {
            let m = WORKING_SPACE.from_xyz_matrix().expect("Rec.2020 from XYZ");
            let mut flat = Vec::with_capacity(9);
            for row in m.rows {
                for v in row {
                    flat.push(v as f32);
                }
            }
            flat
        };
        RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            width,
            height,
            channels: 1,
            layout: greycard_core::raw::SensorLayout::Cfa(pattern),
            samples: Samples::U16(samples),
            levels: Levels {
                black: LevelPattern::uniform(black as f32, 1),
                white: LevelPattern::uniform((black + range) as f32, 1),
            },
            as_shot_coefficients: Some([2.0, 1.0, 4.0]),
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix: matrix,
                forward_matrix: None,
            }],
            crop: None,
            orientation: Orientation::Normal,
            shot: Default::default(),
        }
    }

    fn channel_means(data: impl Iterator<Item = f32>) -> [f64; 3] {
        let mut sum = [0.0f64; 3];
        let mut n = 0u64;
        for (i, v) in data.enumerate() {
            sum[i % 3] += v as f64;
            if i % 3 == 0 {
                n += 1;
            }
        }
        sum.map(|s| s / n.max(1) as f64)
    }

    /// The merge is camera-native samples with the white balance
    /// divided back out, so everything that turns it into a picture
    /// has to put the gains back on before the matrix. This drives the
    /// whole write path — `stack_camera`, the DNG, the TIFF and the
    /// embedded preview — and compares what comes back out of each
    /// against a plain `develop` of the same frame.
    ///
    /// It is the test that would have caught the bug it was written
    /// for: without the gains the preview's channel means came out
    /// R 0.20 G 0.40 B 0.19 against a develop's 0.36 / 0.36 / 0.35, a
    /// green picture that nothing else in the suite looked at.
    #[test]
    fn the_written_stack_carries_the_white_balance() {
        let dir = std::env::temp_dir().join(format!("greycard-stack-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let frame = textured_frame(96, 72);
        let settings = DevelopSettings {
            demosaic: DemosaicMethod::Bilinear,
            highlights: HighlightMode::Clip,
            chromatic_aberration: None,
            ..Default::default()
        };
        // What the merge should look like once it is a picture again.
        let want = develop(&frame, &settings).expect("a develop");
        let want_means = channel_means(want.image.data.iter().copied());

        // Two copies of one frame: the merge is that frame, so
        // anything the write path does to it is the only difference.
        let demosaiced = demosaic(&frame, &settings).expect("a demosaic");
        let (merged, report) = stack::stack_camera(
            &[demosaiced.image.clone(), demosaiced.image.clone()],
            &stack::Options::default(),
        )
        .expect("a camera stack");
        assert_eq!(report.used(), 2, "{:#?}", report.frames);
        let camera_means = channel_means(merged.data.iter().copied());
        let straight = channel_means(demosaiced.image.data.iter().copied());
        for (a, b) in camera_means.iter().zip(&straight) {
            assert!(
                (a - b).abs() < 1e-3,
                "the merge of one frame twice is not that frame: {camera_means:?} against \
                 {straight:?}"
            );
        }

        let balance = &demosaiced.white_balance;
        let metadata = greycard_core::decode::rawler_backend::RawMetadata::default();

        // The DNG: written, decoded again, developed, and its means
        // compared with the develop of the original.
        let dng = dir.join("stack.dng");
        write_stack(
            merged.clone(),
            &frame,
            &metadata,
            balance,
            StackFormat::Dng,
            &dng,
            None,
            true,
        )
        .expect("writing the dng");
        let back = decode::decode_path(&dng).expect("decoding the dng");
        let developed = develop(&back, &settings).expect("developing the dng");
        let dng_means = channel_means(developed.image.data.iter().copied());
        println!("develop {want_means:?}\n    dng {dng_means:?}");
        for (a, b) in dng_means.iter().zip(&want_means) {
            assert!(
                (a - b).abs() < 0.01,
                "the DNG develops to {dng_means:?}, the source to {want_means:?}"
            );
        }

        // The TIFF: the same picture, written straight.
        let tiff = dir.join("stack.tif");
        write_stack(
            merged.clone(),
            &frame,
            &metadata,
            balance,
            StackFormat::Tiff,
            &tiff,
            None,
            false,
        )
        .expect("writing the tiff");
        let got = image::open(&tiff).expect("reading the tiff").into_rgb16();
        let tiff_means = channel_means(got.as_raw().iter().map(|v| *v as f32 / 65535.0));
        println!("   tiff {tiff_means:?}");
        for (a, b) in tiff_means.iter().zip(&want_means) {
            assert!(
                (a - b).abs() < 0.01,
                "the TIFF holds {tiff_means:?}, the source develops to {want_means:?}"
            );
        }

        // And the embedded preview, which is its own path into the
        // working space and was the one that was wrong.
        let preview = stack_preview(&merged, balance, 32).expect("a preview");
        assert!(preview.width() <= 32 && preview.width() > 8, "{preview:?}");
        let preview_means = channel_means(preview.as_raw().iter().map(|v| {
            let v = *v as f32 / 255.0;
            // Undo the sRGB curve the preview writes.
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }));
        println!("preview {preview_means:?}");
        for (a, b) in preview_means.iter().zip(&want_means) {
            assert!(
                (a - b).abs() < 0.03,
                "the preview renders {preview_means:?}, the source develops to {want_means:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A format the merge is not written in is refused before anything
    /// is created, so a bad name cannot truncate a file that was there.
    #[test]
    fn a_refused_format_creates_nothing() {
        let dir = std::env::temp_dir().join(format!("greycard-stack-fmt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let there = dir.join("out.jpg");
        std::fs::write(&there, b"not to be touched").expect("a file to leave alone");
        let files = [dir.join("a.cr3"), dir.join("b.cr3")];
        let e = stack_files(
            &files,
            Some(&there),
            Reference::Middle,
            2.0,
            0.6,
            false,
            OnExists::Overwrite,
        )
        .expect_err("a jpg");
        assert!(e.to_string().contains("neither"), "{e}");
        assert_eq!(
            std::fs::read(&there).expect("the file is still there"),
            b"not to be touched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_frame_is_not_a_stack() {
        let one = [PathBuf::from("a.cr3")];
        let e = stack_files(
            &one,
            Some(std::path::Path::new("out.dng")),
            Reference::Middle,
            2.0,
            0.6,
            false,
            OnExists::Skip,
        )
        .expect_err("one frame");
        assert!(e.to_string().contains("at least two"), "{e}");
    }
}
