//! The worker's thread: the loop that takes the queue's jobs in turn.

use super::handle::{Holds, holds};
use super::prefetch::{self, Due, Fetched};
use super::shelf::{FileKey, Shelf};
use super::*;

/// What a develop left out of what its edit asked for, in words for a
/// log line: a learned denoiser or a fill whose model is not in the
/// store, or that could not run. An export writes the picture without
/// them, as the window shows it, and says so. `base` is the base the
/// develop ran on: one kept from an earlier develop reports `Kept`,
/// and the stand-in it may be is said from what it carries.
pub(super) fn left_out_of(outcome: &Outcome, base: Option<&FrameKept>) -> Vec<String> {
    let Outcome::Developed { learned, fills, .. } = outcome else {
        return Vec::new();
    };
    let stood_in = base
        .and_then(|b| b.meta.stood_in.as_ref())
        .map(super::develop::stood_in_report);
    let learned = match (learned, stood_in.as_ref()) {
        (LearnedReport::Kept, Some(stood_in)) => stood_in,
        (report, _) => report,
    };
    left_out_words(learned, base.and_then(|b| b.meta.gave_way.as_ref()), fills)
}

/// [`left_out_of`]'s words for a develop's learned denoiser, the
/// demosaic that gave way, and its fills.
pub(super) fn left_out_words(
    learned: &LearnedReport,
    gave_way: Option<&String>,
    fills: &FillReport,
) -> Vec<String> {
    let mut out = Vec::new();
    match learned {
        LearnedReport::Missing(model) => out.push(format!(
            "the learned denoiser: {} is not downloaded; the engine's denoise instead",
            model.name
        )),
        LearnedReport::Failed(why) => out.push(format!(
            "the learned denoiser failed ({why}); the engine's denoise instead"
        )),
        LearnedReport::Unavailable(why) => out.push(format!("the denoise: {why}; none ran")),
        _ => {}
    }
    if let Some(why) = gave_way {
        out.push(why.clone());
    }
    for (name, model) in &fills.missing {
        out.push(format!(
            "fill {name}: {} is not downloaded; left as it was",
            model.name
        ));
    }
    for name in &fills.no_gpu {
        out.push(format!(
            "fill {name}: the remove needs a GPU; left as it was"
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
    edited: Arc<crate::edited_pictures::Edited>,
    background: Option<Arc<Background>>,
) {
    use crate::walk::Hold;
    let wake = || {
        if let Some(b) = &background {
            b.pool.wake();
        }
    };
    let set_lenses = |lenses: Option<Arc<greycard_lens::Database>>| {
        if let Some(b) = &background {
            b.reducer.set_lenses(lenses);
        }
    };
    let (lock, cv) = &*queue;
    let mut ai = Ai::new();
    match greycard_ai::Store::user() {
        Ok(store) => {
            tracing::info!("model store {}", store.root().display());
            // Before the first develop: a learned denoiser that lost
            // this card on an earlier run is skipped from the start (the
            // read logs it, with the record's path).
            crate::ai::off_card_seam();
            greycard_ai::runtime::read_denoisers_off_card(store.root(), crate::ai::providers);
        }
        Err(e) => tracing::warn!("no model store: {e}"),
    }
    let mut lenses: Option<Arc<greycard_lens::Database>> = greycard_lens::Store::user()
        .ok()
        .and_then(|s| s.load())
        .map(|(db, _)| Arc::new(db));
    set_lenses(lenses.clone());
    // The Adobe lens profiles listed ahead, off this thread, so an open
    // lists them from the listing held and never waits on the walk.
    if let Some(dir) = greycard_edit_io::profiles::store_dir() {
        greycard_lens::lcp::look_in(&dir, false, Box::new(|_| {}));
    }
    // The learned models for the masks of frames not open
    // (`Job::MasksFor`), made on the first such frame and dropped when
    // none is waiting, as `set_ai` is for a set's frames.
    let mut masks_ai: Option<Ai> = None;
    // The same for a person picked on one frame and looked for on the
    // others (`Job::PersonOn`), dropped when none is waiting.
    let mut people_ai: Option<Ai> = None;
    let cache = greycard_ai::DenoiseCache::user()
        .inspect_err(|e| tracing::warn!("no denoise cache: {e}"))
        .ok();
    let mut input: Option<Input> = None;
    // The open file's path, for the export's account of its source.
    let mut opened_path: Option<PathBuf> = None;
    // The file open before it, whose base the open shelved: no prefetch
    // pushes it off the shelf.
    let mut left_path: Option<PathBuf> = None;
    // The open file with what the file system and its decode said of
    // it as it was opened: what its base is shelved under when another
    // is opened.
    let mut opened_file: Option<FileKey> = None;
    // What the file said about itself, for the export's EXIF.
    let mut metadata: Option<Arc<greycard_core::decode::RawMetadata>> = None;
    // The open file's quarter turns, from the job that opened it and
    // from every develop after.
    let mut turn: u8 = 0;
    // What the open frame's walks kept, so a change to a later stage
    // costs only the stages from it.
    let mut base: Option<FrameKept> = None;
    // The bases of the frames left behind, for a return to one to
    // skip its base (`shelf`). Made by the first open, so a worker that
    // never opens a frame (an export's from the command line) has none,
    // reads nothing for it, and says nothing of it.
    let mut shelf: Option<Shelf> = None;
    // The engine's GPU ops, once the window has a device to give.
    let mut gpu: Option<greycard_gpu::Context> = None;
    // The learned models for a set's frames other than the open one,
    // made on the first such frame and dropped with the set: what the
    // open file's own `ai` holds (its masks, its fills) is for it, and
    // a set run past it must not take that away.
    let mut set_ai: Option<Ai> = None;
    // Sets whose frames met a short the card did not recover from: their
    // later frames ask the card once, not three times.
    let mut short_sets = ShortSets::default();
    // Sets whose frames were refused a run for the card, no fallback
    // taking it, or tried again for one: their later frames take a
    // mask's CPU at once and are not tried again. A short a smaller tile
    // handled is neither: no wait was spent on it.
    let mut failed_sets = ShortSets::default();
    // A develop's fills asked for again after a short, once.
    let mut asked_again = false;
    // A part's people were found and the part is not made yet: a
    // person is being picked, and the People model is wanted on the
    // click, whatever the edit says meanwhile.
    let mut picking = false;
    // The viewport's develop as a texture on the device, for the edit's
    // pictures to be read back from; none while a develop runs.
    let mut shown: Option<ShownTexture> = None;
    // A save of the open frame whose develop is still to land.
    let mut pending: Option<Keep> = None;
    // The generation of the last open whose develop landed: the
    // neighbors the window sent with that open are the ones prefetched.
    let mut landed: Option<u64> = None;
    // The file whose open failed last, with what it failed of, until an
    // open succeeds: a develop of that frame is answered with it.
    let mut failed_open: Option<(PathBuf, String)> = None;
    // What the log was last told of the prefetch's depth and the
    // walk's direction, said again only when either changes.
    let mut depth_said: Option<(prefetch::Depth, prefetch::Side)> = None;
    // What the reads of the neighbors' sidecars keep: the ones that did
    // not serve, not read again until they change, and what each folder
    // is on.
    let mut peeks = prefetch::Peeks::default();
    // The generation of the last develop delivered, open or not: an ask
    // for the refined masks' picture of another is let go.
    let mut developed: Option<u64> = None;
    // The device was lost this session: the ops are the CPU's for the
    // rest of it, and a device handed over again is not taken.
    let mut gpu_gone = false;
    // A job panicked with wgpu's words for a lost device.
    let mut lost_seen = false;
    // A fill round in hand: the develop to make again on the CPU should
    // the device be lost in it.
    let mut fill_redo: Option<Job> = None;
    // The open frame's develop landed with fills still to make: its
    // edit, turn, and generation, how far its fills have got, and how
    // many are left. A develop asked for gives them up; leaving the
    // editor pauses them.
    let mut fills_due: Option<FillsDue> = None;
    // The open frame's develop landed sharpened only as far as the
    // window showed it: the whole sharpen is to follow. A develop asked
    // for gives it up, and makes its own.
    let mut whole_due: Option<WholeDue> = None;
    // What a test has the open frame's fills let go of the card, where
    // the worker has no device.
    #[cfg(test)]
    let mut fills_release_by_test: Option<FillsReleaseByTest> = None;
    // A test's card's free memory as it set it: what each export frame
    // starts from where a test's release lets go, its walk making the
    // sets again.
    #[cfg(test)]
    let mut free_by_test_at: Option<u64> = None;
    loop {
        let mut device = None;
        let mut built = None;
        // A lost device, found by the ops (a wait that failed, an op's
        // error), by a panic, or by the window's device-lost callback:
        // the context goes, and the develop on screen, a texture on
        // the lost device, is made again on the CPU unless the editor
        // is starting again.
        let told = lock.lock().expect("worker queue").gpu_lost.take();
        if !gpu_gone && (told.is_some() || lost_seen || gpu.as_ref().is_some_and(|c| c.is_lost())) {
            gpu_gone = true;
            let again = told.unwrap_or(true);
            let redo = lose_gpu(&mut gpu, &mut base, &mut shown, again)
                .or_else(|| fill_redo.take().filter(|_| again));
            whole_due = None;
            if let Some(job) = redo {
                lock.lock()
                    .expect("worker queue")
                    .develop
                    .get_or_insert(job);
            }
        }
        lost_seen = false;
        // A fill round that ended, whatever its end, leaves nothing to
        // develop again.
        fill_redo = None;
        let job = {
            let mut q = lock.lock().expect("worker queue");
            loop {
                // The editor is leaving: the engine's GPU context and
                // everything it holds are dropped here, on the thread
                // that made them, before the process goes.
                if q.stopping {
                    // A save waiting on the settle: the settle first,
                    // and the save from it, as a session that never
                    // sharpened a part first would have kept.
                    if let Some(due) = whole_due.as_ref().filter(|d| {
                        save_waits(pending.as_ref(), d, base.as_ref(), lenses.as_deref())
                    }) {
                        break Job::Settle {
                            generation: due.generation,
                        };
                    }
                    return;
                }
                // The store first, so a test's other seams sent with it
                // land on the models it makes.
                #[cfg(test)]
                if let Some(store) = q.models_by_test.take() {
                    ai = Ai::for_test_on_disk(store);
                }
                #[cfg(test)]
                if let Some(v) = q.fill_by_test.take() {
                    ai.fill_by_test = Some(v);
                }
                #[cfg(test)]
                {
                    q.release_left_by_test |= ai.holds_card_release();
                    if let Some(r) = q.fills_release_by_test.take() {
                        fills_release_by_test = Some(r);
                    }
                }
                #[cfg(test)]
                if let Some(budget) = q.shelf_by_test.take() {
                    shelf = Some(Shelf::new(budget));
                }
                #[cfg(test)]
                if let Some((free, freed)) = q.free_by_test.take() {
                    free_by_test_at = Some(free);
                    ai.free_memory_by_test = Some(free);
                    ai.freed_by_test = freed;
                    ai.set_providers_by_test(vec![
                        greycard_ai::Provider::WebGpu,
                        greycard_ai::Provider::Cpu,
                    ]);
                }
                #[cfg(test)]
                if let Some(unet) = q.unet_by_test.take() {
                    ai.unet_by_test = Some(unet);
                }
                if let Some(check) = q.remote_check.take() {
                    peeks.set_remote(check);
                }
                if let Some(db) = q.lenses.take() {
                    lenses = Some(db);
                    set_lenses(lenses.clone());
                    edited.set_lenses(crate::edited_pictures::lens_database());
                    forget_bases(&mut base, &mut shelf, gpu.as_ref());
                }
                if let Some(d) = q.gpu.take()
                    && !gpu_gone
                {
                    device = Some(d);
                }
                if let Some(ctx) = q.context.take()
                    && !gpu_gone
                {
                    built = Some(ctx);
                }
                for (path, now) in std::mem::take(&mut q.edits_changed) {
                    drop_changed(shelf.as_mut(), &path, now.as_ref(), lenses.as_deref());
                }
                if q.forget_subject {
                    q.forget_subject = false;
                    ai.forget_subject();
                }
                if q.models_arrived {
                    q.models_arrived = false;
                    for (key, shape, model) in ai.retry_fallbacks() {
                        if !q.masks.iter().any(|(k, _, _)| *k == key) {
                            q.masks.push_back((key, shape, model));
                        }
                    }
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
                if q.landscape {
                    q.landscape = false;
                    break Job::Landscape;
                }
                if let Some(job) = q.mask_picture.take() {
                    break job;
                }
                // The open frame's next fill, before the masks, which a
                // fill landing would ask for again; an export first, which
                // makes its own fills in its one walk. Paused while the
                // editor is left for the grid or the cull, what the remove
                // held for them let go; the frame shown again wakes the
                // queue, and they go on.
                if let Some(due) = &fills_due {
                    if edited.any_open() {
                        if let Some(job) = q.exports.pop_front() {
                            break job;
                        }
                        break Job::Fill {
                            generation: due.generation,
                        };
                    }
                    ai.release_fills();
                    // The card given back off the queue's lock: a
                    // drop and a trim take a few hundred milliseconds,
                    // and every send from the window waits on it.
                    if give_back_due(&ai, [&people_ai, &set_ai, &masks_ai]) {
                        drop(q);
                        give_back(
                            &mut ai,
                            [&mut people_ai, &mut set_ai, &mut masks_ai],
                            false,
                            lock,
                        );
                        q = lock.lock().expect("worker queue");
                        continue;
                    }
                }
                // A new base's masks once the develops have settled.
                settle_due(&mut q, Instant::now());
                if let Some((key, shape, model)) = q.masks.pop_front() {
                    break Job::Mask { key, shape, model };
                }
                // The whole sharpen of the develop on screen, behind any
                // develop asked for and the masks (none of which reads
                // the sharpen) and ahead of the rest: the picture at
                // rest is the edit's.
                if let Some(due) = whole_due.as_ref().filter(|d| !d.parked) {
                    break Job::Settle {
                        generation: due.generation,
                    };
                }
                // An export before a person looked for: the export was
                // asked first and is the thing delivered, while a frame
                // the run has not reached yet can still be picked by
                // hand.
                if let Some(job) = q.exports.pop_front() {
                    break job;
                }
                if let Some(job) = q.people_on.pop_front() {
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
                // Nothing asked for: a neighbor's base onto the shelf,
                // off the lock, and the queue looked at again after.
                let now = prefetch::Idle {
                    landed,
                    open: base.as_ref(),
                    waiting: pending.is_some(),
                    shelf: shelf.as_ref(),
                    idle: edited.idle_for(),
                    frame_open: edited.any_open(),
                    device_lost: q.gpu_lost.is_some(),
                };
                let due = prefetch::due(&mut q.neighbors, &mut depth_said, &now);
                prefetch::settle_reservation(&due, &edited, wake);
                match due {
                    Due::Fetch(n) => {
                        let n = *n;
                        // The prefetch and a reduced develop are never
                        // in hand at once: the flag holds the pool's
                        // next one back, and one already begun has the
                        // prefetch wait for it with the flag still up,
                        // a reservation the pool honors, so the
                        // prefetch is next and not behind every reduced
                        // develop still queued.
                        if !prefetch::claim(
                            || edited.set_prefetching(true),
                            || background.as_ref().map_or(0, |b| b.pool.edits_busy()),
                        ) {
                            if let Some(waiting) = q.neighbors.as_mut() {
                                waiting.waiting.push_front(n);
                            }
                            q = cv
                                .wait_timeout(q, prefetch::BEHIND_REDUCED)
                                .expect("worker queue")
                                .0;
                            continue;
                        }
                        let (round, keep) = q
                            .neighbors
                            .as_ref()
                            .map(|w| (w.round, prefetch::keep(w, left_path.as_deref())))
                            .unwrap_or_default();
                        drop(q);
                        let generation = landed;
                        // A neighbor whose edit the window does not hold:
                        // its sidecar read here, in this slot, asking
                        // before and after whether to give way.
                        let asked = || {
                            let q = lock.lock().expect("worker queue");
                            prefetch::asked(&q, edited.any_open())
                        };
                        let ready =
                            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                prefetch::read_first(&n, &mut peeks, &asked)
                            })) {
                                Ok(prefetch::Peeked::Ready(ready)) => Ok(*ready),
                                Ok(prefetch::Peeked::GaveWay) => Err(true),
                                Ok(prefetch::Peeked::Skipped(_)) => Err(false),
                                Err(payload) => {
                                    let line = format!(
                                        "the prefetch of {}: the worker panicked reading its \
                                     sidecar: {}",
                                        file_label(&n.path),
                                        panic_message(payload.as_ref())
                                    );
                                    tracing::warn!("{line}");
                                    prefetch::tap(&line);
                                    Err(false)
                                }
                            };
                        let ready = match ready {
                            Ok(ready) => ready,
                            Err(gave_way) => {
                                edited.set_prefetching(false);
                                wake();
                                q = lock.lock().expect("worker queue");
                                // Read again from the start once quiet,
                                // as one abandoned between stages is.
                                if gave_way
                                    && let Some(waiting) = q.neighbors.as_mut()
                                    && Some(waiting.generation) == generation
                                {
                                    waiting.gave_way(n, round);
                                }
                                continue;
                            }
                        };
                        let in_hand = opened_path.as_deref().zip(base.as_ref());
                        let shelf = shelf.as_mut().expect("a shelf is due to have one");
                        // What the list names first in line to stay,
                        // a base it skips as shelved among them.
                        shelf.bring_forward(&keep);
                        let ahead = |facts: &crate::stages::Facts| {
                            let q = lock.lock().expect("worker queue");
                            prefetch::ahead(&q, edited.any_open(), &ready, facts, lenses.as_deref())
                        };
                        let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            prefetch::prefetch(
                                &ready,
                                in_hand,
                                &keep,
                                shelf,
                                &mut gpu,
                                lenses.as_deref(),
                                &ahead,
                            )
                        }));
                        edited.set_prefetching(false);
                        wake();
                        // A device the prefetch let go is gone for the open
                        // frame too.
                        if gpu.is_none()
                            && let Some(b) = base.as_mut()
                        {
                            b.kept.forget_device();
                        }
                        q = lock.lock().expect("worker queue");
                        match held {
                            // Given way to a request on the open frame:
                            // made again from the start once it has been
                            // quiet for longer than before.
                            //
                            // Not when it gave way to a change of its own
                            // edit: the window sends it again under the
                            // new one, and made under the old it would sit
                            // on the shelf past the drop, taking the room
                            // of the frame just left.
                            Ok(Fetched::Abandoned(_, facts)) => {
                                let moved =
                                    prefetch::moved_under(&q, &ready, &facts, lenses.as_deref());
                                if moved {
                                    prefetch::note(format!(
                                        "the prefetch of {} let go: its edit changed",
                                        file_label(&n.path)
                                    ));
                                }
                                if !moved
                                    && let Some(waiting) = q.neighbors.as_mut()
                                    && Some(waiting.generation) == generation
                                {
                                    waiting.gave_way(n, round);
                                }
                            }
                            Ok(_) => {}
                            // Its base was not shelved, and the shelf's
                            // were finished before; the rest are let go.
                            Err(payload) => {
                                lost_seen |= says_lost(&panic_message(payload.as_ref()));
                                let line = format!(
                                    "the prefetch of {}: the worker panicked: {}; \
                                     the rest of the neighbors let go",
                                    file_label(&n.path),
                                    panic_message(payload.as_ref())
                                );
                                tracing::warn!("{line}");
                                prefetch::tap(&line);
                                q.neighbors = None;
                            }
                        }
                        continue;
                    }
                    Due::Wait(wait) => {
                        let wait = masks::settling_left(&q.settling, Instant::now())
                            .map_or(wait, |left| wait.min(left));
                        q = cv.wait_timeout(q, wait).expect("worker queue").0;
                        continue;
                    }
                    Due::Nothing => {}
                }
                // Woken for the masks a new base asks for, once due.
                q = match masks::settling_left(&q.settling, Instant::now()) {
                    Some(left) => cv.wait_timeout(q, left).expect("worker queue").0,
                    None => cv.wait(q).expect("worker queue"),
                };
            }
        };
        // No caller hands over both; a device waiting beside a context
        // would be dropped unseen.
        if built.is_some() && device.is_some() {
            debug_assert!(false, "a device and a context both handed over");
            tracing::warn!(
                "engine ops: a context and a device both handed over; the device dropped"
            );
        }
        let built = built.map(Ok).or_else(|| {
            device
                .take()
                .map(|(d, q)| greycard_gpu::Context::from_device(&d, &q))
        });
        if let Some(made) = built {
            match made {
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
                    // What the open frame kept on a device before is of
                    // no use on this one.
                    if let Some(b) = base.as_mut() {
                        b.kept.forget_device();
                    }
                }
                Err(e) => tracing::warn!("engine ops stay on the CPU: {e}"),
            }
        }
        if matches!(job, Job::Gpu) {
            continue;
        }
        // A develop asked for gives up the fills of the last; any job
        // but the next fill or a save lets go of what the remove held
        // for them, so another model never loads beside it.
        if matches!(job, Job::Open { .. } | Job::Develop { .. }) {
            fills_due = None;
        }
        // What takes the settle's place: an open, or a develop of the
        // frame in hand, the Original's aside among them (the window
        // develops the edit again after it). A develop of another file
        // makes nothing, and leaves the settle and the texture as they
        // are. The aside parks the settle rather than dropping it: not
        // made while the Original is up, but made for a save waiting on
        // it if the frame is left then.
        let replaces = match &job {
            Job::Open { .. } => true,
            Job::Develop { of, .. } | Job::DevelopAside { of, .. } => {
                holds(failed_open.as_ref(), opened_path.as_deref(), of.as_ref()) == Holds::Frame
            }
            _ => false,
        };
        // The frame left with a save waiting on its settle: the settle
        // made now, undelivered (the window has gone on), and the save
        // made from it, so the frame's pictures are the whole develop's.
        if matches!(job, Job::Open { .. })
            && let Some(due) = whole_due
                .take_if(|d| save_waits(pending.as_ref(), d, base.as_ref(), lenses.as_deref()))
            && let Some(f) = &input
        {
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut round = Round::settle();
                let (outcome, _) = develop_round(
                    f,
                    &due.edit,
                    due.turn,
                    due.generation,
                    &mut base,
                    &mut ai,
                    (cache.as_ref(), lenses.as_deref()),
                    &mut gpu,
                    &deliver,
                    &mut round,
                    &crate::walk::NoHold,
                );
                ShownTexture::of(&due.edit, due.turn, &outcome, base.as_ref())
            }));
            match made {
                Ok(Some(s)) => shown = Some(s),
                Ok(None) => {}
                Err(payload) => {
                    let message = panic_message(payload.as_ref());
                    lost_seen |= says_lost(&message);
                    tracing::warn!("the settle of the frame left: the worker panicked: {message}");
                }
            }
        }
        if replaces {
            if matches!(job, Job::DevelopAside { .. }) {
                if let Some(due) = whole_due.as_mut() {
                    due.parked = true;
                }
            } else {
                whole_due = None;
            }
        }
        if !matches!(
            job,
            Job::Fill { .. } | Job::Settle { .. } | Job::Keep { .. }
        ) {
            ai.release_fills();
            // The card given back before another model loads, once a
            // fill's model ran: a develop after it loads the denoiser
            // afresh rather than beside the remove's cached buffers.
            give_back(
                &mut ai,
                [&mut people_ai, &mut set_ai, &mut masks_ai],
                false,
                lock,
            );
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
                shown: shown.as_ref(),
                base: base.as_mut(),
                gpu: gpu.as_ref(),
                lenses: lenses.as_deref(),
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
                let message = panic_message(payload.as_ref());
                lost_seen |= says_lost(&message);
                tracing::warn!("the edit's pictures: the worker panicked: {message}");
                None
            });
            continue;
        }
        // The frame is about to be left, or its picture replaced: a save
        // still waiting on it is made from what is in hand now, or let go.
        // Any other job leaves it waiting (the kind is asked first: a
        // `take` ahead of it would drop the save on a fill's round, the
        // masks' picture, or the settle).
        if matches!(job, Job::Open { .. } | Job::Develop { .. })
            && let Some(keep) = pending.take()
        {
            let leaving = match &job {
                Job::Open { .. } => true,
                Job::Develop { edit, turn, .. } => {
                    !(*turn % 4 == keep.turn
                        && base.as_ref().is_some_and(|b| {
                            crate::stages::same_develop(
                                edit,
                                &keep.edit,
                                &b.facts_at(keep.turn),
                                lenses.as_deref(),
                            )
                        }))
                }
                _ => false,
            };
            let hand = InHand {
                opened: opened_path.as_deref(),
                shown: shown.as_ref(),
                base: base.as_mut(),
                gpu: gpu.as_ref(),
                lenses: lenses.as_deref(),
            };
            pending = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                keep_open(keep, leaving, hand, &mut ai, &edited, &deliver)
            }))
            .unwrap_or(None);
        }
        // The texture goes before a develop makes the next, so the two are
        // never held at once.
        if matches!(job, Job::Fill { .. } | Job::Settle { .. })
            || (replaces && matches!(job, Job::Open { .. } | Job::Develop { .. }))
        {
            shown = None;
        }
        // A person looked for on a frame of a run of "use this person on
        // all", on an `Ai` of its own with the editor's People model
        // lent to it, so the open frame's views and people stay its
        // own. A panic in it is that frame's answer: the open frame's
        // develop, denoise and masks are not what failed and are kept;
        // the borrowed `Ai` is not trusted and goes, the model with it.
        if let Job::PersonOn(job) = job {
            let other = people_ai.get_or_insert_with(Ai::new);
            ai.lend_parts(other);
            let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                person_on(&job, other, lenses.as_deref())
            }));
            let found = match held {
                Ok(found) => {
                    if let Some(other) = people_ai.as_mut() {
                        other.lend_parts(&mut ai);
                    }
                    found
                }
                Err(payload) => {
                    tracing::warn!(
                        "the person picked, on {}: the worker panicked: {}",
                        job.frame.said(),
                        panic_message(payload.as_ref())
                    );
                    people_ai = None;
                    Err(format!(
                        "the worker panicked: {}",
                        panic_message(payload.as_ref())
                    ))
                }
            };
            // The run is over: its `Ai` goes, and the model with it
            // unless the open frame has a People shape of its own, as a
            // develop of an edit without one would have let it go.
            if lock.lock().expect("worker queue").people_on.is_empty() {
                people_ai = None;
                if !picking && !base.as_ref().is_some_and(|b| has_live_part(&b.edit)) {
                    ai.release_parts();
                }
            }
            deliver(Outcome::PersonOn {
                batch: job.batch,
                frame: job.frame,
                found,
            });
            continue;
        }
        // A frame of a set: written, counted, and the set said to be
        // done after its last; a panic in it is that frame's failure.
        if let Job::ExportFrame { set, index, frame } = job {
            let mut panicked = false;
            // The frame was tried again for a short.
            let mut retried = false;
            // A short card is asked again a few seconds apart, the set's
            // cancel looked at between, once only after the set met one.
            let patience = crate::ai::Patience {
                tries: if short_sets.contains(&set) {
                    1
                } else {
                    crate::ai::PATIENT_TRIES
                },
                // A test's set asks again at once.
                wait: if cfg!(test) {
                    std::time::Duration::ZERO
                } else {
                    crate::ai::PATIENT_WAIT
                },
                canceled: {
                    let set = set.clone();
                    Arc::new(move || set.is_canceled())
                },
            };
            ai.set_patience(Some(patience.clone()));
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
                    // Short after its tries, the frame is tried once more
                    // with the card given back across the worker's models:
                    // the editor's own may hold what this frame wants.
                    let mut tried_again = false;
                    let held = loop {
                        // A mask short of the card falls to the CPU only
                        // once the card was given back and the frame tried
                        // again, or in a set whose frame before met a short.
                        let fall = tried_again || failed_sets.contains(&set);
                        ai.set_cpu_fallback(fall);
                        // A removal at 1024 falls to 512 likewise.
                        ai.set_fill_fallback(fall);
                        if let Some(o) = set_ai.as_mut() {
                            o.set_cpu_fallback(fall);
                            o.set_fill_fallback(fall);
                        }
                        // As the window's jobs: a removal, a mask, or the
                        // learned denoiser short of the card has the engine
                        // let go of what a develop makes again.
                        let release = gpu.as_ref().map(card_release);
                        #[cfg(test)]
                        let release =
                            release.or_else(|| fills_release_by_test.clone().map(|r| r.release()));
                        #[cfg(test)]
                        if fills_release_by_test.is_some() && free_by_test_at.is_some() {
                            ai.free_memory_by_test = free_by_test_at;
                        }
                        ai.set_card_release(release);
                        let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let note = resolved.note();
                            let Some(path) = resolved.path().map(std::path::Path::to_path_buf)
                            else {
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
                            // The frame on screen is the open file: walked from
                            // what the window's walks kept, which it leaves as
                            // they were. Any other is opened and developed on
                            // the side, and the open file's state is left as
                            // it was.
                            let written = if opened_path.as_deref() == Some(frame.source.as_path())
                            {
                                let edit = seeded(&frame.edit, frame.seed_blend, input.as_ref());
                                rendered = edit.clone();
                                export_open(
                                    &edit,
                                    frame.turn % 4,
                                    input.as_ref(),
                                    base.as_ref(),
                                    // Its fills are said under the generation
                                    // no develop has, as a set frame's are.
                                    &mut WorkerModels::new(&mut ai, cache.as_ref(), &deliver, 0),
                                    lenses.as_deref(),
                                    &mut gpu,
                                    Written {
                                        settings: &set.settings,
                                        path: &path,
                                        source: opened_path.as_deref(),
                                        metadata: metadata.as_deref(),
                                        time_shift: frame.time_shift,
                                    },
                                )
                            } else {
                                // The editor's People model, lent for the
                                // frame rather than a second one loaded.
                                let other = set_ai.get_or_insert_with(Ai::new);
                                other.set_patience(Some(patience.clone()));
                                other.set_cpu_fallback(fall);
                                other.set_fill_fallback(fall);
                                other.set_card_release(gpu.as_ref().map(card_release));
                                ai.lend_parts(other);
                                let written = export_other(
                                    &frame,
                                    other,
                                    cache.as_ref(),
                                    lenses.as_deref(),
                                    &mut gpu,
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
                        ai.set_card_release(None);
                        if let Some(o) = set_ai.as_mut() {
                            o.set_card_release(None);
                        }
                        let short =
                            ai.short_failed() || set_ai.as_ref().is_some_and(Ai::short_failed);
                        if !tried_again
                            && !failed_sets.contains(&set)
                            && short
                            && matches!(held, Ok(crate::queue::Done::Failed { .. }))
                        {
                            let gave = give_back(
                                &mut ai,
                                [&mut people_ai, &mut set_ai, &mut masks_ai],
                                true,
                                lock,
                            );
                            // Tried again too where nothing was given back
                            // but a removal at 1024 was refused: in the
                            // frame tried again it runs at 512 where 1024
                            // is still short. Not for a canceled set.
                            let large = ai.card_given_back()
                                | set_ai.as_mut().is_some_and(Ai::card_given_back);
                            if gave || (large && !set.is_canceled()) {
                                tried_again = true;
                                retried = true;
                                ai.retry_patiently();
                                if let Some(o) = set_ai.as_mut() {
                                    o.retry_patiently();
                                }
                                tracing::info!("the card given back; the frame tried again");
                                continue;
                            }
                        }
                        break held;
                    };
                    held.unwrap_or_else(|payload| {
                        panicked = true;
                        lost_seen |= says_lost(&panic_message(payload.as_ref()));
                        crate::queue::Done::Failed {
                            message: format!(
                                "the worker panicked: {}",
                                panic_message(payload.as_ref())
                            ),
                        }
                    })
                });
            // A device the export's walk let go is gone for the open
            // frame too, as after a prefetch.
            if gpu.is_none()
                && let Some(b) = base.as_mut()
            {
                b.kept.forget_device();
            }
            let hit = ai.hit_short() || set_ai.as_ref().is_some_and(Ai::hit_short);
            let failed =
                retried || ai.short_failed() || set_ai.as_ref().is_some_and(Ai::short_failed);
            // A mask the frame made on the CPU, or refused: the card is
            // given back across the worker's models after it.
            let mask_short =
                ai.take_mask_short() | set_ai.as_mut().is_some_and(Ai::take_mask_short);
            ai.set_patience(None);
            ai.set_cpu_fallback(false);
            ai.set_fill_fallback(false);
            if let Some(o) = set_ai.as_mut() {
                o.set_patience(None);
                o.set_cpu_fallback(false);
                o.set_fill_fallback(false);
            }
            if hit {
                short_sets.insert(&set);
            }
            if failed {
                failed_sets.insert(&set);
            }
            if finished {
                short_sets.remove(&set);
                failed_sets.remove(&set);
            }
            #[cfg(test)]
            lock.lock()
                .expect("worker queue")
                .waits_by_test
                .push(crate::ai::waits_by_test());
            if panicked {
                // The base in hand may be half made; the shelf's were
                // finished before they were put there and no job
                // touches them, so they stay.
                discard_base(&mut base, gpu.as_ref());
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
            // A frame whose fills ran a model gives the card back
            // before the next frame loads its denoiser.
            give_back(
                &mut ai,
                [&mut people_ai, &mut set_ai, &mut masks_ai],
                mask_short,
                lock,
            );
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
        let developing = matches!(
            job,
            Job::Open { .. }
                | Job::Develop { .. }
                | Job::DevelopAside { .. }
                | Job::Fill { .. }
                | Job::Settle { .. }
        );
        // A picture with no Part on it lets the People model go: it
        // holds about a gigabyte, and two with a picture's crops.
        if matches!(job, Job::Open { .. }) {
            picking = false;
        }
        // Nor while a run of "use this person on all" still has frames
        // queued: it borrows the model between them, and would load it
        // from disk again for every frame while the user browses. Its
        // last frame lets it go.
        if let Job::Open { edit, .. } | Job::Develop { edit, .. } = &job
            && !has_live_part(edit)
            && !picking
            && lock.lock().expect("worker queue").people_on.is_empty()
        {
            ai.release_parts();
        }
        if developing && let Some(b) = &background {
            b.hold(true);
        }
        let mut blame = Blame::of(&job);
        // The file an open is for: one that panics before the file is
        // the worker's is a failed open of it, as one whose read fails.
        let opening = match &job {
            Job::Open { path, .. } => Some(path.clone()),
            _ => None,
        };
        // A panic in a job is the hook's to write, with its
        // backtrace; here it becomes the failure the UI is waiting
        // on, and the develop state is dropped rather than trusted.
        // The develop on screen, should the device be lost in a fill
        // round: the window's picture is its first round's texture,
        // which `shown` let go of.
        fill_redo = match (&job, &fills_due, &whole_due) {
            (Job::Fill { generation }, Some(due), _)
                if due.generation == *generation && due.highlight =>
            {
                Some(Job::Develop {
                    edit: due.edit.clone(),
                    generation: *generation,
                    turn: due.turn,
                    of: None,
                })
            }
            // As a fill round's: the first round's texture on screen,
            // which `shown` let go of.
            (Job::Settle { generation }, _, Some(due)) if due.generation == *generation => {
                Some(Job::Develop {
                    edit: due.edit.clone(),
                    generation: *generation,
                    turn: due.turn,
                    of: None,
                })
            }
            _ => None,
        };
        // A save waits on the settle: it does not give way to the frame
        // left or the editor closing.
        let settle_for_save = whole_due
            .as_ref()
            .is_some_and(|d| save_waits(pending.as_ref(), d, base.as_ref(), lenses.as_deref()));
        let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
            Job::Open {
                path,
                version,
                edit,
                generation,
                seed_blend,
                turn: wanted,
                stance_known,
                learned,
            } => {
                // What the file system says of the file before it is
                // read, so a write that lands during the decode is a
                // file the shelf has not seen; what the decode says of
                // the frame is added once it has said it.
                let coming = FileKey::of(&path);
                // A decoder that panics fails the read as one that errs,
                // in the same words a panic anywhere fails a job in, so
                // what follows (the fold, the failure remembered) is the
                // same.
                let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let read = open_told(&path, stance_known, &deliver);
                    #[cfg(test)]
                    {
                        let mut q = lock.lock().expect("worker queue");
                        let (held, panics) = (
                            q.open_held_by_test.take(),
                            q.open_panics_by_test
                                .take_if(|at| *at == super::OpenPanic::Reading),
                        );
                        drop(q);
                        if let Some((told, go)) = held {
                            let _ = told.send(());
                            let _ = go.recv();
                        }
                        if panics.is_some() {
                            panic!("a test's decoder");
                        }
                    }
                    read
                }))
                .unwrap_or_else(|payload| {
                    let message = panic_message(payload.as_ref());
                    lost_seen |= says_lost(&message);
                    Err(anyhow::anyhow!(super::panicked(&message)))
                });
                // A develop of this frame asked for while its file was
                // read (a slider's, the Original's, an undo) moved the
                // window on from the open's generation: it is this open
                // now, under its edit, generation, and turn, so what the
                // open says of the frame lands at the generation the
                // window waits on, and the develop runs once, under the
                // newer edit, its blend seeded as the open's would be.
                let (edit, generation, wanted) = match lock
                    .lock()
                    .expect("worker queue")
                    .take_develop_of(&path, &version, generation)
                {
                    Some(folded) => {
                        blame = Blame::Develop(folded.1);
                        folded
                    }
                    None => (edit, generation, wanted),
                };
                match read {
                    Ok((opened, m)) => {
                        #[cfg(test)]
                        if lock
                            .lock()
                            .expect("worker queue")
                            .open_panics_by_test
                            .take_if(|at| *at == super::OpenPanic::Landing)
                            .is_some()
                        {
                            panic!("a test's open, after its read");
                        }
                        failed_open = None;
                        let taken = m
                            .exif
                            .date_time_original
                            .as_deref()
                            .or(m.exif.create_date.as_deref())
                            .and_then(greycard_library::normalize_taken);
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
                            // A picture's "as shot" is its own white,
                            // which its offset sliders are reckoned from.
                            Input::Picture { meta, .. } => (
                                {
                                    let own = meta.own_white();
                                    (own.cct, own.duv)
                                },
                                None,
                                None,
                                SourceKind::Picture {
                                    space: meta.space.clone(),
                                    bits: meta.bits,
                                },
                            ),
                        };
                        // A merge's recipe, asked after the develop.
                        let recipe = match &opened {
                            Input::Raw(f) => {
                                f.dng.merged.clone().filter(|tag| tag.recipe.is_some())
                            }
                            Input::Picture { .. } => None,
                        };
                        let (lens_options, lens_look, used) = {
                            let (make, model, shot) = opened.identity();
                            let identity = (make, model, shot, opened.is_raw());
                            let (options, used) = crate::lens::options(
                                lenses.as_deref(),
                                identity,
                                &edit.lens.chosen,
                            );
                            (
                                options,
                                LensLook::of(lenses.clone(), identity, generation),
                                used,
                            )
                        };
                        deliver(Outcome::Opened {
                            generation,
                            as_shot,
                            frame,
                            profile,
                            lens: lens_report(lenses.as_deref(), &opened),
                            lens_options,
                            kind,
                            shot: {
                                let (make, model, shot) = opened.identity();
                                shot.summary(make, model)
                            },
                            taken,
                            blend,
                        });
                        // The look behind the open, asked once its list is
                        // out, so another list it finds lands after it; kept
                        // for the window to ask again.
                        lock.lock().expect("worker queue").lens_look = Some(lens_look.clone());
                        lens_look.ask(&edit.lens.chosen, used, false, &deliver);
                        // The base in hand is shelved for a return to its
                        // file, and this file's taken back if it is there:
                        // a raw's base on the shelf was made from the same
                        // bytes under the same edit's base, so the develop
                        // below keeps it, as it keeps one between two
                        // slider moves. The masks' models start again
                        // either way (`ai.forget`), keyed by the base's
                        // stamp on this file.
                        turn = wanted % 4;
                        let coming = coming.map(|k| k.decoded(&opened));
                        swap_base(
                            &mut base,
                            shelf.get_or_insert_with(Shelf::user),
                            (opened_file.take(), coming.as_ref().map(|k| (k, &opened))),
                            (&edit, turn, &|e: &Edit, x| stands_in(&ai, e, x)),
                            gpu.as_ref(),
                            lenses.as_deref(),
                        );
                        ai.forget(Some(path.clone()));
                        // The file is the worker's before the develop, so
                        // a panic in the develop leaves the worker and the
                        // UI, which has its Opened, on the same file.
                        input = Some(opened);
                        prefetch::opened(&mut opened_path, &mut left_path, path);
                        opened_file = coming;
                        let opened = input.as_ref().expect("just set");
                        let mut round = Round::first();
                        ai.clear_shorts();
                        asked_again = false;
                        // The learned denoiser short of the card may have
                        // the engine let go of what the develop makes
                        // again, as a removal may.
                        let release = gpu.as_ref().map(card_release);
                        #[cfg(test)]
                        let release =
                            release.or_else(|| fills_release_by_test.clone().map(|r| r.release()));
                        ai.set_card_release(release);
                        let (mut outcome, _) = develop_round(
                            opened,
                            &edit,
                            turn,
                            generation,
                            &mut base,
                            &mut ai,
                            (cache.as_ref(), lenses.as_deref()),
                            &mut gpu,
                            &deliver,
                            &mut round,
                            &Newer(lock),
                        );
                        ai.set_card_release(None);
                        fills_due =
                            FillsDue::after(&mut outcome, &edit, turn, round, None, base.as_mut());
                        shown = ShownTexture::of(&edit, turn, &outcome, base.as_ref());
                        after_develop(
                            lock,
                            masks::to_ask_again(&edit, base.as_ref(), &ai),
                            Instant::now(),
                        );
                        landed = base.is_some().then_some(generation);
                        developed =
                            matches!(outcome, Outcome::Developed { .. }).then_some(generation);
                        lock.lock().expect("worker queue").landed =
                            landed.map(|g| (g, opened_path.clone().unwrap_or_default()));
                        // The frame's masks made already go just ahead
                        // of its picture, so it is drawn with them.
                        if matches!(outcome, Outcome::Developed { .. })
                            && let Some(b) = &base
                        {
                            for mask in masks::kept_for_open(
                                &edit,
                                &learned,
                                b,
                                &mut ai,
                                opened_path.as_ref(),
                            ) {
                                deliver(mask);
                            }
                        }
                        // A first round makes no fill (its budget is
                        // none), so none is refused here: the fills'
                        // own rounds ask the card.
                        deliver(outcome);
                        // After the picture: the frames' heads are read
                        // (their whole files where two share a head).
                        if let (Some(tag), Some(path)) = (&recipe, &opened_path) {
                            deliver(Outcome::MergeAgain {
                                path: path.clone(),
                                can: crate::merge::can_denoise_again(path, tag),
                            });
                        }
                    }
                    Err(e) => {
                        landed = None;
                        lock.lock().expect("worker queue").landed = None;
                        // Its develops asked for after this are answered
                        // with it (`handle::holds`).
                        failed_open = Some((path, e.to_string()));
                        deliver(Outcome::Failed {
                            generation,
                            message: e.to_string(),
                        })
                    }
                }
            }
            Job::Develop {
                edit,
                generation,
                turn: wanted,
                of,
            } => {
                let held = holds(failed_open.as_ref(), opened_path.as_deref(), of.as_ref());
                if let Holds::Failed(message) = held {
                    deliver(Outcome::Failed {
                        generation,
                        message: message.to_string(),
                    });
                } else if held == Holds::Other {
                    not_held(of.as_ref(), opened_path.as_deref());
                } else if let Some(f) = &input {
                    turn = wanted % 4;
                    let seen = lock.lock().expect("worker queue").seen;
                    let mut round = Round::first_seen(seen);
                    ai.clear_shorts();
                    asked_again = false;
                    // As the open's (above).
                    let release = gpu.as_ref().map(card_release);
                    #[cfg(test)]
                    let release =
                        release.or_else(|| fills_release_by_test.clone().map(|r| r.release()));
                    ai.set_card_release(release);
                    let (mut outcome, _) = develop_round(
                        f,
                        &edit,
                        turn,
                        generation,
                        &mut base,
                        &mut ai,
                        (cache.as_ref(), lenses.as_deref()),
                        &mut gpu,
                        &deliver,
                        &mut round,
                        &Newer(lock),
                    );
                    ai.set_card_release(None);
                    fills_due =
                        FillsDue::after(&mut outcome, &edit, turn, round, None, base.as_mut());
                    shown = ShownTexture::of(&edit, turn, &outcome, base.as_ref()).map(|s| {
                        ShownTexture {
                            quick: round.quick.is_some(),
                            ..s
                        }
                    });
                    // Sharpened only as far as the window shows, on the
                    // device or on the CPU after the device declined:
                    // the whole sharpen next, unless a develop is asked
                    // first.
                    whole_due = round
                        .quick
                        .filter(|_| matches!(outcome, Outcome::Developed { .. }))
                        .map(|quick| WholeDue {
                            edit: edit.clone(),
                            turn,
                            generation,
                            quick,
                            parked: false,
                        });
                    after_develop(
                        lock,
                        masks::to_ask_again(&edit, base.as_ref(), &ai),
                        Instant::now(),
                    );
                    developed = matches!(outcome, Outcome::Developed { .. }).then_some(generation);
                    // No fill refused in a first round (see Open).
                    deliver(outcome);
                }
            }
            // The whole sharpen of a develop whose first round sharpened
            // only what the window showed: walked again from the point
            // kept before the sharpen, giving way to a newer develop
            // between the sharpen's submissions, and delivered as the
            // develop's own.
            Job::Settle { generation } => {
                if let (Some(due), Some(f)) = (whole_due.take(), &input)
                    && due.generation == generation
                {
                    turn = due.turn;
                    ai.clear_shorts();
                    // The view moved past what the first round sharpened
                    // (a pan, a zoom, a turn): a first round of what is
                    // seen now, from the point kept before the sharpen,
                    // delivered as the develop's, the settle after it.
                    // Not on the way out, where only the save is wanted.
                    let (seen, stopping) = {
                        let q = lock.lock().expect("worker queue");
                        (q.seen, q.stopping)
                    };
                    if !stopping && let Some(seen) = seen.filter(|s| !due.quick.shows(s)) {
                        let mut round = Round::first_seen(Some(seen));
                        let (outcome, _) = develop_round(
                            f,
                            &due.edit,
                            turn,
                            generation,
                            &mut base,
                            &mut ai,
                            (cache.as_ref(), lenses.as_deref()),
                            &mut gpu,
                            &deliver,
                            &mut round,
                            &crate::walk::NoHold,
                        );
                        shown =
                            ShownTexture::of(&due.edit, turn, &outcome, base.as_ref()).map(|s| {
                                ShownTexture {
                                    quick: round.quick.is_some(),
                                    ..s
                                }
                            });
                        whole_due = round
                            .quick
                            .filter(|_| matches!(outcome, Outcome::Developed { .. }))
                            .map(|quick| WholeDue { quick, ..due });
                        developed =
                            matches!(outcome, Outcome::Developed { .. }).then_some(generation);
                        deliver(outcome);
                        return;
                    }
                    // On the way out the settle is made for the save
                    // that waits on it and gives way to nothing, as the
                    // inline settle at an open: a newer develop or a
                    // view moved is never picked once the editor
                    // leaves, and giving way to it would only run the
                    // settle again until the stop's wait ran out.
                    let settling = Settling {
                        lock,
                        quick: due.quick,
                        held_for_save: settle_for_save,
                    };
                    let hold: &dyn crate::walk::Hold = if stopping {
                        &crate::walk::NoHold
                    } else {
                        &settling
                    };
                    let mut round = Round::settle();
                    let (outcome, _) = develop_round(
                        f,
                        &due.edit,
                        turn,
                        generation,
                        &mut base,
                        &mut ai,
                        (cache.as_ref(), lenses.as_deref()),
                        &mut gpu,
                        &deliver,
                        &mut round,
                        hold,
                    );
                    // Given way: to a newer develop, which takes its
                    // place, or to the view moved, which the settle's
                    // next go sharpens first.
                    if matches!(outcome, Outcome::Failed { .. }) && round.gave_way {
                        whole_due = Some(due);
                        return;
                    }
                    shown = ShownTexture::of(&due.edit, turn, &outcome, base.as_ref());
                    developed = matches!(outcome, Outcome::Developed { .. }).then_some(generation);
                    deliver(outcome);
                }
            }
            Job::DevelopAside {
                edit,
                generation,
                turn: wanted,
                of,
            } => {
                let held = holds(failed_open.as_ref(), opened_path.as_deref(), of.as_ref());
                if let Holds::Failed(message) = held {
                    deliver(Outcome::Failed {
                        generation,
                        message: message.to_string(),
                    });
                } else if held == Holds::Other {
                    not_held(of.as_ref(), opened_path.as_deref());
                } else if let Some(f) = &input {
                    ai.clear_shorts();
                    let outcome = develop_aside(
                        f,
                        &edit,
                        wanted % 4,
                        generation,
                        base.as_ref(),
                        &mut ai,
                        (cache.as_ref(), lenses.as_deref()),
                        &mut gpu,
                        &deliver,
                    );
                    deliver(outcome);
                }
            }
            // The next of the open frame's fills: a develop of the same
            // edit from before its retouch, the fill made, the rest left
            // for the rounds after, and its picture delivered as the
            // develop's own.
            Job::Fill { generation } => {
                if let (Some(due), Some(f)) = (fills_due.take(), &input)
                    && due.generation == generation
                {
                    turn = due.turn;
                    // The remove keeps its UNet for the fill after this.
                    ai.fills_queued(due.left > 1);
                    let mut round = Round::next(due.round.ahead);
                    ai.clear_shorts();
                    // Asked again after the card was given back across
                    // the models, a removal still short of 1024 runs at
                    // 512 in this round; cleared after, as below.
                    ai.set_fill_fallback(asked_again);
                    // A removal short of the card may have the engine
                    // let go of what a develop makes again, as the open
                    // frame's masks may; cleared after, and after a
                    // panic (`open_frame_masks_done`). Never the CPU.
                    let release = gpu.as_ref().map(card_release);
                    #[cfg(test)]
                    let release =
                        release.or_else(|| fills_release_by_test.clone().map(|r| r.release()));
                    ai.set_card_release(release);
                    let (mut outcome, _) = develop_round(
                        f,
                        &due.edit,
                        turn,
                        generation,
                        &mut base,
                        &mut ai,
                        (cache.as_ref(), lenses.as_deref()),
                        &mut gpu,
                        &deliver,
                        &mut round,
                        &Newer(lock),
                    );
                    ai.set_card_release(None);
                    ai.set_fill_fallback(false);
                    #[cfg(test)]
                    lock.lock()
                        .expect("worker queue")
                        .ran_at_by_test
                        .append(&mut ai.ran_at_by_test);
                    // Given way to a newer develop in the making of its
                    // highlight fill: nothing kept, nothing delivered,
                    // and the newer develop's rounds take over.
                    if round.gave_way {
                        return;
                    }
                    // The fills still ahead, before a queue that ends
                    // stuck lets them go: what one asked again has left.
                    let ahead = match &outcome {
                        Outcome::Developed { fills, .. } => fills.pending.len(),
                        _ => 0,
                    };
                    fills_due = FillsDue::after(
                        &mut outcome,
                        &due.edit,
                        turn,
                        round,
                        Some(due.left),
                        base.as_mut(),
                    );
                    let ended = fills_due.is_none();
                    if ended {
                        ai.release_fills();
                    }
                    shown = ShownTexture::of(&due.edit, turn, &outcome, base.as_ref());
                    after_develop(
                        lock,
                        masks::to_ask_again(&due.edit, base.as_ref(), &ai),
                        Instant::now(),
                    );
                    developed = matches!(outcome, Outcome::Developed { .. }).then_some(generation);
                    deliver(outcome);
                    // The last fill's picture is on its way: the card
                    // given back, a queue of one included, or across the
                    // worker's models after a fill it was short of.
                    let gave = ask_fills_again(
                        &mut ai,
                        [&mut people_ai, &mut set_ai, &mut masks_ai],
                        lock,
                        &mut asked_again,
                        &mut fills_due,
                        FillsDue {
                            edit: due.edit.clone(),
                            turn,
                            generation,
                            round: Round::next(round.ahead),
                            left: ahead.max(1),
                            highlight: false,
                        },
                    );
                    if !gave && ended {
                        give_back(
                            &mut ai,
                            [&mut people_ai, &mut set_ai, &mut masks_ai],
                            false,
                            lock,
                        );
                    }
                }
            }
            Job::Mask { key, shape, model } => {
                ai.clear_shorts();
                if matches!(shape, Shape::Part { .. }) {
                    picking = false;
                }
                let status = match &shape {
                    Shape::Part { route, .. } => crate::ai::part_on_cpu(route),
                    _ => crate::ai::part_on_cpu(&greycard_edit::mask::Route::Whole),
                };
                open_frame_masks(
                    &mut ai,
                    gpu.as_ref(),
                    &deliver,
                    Some(key),
                    opened_path.clone(),
                    status,
                );
                let outcome = match &base {
                    Some(b) => {
                        match ai.raster_retouched(
                            b.base(),
                            &b.image,
                            masks::retouched_of(b, None),
                            &b.edit,
                            b.meta.source,
                            b.turn,
                            key,
                            &shape,
                            model,
                        ) {
                            Ok((made, print)) => {
                                ai.given(key, &shape, model, print, b.print);
                                Outcome::Mask {
                                    key,
                                    shape,
                                    file: opened_path.clone(),
                                    raster: made.raster,
                                    provider: made.provider.map(|p| p.label()),
                                    seconds: made.seconds,
                                    note: made.note,
                                    part: made.part,
                                }
                            }
                            Err(message) => Outcome::MaskFailed { key, message },
                        }
                    }
                    None => Outcome::MaskFailed {
                        key,
                        message: "nothing developed yet".into(),
                    },
                };
                open_frame_masks_done(&mut ai);
                deliver(outcome);
                // A mask the card was short of, made on the CPU: given
                // back across the worker's models, so the next run
                // finds the room.
                if ai.take_mask_short() {
                    give_back(
                        &mut ai,
                        [&mut people_ai, &mut set_ai, &mut masks_ai],
                        true,
                        lock,
                    );
                }
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
                        if let Some(b) = &background {
                            b.reducer.masks_done(&frame, true);
                        }
                        deliver(Outcome::EditedKept { frame });
                    }
                    // A frame tried and found wanting stays so until a
                    // model is fetched: asked again now, it would be
                    // decoded and developed again for the same answer.
                    MasksMade::Short => {
                        if let Some(b) = &background {
                            b.reducer.masks_done(&frame, false);
                        }
                    }
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
                ai.clear_shorts();
                open_frame_masks(
                    &mut ai,
                    gpu.as_ref(),
                    &deliver,
                    None,
                    opened_path.clone(),
                    crate::ai::part_on_cpu(&greycard_edit::mask::Route::Whole),
                );
                let people = match &base {
                    Some(b) => ai
                        .people(b.stamp, &b.image, b.meta.source, b.turn)
                        .map(|(found, provider, seconds)| (found, provider.label(), seconds)),
                    None => Err("nothing developed yet".into()),
                };
                open_frame_masks_done(&mut ai);
                deliver(Outcome::People {
                    file: opened_path.clone(),
                    people,
                });
                if ai.take_mask_short() {
                    give_back(
                        &mut ai,
                        [&mut people_ai, &mut set_ai, &mut masks_ai],
                        true,
                        lock,
                    );
                }
            }
            Job::Landscape => {
                ai.clear_shorts();
                open_frame_masks(
                    &mut ai,
                    gpu.as_ref(),
                    &deliver,
                    None,
                    opened_path.clone(),
                    crate::ai::part_on_cpu(&greycard_edit::mask::Route::Whole),
                );
                let offers = match &base {
                    Some(b) => ai
                        .landscape(b.stamp, &b.image, &b.edit, b.meta.source)
                        .map(|(found, provider, seconds)| (found, provider.label(), seconds)),
                    None => Err("nothing developed yet".into()),
                };
                open_frame_masks_done(&mut ai);
                deliver(Outcome::Landscape {
                    file: opened_path.clone(),
                    offers,
                });
                if ai.take_mask_short() {
                    give_back(
                        &mut ai,
                        [&mut people_ai, &mut set_ai, &mut masks_ai],
                        true,
                        lock,
                    );
                }
            }
            Job::MaskPicture { edit, generation } => {
                // An ask behind a newer develop is that develop's to
                // answer, which makes its own picture.
                let current = developed == Some(generation);
                let picture = base
                    .as_mut()
                    .filter(|_| current)
                    .and_then(|b| super::masks::picture_for(b, &edit));
                deliver(Outcome::MaskPicture {
                    generation,
                    picture,
                });
            }
            Job::Fetch { .. }
            | Job::FetchRuntime { .. }
            | Job::FetchLenses
            | Job::Gpu
            | Job::Thumbnail { .. }
            | Job::CachedThumbnail { .. }
            | Job::ExportSet { .. }
            | Job::ExportFrame { .. }
            | Job::PersonOn(_)
            | Job::Keep { .. } => {
                unreachable!(
                    "fetches and thumbnails run on threads of their own; the device, a set's frames, a save and a person looked for are taken above"
                )
            }
        }));
        if held.is_ok() {
            fill_redo = None;
        }
        if let Err(payload) = held {
            fills_due = None;
            whole_due = None;
            ai.release_fills();
            // A mask's or a fill's job that panicked leaves nothing of
            // its own behind for a develop's or a save's masks or fills
            // to call, nor a mask's choice of the CPU.
            open_frame_masks_done(&mut ai);
            give_back(
                &mut ai,
                [&mut people_ai, &mut set_ai, &mut masks_ai],
                false,
                lock,
            );
            lost_seen |= says_lost(&panic_message(payload.as_ref()));
            // The base in hand is not trusted; the shelf's stay, as
            // after a panic in a set's frame.
            discard_base(&mut base, gpu.as_ref());
            shown = None;
            // The next stamp restarts with the base; what was cached
            // under the old ones goes with them.
            ai.forget(opened_path.clone());
            let message = panic_message(payload.as_ref());
            // An open that panicked before its file was the worker's is
            // a failed open of that file, as one whose read failed.
            if let Some(path) = opening.filter(|p| opened_path.as_ref() != Some(p)) {
                landed = None;
                lock.lock().expect("worker queue").landed = None;
                failed_open = Some((path, super::panicked(&message)));
            }
            if let Some(outcome) = blame.outcome(message) {
                deliver(outcome);
            }
        }
        if developing {
            edited.touch();
            if let Some(b) = &background {
                b.hold(false);
            }
            // A save that was waiting for this develop: its pictures now.
            if let Some(keep) = pending.take() {
                let hand = InHand {
                    opened: opened_path.as_deref(),
                    shown: shown.as_ref(),
                    base: base.as_mut(),
                    gpu: gpu.as_ref(),
                    lenses: lenses.as_deref(),
                };
                pending = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    keep_open(keep, false, hand, &mut ai, &edited, &deliver)
                }))
                .unwrap_or(None);
            }
        }
    }
}

/// The open frame's body and shot, for a look at the profile folder
/// behind its open ([`LensLook::ask`]): kept on the queue so the window
/// can ask one again ([`super::Worker::look_at_lens_profiles`]).
#[derive(Clone)]
pub(super) struct LensLook {
    lenses: Option<Arc<greycard_lens::Database>>,
    make: String,
    model: String,
    shot: greycard_core::raw::Shot,
    raw: bool,
    /// The open's generation, which the window takes the list for.
    generation: u64,
}

impl LensLook {
    pub(super) fn of(
        lenses: Option<Arc<greycard_lens::Database>>,
        (make, model, shot, raw): crate::lens::Identity,
        generation: u64,
    ) -> Self {
        Self {
            lenses,
            make: make.to_string(),
            model: model.to_string(),
            shot: shot.clone(),
            raw,
            generation,
        }
    }

    /// A look at the profile folder off the caller's thread, walking it
    /// again when `force` is set: the rows shown came from the listing
    /// of generation `used` (none for none held), so a look ending with
    /// another (a profile added, removed, or rewritten since, or the
    /// start's walk landing after the open) is another list, said as
    /// [`Outcome::LensOptions`] for the open, with `chosen` among them.
    pub(super) fn ask(
        self,
        chosen: &greycard_edit::LensChoice,
        used: Option<u64>,
        force: bool,
        deliver: &Deliver,
    ) {
        let Some(dir) = greycard_edit_io::profiles::store_dir() else {
            return;
        };
        let (chosen, deliver, profiles) = (chosen.clone(), deliver.clone(), dir.clone());
        greycard_lens::lcp::look_in(
            &dir,
            force,
            Box::new(move |listing| {
                if used == Some(listing.generation) {
                    return;
                }
                let options = crate::lens::options_from(
                    self.lenses.as_deref(),
                    (&self.make, &self.model, &self.shot, self.raw),
                    Some(&profiles),
                    &listing.entries,
                    &chosen,
                );
                deliver(Outcome::LensOptions {
                    generation: self.generation,
                    options,
                });
            }),
        );
    }
}

/// A develop sent for another file than the one in hand, which the
/// window does not send (`edit::develops_here`): made, it would be a
/// picture of the wrong file under this one's name, so it is left.
fn not_held(of: Option<&super::DevelopOf>, opened: Option<&std::path::Path>) {
    tracing::warn!(
        "a develop of {} left: the file in hand is {}",
        of.map_or_else(String::new, |o| file_label(&o.0)),
        opened.map_or_else(|| "none".to_string(), file_label)
    );
}

/// A newer open or develop of the window's, which a round making the
/// highlight fill gives way to. A develop aside does not: it leaves the
/// fills due as they are.
struct Newer<'a>(&'a Mutex<Queue>);

impl Newer<'_> {
    fn asked(&self) -> bool {
        #[allow(unused_mut)]
        let mut q = self.0.lock().expect("worker queue");
        // A test's develop, sent as the round asks its first.
        #[cfg(test)]
        if let Some(job) = q.develop_on_fill_ask.take() {
            q.develop = Some(job);
        }
        q.stopping || matches!(q.develop, Some(Job::Open { .. } | Job::Develop { .. }))
    }
}

impl crate::walk::Hold for Newer<'_> {
    fn give_way(&self, _next: &crate::stages::Stage) -> bool {
        self.asked()
    }

    fn give_way_inside(&self, _stage: &crate::stages::Stage) -> bool {
        self.asked()
    }
}

/// The develop on screen sharpened only as far as the window showed
/// it: what the settle walks.
pub(super) struct WholeDue {
    edit: Edit,
    turn: u8,
    generation: u64,
    /// What the picture on screen sharpened.
    quick: super::seen::Quick,
    /// The Original is up: the settle is not made, unless the frame is
    /// left or the editor closes with a save waiting on it.
    parked: bool,
}

/// Whether the save waiting, `pending`, is of the develop `due` is the
/// settle of: then the settle is made before it is let go.
fn save_waits(
    pending: Option<&Keep>,
    due: &WholeDue,
    base: Option<&FrameKept>,
    lenses: Option<&greycard_lens::Database>,
) -> bool {
    let (Some(keep), Some(b)) = (pending, base) else {
        return false;
    };
    keep.turn == due.turn
        && crate::stages::same_develop(&due.edit, &keep.edit, &b.facts_at(keep.turn), lenses)
}

/// The settle's hold: it gives way to a develop or an aside of the
/// frame (a save waiting on it then waits for the newer develop's, or
/// is let go for another edit's), to the view moved past what the
/// picture on screen sharpened, and to the editor closing or an open,
/// unless a save waits on it: then the frame's pictures are made first.
struct Settling<'a> {
    lock: &'a Mutex<Queue>,
    quick: super::seen::Quick,
    held_for_save: bool,
}

impl Settling<'_> {
    fn asked(&self) -> bool {
        #[allow(unused_mut)]
        let mut q = self.lock.lock().expect("worker queue");
        #[cfg(test)]
        {
            if let Some(job) = q.develop_on_fill_ask.take() {
                q.develop = Some(job);
            }
            if let Some(seen) = q.seen_on_settle_ask.take() {
                q.seen = Some(seen);
            }
        }
        let leaving = q.stopping || matches!(q.develop, Some(Job::Open { .. }));
        let newer = matches!(
            q.develop,
            Some(Job::Develop { .. } | Job::DevelopAside { .. })
        );
        (leaving && !self.held_for_save) || newer || q.seen.is_some_and(|s| !self.quick.shows(&s))
    }
}

impl crate::walk::Hold for Settling<'_> {
    fn give_way(&self, _next: &crate::stages::Stage) -> bool {
        self.asked()
    }

    fn give_way_inside(&self, _stage: &crate::stages::Stage) -> bool {
        self.asked()
    }
}

/// The fills the open frame's develop left to make after its picture
/// landed: what the next round walks.
pub(super) struct FillsDue {
    edit: Edit,
    turn: u8,
    generation: u64,
    round: Round,
    /// How many are left to make.
    left: usize,
    /// The highlight fill is among them.
    highlight: bool,
}

impl FillsDue {
    /// What a develop of `edit` at `turn` in `round` left to make, from
    /// its `outcome`: none when it failed or left none, nor when it was
    /// a fill's round, with `before` left before it, that made and
    /// failed none or left no fewer, so the rounds always end (a model
    /// that will not load fails the same fill each round). Rounds ended
    /// with fills left say none to come, so the window drops their
    /// outlines and its bar, and the frame's pictures are not kept for
    /// the edit's without them. Nor is its patched picture: a fill
    /// still to come may have had a stand-in there
    /// (`Models::stand_in`), which a later develop resuming from it
    /// would show for good; let go, the next walk asks again and shows
    /// the patch as it was.
    pub(super) fn after(
        outcome: &mut Outcome,
        edit: &Edit,
        turn: u8,
        round: Round,
        before: Option<usize>,
        base: Option<&mut FrameKept>,
    ) -> Option<Self> {
        let Outcome::Developed {
            generation, fills, ..
        } = outcome
        else {
            return None;
        };
        let moved = !fills.made.is_empty() || !fills.failed.is_empty();
        let stuck = before.is_some_and(|left| !moved || fills.pending.len() >= left);
        if stuck && !fills.pending.is_empty() {
            let left: Vec<String> = edit
                .retouch
                .patches
                .iter()
                .filter(|p| fills.pending.contains(&p.id))
                .map(|p| format!("fill {}: its rounds ended; left as it was", p.name()))
                .collect();
            for why in &left {
                tracing::info!("{why}");
            }
            fills.pending.clear();
            if let Some(h) = base {
                h.filling = false;
                h.left_out.extend(left);
                h.kept.drop_unmade();
            }
        }
        if fills.pending.is_empty() && !fills.highlight_pending {
            return None;
        }
        Some(Self {
            edit: edit.clone(),
            turn,
            generation: *generation,
            round,
            left: fills.pending.len(),
            highlight: fills.highlight_pending,
        })
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
pub(super) fn during_develop(threads: usize) -> usize {
    std::env::var("GREYCARD_THUMB_DURING_DEVELOP")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(0, |n| n.min(threads))
}

/// A path's file name, for a status line.
pub(super) fn file_label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The device is lost: the context goes, and with it everything the
/// develop in hand kept on the device (the uploaded picture, the
/// sharpen's input, the dehaze's reduced copy), and the texture on
/// screen. The base's own picture is on the CPU and stays; one whose CA
/// or lens ran on the device is made again by the next develop, as the
/// export's would be. Said once. With `again`, the develop that left
/// its picture on the lost device is returned, to be made again on the
/// CPU under the same generation, so the window gets its picture back.
pub(super) fn lose_gpu(
    gpu: &mut Option<greycard_gpu::Context>,
    base: &mut Option<FrameKept>,
    shown: &mut Option<ShownTexture>,
    again: bool,
) -> Option<Job> {
    if let Some(ctx) = gpu.take() {
        ctx.mark_lost();
    }
    if let Some(b) = base.as_mut() {
        b.kept.forget_device();
    }
    let on_screen = shown.take();
    let redo = on_screen.filter(|_| again).map(|s| Job::Develop {
        edit: s.edit,
        generation: s.generation,
        turn: s.turn,
        of: None,
    });
    tracing::warn!(
        "the GPU device was lost; the engine's ops on the CPU from now on{}",
        if redo.is_some() {
            ", the picture on screen developed again there"
        } else {
            ""
        }
    );
    redo
}

/// Forget the base, and with it the ops' working textures on the GPU:
/// a develop that panicked is not trusted, and develops afresh.
fn discard_base(base: &mut Option<FrameKept>, gpu: Option<&greycard_gpu::Context>) {
    if base.take().is_some()
        && let Some(gpu) = gpu
    {
        gpu.release();
    }
}

/// A new lens database: the base in hand and every base on the shelf
/// were made under the old one, and the lens is in the base, so all of
/// them go, the learned pair with them, as the base alone went before
/// there was a shelf. A shelf not made yet stays unmade.
pub(super) fn forget_bases(
    base: &mut Option<FrameKept>,
    shelf: &mut Option<Shelf>,
    gpu: Option<&greycard_gpu::Context>,
) {
    discard_base(base, gpu);
    if let Some(shelf) = shelf {
        shelf.clear();
    }
}

/// The edit of the file at `path` changed without an open of it, to
/// what it holds `now` when the window had that: its bases on the
/// shelf that no open would take now are let go and said, rather than
/// sit until the budget pushes them out. A shelf not made yet stays
/// unmade.
pub(super) fn drop_changed(
    shelf: Option<&mut Shelf>,
    path: &std::path::Path,
    now: Option<&FileNow>,
    lenses: Option<&greycard_lens::Database>,
) {
    let Some(shelf) = shelf else {
        return;
    };
    let gone = shelf.drop_changed(path, now, lenses);
    if gone > 0 {
        prefetch::note(format!(
            "shelved base of {} dropped: edit changed{}",
            file_label(path),
            if gone > 1 {
                format!(" ({gone} bases)")
            } else {
                String::new()
            }
        ));
    }
}

/// Whether the engine's own path would stand in for the learned
/// denoiser an edit asks for on a frame, X-Trans or not ([`stands_in`]).
pub(super) type StandsIn<'a> = &'a dyn Fn(&Edit, bool) -> bool;

/// Whether a frame opened is an X-Trans raw, whose tier is its own
/// network.
fn input_xtrans(input: &Input) -> bool {
    matches!(input, Input::Raw(f)
        if matches!(&f.layout, greycard_core::raw::SensorLayout::Cfa(p) if p.is_xtrans()))
}

/// Whether the frame a base was made of is an X-Trans raw.
fn facts_xtrans(facts: &crate::stages::Facts) -> bool {
    matches!(&facts.input, crate::stages::InputKind::Raw { pattern: Some(p), .. } if p.is_xtrans())
}

/// The open frame's base set aside for the frame `coming` to be
/// opened under `edit` at `turn`: what the frame in hand kept put on
/// the shelf as the frame `leaving`'s, its learned pair with it, and
/// the coming frame's taken back from the shelf when it is there for
/// the same file with a base the walk of `edit` at `turn` starts from,
/// the engine's own path standing in for the learned denoiser where
/// `stands_in` says it would for an edit ([`stands_in`]): for the
/// coming frame's, and for the leaving frame's, whose stand-in is
/// shelved only while it would still stand in.
/// Whatever the GPU held for the base in hand goes, as when the base
/// was discarded; a base taken back is uploaded again by its develop.
///
/// The base in hand is shelved before the coming one is looked for, so
/// a frame opened again (another version of the same file) finds its
/// own; the shelf is brought under its budget only after, so the base
/// taken back is never one it lets go. A file the file system would
/// not answer for (`None`) is not shelved or taken.
pub(super) fn swap_base(
    base: &mut Option<FrameKept>,
    shelf: &mut Shelf,
    (leaving, coming): (Option<FileKey>, Option<(&FileKey, &Input)>),
    (edit, turn, stands_in): (&Edit, u8, StandsIn<'_>),
    gpu: Option<&greycard_gpu::Context>,
    lenses: Option<&greycard_lens::Database>,
) {
    if let Some(b) = base.take() {
        if let Some(gpu) = gpu {
            gpu.release();
        }
        if let Some(file) = leaving {
            let standing = stands_in(&b.edit, facts_xtrans(&b.facts));
            shelf.put(file, b, standing);
        }
    }
    if let Some((file, opened)) = coming
        && let Some(mut b) = shelf.take(
            file,
            edit,
            turn,
            (gpu.is_some(), stands_in(edit, input_xtrans(opened))),
            lenses,
        )
    {
        // The shelf matched the file's path, stat, and decode: the
        // frame opened is the one the base was made from, read again.
        b.kept.adopt(&opened.walked());
        let line = format!("the base of {} from the shelf", file_label(&file.path));
        tracing::info!("{line}");
        prefetch::tap(&line);
        *base = Some(b);
    }
    let gone = shelf.fit();
    tracing::info!(
        "the shelf of frames left: {}{}",
        shelf.said(),
        if gone > 0 {
            format!(", {gone} let go")
        } else {
            String::new()
        }
    );
}

/// Whether the learned denoiser `edit` asks for is not to be had as a
/// walk of it starts on a frame, X-Trans where `xtrans`, so the
/// engine's own path stands in from the first step: the tier's model
/// for that mosaic is not in the store, or there is no runtime
/// ([`Ai::denoiser_there`], what the walk asks first). A stand-in the
/// walk falls to later, for the card's memory or a failed run, is not
/// this: the next walk asks the card again.
pub(super) fn stands_in(ai: &Ai, edit: &Edit, xtrans: bool) -> bool {
    edit.noise
        .tier()
        .and_then(|t| greycard_ai::denoiser_for(t, xtrans))
        .is_some_and(|model| ai.denoiser_there(model).is_err())
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

/// What a develop asks of the masks: those whose fills it moved at
/// once ([`ask_again`]), those its new base asks for held until the
/// develops settle ([`masks::settle`]).
pub(super) fn after_develop(
    lock: &Mutex<Queue>,
    (now, rebased): (Vec<masks::Ask>, Vec<masks::Ask>),
    at: Instant,
) {
    ask_again(lock, now);
    masks::settle(
        &mut lock.lock().expect("worker queue").settling,
        rebased,
        at,
    );
}

/// The masks a new base asked for moved to the masks wanted, once the
/// develops have been quiet long enough at `now`; a key the panel has
/// queued meanwhile keeps the panel's ask.
pub(super) fn settle_due(q: &mut Queue, now: Instant) {
    if let Some(asks) = masks::settled(&mut q.settling, now) {
        for ask in asks {
            if !q.masks.iter().any(|(k, _, _)| *k == ask.0) {
                q.masks.push_back(ask);
            }
        }
    }
}

/// The learned masks a develop's retouch changed the patches of, queued
/// again behind it: the panel is given each one as it is made. A key
/// already queued is left as it is: that ask is the panel's, newer than
/// the shape recorded here, and is made under the same patches.
pub(super) fn ask_again(
    lock: &Mutex<Queue>,
    again: Vec<(Key, Shape, Option<&'static greycard_ai::Model>)>,
) {
    if again.is_empty() {
        return;
    }
    let mut q = lock.lock().expect("worker queue");
    for (key, shape, model) in again {
        if !q.masks.iter().any(|(k, _, _)| *k == key) {
            q.masks.push_back((key, shape, model));
        }
    }
}

/// The open frame's Mask, People, or Landscape job begins: a learned
/// mask short of the card may run on the CPU, after the engine lets go
/// of what it can ([`card_release`]), and the window is told before
/// the run with `status`, the line the parts' runs on the CPU say,
/// for the mask at `key` (none for People and Landscape) of `file`.
fn open_frame_masks(
    ai: &mut Ai,
    gpu: Option<&greycard_gpu::Context>,
    deliver: &Deliver,
    key: Option<crate::ai::Key>,
    file: Option<PathBuf>,
    status: &'static str,
) {
    ai.set_cpu_fallback(true);
    ai.set_card_release(gpu.map(card_release));
    let told = deliver.clone();
    ai.set_on_cpu(Some(Box::new(move || {
        told(Outcome::MaskOnCpu {
            key,
            file: file.clone(),
            status,
        })
    })));
}

/// The open frame's mask job is over, or panicked: the CPU is not to be
/// fallen to again until the next, and nothing of it is left to call.
fn open_frame_masks_done(ai: &mut Ai) {
    ai.set_cpu_fallback(false);
    ai.set_fill_fallback(false);
    ai.set_card_release(None);
    ai.set_on_cpu(None);
}

/// What the open frame's learned masks, removals, and learned denoiser,
/// and an export's, may let go of the card when it is short of them
/// ([`Ai::set_card_release`]): the context's kept working sets, the
/// sharpen's and the local contrast's, which the next develop makes
/// again, as after an export of another size. The open frame's own
/// points on the device and the picture on screen are not the
/// context's and stay; the shelf and the prefetch hold nothing on the
/// device ([`crate::walk::Kept::shelve`]). Called on the worker's
/// thread inside a mask's job, with no develop running, or inside a
/// walk (a develop's or an export's at its learned denoiser or its
/// retouch), between the ops' runs; it takes each op's lock on its
/// set, which only a running op holds, never the queue's. The walk
/// makes the sets again past them.
pub(super) fn card_release(ctx: &greycard_gpu::Context) -> crate::ai::CardRelease {
    let releaser = ctx.releaser();
    Box::new(move || {
        let freed = releaser.release();
        vec![
            ("the sharpen's working set", freed.sharpen),
            ("the local contrast's working set", freed.local_contrast),
        ]
    })
}

/// What a test has the open frame's develops, fills, and exports let go
/// of the card where the worker has no device: `bytes` a release, each
/// counted in `calls`.
#[cfg(test)]
#[derive(Clone)]
pub(super) struct FillsReleaseByTest {
    pub bytes: u64,
    pub calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl FillsReleaseByTest {
    fn release(self) -> crate::ai::CardRelease {
        Box::new(move || {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            vec![("a test's working set", self.bytes)]
        })
    }
}

/// [`crate::ai::give_back_card`] over the worker's models: its own and
/// those it keeps for People, an export's other frames, and the masks
/// of frames not open, which share ONNX Runtime's one WebGPU context.
fn give_back(ai: &mut Ai, others: [&mut Option<Ai>; 3], force: bool, lock: &Mutex<Queue>) -> bool {
    let mut all: Vec<&mut Ai> = vec![ai];
    all.extend(others.into_iter().filter_map(Option::as_mut));
    #[allow(unused_mut)]
    let mut gave = crate::ai::give_back_card(&mut all, force);
    #[cfg(test)]
    {
        let mut q = lock.lock().expect("worker queue");
        let elsewhere = if force { q.elsewhere_by_test } else { 0 };
        if elsewhere > 0
            && let Some(free) = all[0].free_memory_by_test.as_mut()
        {
            *free += elsewhere;
            gave = true;
        }
        if gave {
            q.gave_back_by_test += 1;
        }
    }
    #[cfg(not(test))]
    let _ = lock;
    gave
}

/// Sets whose frames met a short the card did not recover from, whose
/// later frames ask once: held weakly and matched by the set itself, so
/// one let go or canceled is forgotten and never mistaken for a new
/// set made where it was.
#[derive(Default)]
pub(super) struct ShortSets(Vec<std::sync::Weak<crate::queue::Set>>);

impl ShortSets {
    pub(super) fn contains(&self, set: &Arc<crate::queue::Set>) -> bool {
        self.0
            .iter()
            .any(|w| w.upgrade().is_some_and(|s| Arc::ptr_eq(&s, set)))
    }

    pub(super) fn insert(&mut self, set: &Arc<crate::queue::Set>) {
        self.0
            .retain(|w| w.upgrade().is_some_and(|s| !s.is_canceled()));
        if !self.contains(set) {
            self.0.push(Arc::downgrade(set));
        }
    }

    pub(super) fn remove(&mut self, set: &Arc<crate::queue::Set>) {
        self.0.retain(|w| {
            w.upgrade()
                .is_some_and(|s| !Arc::ptr_eq(&s, set) && !s.is_canceled())
        });
    }
}

/// After a develop round: a fill the card was short of gives the card
/// back across the worker's models, and when that let anything go, or
/// a removal at 1024 was refused, which now runs at 512 where 1024 is
/// still short ([`Ai::card_given_back`]), the fills are asked for again
/// with `again`, once a develop. True when the card was given back.
fn ask_fills_again(
    ai: &mut Ai,
    others: [&mut Option<Ai>; 3],
    lock: &Mutex<Queue>,
    asked_again: &mut bool,
    fills_due: &mut Option<FillsDue>,
    again: FillsDue,
) -> bool {
    if !ai.take_fill_short() {
        return false;
    }
    let gave = give_back(ai, others, true, lock);
    let large = ai.card_given_back();
    if (gave || large) && fills_due.is_none() && !*asked_again {
        *asked_again = true;
        *fills_due = Some(again);
    }
    gave
}

/// Whether [`give_back`] would let anything go now
/// ([`crate::ai::give_back_due`]).
fn give_back_due(ai: &Ai, others: [&Option<Ai>; 3]) -> bool {
    let mut all: Vec<&Ai> = vec![ai];
    all.extend(others.into_iter().filter_map(Option::as_ref));
    crate::ai::give_back_due(&all)
}
