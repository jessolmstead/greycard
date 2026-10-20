//! The develop: a job's picture from the base.

use super::*;

/// Develop `edit`: the base from the cache when only the local
/// contrast, the dehaze or the sharpen differs, else the engine,
/// then those three on a copy, in that order: the sharpen's
/// contrast threshold is measured on the picture it gets, which
/// should be the one with its haze gone. With the
/// learned denoiser on, its answer is kept and blended by strength;
/// when it is not to be had the engine's own path stands in. A
/// picture that is not a raw is its own base: nothing before the lens
/// applies to it.
///
/// With `gpu`, the Detail section and the sharpen run there: the
/// patched picture is uploaded once and kept with the base, the local
/// contrast and the dehaze make the picture before the sharpen from it
/// on a Detail move and that is kept too, and the result is a texture
/// the viewport draws as it is, so a Detail slider costs the Detail
/// section and the sharpen alone, and a sharpen slider the sharpen
/// alone. The dehaze's fit runs on the CPU between its two GPU halves,
/// from a reduced copy the GPU makes, which is kept for the next
/// Dehaze move. No CPU picture comes back then; the export, which
/// passes no `gpu`, develops its own on the reference path. A GPU
/// error (out of memory, a lost device) is warned once and the context
/// is dropped: the session stays on the CPU from then on.
#[allow(clippy::too_many_arguments)]
pub(super) fn develop_job(
    input: &Input,
    edit: &Edit,
    turn: u8,
    generation: u64,
    base: &mut Option<Base>,
    learned: &mut Option<LearnedBase>,
    ai: &mut Ai,
    cache: Option<&greycard_ai::DenoiseCache>,
    lenses: Option<&greycard_lens::Database>,
    gpu: &mut Option<greycard_gpu::Context>,
    deliver: &Deliver,
) -> (Outcome, Option<Arc<WorkingImage>>) {
    let start = Instant::now();
    let stamp = base.as_ref().map_or(1, |b| b.stamp + 1);
    let mut report = LearnedReport::Off;
    let mut ca_note = None;
    // Only the frame's turn moved, and nothing in the base cares
    // which way up the picture is: the base is turned where it
    // stands, a permutation of its pixels, and is then kept below.
    if let Some(b) = base.as_mut()
        && b.turn != turn
        && b.edit.same_base(edit)
        && (gpu.is_some() || !b.ca_on_gpu)
        && turns_exactly(edit, lenses)
    {
        let started = Instant::now();
        turn_base(b, learned, turn, stamp, gpu.as_ref());
        tracing::info!(
            "base turned to {turn} quarter turns in {:.2} s",
            started.elapsed().as_secs_f64()
        );
    }
    // A base is kept while its edit's base is the same, and, without
    // a GPU (the export), only if its CA correction was not the GPU's:
    // the export's picture is the reference's, so it makes the base
    // afresh on the CPU, and the session keeps that one.
    let fresh_base = !base.as_ref().is_some_and(|b| {
        b.edit.same_base(edit) && b.turn == turn && (gpu.is_some() || !b.ca_on_gpu)
    });
    // Where the base came from, for the log line: one taken back from
    // the shelf says so on its first develop, and is kept after.
    let from = if fresh_base {
        BaseFrom::Made
    } else if base
        .as_mut()
        .is_some_and(|b| std::mem::take(&mut b.from_shelf))
    {
        BaseFrom::Shelf
    } else {
        BaseFrom::Kept
    };
    let mut stages = Stages::new();
    if fresh_base {
        let settings = base_settings(edit, input, turn);
        let tier = edit
            .noise
            .tier()
            .map(|t| greycard_ai::denoiser(t).expect("the edit's tiers are the registry's"));
        let ca_run = CaRun::new(gpu.as_ref());
        let run_ca =
            |samples: &[f32], w: usize, h: usize, pattern: &CfaPattern, options: &CaOptions| {
                ca_run.correct(samples, w, h, pattern, options)
            };
        let ca: Option<CaCorrector<'_>> = Some(&run_ca);
        let made = match (input, tier) {
            (Input::Picture { image, meta }, _) => Ok(Base {
                edit: edit.clone(),
                turn,
                image: match quarters(turn) {
                    Some(o) => Arc::new(greycard_core::develop::orient((**image).clone(), o)),
                    None => image.clone(),
                },
                guide: Arc::new(crate::finish::Guide::NONE),
                white: WhiteBase {
                    clip: meta.clip_level(),
                    ..WhiteBase::IDENTITY
                },
                radius: None,
                clip_level: meta.clip_level(),
                source: crate::finish::Source::Display,
                stamp,
                patched: None,
                pre: None,
                uploaded: None,
                haze: None,
                planes: None,
                ca_on_gpu: false,
                stand_in: None,
                gave_way: None,
                profile: None,
                from_shelf: false,
            }),
            (Input::Raw(frame), tier)
                if let Some(why) = denoise_unavailable(frame, tier.is_some()) =>
            {
                // Nothing to try: the engine's develop, as a learned
                // tier that is not downloaded falls back to it, said so.
                report = LearnedReport::Unavailable(why);
                engine_base(frame, edit, turn, &settings, stamp, ca, &mut stages)
            }
            (Input::Raw(frame), Some(model)) => {
                match learned_base(
                    frame,
                    edit,
                    turn,
                    &settings,
                    model,
                    learned,
                    ai,
                    cache,
                    stamp,
                    ca,
                    gpu.is_some(),
                    &mut stages,
                ) {
                    Ok((b, r)) => {
                        report = r;
                        Ok(b)
                    }
                    Err(r) => {
                        // The line times the base that stands in.
                        report = r;
                        stages.clear();
                        engine_base(frame, edit, turn, &settings, stamp, ca, &mut stages)
                    }
                }
            }
            (Input::Raw(frame), None) => {
                engine_base(frame, edit, turn, &settings, stamp, ca, &mut stages)
            }
        };
        let (ran, failed) = ca_run.into_parts();
        if let Some(e) = failed {
            // Once is enough: the device is not trusted again this
            // session, and every op from here is the CPU's.
            tracing::warn!("the GPU CA correction: {e}; the CPU's from now on");
            if let Some(ctx) = gpu.take() {
                ctx.release();
            }
        }
        ca_note = ran;
        ca_out_of_prepare(&mut stages, ca_note);
        let ca_on_gpu = matches!(ca_note, Some((CaRan::Gpu, _)));
        // A learned pair made in this develop carries this develop's
        // CA; a pair kept from an earlier one already says where its
        // own ran, and the base took that from it.
        if ca_on_gpu
            && matches!(
                report,
                LearnedReport::Ran { .. } | LearnedReport::Cached { .. }
            )
            && let Some(l) = learned.as_mut()
        {
            l.ca_on_gpu = true;
        }
        match made {
            Ok(mut b) => {
                b.ca_on_gpu |= ca_on_gpu;
                // The profile the develop resolved its white through,
                // chosen as `white_balance_for` in the engine chooses
                // it: the edit's DCP, else the file's own.
                if let Input::Raw(frame) = input {
                    b.profile = resolved_profile(&settings, frame);
                }
                b.stand_in = matches!(
                    report,
                    LearnedReport::Missing(_)
                        | LearnedReport::Failed(_)
                        | LearnedReport::Unavailable(_)
                )
                .then(|| report.clone());
                lens_step(&mut b, input, edit, lenses, &mut stages);
                guide_step(&mut b, &mut stages);
                *base = Some(b)
            }
            Err(e) => {
                return (
                    Outcome::Failed {
                        generation,
                        message: e.to_string(),
                    },
                    None,
                );
            }
        }
    } else if edit.noise.tier().is_some() {
        report = LearnedReport::Kept;
    }
    let b = base.as_mut().expect("a base was just made");
    // The retouch, its patches given sources where they had none,
    // applied on a copy of the base and kept for the next develop.
    let chosen = edit.retouch.choose_sources(&b.image);
    let retouch = edit.retouch.with_sources(&chosen);
    let sources = retouch.chosen_patches(&chosen);
    let mut fills = FillReport::default();
    // A kept picture with fills left unmade serves while the model
    // is still missing, and no longer once it has been fetched.
    let patched = if retouch.is_empty() {
        b.image.clone()
    } else if let Some((r, image, whole)) = &b.patched
        && *r == retouch
        && (*whole || !ai.fill_available())
    {
        image.clone()
    } else {
        // Timed with its fills, which have a part of their own too.
        let started = Instant::now();
        let mut image = (*b.image).clone();
        retouch.apply_with(&mut image, |patch, region, picture| {
            // A fill the model has to make is said so before it
            // starts, since it takes a while.
            let kept = ai.fill_kept(patch);
            if !kept && ai.fill_available() {
                deliver(Outcome::Filling {
                    generation,
                    name: patch.name(),
                });
            }
            let started = Instant::now();
            match ai.fill(patch, region, picture) {
                Ok(data) => {
                    if !kept {
                        fills.made.push(patch.name());
                        fills.seconds += started.elapsed().as_secs_f64();
                    }
                    Some(data)
                }
                Err(NoFill::Missing) => {
                    fills.missing.push(patch.name());
                    None
                }
                Err(NoFill::Failed(why)) => {
                    fills.failed.push((patch.name(), why));
                    None
                }
            }
        });
        let image = Arc::new(image);
        let whole = fills.missing.is_empty() && fills.failed.is_empty();
        b.patched = Some((retouch, image.clone(), whole));
        stages.push(("retouch", started.elapsed().as_secs_f64()));
        image
    };
    // The masks' bands, when a mask asks for one: kept from the last
    // develop when they were made from this picture and hold every
    // band asked for, made otherwise on the CPU on a thread of their
    // own while the develop goes on, whichever path the picture takes,
    // and taken up where the develop is delivered (`PlanesJob::land`).
    //
    // The haze is read after the global's local contrast, as the
    // global's dehaze is fitted; a move of the global's Texture or
    // Clarity makes the haze again and keeps the bands.
    let wants = edit.detail_wants();
    let global = edit.detail.options();
    let mut planes = match &b.planes {
        _ if !wants.any() => {
            b.planes = None;
            PlanesJob::Nothing
        }
        Some(k) if k.serve(&patched, &wants, global) => PlanesJob::Kept(k.planes.clone()),
        kept => {
            let reuse = kept
                .as_ref()
                .filter(|k| Arc::ptr_eq(&k.patched, &patched))
                .map(|k| k.planes.clone());
            b.planes = None;
            PlanesJob::start(patched.clone(), global, b.clip_level, wants, reuse)
        }
    };
    // The local contrast and the dehaze on one copy of the patched
    // picture, on the CPU; none when neither is asked for.
    let clip_level = b.clip_level;
    let before_sharpen = |copy: &mut Option<WorkingImage>| {
        let detail = edit.detail.options().map(|options| {
            let started = Instant::now();
            let image = copy.get_or_insert_with(|| (*patched).clone());
            let stats = local_contrast(image, &options, clip_level);
            (stats, started.elapsed().as_secs_f64())
        });
        let dehaze_stats = edit.detail.dehaze_options().map(|options| {
            let image = copy.get_or_insert_with(|| (*patched).clone());
            dehaze::dehaze(image, &options)
        });
        (detail, dehaze_stats)
    };
    // The ops on the GPU, when there is one. With the sharpen on, the
    // picture before it is kept on the device from the last develop
    // when nothing before the sharpen changed, and is made there by
    // the Detail section from the patched picture's upload when the
    // section moved. With the sharpen off, the Detail section writes
    // the viewport's texture itself; with neither on, the ops'
    // textures are let go.
    match (gpu.as_ref(), edit.sharpen.options()) {
        (Some(ctx), Some(options)) => {
            let kept = b
                .pre
                .as_ref()
                .is_some_and(|p| Arc::ptr_eq(&p.patched, &patched) && p.detail == edit.detail);
            let mut ran = Ran::default();
            let made = if kept {
                Ok(())
            } else {
                make_pre_sharpen(ctx, b, &patched, edit, &before_sharpen, &mut ran)
            };
            let sharpened = made.and_then(|()| {
                let pre = b.pre.as_ref().expect("made, or kept");
                let texture = ctx.viewport_texture(pre.image.width(), pre.image.height());
                ctx.sharpen(&pre.image, &options, b.radius, b.clip_level, &texture)
                    .map(|stats| (stats, texture))
            });
            match sharpened {
                Ok((stats, texture)) => {
                    let planes = planes.land(b);
                    let pre = b.pre.as_ref().expect("made, or kept");
                    let seconds = start.elapsed().as_secs_f64();
                    stages.extend(ran.upload.map(|s| ("upload", s)));
                    note_developed(
                        pre.image.width(),
                        pre.image.height(),
                        from,
                        ca_note,
                        &report,
                        &fills,
                        &stages,
                        ran.detail,
                        pre.dehaze_stats
                            .map(|_| ran.dehaze.unwrap_or(DehazeRan::Kept)),
                        Some(Sharpened::Gpu),
                        seconds,
                    );
                    return (
                        Outcome::Developed {
                            generation,
                            turn,
                            image: Developed::Texture(texture),
                            kind: b.source,
                            guide: b.guide.clone(),
                            planes,
                            white: b.white,
                            seconds,
                            detail: pre
                                .detail_stats
                                .map(|s| (s, ran.detail.unwrap_or(DetailRan::Kept))),
                            sharpen: Some(stats),
                            dehaze: pre.dehaze_stats,
                            sources,
                            learned: report,
                            fills,
                        },
                        None,
                    );
                }
                Err(greycard_gpu::Error::Unsupported(why)) => {
                    // This picture's, not the device's (one it cannot
                    // hold): the CPU for this develop, the context kept.
                    tracing::info!("the ops on the CPU: {why}");
                    b.pre = None;
                    b.uploaded = None;
                    b.haze = None;
                    ctx.release();
                }
                Err(e) => {
                    // Once is enough: the device is not trusted again
                    // this session, and every develop from here is the
                    // CPU's.
                    tracing::warn!("the GPU ops: {e}; the CPU's from now on");
                    b.pre = None;
                    b.uploaded = None;
                    b.haze = None;
                    if let Some(ctx) = gpu.take() {
                        ctx.release();
                    }
                }
            }
        }
        (Some(ctx), None) => {
            // The sharpen is off: its input and its planes go, whichever
            // path the develop takes.
            b.pre = None;
            ctx.release_sharpen();
            if edit.detail.options().is_some() || edit.detail.dehaze_options().is_some() {
                let mut ran = Ran::default();
                match detail_on_gpu(ctx, b, &patched, edit, true, &mut ran) {
                    Ok(Some(made)) => {
                        let planes = planes.land(b);
                        let texture = made.texture.expect("a viewport texture was asked for");
                        let seconds = start.elapsed().as_secs_f64();
                        stages.extend(ran.upload.map(|s| ("upload", s)));
                        note_developed(
                            texture.width(),
                            texture.height(),
                            from,
                            ca_note,
                            &report,
                            &fills,
                            &stages,
                            ran.detail,
                            ran.dehaze,
                            None,
                            seconds,
                        );
                        return (
                            Outcome::Developed {
                                generation,
                                turn,
                                image: Developed::Texture(texture),
                                kind: b.source,
                                guide: b.guide.clone(),
                                planes,
                                white: b.white,
                                seconds,
                                detail: made.detail.map(|s| (s, DetailRan::Gpu)),
                                sharpen: None,
                                dehaze: made.dehaze,
                                sources,
                                learned: report,
                                fills,
                            },
                            None,
                        );
                    }
                    Ok(None) => {
                        // This picture's, not the device's: the CPU for
                        // this develop, the context kept.
                        b.uploaded = None;
                        b.haze = None;
                        ctx.release();
                    }
                    Err(e) => {
                        tracing::warn!("the GPU Detail section: {e}; the CPU's from now on");
                        b.uploaded = None;
                        b.haze = None;
                        if let Some(ctx) = gpu.take() {
                            ctx.release();
                        }
                    }
                }
            } else {
                // The CPU's develop: nothing on the device is of use.
                b.uploaded = None;
                b.haze = None;
                ctx.release();
            }
        }
        (None, _) => {}
    }
    let mut copy: Option<WorkingImage> = None;
    let (detail, dehaze_stats) = before_sharpen(&mut copy);
    let (stats, mask) = match edit.sharpen.options() {
        Some(options) => {
            let image = copy.get_or_insert_with(|| (*patched).clone());
            let (stats, mask) = sharpen_with_mask(image, &options, b.radius, b.clip_level);
            (Some(stats), Some(mask))
        }
        None => (None, None),
    };
    let image = match copy {
        Some(image) => Arc::new(image),
        None => patched,
    };
    let halves = Arc::new(timed(&mut stages, "halves", || {
        Halves::from_image(&image, mask.as_deref())
    }));
    let seconds = start.elapsed().as_secs_f64();
    note_developed(
        image.width as u32,
        image.height as u32,
        from,
        ca_note,
        &report,
        &fills,
        &stages,
        detail.as_ref().map(|(_, s)| DetailRan::Cpu(*s)),
        dehaze_stats.map(|_| DehazeRan::Cpu),
        stats.map(|_| Sharpened::Cpu),
        seconds,
    );
    (
        Outcome::Developed {
            generation,
            turn,
            image: Developed::Halves(halves),
            kind: b.source,
            guide: b.guide.clone(),
            planes: planes.land(b),
            white: b.white,
            seconds,
            detail: detail.map(|(s, secs)| (s, DetailRan::Cpu(secs))),
            sharpen: stats,
            dehaze: dehaze_stats,
            sources,
            learned: report,
            fills,
        },
        Some(image),
    )
}

/// The engine's settings a fresh base is made under for `edit` at the
/// frame's `turn`: the edit's own, less the sharpen and the dehaze,
/// which come after the base.
pub(super) fn base_settings(edit: &Edit, input: &Input, turn: u8) -> DevelopSettings {
    DevelopSettings {
        sharpen: None,
        dehaze: None,
        // The frame's quarter turns, composed onto the camera's tag:
        // the engine's one orientation step does both, so nothing
        // after it can disagree about which way up the picture is. A
        // file that is not a raw was turned at decode, so its turn is
        // applied to the picture by the caller.
        orientation: match input {
            Input::Raw(f) => Some(f.orientation.turned(i32::from(turn))),
            Input::Picture { .. } => None,
        },
        ..edit.settings()
    }
}

/// The CA correction of a fresh base: on the GPU when there is one,
/// the reference's otherwise and on any error. A picture the device
/// cannot hold is this op's to decline, on the CPU for this base and
/// with the context kept; a device error is remembered for the caller
/// to act on after the base. What ran and how long is noted for the
/// log.
pub(super) struct CaRun<'a> {
    ctx: Option<&'a greycard_gpu::Context>,
    ran: std::cell::Cell<Option<(CaRan, f64)>>,
    failed: std::cell::Cell<Option<greycard_gpu::Error>>,
}

impl<'a> CaRun<'a> {
    pub(super) fn new(ctx: Option<&'a greycard_gpu::Context>) -> Self {
        Self {
            ctx,
            ran: std::cell::Cell::new(None),
            failed: std::cell::Cell::new(None),
        }
    }

    /// The correction of one mosaic, as the engine's `CaCorrector`.
    pub(super) fn correct(
        &self,
        samples: &[f32],
        w: usize,
        h: usize,
        pattern: &CfaPattern,
        options: &CaOptions,
    ) -> greycard_core::Result<(Vec<f32>, CaStats)> {
        if let Some(ctx) = self.ctx {
            let started = Instant::now();
            match ctx.correct_ca(samples, w, h, pattern, options) {
                Ok(out) => {
                    self.ran
                        .set(Some((CaRan::Gpu, started.elapsed().as_secs_f64())));
                    return Ok(out);
                }
                Err(greycard_gpu::Error::Unsupported(why)) => {
                    tracing::info!("the CA correction on the CPU: {why}");
                }
                Err(e) => self.failed.set(Some(e)),
            }
        }
        let started = Instant::now();
        let out = correct_ca(samples, w, h, pattern, options);
        self.ran
            .set(Some((CaRan::Cpu, started.elapsed().as_secs_f64())));
        out
    }

    /// Where the CA ran and its seconds, when it ran, and the device's
    /// error, when it had one.
    pub(super) fn into_parts(self) -> (Option<(CaRan, f64)>, Option<greycard_gpu::Error>) {
        (self.ran.take(), self.failed.take())
    }
}

/// The CA runs inside the prepare and has a part of its own on the
/// line: the prepare's is the rest of it.
pub(super) fn ca_out_of_prepare(stages: &mut Stages, ca: Option<(CaRan, f64)>) {
    if let Some((_, ca)) = ca
        && let Some((_, s)) = stages.iter_mut().find(|(name, _)| *name == "prepare")
    {
        *s = (*s - ca).max(0.0);
    }
}

/// The profile a raw's develop resolved its white through, chosen as
/// `white_balance_for` in the engine chooses it: the edit's DCP, else
/// the file's own.
pub(super) fn resolved_profile(
    settings: &DevelopSettings,
    frame: &RawFrame,
) -> Option<Arc<CameraProfile>> {
    settings
        .profile
        .as_ref()
        .map(|p| p.camera.clone())
        .or_else(|| profile_from_frame(frame).ok())
        .map(Arc::new)
}

/// A fresh base's lens step: the edit's lens corrections, then its
/// defringe, timed as "lens" when either runs.
pub(super) fn lens_step(
    b: &mut Base,
    input: &Input,
    edit: &Edit,
    lenses: Option<&greycard_lens::Database>,
    stages: &mut Stages,
) {
    let corrects = !edit.lens.is_identity(lenses.is_some());
    let defringe = edit.lens.defringe();
    if !corrects && defringe.is_none() {
        return;
    }
    let started = Instant::now();
    let held = std::mem::replace(&mut b.image, Arc::new(WorkingImage::new(0, 0)));
    let mut image = Arc::try_unwrap(held).unwrap_or_else(|a| (*a).clone());
    if corrects {
        image = correct_lens(image, input, edit, lenses);
    }
    // After the geometry, so the resample does not smear a chroma just
    // neutralized, and before the retouch and the sharpen.
    if let Some(options) = defringe {
        greycard_core::develop::defringe::defringe(&mut image, &options);
    }
    b.image = Arc::new(image);
    stages.push(("lens", started.elapsed().as_secs_f64()));
}

/// A fresh base's guide plane, after the lens and the defringe and
/// before the retouch and the sharpen: the geometry has not moved a
/// pixel yet, so the plane is in the developed picture's own
/// coordinates, which is where the viewport and the export both read
/// it. Neither the retouch's patches nor the sharpen moves a region's
/// mean luminance by anything the guide's scale can see.
pub(super) fn guide_step(b: &mut Base, stages: &mut Stages) {
    b.guide = Arc::new(timed(stages, "guide", || {
        crate::finish::guide_plane(&b.image)
    }));
}

/// What the CPU's steps before the sharpen report: the local
/// contrast's stats and seconds, and the dehaze's stats, each when it
/// ran.
type BeforeSharpen = (Option<(LocalContrastStats, f64)>, Option<DehazeStats>);

/// The patched picture on the device: the one kept with the base
/// when it is this picture's, else uploaded now and kept.
fn uploaded_patched(
    ctx: &greycard_gpu::Context,
    b: &mut Base,
    patched: &Arc<WorkingImage>,
    ran: &mut Ran,
) -> greycard_gpu::Result<Arc<greycard_gpu::Image>> {
    if let Some((p, image)) = b.uploaded.as_ref()
        && Arc::ptr_eq(p, patched)
    {
        return Ok(image.clone());
    }
    b.uploaded = None;
    let image = Arc::new(ran.upload(|| ctx.upload(patched))?);
    b.uploaded = Some((patched.clone(), image.clone()));
    Ok(image)
}

/// Where this develop's local contrast and dehaze ran, for the log;
/// none for an op that did not run in it. And the seconds its uploads
/// of a picture to the device took, none when nothing went up.
#[derive(Default)]
struct Ran {
    detail: Option<DetailRan>,
    dehaze: Option<DehazeRan>,
    upload: Option<f64>,
}

impl Ran {
    /// `send`, an upload, timed into `upload`: the worker's side of
    /// it, the interleave to four channels and the copies into the
    /// queue's staging a band at a time, which is what the develop
    /// waits on.
    fn upload<T>(&mut self, send: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let out = send();
        *self.upload.get_or_insert(0.0) += started.elapsed().as_secs_f64();
        out
    }
}

/// The picture before the sharpen, on the GPU, for `edit`'s Detail
/// section on `patched`, into `b.pre`: [`detail_on_gpu`]'s picture,
/// or, for a picture the ops decline (`Unsupported`), the CPU's local
/// contrast and dehaze (`before_sharpen`), uploaded, with the patched
/// picture's upload, the dehaze's reduced copy and the local
/// contrast's planes let go, since that path has no use for them. The
/// last picture before the sharpen goes first, so that two are never
/// held at once (716 MB at 45 MP). Any other GPU error is the
/// caller's to act on.
fn make_pre_sharpen(
    ctx: &greycard_gpu::Context,
    b: &mut Base,
    patched: &Arc<WorkingImage>,
    edit: &Edit,
    before_sharpen: &dyn Fn(&mut Option<WorkingImage>) -> BeforeSharpen,
    ran: &mut Ran,
) -> greycard_gpu::Result<()> {
    b.pre = None;
    if let Some(made) = detail_on_gpu(ctx, b, patched, edit, false, ran)? {
        b.pre = Some(PreSharpen {
            patched: patched.clone(),
            detail: edit.detail,
            image: made.image.expect("no viewport texture was given"),
            detail_stats: made.detail,
            dehaze_stats: made.dehaze,
        });
        return Ok(());
    }
    b.uploaded = None;
    b.haze = None;
    ctx.release_local_contrast();
    let mut copy = None;
    let (detail, dehaze_stats) = before_sharpen(&mut copy);
    ran.detail = detail.map(|(_, s)| DetailRan::Cpu(s));
    ran.dehaze = dehaze_stats.map(|_| DehazeRan::Cpu);
    let image: &WorkingImage = copy.as_ref().unwrap_or(patched);
    let image = Arc::new(ran.upload(|| ctx.upload(image))?);
    b.pre = Some(PreSharpen {
        patched: patched.clone(),
        detail: edit.detail,
        image,
        detail_stats: detail.map(|(s, _)| s),
        dehaze_stats,
    });
    Ok(())
}

/// What the Detail section made on the device: the picture after it,
/// or the viewport's texture it went into, and its ops' stats.
struct OnGpu {
    image: Option<Arc<greycard_gpu::Image>>,
    texture: Option<greycard_gpu::wgpu::Texture>,
    detail: Option<LocalContrastStats>,
    dehaze: Option<DehazeStats>,
}

/// `edit`'s Detail section on the device, from the patched picture's
/// upload: the local contrast when a slider of it is set, then the
/// dehaze when it is on, each reading the last one's output, as the
/// reference runs them. The dehaze sums the blocks of its input there
/// and reads them back, fits its model on the CPU from them (the
/// reference's own fit), and applies the model there; the reduced
/// copy is kept, so a move of the Dehaze alone fits again from it. With
/// `viewport`, the last op writes a viewport texture, made once the
/// upload has found the picture fits the device; else the result is a
/// new picture for the sharpen, the upload itself when neither op is
/// set. None for a picture the device or an op declines
/// (`Unsupported`: one larger than the device's textures, or an amount
/// so small its strength is zero, which the CPU's path leaves alone),
/// to take the CPU for this develop.
fn detail_on_gpu(
    ctx: &greycard_gpu::Context,
    b: &mut Base,
    patched: &Arc<WorkingImage>,
    edit: &Edit,
    viewport: bool,
    ran: &mut Ran,
) -> greycard_gpu::Result<Option<OnGpu>> {
    let declined = |op: &str, why: String| {
        tracing::info!("the {op} on the CPU: {why}");
        Ok(None)
    };
    let uploaded = match uploaded_patched(ctx, b, patched, ran) {
        Ok(uploaded) => uploaded,
        Err(greycard_gpu::Error::Unsupported(why)) => return declined("Detail section", why),
        Err(e) => return Err(e),
    };
    let texture = viewport.then(|| ctx.viewport_texture(uploaded.width(), uploaded.height()));
    let viewport = texture.as_ref();
    let local = edit.detail.options();
    let dehaze = edit.detail.dehaze_options();
    let (image, detail) = match (local, dehaze, viewport) {
        (None, _, _) => {
            ctx.release_local_contrast();
            (uploaded, None)
        }
        // The local contrast last, into the viewport's texture.
        (Some(options), None, Some(out)) => {
            return match ctx.local_contrast_to_viewport(&uploaded, &options, b.clip_level, out) {
                Ok(stats) => {
                    ran.detail = Some(DetailRan::Gpu);
                    b.haze = None;
                    Ok(Some(OnGpu {
                        image: None,
                        texture,
                        detail: Some(stats),
                        dehaze: None,
                    }))
                }
                Err(greycard_gpu::Error::Unsupported(why)) => declined("local contrast", why),
                Err(e) => Err(e),
            };
        }
        (Some(options), _, _) => match ctx.local_contrast(&uploaded, &options, b.clip_level) {
            Ok((image, stats)) => {
                ran.detail = Some(DetailRan::Gpu);
                (Arc::new(image), Some(stats))
            }
            Err(greycard_gpu::Error::Unsupported(why)) => return declined("local contrast", why),
            Err(e) => return Err(e),
        },
    };
    let Some(options) = dehaze else {
        b.haze = None;
        return Ok(Some(OnGpu {
            image: Some(image),
            texture,
            detail,
            dehaze: None,
        }));
    };
    // The reduced copy: kept from the last develop when it was made
    // from this picture through the same local contrast, else summed
    // here and read back.
    let reduced = match b.haze.as_ref() {
        Some(k) if Arc::ptr_eq(&k.patched, patched) && k.detail == local => k.reduced.clone(),
        _ => {
            b.haze = None;
            let reduced = Arc::new(ctx.dehaze_reduced(&image)?);
            b.haze = Some(KeptHaze {
                patched: patched.clone(),
                detail: local,
                reduced: reduced.clone(),
            });
            reduced
        }
    };
    let Some(model) = reduced.model(&options) else {
        return declined(
            "dehaze",
            format!("a strength of zero at {}", options.amount),
        );
    };
    let applied = match viewport {
        Some(out) => ctx
            .dehaze_to_viewport(&image, &model, out)
            .map(|stats| (None, stats)),
        None => ctx
            .dehaze(&image, &model)
            .map(|(image, stats)| (Some(Arc::new(image)), stats)),
    };
    match applied {
        Ok((image, stats)) => {
            ran.dehaze = Some(DehazeRan::Gpu);
            Ok(Some(OnGpu {
                image,
                texture,
                detail,
                dehaze: Some(stats),
            }))
        }
        Err(greycard_gpu::Error::Unsupported(why)) => declined("dehaze", why),
        Err(e) => Err(e),
    }
}

/// Where a develop's sharpen ran.
#[derive(Clone, Copy)]
enum Sharpened {
    Cpu,
    Gpu,
}

/// Where a base develop's CA correction ran.
#[derive(Clone, Copy)]
pub(super) enum CaRan {
    Cpu,
    Gpu,
}

/// Where a develop's base came from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum BaseFrom {
    /// Made in this develop.
    Made,
    /// The last develop's, kept.
    Kept,
    /// Taken back from the shelf of frames left behind.
    Shelf,
}

/// One line for a develop: what was made anew and what was kept,
/// and the seconds, the same the outcome carries. A model that is
/// missing is a state, named here and offered by the UI, not warned
/// on every develop; one that would not load is warned where the
/// load fails. A fill that failed is warned here, with its name.
#[allow(clippy::too_many_arguments)]
fn note_developed(
    width: u32,
    height: u32,
    from: BaseFrom,
    ca: Option<(CaRan, f64)>,
    learned: &LearnedReport,
    fills: &FillReport,
    stages: &Stages,
    detail: Option<DetailRan>,
    dehazed: Option<DehazeRan>,
    sharpened: Option<Sharpened>,
    total: f64,
) {
    for (name, why) in &fills.failed {
        tracing::warn!("fill {name}: {why}");
    }
    tracing::info!(
        "{}",
        developed_line(
            width, height, from, ca, learned, fills, stages, detail, dehazed, sharpened, total
        )
    );
}

/// [`note_developed`]'s line: where the base came from, then the CA
/// and the denoiser, then each stage this develop ran and its seconds
/// (a fresh base's own, the retouch, and the picture's way to the
/// viewport: its upload to the device or its half floats), then the
/// fills and the ops after the base, and the total.
#[allow(clippy::too_many_arguments)]
fn developed_line(
    width: u32,
    height: u32,
    from: BaseFrom,
    ca: Option<(CaRan, f64)>,
    learned: &LearnedReport,
    fills: &FillReport,
    stages: &Stages,
    detail: Option<DetailRan>,
    dehazed: Option<DehazeRan>,
    sharpened: Option<Sharpened>,
    total: f64,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(
        match from {
            BaseFrom::Made => "base made",
            BaseFrom::Kept => "base kept",
            BaseFrom::Shelf => "base from the shelf",
        }
        .into(),
    );
    match ca {
        Some((CaRan::Gpu, s)) => parts.push(format!("CA on the GPU {s:.2} s")),
        Some((CaRan::Cpu, s)) => parts.push(format!("CA {s:.2} s")),
        None => {}
    }
    match learned {
        LearnedReport::Off => {}
        LearnedReport::Ran {
            version,
            provider,
            seconds,
        } => parts.push(format!("denoiser {version} on {provider} {seconds:.2} s")),
        LearnedReport::Cached { seconds } => parts.push(format!("denoiser cached {seconds:.2} s")),
        LearnedReport::Kept => parts.push("denoiser kept".into()),
        LearnedReport::Missing(_) => parts.push("denoiser missing".into()),
        LearnedReport::Failed(_) => parts.push("denoiser failed".into()),
        LearnedReport::Unavailable(_) => parts.push("no learned denoiser for this mosaic".into()),
    }
    for (name, s) in stages {
        parts.push(format!("{name} {s:.2} s"));
    }
    if !fills.made.is_empty() {
        parts.push(format!(
            "{} fill(s) {:.2} s",
            fills.made.len(),
            fills.seconds
        ));
    }
    if !fills.missing.is_empty() {
        parts.push(format!("{} fill(s) missing", fills.missing.len()));
    }
    match detail {
        Some(DetailRan::Gpu) => parts.push("local contrast on the GPU".into()),
        Some(DetailRan::Cpu(s)) => parts.push(format!("local contrast {s:.2} s")),
        Some(DetailRan::Kept) | None => {}
    }
    match dehazed {
        Some(DehazeRan::Gpu) => parts.push("dehaze on the GPU".into()),
        Some(DehazeRan::Cpu) => parts.push("dehaze".into()),
        Some(DehazeRan::Kept) | None => {}
    }
    match sharpened {
        Some(Sharpened::Cpu) => parts.push("sharpen".into()),
        Some(Sharpened::Gpu) => parts.push("sharpen on the GPU".into()),
        None => {}
    }
    format!(
        "developed {width}x{height}: {}, total {total:.2} s",
        parts.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh base's line names each of its stages with its seconds,
    /// the prepare after the CA, between the parts it had before; a
    /// base from the shelf says so, with its upload; a kept one is the
    /// line it was.
    #[test]
    fn the_develop_s_line_times_its_stages() {
        let stages: Stages = vec![
            ("prepare", 0.31),
            ("demosaic", 0.92),
            ("finish", 0.204),
            ("guide", 0.02),
            ("upload", 0.12),
        ];
        let line = developed_line(
            8192,
            5464,
            BaseFrom::Made,
            Some((CaRan::Gpu, 0.25)),
            &LearnedReport::Off,
            &FillReport::default(),
            &stages,
            None,
            None,
            Some(Sharpened::Gpu),
            2.5,
        );
        assert_eq!(
            line,
            "developed 8192x5464: base made, CA on the GPU 0.25 s, prepare 0.31 s, \
             demosaic 0.92 s, finish 0.20 s, guide 0.02 s, upload 0.12 s, \
             sharpen on the GPU, total 2.50 s"
        );

        let line = developed_line(
            8192,
            5464,
            BaseFrom::Shelf,
            None,
            &LearnedReport::Kept,
            &FillReport::default(),
            &vec![("upload", 0.118)],
            None,
            None,
            Some(Sharpened::Gpu),
            0.4,
        );
        assert_eq!(
            line,
            "developed 8192x5464: base from the shelf, denoiser kept, upload 0.12 s, \
             sharpen on the GPU, total 0.40 s"
        );

        let line = developed_line(
            300,
            200,
            BaseFrom::Kept,
            None,
            &LearnedReport::Off,
            &FillReport::default(),
            &Stages::new(),
            Some(DetailRan::Cpu(0.5)),
            Some(DehazeRan::Cpu),
            Some(Sharpened::Cpu),
            0.75,
        );
        assert_eq!(
            line,
            "developed 300x200: base kept, local contrast 0.50 s, dehaze, sharpen, total 0.75 s"
        );
    }
}
