//! Making the base a develop starts from: the engine's, the learned
//! denoiser's, or the last one turned.

use super::*;

/// Whether a base made at one frame turn, turned by a permutation of
/// its pixels, is the base made at another, to the bit.
///
/// Everything the engine does before its orientation step reads the
/// mosaic, which a turn does not touch, and the step itself is a
/// permutation (`develop::orient`), so the engine's picture at one
/// turn is its picture at another permuted. What comes after it on
/// the turned picture is the question. The lens correction is
/// radial about the center, and the defringe's blur and its test
/// against the frame's mean deviation are symmetric, so each is the
/// same picture turned in exact arithmetic; in floats neither is,
/// since a turn changes the order of the sums and which side of a
/// half a coordinate rounds to. A frame with either on is developed
/// afresh at its new turn, as it always was.
pub(super) fn turns_exactly(edit: &Edit, lenses: Option<&greycard_lens::Database>) -> bool {
    edit.lens.is_identity(lenses.is_some()) && edit.lens.defringe().is_none()
}

/// The kept base, and the learned denoiser's pair beside it when it
/// was made at the same turn, turned to the frame's `turn`: their
/// pixels permuted, the guide plane read again off the turned
/// picture, and everything kept after the base let go, since the
/// retouch's patches and the picture before the sharpen were placed
/// on the base the other way up. A new `stamp`, so the learned masks
/// made on the old base are not taken for this one's.
pub(super) fn turn_base(
    b: &mut Base,
    learned: &mut Option<LearnedBase>,
    turn: u8,
    stamp: u64,
    gpu: Option<&greycard_gpu::Context>,
) {
    let Some(o) = quarters((turn % 4 + 4 - b.turn % 4) % 4) else {
        return;
    };
    let turned = |image: Arc<WorkingImage>| {
        let image = Arc::try_unwrap(image).unwrap_or_else(|a| (*a).clone());
        Arc::new(greycard_core::develop::orient(image, o))
    };
    // The blend at either end of its strength is one of the pair
    // itself, shared rather than copied: turned once, and shared on.
    let mut image = std::mem::replace(&mut b.image, Arc::new(WorkingImage::new(0, 0)));
    if let Some(l) = learned.as_mut().filter(|l| l.turn == b.turn) {
        let is_model = Arc::ptr_eq(&image, &l.model);
        let is_plain = Arc::ptr_eq(&image, &l.plain);
        if is_model || is_plain {
            image = Arc::new(WorkingImage::new(0, 0));
        }
        let empty = || Arc::new(WorkingImage::new(0, 0));
        l.model = turned(std::mem::replace(&mut l.model, empty()));
        l.plain = turned(std::mem::replace(&mut l.plain, empty()));
        l.turn = turn;
        if is_model {
            image = l.model.clone();
        } else if is_plain {
            image = l.plain.clone();
        } else {
            image = turned(image);
        }
    } else {
        image = turned(image);
    }
    b.image = image;
    b.guide = Arc::new(crate::finish::guide_plane(&b.image));
    b.turn = turn;
    b.stamp = stamp;
    b.patched = None;
    b.haze = None;
    b.planes = None;
    let kept = b.pre.take().is_some() | b.uploaded.take().is_some();
    if kept && let Some(ctx) = gpu {
        ctx.release();
    }
}

/// The engine's own develop of `edit`, before its sharpen, its CA
/// correction by `ca` when one is given (the GPU's).
pub(super) fn engine_base(
    frame: &RawFrame,
    edit: &Edit,
    turn: u8,
    settings: &DevelopSettings,
    stamp: u64,
    ca: Option<CaCorrector<'_>>,
) -> anyhow::Result<Base> {
    let d = develop_with(frame, settings, ca)?;
    let gave_way = d
        .demosaic
        .filter(|ran| *ran != settings.demosaic && settings.demosaic != DemosaicMethod::default())
        .map(|ran| {
            format!(
                "the demosaic: {} is not one for this raw's mosaic; {} instead",
                settings.demosaic.name(),
                ran.name()
            )
        });
    Ok(Base {
        edit: edit.clone(),
        turn,
        image: Arc::new(d.image),
        guide: Arc::new(crate::finish::Guide::NONE),
        white: WhiteBase::from(&d.white_balance, d.clip_level),
        radius: d.sharpen_radius,
        clip_level: d.clip_level,
        source: crate::finish::Source::Scene,
        stamp,
        patched: None,
        pre: None,
        uploaded: None,
        haze: None,
        planes: None,
        ca_on_gpu: false,
        stand_in: None,
        gave_way,
        profile: None,
    })
}

/// The base with the learned denoiser: its answer from `learned` when
/// that was made for the same mosaic and tier, else from the disk
/// cache, else the network run once and kept; then the blend by the
/// edit's strength. The error is why the engine's path must stand in.
#[allow(clippy::too_many_arguments)]
pub(super) fn learned_base(
    frame: &RawFrame,
    edit: &Edit,
    turn: u8,
    settings: &DevelopSettings,
    model: &'static greycard_ai::Model,
    learned: &mut Option<LearnedBase>,
    ai: &mut Ai,
    cache: Option<&greycard_ai::DenoiseCache>,
    stamp: u64,
    ca: Option<CaCorrector<'_>>,
    has_gpu: bool,
) -> Result<(Base, LearnedReport), LearnedReport> {
    let report = if learned
        .as_ref()
        .is_some_and(|l| pair_serves(l, turn, edit, has_gpu))
    {
        LearnedReport::Kept
    } else {
        let denoiser = ai.denoiser(model).map_err(|e| match e {
            NoDenoiser::Missing => LearnedReport::Missing(model),
            NoDenoiser::Failed(m) => LearnedReport::Failed(m),
        })?;
        // The network is the demosaic and the denoise both.
        let settings = DevelopSettings {
            denoise: None,
            ..settings.clone()
        };
        let (made, report) = run_learned(frame, edit, turn, &settings, model, denoiser, cache, ca)
            .map_err(|e| LearnedReport::Failed(e.to_string()))?;
        *learned = Some(made);
        report
    };
    let l = learned
        .as_ref()
        .expect("a learned base was just made or kept");
    Ok((
        Base {
            edit: edit.clone(),
            turn,
            image: blend(&l.model, &l.plain, edit.noise.learned_strength),
            guide: Arc::new(crate::finish::Guide::NONE),
            white: l.white,
            radius: l.radius,
            clip_level: l.clip_level,
            source: crate::finish::Source::Scene,
            stamp,
            patched: None,
            pre: None,
            uploaded: None,
            haze: None,
            planes: None,
            ca_on_gpu: l.ca_on_gpu,
            stand_in: None,
            gave_way: None,
            profile: None,
        },
        report,
    ))
}

/// The engine's develop with the network between `prepare` and
/// `finish`, and the plain demosaic of the same mosaic beside it. The
/// network's answer comes from the disk cache when it is there, and
/// goes into it when it is not.
#[allow(clippy::too_many_arguments)]
fn run_learned(
    frame: &RawFrame,
    edit: &Edit,
    turn: u8,
    settings: &DevelopSettings,
    model: &'static greycard_ai::Model,
    denoiser: &mut greycard_ai::Denoiser,
    cache: Option<&greycard_ai::DenoiseCache>,
    ca: Option<CaCorrector<'_>>,
) -> anyhow::Result<(LearnedBase, LearnedReport)> {
    use anyhow::Context;
    if let greycard_core::raw::SensorLayout::Cfa(p) = &frame.layout
        && !greycard_core::develop::rcd::is_bayer(p)
    {
        anyhow::bail!("the learned denoiser needs a 2x2 Bayer mosaic, not a {p} one");
    }
    let prepared = prepare_with(frame, settings, true, ca)?;
    let pattern = prepared
        .pattern
        .clone()
        .context("the learned denoiser needs a Bayer mosaic")?;
    let noise = prepared
        .noise_model()
        .context("the learned denoiser needs the frame's noise model")?;
    let tiling = greycard_ai::denoise::Tiling::default();
    let start = Instant::now();
    let key = cache.map(|c| (c, greycard_ai::DenoiseCache::key(model, tiling, &prepared)));
    let (rgb, report) = match key.and_then(|(c, k)| c.read(k, &prepared)) {
        Some(rgb) => (
            rgb,
            LearnedReport::Cached {
                seconds: start.elapsed().as_secs_f64(),
            },
        ),
        None => {
            let rgb = denoiser.run(
                &prepared.samples,
                prepared.width,
                prepared.height,
                &pattern,
                &noise,
                tiling,
            )?;
            let report = LearnedReport::Ran {
                version: denoiser.version().to_string(),
                provider: denoiser.provider().name(),
                seconds: start.elapsed().as_secs_f64(),
            };
            if let Some((c, k)) = key
                && let Err(e) = c.write(k, frame, &prepared, &rgb)
            {
                tracing::warn!("denoise cache {}: {e}", c.path(k).display());
            }
            (rgb, report)
        }
    };
    let (plain_rgb, dual, _) = demosaic_prepared(&prepared, settings)?;
    let plain = finish(prepared.clone(), plain_rgb, dual, None, settings)?;
    let model = finish(prepared, rgb, None, None, settings)?;
    Ok((
        LearnedBase {
            edit: edit.clone(),
            turn,
            model: Arc::new(model.image),
            plain: Arc::new(plain.image),
            white: WhiteBase::from(&model.white_balance, model.clip_level),
            radius: model.sharpen_radius,
            clip_level: model.clip_level,
            // Set by the caller once it knows where the CA ran.
            ca_on_gpu: false,
        },
        report,
    ))
}

/// `plain` taken `strength` of the way to `model`, per sample: the
/// two are one picture in one space, so the mix is that much of the
/// denoise. At the ends no copy is made.
pub(super) fn blend(
    model: &Arc<WorkingImage>,
    plain: &Arc<WorkingImage>,
    strength: f32,
) -> Arc<WorkingImage> {
    let s = strength.clamp(0.0, 1.0);
    if s >= 1.0 {
        return model.clone();
    }
    if s <= 0.0 {
        return plain.clone();
    }
    let mut out = (**plain).clone();
    for (o, m) in out.data.iter_mut().zip(&model.data) {
        *o += (m - *o) * s;
    }
    Arc::new(out)
}
