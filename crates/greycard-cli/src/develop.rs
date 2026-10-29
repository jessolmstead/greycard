//! `greycard develop`: a RAW, or a picture, developed to a linear DNG,
//! a linear Rec.2020 TIFF or an sRGB preview.

use std::path::PathBuf;

use anyhow::{Context, Result};
use greycard_core::decode::RawlerDecoder;
use greycard_core::develop::defringe;
use greycard_core::develop::dehaze::{self, DehazeOptions};
use greycard_core::develop::denoise::{
    DEFAULT_CHROMA, DEFAULT_HYBRID_FROM, DenoiseMethod, DenoiseOptions, Strength,
};
use greycard_core::develop::dual::{DualContrast, ThresholdSource};
use greycard_core::develop::hotpixels::{
    DEFAULT_OTHERS, DEFAULT_RATIO, DEFAULT_SIGMAS, HotPixelOptions,
};
use greycard_core::develop::local_contrast::{LocalContrastOptions, local_contrast};
use greycard_core::develop::nlm::{DEFAULT_CENTER_WEIGHT, DEFAULT_SEARCH, NlmOptions};
use greycard_core::develop::noise::MID_GREY;
use greycard_core::develop::segments::SegmentOptions;
use greycard_core::develop::sharpen;
use greycard_core::develop::{finish, prepare};
use greycard_core::dng::{Compression, LinearDngOptions, write_linear_dng};
use greycard_core::output::{OnExists, Resolved};
use greycard_core::raw::{Orientation, SensorLayout};
use greycard_core::{
    DemosaicMethod, DevelopSettings, HighlightMode, RawFrame, TempTint, WhitePoint, WorkingImage,
    demosaic, develop,
};

use crate::common::{
    SOFTWARE, denoiser_tiers, megabytes, on_exists_named, srgb_preview, write_linear_tiff,
};

// The `develop` subcommand's flags. Its help is the doc comment on
// `Command::Develop` in main.rs, so this struct carries none of its own.
#[derive(clap::Args)]
pub(crate) struct DevelopArgs {
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
    /// beside it under the first free "-1" name, write over it,
    /// or leave it and write nothing
    #[arg(long, value_name = "WHAT", default_value = "increment",
          value_parser = on_exists_named)]
    on_exists: OnExists,
    /// Scene white as a color temperature in Kelvin (default: as shot)
    #[arg(long)]
    temp: Option<f64>,
    /// Tint as Duv, positive toward green; only used with --temp
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    tint: f64,
    /// Exposure adjustment in stops, preview only
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    exposure: f64,
    /// Demosaic algorithm: amaze-vng4, rcd-vng4, amaze, rcd, vng4 or
    /// bilinear for a Bayer raw; markesteijn-3, markesteijn-1 or bilinear
    /// for X-Trans. Left out, the raw's mosaic's default (amaze-vng4,
    /// markesteijn-3); one the mosaic does not take is refused
    #[arg(long)]
    demosaic: Option<DemosaicMethod>,
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
    /// Leave hot and dead photosites as shot. The repair is on by
    /// default: it takes a photosite far out from its own color's
    /// neighbors only when the other colors round it sit at their
    /// background, which a glint, a star or a catchlight never does
    #[arg(long)]
    no_hot_pixels: bool,
    /// The old switch, from when the repair was off by default: accepted
    /// and ignored, with a warning, so scripts that pass it still run
    #[arg(long, hide = true)]
    hot_pixels: bool,
    /// How far out a photosite must be, in standard deviations of the
    /// measured noise beyond its brightest or darkest same-color neighbor
    #[arg(long, default_value_t = DEFAULT_SIGMAS)]
    hot_sigmas: f32,
    /// The factor a hot photosite must exceed its brightest same-color
    /// neighbor by
    #[arg(long, default_value_t = DEFAULT_RATIO)]
    hot_ratio: f32,
    /// The factor the other colors' adjacent photosites may stand above
    /// their own background with the site still a defect ("inf" leaves
    /// the test out)
    #[arg(long, default_value_t = DEFAULT_OTHERS)]
    hot_others: f32,
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
}

/// The `develop` subcommand: its flags made into the engine's
/// settings and the lens's, then the develop.
pub(crate) fn run(args: DevelopArgs) -> Result<()> {
    let DevelopArgs {
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
        no_hot_pixels,
        hot_pixels,
        hot_sigmas,
        hot_ratio,
        hot_others,
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
    } = args;
    if dng.is_none() && output.is_none() && preview.is_none() {
        anyhow::bail!("nothing to write: pass --dng, --output and/or --preview");
    }
    if hot_pixels {
        tracing::warn!(
            "--hot-pixels does nothing now: the repair is on by default, and \
             --no-hot-pixels turns it off"
        );
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
        // Settled per file once its mosaic is known
        // (`demosaic_for`).
        demosaic: demosaic.unwrap_or_default(),
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
        hot_pixels: (!no_hot_pixels).then_some(HotPixelOptions {
            sigmas: hot_sigmas,
            ratio: hot_ratio,
            others: hot_others,
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
    // The model named, and then whether anything can run it:
    // before the frame, which a machine without ONNX Runtime
    // would otherwise decode and prepare only to stop at the
    // load.
    if ai_denoise.is_some()
        && let Err(why) = greycard_ai::runtime::status()
    {
        anyhow::bail!("--ai-denoise: {why}");
    }
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
        demosaic,
        exposure,
        ai_denoise.as_ref(),
        &lens,
        detail,
    )
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
    // Before the model is loaded or the frame prepared: the network
    // takes a 2x2 Bayer mosaic and nothing else.
    if let SensorLayout::Cfa(p) = &frame.layout
        && !greycard_core::develop::rcd::is_bayer(p)
    {
        return Err(greycard_core::error::Error::Unsupported(format!(
            "the learned denoiser takes a 2x2 Bayer mosaic, not a {p} one"
        ))
        .into());
    }
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
        denoiser.provider().label(),
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
    let tiling = denoiser.tiling();
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
    (make, model, shot, raw): (&str, &str, &greycard_core::raw::Shot, bool),
    lens: &greycard_edit::Lens,
    db: Option<&greycard_lens::Database>,
) -> Result<()> {
    let profile = db.and_then(|db| {
        let l = db.lookup_shot(make, model, shot, raw);
        match (l.lens, l.camera) {
            (Some(lens), camera) => {
                tracing::info!(
                    "lens profile {}{}",
                    lens.name(),
                    if l.from_body {
                        ", the fixed lens, taken from the body"
                    } else {
                        ""
                    }
                );
                let p = greycard_lens::Profile {
                    lens,
                    camera,
                    from_body: l.from_body,
                };
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
                Some(greycard_lens::Profile {
                    lens,
                    camera,
                    from_body: l.from_body,
                })
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
    identity: (&str, &str, &greycard_core::raw::Shot, bool),
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

/// The demosaic for a raw of `layout`: the one asked when its mosaic
/// takes it, refused in words when it does not; the mosaic's default
/// when none was asked.
fn demosaic_for(layout: &SensorLayout, asked: Option<DemosaicMethod>) -> Result<DemosaicMethod> {
    let SensorLayout::Cfa(pattern) = layout else {
        return Ok(asked.unwrap_or_default());
    };
    match asked {
        None => Ok(DemosaicMethod::default_for(pattern)),
        Some(m) if m.takes(pattern) => Ok(m),
        Some(m) => {
            let takes: Vec<&str> = DemosaicMethod::for_pattern(pattern)
                .iter()
                .map(|m| m.name())
                .collect();
            anyhow::bail!(
                "--demosaic {} is not a demosaic for this raw's {pattern} mosaic; it takes {}",
                m.name(),
                takes.join(", ")
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_develop(
    file: &std::path::Path,
    outputs: &Outputs<'_>,
    settings: &DevelopSettings,
    demosaic_asked: Option<DemosaicMethod>,
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
        demosaic: demosaic_for(&frame.layout, demosaic_asked)?,
        ..settings.clone()
    };
    let mut developed = match ai_denoise {
        Some(model) => develop_learned(&frame, settings, model),
        None => develop(&frame, settings).map_err(anyhow::Error::from),
    }
    .with_context(|| format!("developing {}", file.display()))?;
    developed.sharpen = correct_and_sharpen(
        &mut developed.image,
        (&frame.make, &frame.model, &frame.shot, true),
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
        (&p.make, &p.model, &p.shot, false),
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
    /// A negative value after a space parses for every signed flag,
    /// as `--tint=-4` always did.
    #[test]
    fn a_signed_flag_takes_a_negative_value_after_a_space() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            args: DevelopArgs,
        }
        let cli = Cli::try_parse_from([
            "greycard",
            "a.CR3",
            "--temp",
            "5000",
            "--tint",
            "-4",
            "--exposure",
            "-0.5",
        ])
        .unwrap();
        assert_eq!(cli.args.tint, -4.0);
        assert_eq!(cli.args.exposure, -0.5);
    }

    /// Left out, the demosaic is the mosaic's default; asked, it is
    /// taken where the mosaic takes it and refused where it does not.
    #[test]
    fn the_demosaic_flag_follows_the_raw_s_mosaic() {
        use greycard_core::raw::CfaPattern;
        let xtrans = SensorLayout::Cfa(CfaPattern::xtrans());
        let bayer = SensorLayout::Cfa(CfaPattern::rggb());
        assert_eq!(
            demosaic_for(&xtrans, None).unwrap(),
            DemosaicMethod::Markesteijn3
        );
        assert_eq!(
            demosaic_for(&bayer, None).unwrap(),
            DemosaicMethod::AmazeVng4
        );
        assert_eq!(
            demosaic_for(&xtrans, Some(DemosaicMethod::Markesteijn1)).unwrap(),
            DemosaicMethod::Markesteijn1
        );
        let refused = demosaic_for(&xtrans, Some(DemosaicMethod::Amaze))
            .unwrap_err()
            .to_string();
        assert_eq!(
            refused,
            "--demosaic amaze is not a demosaic for this raw's 6x6 (X-Trans) mosaic; \
             it takes markesteijn-3, markesteijn-1, bilinear"
        );
        assert!(demosaic_for(&bayer, Some(DemosaicMethod::Markesteijn3)).is_err());
    }

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
            ("Greycard", "Test", &shot, true),
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
            ("Greycard", "Test", &shot, true),
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
            ("Greycard", "Test", &shot, true),
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
}
