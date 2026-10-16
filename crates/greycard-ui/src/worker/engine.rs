//! The worker's thread: the loop that takes the queue's jobs in turn.

use super::*;

/// What a develop left out of what its edit asked for, in words for a
/// log line: a learned denoiser or a fill whose model is not in the
/// store, or that could not run. An export writes the picture without
/// them, as the window shows it, and says so. `base` is the base the
/// develop ran on: one kept from an earlier develop reports `Kept`,
/// and the stand-in it may be is said from what it carries.
pub(super) fn left_out_of(outcome: &Outcome, base: Option<&Base>) -> Vec<String> {
    let Outcome::Developed { learned, fills, .. } = outcome else {
        return Vec::new();
    };
    let learned = match (learned, base.and_then(|b| b.stand_in.as_ref())) {
        (LearnedReport::Kept, Some(stood_in)) => stood_in,
        (report, _) => report,
    };
    let mut out = Vec::new();
    match learned {
        LearnedReport::Missing(model) => out.push(format!(
            "the learned denoiser: {} is not downloaded; the engine's denoise instead",
            model.name
        )),
        LearnedReport::Failed(why) => out.push(format!(
            "the learned denoiser failed ({why}); the engine's denoise instead"
        )),
        _ => {}
    }
    for name in &fills.missing {
        out.push(format!(
            "fill {name}: {} is not downloaded; left as it was",
            greycard_ai::FILL.name
        ));
    }
    for (name, why) in &fills.failed {
        out.push(format!(
            "fill {name}: the fill model failed ({why}); left as it was"
        ));
    }
    out
}

pub(super) fn run(
    queue: Arc<(Mutex<Queue>, Condvar)>,
    deliver: Deliver,
    pool: Arc<crate::thumbpool::Pool>,
    edited: Arc<crate::edited::Edited>,
    reducer: Arc<crate::reduced::Reducer>,
) {
    let (lock, cv) = &*queue;
    let mut ai = Ai::new();
    match greycard_ai::Store::user() {
        Ok(store) => tracing::info!("model store {}", store.root().display()),
        Err(e) => tracing::warn!("no model store: {e}"),
    }
    let mut lenses: Option<Arc<greycard_lens::Database>> = greycard_lens::Store::user()
        .ok()
        .and_then(|s| s.load())
        .map(|(db, _)| Arc::new(db));
    reducer.set_lenses(lenses.clone());
    // The learned models for the masks of frames not open
    // (`Job::MasksFor`), made on the first such frame and dropped when
    // none is waiting, as `set_ai` is for a set's frames.
    let mut masks_ai: Option<Ai> = None;
    let cache = greycard_ai::DenoiseCache::user()
        .inspect_err(|e| tracing::warn!("no denoise cache: {e}"))
        .ok();
    let mut input: Option<Input> = None;
    // The open file's path, for the export's account of its source.
    let mut opened_path: Option<PathBuf> = None;
    // What the file said about itself, for the export's EXIF.
    let mut metadata: Option<Arc<greycard_core::decode::RawMetadata>> = None;
    // The open file's quarter turns, from the job that opened it and
    // from every develop after.
    let mut turn: u8 = 0;
    // The last develop, kept for an export under the same settings
    // and the same turn.
    let mut last: Option<Last> = None;
    // The last develop before its dehaze and sharpen, so a change to
    // either costs only those.
    let mut base: Option<Base> = None;
    // The learned denoiser's answer, kept beside the base so its
    // strength is a blend and not another run of the network.
    let mut learned: Option<LearnedBase> = None;
    // The engine's GPU ops, once the window has a device to give.
    let mut gpu: Option<greycard_gpu::Context> = None;
    // The learned models for a set's frames other than the open one,
    // made on the first such frame and dropped with the set: what the
    // open file's own `ai` holds (its masks, its fills) is for it, and
    // a set run past it must not take that away.
    let mut set_ai: Option<Ai> = None;
    // A part's people were found and the part is not made yet: a
    // person is being picked, and the People model is wanted on the
    // click, whatever the edit says meanwhile.
    let mut picking = false;
    // The viewport's develop as a texture on the device, for the edit's
    // pictures to be read back from; none while a develop runs.
    let mut shown: Option<ShownTexture> = None;
    // A save of the open frame whose develop is still to land.
    let mut pending: Option<Keep> = None;
    loop {
        let mut device = None;
        let job = {
            let mut q = lock.lock().expect("worker queue");
            loop {
                // The editor is leaving: the engine's GPU context and
                // everything it holds are dropped here, on the thread
                // that made them, before the process goes.
                if q.stopping {
                    return;
                }
                if let Some(db) = q.lenses.take() {
                    lenses = Some(db);
                    reducer.set_lenses(lenses.clone());
                    edited.set_lenses(crate::edited::lens_database());
                    discard_base(&mut base, gpu.as_ref());
                    learned = None;
                }
                if let Some(d) = q.gpu.take() {
                    device = Some(d);
                }
                if q.forget_subject {
                    q.forget_subject = false;
                    ai.forget_subject();
                }
                if let Some(job) = q.keeps.pop_front() {
                    break job;
                }
                if let Some(job) = q.develop.take() {
                    break job;
                }
                if q.people {
                    q.people = false;
                    break Job::People;
                }
                if let Some((key, shape, model)) = q.masks.pop_front() {
                    break Job::Mask { key, shape, model };
                }
                if let Some(job) = q.exports.pop_front() {
                    break job;
                }
                // Not while a frame is open in the editor: a decode and
                // a model run beside the develop the user waits on.
                if !edited.any_open()
                    && let Some(frame) = q.masks_for.pop_front()
                {
                    break Job::MasksFor(frame);
                }
                if device.is_some() {
                    // Nothing else to do: build the context now, off
                    // the lock, and wait again.
                    break Job::Gpu;
                }
                q = cv.wait(q).expect("worker queue");
            }
        };
        if let Some((d, q)) = device.take() {
            match greycard_gpu::Context::from_device(&d, &q) {
                Ok(ctx) => {
                    let planes = if ctx.keeps_local_contrast_planes() {
                        "kept"
                    } else {
                        "made per run"
                    };
                    tracing::info!(
                        "engine ops on the GPU: {}, the local contrast's planes {planes}",
                        ctx.name()
                    );
                    gpu = Some(ctx);
                }
                Err(e) => tracing::warn!("engine ops stay on the CPU: {e}"),
            }
        }
        if matches!(job, Job::Gpu) {
            continue;
        }
        // A save of the open frame: its edit's pictures from the develop
        // in hand, or held for the develop still to land.
        if let Job::Keep {
            frame,
            edit,
            turn,
            iso,
        } = job
        {
            let hand = InHand {
                opened: opened_path.as_deref(),
                last: last.as_ref(),
                shown: shown.as_ref(),
                base: base.as_ref(),
                gpu: gpu.as_ref(),
            };
            let keep = Keep {
                frame,
                edit,
                turn,
                iso,
            };
            let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                keep_open(keep, false, hand, &mut ai, &edited, &deliver)
            }));
            pending = held.unwrap_or_else(|payload| {
                tracing::warn!(
                    "the edit's pictures: the worker panicked: {}",
                    panic_message(payload.as_ref())
                );
                None
            });
            continue;
        }
        // The frame is about to be left, or its picture replaced: a save
        // still waiting on it is made from what is in hand now, or let go.
        if let Some(keep) = pending.take()
            && matches!(job, Job::Open { .. } | Job::Develop { .. })
        {
            let leaving = match &job {
                Job::Open { .. } => true,
                Job::Develop { edit, turn, .. } => {
                    !(edit.same_develop(&keep.edit) && *turn % 4 == keep.turn)
                }
                _ => false,
            };
            let hand = InHand {
                opened: opened_path.as_deref(),
                last: last.as_ref(),
                shown: shown.as_ref(),
                base: base.as_ref(),
                gpu: gpu.as_ref(),
            };
            pending = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                keep_open(keep, leaving, hand, &mut ai, &edited, &deliver)
            }))
            .unwrap_or(None);
        }
        // The texture goes before a develop makes the next, so the two are
        // never held at once.
        if matches!(job, Job::Open { .. } | Job::Develop { .. }) {
            shown = None;
        }
        // A frame of a set: written, counted, and the set said to be
        // done after its last; a panic in it is that frame's failure.
        if let Job::ExportFrame { set, index, frame } = job {
            let mut panicked = false;
            // The edit the file was written under: the frame's, its
            // learned blend seeded from the ISO when it was still to
            // be, as a first open would have seeded it.
            let mut rendered = frame.edit.clone();
            let (done, finished) =
                crate::queue::step(&set, index, &crate::queue::label(&frame), || {
                    // The name the policy chose, which the status line says:
                    // the one written, not the one asked for.
                    let resolved = crate::queue::resolve(&set, &frame);
                    deliver(Outcome::SetFrameStarted {
                        set: set.clone(),
                        index,
                        name: file_label(resolved.path().unwrap_or(&frame.out)),
                    });
                    let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let note = resolved.note();
                        let Some(path) = resolved.path().map(std::path::Path::to_path_buf) else {
                            return crate::queue::Done::Skipped {
                                path: frame.out.clone(),
                            };
                        };
                        // A subfolder is made as its first frame goes in; a
                        // chosen folder is there, and one that is not (a
                        // drive gone) is never made on the disk under it.
                        if set.sub.is_some()
                            && let Some(dir) = path.parent()
                            && let Err(e) = std::fs::create_dir_all(dir)
                        {
                            return crate::queue::Done::Failed {
                                message: format!("making {}: {e}", dir.display()),
                            };
                        }
                        let start = Instant::now();
                        // The frame on screen is the open file: its picture
                        // is the last develop's when that was of this edit.
                        // Any other is opened and developed on the side,
                        // and the open file's state is left as it was.
                        let written = if opened_path.as_deref() == Some(frame.source.as_path()) {
                            let edit = seeded(&frame.edit, frame.seed_blend, input.as_ref());
                            rendered = edit.clone();
                            open_picture(
                                &edit,
                                frame.turn % 4,
                                input.as_ref(),
                                &mut last,
                                &mut base,
                                &mut learned,
                                &mut ai,
                                cache.as_ref(),
                                lenses.as_deref(),
                                &deliver,
                            )
                            .and_then(|(image, mut left_out)| {
                                left_out.extend(write_export(
                                    image,
                                    &edit,
                                    base.as_ref(),
                                    &mut ai,
                                    opened_path.as_deref(),
                                    metadata.as_deref(),
                                    &set.settings,
                                    &path,
                                )?);
                                Ok(left_out)
                            })
                        } else {
                            // The editor's People model, lent for the
                            // frame rather than a second one loaded.
                            let other = set_ai.get_or_insert_with(Ai::new);
                            ai.lend_parts(other);
                            let written = export_other(
                                &frame,
                                other,
                                cache.as_ref(),
                                lenses.as_deref(),
                                &deliver,
                                &set.settings,
                                &path,
                            );
                            other.lend_parts(&mut ai);
                            written.map(|(edit, left_out)| {
                                rendered = edit;
                                left_out
                            })
                        };
                        match written {
                            Ok(left_out) => crate::queue::Done::Exported {
                                path,
                                seconds: start.elapsed().as_secs_f64(),
                                note,
                                left_out,
                            },
                            Err(message) => crate::queue::Done::Failed { message },
                        }
                    }));
                    held.unwrap_or_else(|payload| {
                        panicked = true;
                        crate::queue::Done::Failed {
                            message: format!(
                                "the worker panicked: {}",
                                panic_message(payload.as_ref())
                            ),
                        }
                    })
                });
            if panicked {
                discard_base(&mut base, gpu.as_ref());
                learned = None;
                last = None;
                ai.forget(opened_path.clone());
                set_ai = None;
            }
            deliver(Outcome::SetFrameDone {
                set: set.clone(),
                index,
                source: frame.source,
                edit: rendered,
                version: frame.version.map(|v| v.id),
                done,
            });
            if finished {
                // The set's own models go with it.
                set_ai = None;
                let tally = set.tally();
                deliver(Outcome::SetDone { set, tally });
            }
            continue;
        }
        // A develop has the machine: the thumbnails' threads are held
        // to what `during_develop` allows until it is done.
        let developing = matches!(job, Job::Open { .. } | Job::Develop { .. });
        // A picture with no Part on it lets the People model go: it
        // holds about a gigabyte, and two with a picture's crops.
        if matches!(job, Job::Open { .. }) {
            picking = false;
        }
        if let Job::Open { edit, .. } | Job::Develop { edit, .. } = &job
            && !has_live_part(edit)
            && !picking
        {
            ai.release_parts();
        }
        if developing {
            pool.set_limit(during_develop(pool.threads()));
        }
        let blame = Blame::of(&job);
        // A panic in a job is the hook's to write, with its
        // backtrace; here it becomes the failure the UI is waiting
        // on, and the develop state is dropped rather than trusted.
        let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
            Job::Open {
                path,
                version: _,
                edit,
                generation,
                seed_blend,
                turn: wanted,
            } => match timed_open(&path) {
                Ok((opened, m)) => {
                    metadata = Some(Arc::new(m));
                    let blend = match &opened {
                        Input::Raw(f) if seed_blend => Some(Noise::blend_for_iso(f.shot.iso)),
                        _ => None,
                    };
                    let edit = {
                        let mut edit = edit;
                        if let Some(b) = blend {
                            edit.noise.learned_strength = b;
                        }
                        edit
                    };
                    let (as_shot, frame, profile, kind) = match &opened {
                        Input::Raw(f) => {
                            let profile = profile_from_frame(f).ok().map(Box::new);
                            let as_shot = profile
                                .as_ref()
                                .and_then(|p| as_shot_temp_tint(f, p).ok())
                                .map(|tt| (tt.cct, tt.duv))
                                .unwrap_or((5500.0, 0.0));
                            (as_shot, Some(f.clone()), profile, SourceKind::Raw)
                        }
                        Input::Picture { meta, .. } => (
                            (5500.0, 0.0),
                            None,
                            None,
                            SourceKind::Picture {
                                space: meta.space.clone(),
                                bits: meta.bits,
                            },
                        ),
                    };
                    deliver(Outcome::Opened {
                        generation,
                        as_shot,
                        frame,
                        profile,
                        lens: lens_report(lenses.as_deref(), &opened),
                        kind,
                        shot: {
                            let (make, model, shot) = opened.identity();
                            shot.summary(make, model)
                        },
                        blend,
                    });
                    discard_base(&mut base, gpu.as_ref());
                    learned = None;
                    ai.forget(Some(path.clone()));
                    // The file is the worker's before the develop, so
                    // a panic in the develop leaves the worker and the
                    // UI, which has its Opened, on the same file.
                    input = Some(opened);
                    opened_path = Some(path);
                    let opened = input.as_ref().expect("just set");
                    turn = wanted % 4;
                    let (outcome, image) = develop_job(
                        opened,
                        &edit,
                        turn,
                        generation,
                        &mut base,
                        &mut learned,
                        &mut ai,
                        cache.as_ref(),
                        lenses.as_deref(),
                        &mut gpu,
                        &deliver,
                    );
                    shown = ShownTexture::of(&edit, turn, &outcome, base.as_ref());
                    last =
                        image.and_then(|i| base.as_ref().map(|b| Last::made(edit, b, i, &outcome)));
                    deliver(outcome);
                }
                Err(e) => deliver(Outcome::Failed {
                    generation,
                    message: e.to_string(),
                }),
            },
            Job::Develop {
                edit,
                generation,
                turn: wanted,
            } => {
                if let Some(f) = &input {
                    turn = wanted % 4;
                    let (outcome, image) = develop_job(
                        f,
                        &edit,
                        turn,
                        generation,
                        &mut base,
                        &mut learned,
                        &mut ai,
                        cache.as_ref(),
                        lenses.as_deref(),
                        &mut gpu,
                        &deliver,
                    );
                    shown = ShownTexture::of(&edit, turn, &outcome, base.as_ref());
                    last =
                        image.and_then(|i| base.as_ref().map(|b| Last::made(edit, b, i, &outcome)));
                    deliver(outcome);
                }
            }
            Job::Mask { key, shape, model } => {
                if matches!(shape, Shape::Part { .. }) {
                    picking = false;
                }
                let outcome = match &base {
                    Some(b) => {
                        match ai.raster(
                            b.stamp, &b.image, &b.edit, b.source, b.turn, key, &shape, model,
                        ) {
                            Ok(made) => Outcome::Mask {
                                key,
                                shape,
                                file: opened_path.clone(),
                                raster: made.raster,
                                provider: made.provider.map(|p| p.name()),
                                seconds: made.seconds,
                                note: made.note,
                                part: made.part,
                            },
                            Err(message) => Outcome::MaskFailed { key, message },
                        }
                    }
                    None => Outcome::MaskFailed {
                        key,
                        message: "nothing developed yet".into(),
                    },
                };
                deliver(outcome);
            }
            Job::MasksFor(frame) => {
                let ai = masks_ai.get_or_insert_with(Ai::new);
                // Given way between two shapes to a develop, a save or a
                // frame opened: the rest is made after, from the front.
                let waiting = || {
                    let q = lock.lock().expect("worker queue");
                    q.develop.is_some() || !q.keeps.is_empty() || edited.any_open()
                };
                match masks_for(&frame, ai, lenses.as_deref(), &waiting) {
                    MasksMade::All => {
                        reducer.masks_done(&frame, true);
                        deliver(Outcome::EditedKept { frame });
                    }
                    // A frame tried and found wanting stays so until a
                    // model is fetched: asked again now, it would be
                    // decoded and developed again for the same answer.
                    MasksMade::Short => reducer.masks_done(&frame, false),
                    MasksMade::GaveWay => {
                        lock.lock()
                            .expect("worker queue")
                            .masks_for
                            .push_front(frame);
                    }
                }
                if lock.lock().expect("worker queue").masks_for.is_empty() {
                    masks_ai = None;
                }
            }
            Job::People => {
                picking = true;
                let people = match &base {
                    Some(b) => ai
                        .people(b.stamp, &b.image, b.source, b.turn)
                        .map(|(found, provider, seconds)| (found, provider.name(), seconds)),
                    None => Err("nothing developed yet".into()),
                };
                deliver(Outcome::People {
                    file: opened_path.clone(),
                    people,
                });
            }
            Job::Fetch { .. }
            | Job::FetchLenses
            | Job::Gpu
            | Job::Thumbnail { .. }
            | Job::CachedThumbnail { .. }
            | Job::ExportSet { .. }
            | Job::ExportFrame { .. }
            | Job::Keep { .. } => {
                unreachable!(
                    "fetches and thumbnails run on threads of their own; the device, a set's frames and a save are taken above"
                )
            }
        }));
        if let Err(payload) = held {
            discard_base(&mut base, gpu.as_ref());
            learned = None;
            last = None;
            shown = None;
            // The next stamp restarts with the base; what was cached
            // under the old ones goes with them.
            ai.forget(opened_path.clone());
            let message = panic_message(payload.as_ref());
            if let Some(outcome) = blame.outcome(message) {
                deliver(outcome);
            }
        }
        if developing {
            edited.touch();
            pool.set_limit(pool.threads());
            // A save that was waiting for this develop: its pictures now.
            if let Some(keep) = pending.take() {
                let hand = InHand {
                    opened: opened_path.as_deref(),
                    last: last.as_ref(),
                    shown: shown.as_ref(),
                    base: base.as_ref(),
                    gpu: gpu.as_ref(),
                };
                pending = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    keep_open(keep, false, hand, &mut ai, &edited, &deliver)
                }))
                .unwrap_or(None);
            }
        }
    }
}

/// How many of the pool's `threads` may begin a thumbnail while a
/// develop runs: none. Those in hand finish; the rest wait for the
/// develop to be done. Measured on a cold folder of 300, eight
/// threads left running took the first develop from 1.28 s to 1.61 s
/// and four from 1.28 to 1.49, where holding them all kept it at
/// 1.21 and cost the folder 0.8 s of its 3.6. The develop runs rayon
/// over every core and the thumbnails' decodes take cores it wanted.
/// `GREYCARD_THUMB_DURING_DEVELOP` overrides it, for measuring.
fn during_develop(threads: usize) -> usize {
    std::env::var("GREYCARD_THUMB_DURING_DEVELOP")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(0, |n| n.min(threads))
}

/// A path's file name, for a status line.
fn file_label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Forget the base, and with it the ops' working textures on the GPU:
/// a new file or a new lens database develops afresh.
fn discard_base(base: &mut Option<Base>, gpu: Option<&greycard_gpu::Context>) {
    if base.take().is_some()
        && let Some(gpu) = gpu
    {
        gpu.release();
    }
}

/// Whether a live Part is in `edit`'s masks.
fn has_live_part(edit: &Edit) -> bool {
    edit.adjustments.iter().any(|a| {
        a.enabled
            && a.mask
                .live()
                .any(|(_, c)| matches!(c.shape, Shape::Part { .. }))
    })
}
