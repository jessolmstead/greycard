//! The develop: the open frame's picture, a walk of the window's list
//! from what its last walks kept.

use super::*;
use crate::highlight_fill::KeptFill;
use crate::stages::{Keep, LEARNED_DENOISER, Skip, Stage, Step};
use crate::walk::{Buf, Hold, NoHold, NoLearned, Place, walk};

/// [`develop_round`] with every fill made in the one walk, as the
/// tests develop.
// The job, the frame kept, and each model, device, and channel, held apart.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) fn develop_job(
    input: &Input,
    edit: &Edit,
    turn: u8,
    generation: u64,
    held: &mut Option<FrameKept>,
    ai: &mut Ai,
    cache: Option<&greycard_ai::DenoiseCache>,
    lenses: Option<&greycard_lens::Database>,
    gpu: &mut Option<greycard_gpu::Context>,
    deliver: &Deliver,
) -> (Outcome, Option<Arc<WorkingImage>>) {
    develop_round(
        input,
        edit,
        turn,
        generation,
        held,
        ai,
        (cache, lenses),
        gpu,
        deliver,
        &mut Round::all(),
        &NoHold,
    )
}

/// How many of a develop's fills a walk of it makes, and how many the
/// develop had ahead and has made: the window's develop delivers its
/// picture with no fill made ([`Round::first`]), then the worker walks
/// it again for each fill left ([`Round::next`]), so the picture comes
/// first and each fill lands as it is made. The learned highlight fill
/// goes the same way: a first round whose walk would have to make it
/// delivers the picture without it, or with the last fields standing
/// in, and the next round makes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Round {
    budget: Option<usize>,
    pub ahead: (usize, usize),
    /// A develop aside ([`develop_aside`]): what the device holds for
    /// the open frame's own develops is left as it is.
    aside: bool,
    /// What the round does with a highlight fill its walk would make.
    highlight: Highlight,
    /// The round gave way to a newer develop part way through the
    /// highlight fill's making: nothing of it was kept, and its outcome
    /// is not to be delivered.
    pub gave_way: bool,
    /// What the window shows, which the round sharpens alone where it
    /// may ([`super::seen`]); none for the whole frame.
    pub seen: Option<super::Seen>,
    /// The round sharpened only what it was shown, and what that was:
    /// the settle is to follow.
    pub quick: Option<super::seen::Quick>,
    /// The settle: the whole sharpen after a round that sharpened only
    /// what was shown, giving way to a newer develop.
    pub settle: bool,
    /// A first round for a zoom in past what the picture on screen ran,
    /// which gives way as the settle does.
    pub under_hold: bool,
}

/// What a round does with a learned highlight fill its walk would have
/// to make (none kept past it, nor its fields for the same steps).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Highlight {
    /// Makes it in the walk: an export's way, a test's, and a develop
    /// aside's, which has no round after it.
    Inline,
    /// Delivers the picture without it, the develop of the edit with
    /// the fill off, or with the last fields standing in under another
    /// white balance ([`crate::stages::stands_in_for`]), and leaves it
    /// to the next round ([`FillReport::highlight_pending`]).
    First,
    /// Makes it, giving way to a newer develop until it is made.
    Make,
}

impl Round {
    /// Every fill made in the one walk, as an export makes them.
    #[cfg(test)]
    pub(super) fn all() -> Self {
        Self {
            budget: None,
            ahead: (0, 0),
            aside: false,
            highlight: Highlight::Inline,
            gave_way: false,
            seen: None,
            quick: None,
            settle: false,
            under_hold: false,
        }
    }

    /// The window's develop: its picture with every fill kept in
    /// place and none made.
    pub(super) fn first() -> Self {
        Self {
            budget: Some(0),
            ahead: (0, 0),
            aside: false,
            highlight: Highlight::First,
            gave_way: false,
            seen: None,
            quick: None,
            settle: false,
            under_hold: false,
        }
    }

    /// The window's develop sharpened first as far as `seen` shows.
    pub(super) fn first_seen(seen: Option<super::Seen>) -> Self {
        Self {
            seen,
            ..Self::first()
        }
    }

    /// The settle after a first round that sharpened only what was
    /// shown: the whole develop, every fill kept in place as the first
    /// round's, giving way to a newer develop.
    pub(super) fn settle() -> Self {
        Self {
            settle: true,
            ..Self::first()
        }
    }

    /// A first round of what `seen` shows, for a zoom in past the
    /// iterations of the picture on screen: sharpened as far as it
    /// shows, giving way to a newer develop as the settle does.
    pub(super) fn zoomed(seen: super::Seen) -> Self {
        Self {
            under_hold: true,
            ..Self::first_seen(Some(seen))
        }
    }

    /// The next fill of a develop whose rounds so far left `ahead`, and
    /// its highlight fill when it is still to make.
    pub(super) fn next(ahead: (usize, usize)) -> Self {
        Self {
            budget: Some(1),
            ahead,
            aside: false,
            highlight: Highlight::Make,
            gave_way: false,
            seen: None,
            quick: None,
            settle: false,
            under_hold: false,
        }
    }
}

/// How a round walks its list.
enum Walk<'a> {
    /// From what the frame keeps, never giving way.
    Plain,
    /// The list without its highlight fill, the camera picture kept
    /// where the fill would run for the round after to make it from:
    /// the picture is the fill-off edit's, to the bit, and so is what
    /// the frame says of it (the edit its masks see, their print, and
    /// the develop it last delivered).
    Unfilled,
    /// The list with these fields standing in for its highlight fill,
    /// the camera picture kept before them.
    StandIn(&'a Arc<KeptFill>),
    /// The fill's round ([`crate::walk::walk_fill_round`]), asking
    /// `hold` up to the fill's end whether to give way.
    Fill(&'a dyn Hold),
    /// From what the frame keeps, asking `hold` before each stage and
    /// inside the sharpen whether to give way: the settle's.
    Settle(&'a dyn Hold),
    /// As `Plain`, sharpened as far as the window shows, asking `hold`
    /// as the settle does: a zoom in's first round.
    Part(&'a dyn Hold),
}

/// Develop `edit` on the open frame at `turn`: a walk of the window's
/// list ([`window_steps`]) from the last point `held` kept whose steps
/// are this list's, with `ai`'s models. With `gpu`, every stage with a
/// version on the device runs there: the CA and the lens one at a time
/// on pictures in memory, the Detail section and the sharpen as one
/// run with the picture before the sharpen kept on the device, ending
/// at the viewport's texture, so a sharpen slider costs the sharpen
/// alone. Without it (a session with no device), the reference's
/// path from points the device never touched, and the picture comes
/// back with its half floats. A device that declines a picture runs
/// that run on the CPU with its context kept; any other device error
/// lets the context go for the session.
/// The walk makes as many fills as `round` says and notes in it how
/// far the develop's fills have got.
///
/// A learned highlight fill the walk would have to make (no point kept
/// past it, nor fields for the same steps) is the round's to place: a
/// first round leaves it pending and delivers the picture at once,
/// the edit's with the fill off (its lists, keys, prints, and kept
/// points that edit's own) or, under another white balance of the same
/// steps, the last fields applied through the new one on a copy of
/// what the frame keeps, which the frame never keeps; the next round
/// makes it, asking `hold` up to its end whether to give way, and gives
/// way with the frame's kept points as they were.
#[allow(clippy::too_many_arguments)]
pub(super) fn develop_round(
    input: &Input,
    edit: &Edit,
    turn: u8,
    generation: u64,
    held: &mut Option<FrameKept>,
    ai: &mut Ai,
    stores: (
        Option<&greycard_ai::DenoiseCache>,
        Option<&greycard_lens::Database>,
    ),
    gpu: &mut Option<greycard_gpu::Context>,
    deliver: &Deliver,
    round: &mut Round,
    hold: &dyn Hold,
) -> (Outcome, Option<Arc<WorkingImage>>) {
    let facts = input.walked().facts(turn);
    let (steps, plan) = window_steps(edit, &facts, stores.1, gpu.is_some());
    let h = match held {
        Some(h) if h.facts == FrameKept::new(&facts).facts => h,
        _ => held.insert(FrameKept::new(&facts)),
    };
    // The places the walk keeps to: all the CPU's with no device.
    let plan: Vec<Place> = if gpu.is_some() {
        plan
    } else {
        vec![Place::Cpu; plan.len()]
    };
    let pending = steps.fill_at().filter(|&at| {
        round.highlight != Highlight::Inline
            && !round.aside
            && !h.kept.starts_past(&steps, &plan, at)
            && !h
                .kept
                .fill()
                .is_some_and(|k| k.steps[..] == steps.steps[..=at])
    });
    let Some(at) = pending else {
        // The settle, the rest of a round that sharpened only what was
        // shown, which gives way to a newer develop.
        let how = if round.settle {
            Walk::Settle(hold)
        } else if round.under_hold {
            Walk::Part(hold)
        } else {
            Walk::Plain
        };
        return develop_with(
            input, edit, turn, generation, h, ai, stores, gpu, deliver, round, how,
        );
    };
    if round.highlight == Highlight::Make {
        return develop_with(
            input,
            edit,
            turn,
            generation,
            h,
            ai,
            stores,
            gpu,
            deliver,
            round,
            Walk::Fill(hold),
        );
    }
    let stand = h
        .kept
        .fill()
        .filter(|k| crate::stages::stands_in_for(&k.steps, &steps.steps[..=at]))
        .cloned();
    let (mut outcome, picture) = match &stand {
        // On a copy of the frame's: nothing of the stand-in's walk is
        // kept, nor the base, stamp, and edit the masks see, but the
        // camera picture it kept before the fill, which is the fill's
        // list's and the round after's to start from.
        Some(k) => {
            let mut aside = FrameKept::new(&facts);
            aside.kept = h.kept.clone();
            aside.stamp = h.stamp;
            aside.turn = h.turn;
            aside.image = h.image.clone();
            aside.edit = h.edit.clone();
            aside.print = h.print;
            aside.from_shelf = std::mem::take(&mut h.from_shelf);
            let landed = develop_with(
                input,
                edit,
                turn,
                generation,
                &mut aside,
                ai,
                stores,
                gpu,
                deliver,
                round,
                Walk::StandIn(k),
            );
            h.kept.take_camera(&mut aside.kept);
            landed
        }
        None => develop_with(
            input,
            edit,
            turn,
            generation,
            h,
            ai,
            stores,
            gpu,
            deliver,
            round,
            Walk::Unfilled,
        ),
    };
    tracing::debug!(
        "the highlight fill left for the next round; the picture is {}",
        if stand.is_some() {
            "the last fields' stand-in"
        } else {
            "without it"
        }
    );
    if let Outcome::Developed { fills, .. } = &mut outcome {
        fills.highlight_pending = true;
        h.filling = true;
    }
    (outcome, picture)
}

/// [`develop_round`]'s walk of `edit` on the frame kept `h`, as `how`
/// says.
#[allow(clippy::too_many_arguments)]
fn develop_with(
    input: &Input,
    edit: &Edit,
    turn: u8,
    generation: u64,
    h: &mut FrameKept,
    ai: &mut Ai,
    (cache, lenses): (
        Option<&greycard_ai::DenoiseCache>,
        Option<&greycard_lens::Database>,
    ),
    gpu: &mut Option<greycard_gpu::Context>,
    deliver: &Deliver,
    round: &mut Round,
    how: Walk<'_>,
) -> (Outcome, Option<Arc<WorkingImage>>) {
    let start = Instant::now();
    let walking = input.walked();
    let facts = walking.facts(turn);
    let (steps, plan) = window_steps(edit, &facts, lenses, gpu.is_some());
    // The first round's lists: the camera picture kept where the fill
    // would run, and the fill taken out or stood in for; each step's
    // place carried over, the keep's the CPU's as every keep's is.
    let (steps, plan) = match (&how, steps.fill_at()) {
        (Walk::Unfilled | Walk::StandIn(_), Some(at)) => {
            let mut plan = plan;
            plan.insert(at, Place::Cpu);
            let kept = steps.kept_before_fill();
            match how {
                Walk::StandIn(k) => (kept.with_stand_in(k), plan),
                _ => {
                    plan.remove(at + 1);
                    (kept.without_fill(), plan)
                }
            }
        }
        _ => (steps, plan),
    };
    // The window's first round on the device sharpens only what the
    // window shows, where its edit has no fill to make; the settle
    // after it sharpens the rest. The size is the developed picture's,
    // at the turn the window draws.
    round.quick = None;
    let steps = match (&how, round.seen.as_ref()) {
        (Walk::Plain | Walk::Part(_), Some(seen))
            if gpu.is_some() && super::seen::may_quicken(edit) =>
        {
            match super::seen::quickened(&steps, seen, facts.full(), h.meta.radius) {
                Some((steps, quick)) => {
                    round.quick = Some(quick);
                    steps
                }
                None => steps,
            }
        }
        _ => steps,
    };
    // What the frame says it shows: the fill-off edit, when the fill is
    // left for the next round.
    let off;
    let edit = match how {
        Walk::Unfilled => {
            off = Edit {
                highlight_fill: None,
                ..edit.clone()
            };
            &off
        }
        _ => edit,
    };
    // The fill's round leaves the frame as it was until it is through:
    // given way, or failed before the fill was made, it is put back.
    let fill_round = matches!(how, Walk::Fill(_));
    let from_shelf = h.from_shelf;
    if !fill_round {
        h.from_shelf = false;
        h.developed_as = None;
        // The developed picture the last walk kept is the export's and
        // the save's to start from, not the window's: a walk from it
        // would not run the sharpen, whose blend and measure the
        // viewport shows, and a walk with a device ends at the
        // viewport's texture. (The fill round's list never starts from
        // it: the picture without the fill is another list's.)
        h.kept.take_working(Keep::Developed);
    }
    let print = ai.base_print(edit, turn, crate::mask_base::Size::Full);
    // A round whose base is not kept makes it again, the learned
    // denoiser with it: not beside a UNet the queue held.
    if round.budget.is_some_and(|b| b > 0) && !h.kept.holds(Keep::Base) {
        ai.let_go_held_unet();
    }
    let mut models = WorkerModels::new(ai, cache, deliver, generation);
    models.edit = edit.clone();
    models.turn = turn;
    models.budget = round.budget;
    models.ahead = round.ahead;
    // A fill to make: the walk starts before the retouch, whatever a
    // point past it would serve.
    if round.budget.is_some_and(|b| b > 0) && !fill_round {
        h.kept.drop_unmade();
    }
    // What the frame holds of the picture without the fill besides its
    // kept points: its base, its bands, the refined masks' picture, and
    // its guide, set aside for the fill's round, so the round's walk is
    // the last to hold that picture and lets it go once the fill is
    // made.
    let unfilled = fill_round.then(|| {
        (
            std::mem::replace(&mut h.image, Arc::new(WorkingImage::new(0, 0))),
            h.planes.take(),
            h.mask_picture.take(),
            std::mem::replace(&mut h.guide, Arc::new(greycard_render::finish::Guide::NONE)),
        )
    });
    // The card's free memory before and after a develop on it, for the
    // log: what the window's own develop takes without asking, which
    // the floor the models' sessions are kept over is measured from.
    let free_before = gpu.as_ref().and_then(|_| crate::ai::card_free_for_log());
    let (walked, unfilled) = match how {
        Walk::Settle(hold) | Walk::Part(hold) => (
            walk(&steps, &plan, &walking, &mut h.kept, &mut models, gpu, hold),
            None,
        ),
        Walk::Plain | Walk::Unfilled | Walk::StandIn(_) => (
            walk(
                &steps,
                &plan,
                &walking,
                &mut h.kept,
                &mut models,
                gpu,
                &NoHold,
            ),
            None,
        ),
        Walk::Fill(hold) => crate::walk::walk_fill_round(
            &steps,
            &plan,
            &walking,
            &mut h.kept,
            &mut models,
            gpu,
            hold,
            unfilled,
        ),
    };
    let unfilled = unfilled.flatten();
    // A walk of the fill's list made or kept its filled picture: the
    // camera picture kept for it is of no more use.
    if matches!(how, Walk::Plain) && walked.is_ok() && steps.fill_at().is_some() {
        h.kept.forget_camera();
    }
    // The frame as it was, handed back with its kept points by a round
    // that gave way or failed before its fill was made; or emptied by
    // one that failed after it: then the next develop makes its base
    // anew, and the frame says it shows nothing yet.
    if let Some((image, planes, mask_picture, guide)) = unfilled {
        h.image = image;
        h.planes = planes;
        h.mask_picture = mask_picture;
        h.guide = guide;
    } else if fill_round && walked.is_err() {
        debug_assert!(h.kept.is_empty());
        h.developed_as = None;
    }
    if fill_round && walked.is_ok() {
        h.from_shelf = false;
    }
    let walked = match walked {
        Ok(walked) => walked,
        Err(crate::walk::Stopped::GaveWay(stage)) => {
            tracing::debug!("the round gave way before or in the {stage}");
            round.gave_way = true;
            return (
                Outcome::Failed {
                    generation,
                    message: format!("gave way in the {stage}"),
                },
                None,
            );
        }
        Err(e) => {
            // A fill round that failed leaves nothing for the edit's
            // pictures to wait on: it is said as any develop's failure.
            if matches!(how, Walk::Fill(_)) {
                h.filling = false;
            }
            return (
                Outcome::Failed {
                    generation,
                    message: e.to_string(),
                },
                None,
            );
        }
    };
    let card_free = free_before
        .filter(|_| walked.places.contains(&Place::Gpu))
        .and_then(|before| crate::ai::card_free_for_log().map(|after| (before, after)));
    // A base made in this walk is a new picture for the masks; one
    // turned where it stood is too, though it is the base kept.
    let turned = walked.from == Some(Keep::Base) && h.turn != turn && h.stamp > 0;
    let made = matches!(walked.from, None | Some(Keep::Pair | Keep::Camera));
    if made || turned {
        h.stamp += 1;
    }
    if turned {
        tracing::info!("base turned to {turn} quarter turns");
    }
    let from = if made {
        BaseFrom::Made
    } else if from_shelf {
        BaseFrom::Shelf
    } else {
        BaseFrom::Kept
    };
    // The masks' models see the base under the edit it was made for,
    // as their kept work of it is known by the base's stamp.
    if made || turned || h.stamp == 0 {
        h.edit = edit.clone();
    }
    h.turn = turn;
    // The masks' print is the edit's now, not the one the base was made
    // under: an export of this edit works it out alike, so a change the
    // base did not need makes the masks again on the same picture
    // rather than leave the viewport's apart from the export's.
    h.print = print;
    h.meta = walked.meta.clone();
    if let Some(image) = h.kept.picture(Keep::Base) {
        h.image = image.clone();
    }
    h.guide = walked
        .taps
        .guide
        .clone()
        .unwrap_or_else(|| Arc::new(greycard_render::finish::Guide::NONE));
    // The range masks' picture from before the Detail section is the
    // patched picture kept here, shared, not copied: the viewport puts
    // it on the GPU beside the planes.
    h.planes = greycard_render::detail::DetailPlanes::with_sample(
        walked.taps.planes.clone(),
        edit.samples_before_detail().then(|| h.patched_picture()),
        walked.meta.clip_level,
    );
    // Where each stage ran, and whether this walk ran it.
    let at = |want: &dyn Fn(&Stage) -> bool| {
        steps
            .steps
            .iter()
            .position(|s| matches!(s, Step::Op(stage) if want(stage)))
    };
    let on_device = |i: usize| walked.places.get(i) == Some(&Place::Gpu);
    let seconds_of = |name: &str| {
        walked
            .seconds
            .iter()
            .filter(|(n, _)| *n == name)
            .map(|(_, s)| s)
            .sum::<f64>()
    };
    h.on_gpu = steps.steps.iter().zip(&walked.places).any(|(s, p)| {
        *p == Place::Gpu
            && matches!(
                s,
                Step::Op(Stage::Ca(_) | Stage::Demosaic(..) | Stage::Lens(_))
            )
    });
    // A develop with a device lent that came back in memory ran
    // nothing of the edit there past the inline ops: what the device
    // holds was for another edit, and goes. One without a device
    // leaves the window's as they are.
    if gpu.is_some() && !matches!(walked.buf, Buf::Texture(_)) {
        h.kept.forget_device();
    }
    // A run the device declined went on on the CPU, which kept its
    // picture before the sharpen in memory: a picture the size of the
    // frame on the frames largest, which the window keeps only on the
    // device; it goes. (With the Detail section off it is the patched
    // picture itself, which costs nothing more.)
    if gpu
        .as_ref()
        .and_then(|_| h.kept.picture(Keep::PreSharpen))
        .is_some_and(|pre| !Arc::ptr_eq(pre, &h.patched_picture()))
    {
        h.kept.take_working(Keep::PreSharpen);
    }
    let patched = h.patched_picture();
    // The refined masks' picture, made only for an edit that refines a
    // mask, and let go when it no longer does: tapped by the walk at the
    // patched picture, or made of it here.
    let mask_picture = if !greycard_render::refine::refines(edit) {
        h.mask_picture = None;
        h.kept.forget_mask_picture();
        None
    } else if let Some(picture) = walked.taps.picture.clone() {
        h.mask_picture = Some(KeptMaskPicture {
            patched: patched.clone(),
            picture: picture.clone(),
        });
        Some(picture)
    } else {
        Some(super::masks::mask_picture(h, &patched))
    };
    if let Some(ctx) = gpu.as_ref().filter(|_| !round.aside) {
        // What the device holds for a stage that is off goes.
        if edit.sharpen.options().is_none() && !edit.detail_wants().sharpen {
            ctx.release_sharpen();
        }
        if edit.detail.options().is_none() {
            ctx.release_local_contrast();
        }
    }
    let ca = at(&|s| matches!(s, Stage::Ca(_)))
        .filter(|&i| i >= walked.begin)
        .map(|i| {
            let ran = if on_device(i) { CaRan::Gpu } else { CaRan::Cpu };
            (ran, seconds_of("CA"))
        });
    let detail_ran = at(&|s| matches!(s, Stage::LocalContrast(_))).map(|i| {
        if i < walked.begin {
            DetailRan::Kept
        } else if on_device(i) {
            DetailRan::Gpu
        } else {
            DetailRan::Cpu(seconds_of("local contrast"))
        }
    });
    let dehaze_ran = at(&|s| matches!(s, Stage::Dehaze(_))).map(|i| {
        if i < walked.begin {
            DehazeRan::Kept
        } else if on_device(i) {
            DehazeRan::Gpu
        } else {
            DehazeRan::Cpu
        }
    });
    let sharpened = at(&|s| matches!(s, Stage::Sharpen(..)))
        .filter(|_| walked.sharpen.is_some())
        .map(|i| {
            if on_device(i) {
                Sharpened::Gpu
            } else {
                Sharpened::Cpu
            }
        });
    let learned = learned_report(
        edit,
        &steps,
        &walked,
        std::mem::replace(&mut models.learned, LearnedReport::Off),
    );
    for (name, why) in stood_down(&steps) {
        tracing::debug!("{name} stood down: {why}");
    }
    models.pending_after(&h.kept);
    round.ahead = models.ahead;
    let mut fills = std::mem::take(&mut models.fills);
    // What the card keeps from the picture shown, worked out afresh on
    // every develop, one that started past the retouch too.
    let failed: Vec<String> = fills.failed.iter().map(|(n, _)| n.clone()).collect();
    fills.short = models.card_shorts(
        &edit.retouch,
        walked.unmade.as_deref(),
        &fills.pending,
        &failed,
    );
    h.filling = !fills.pending.is_empty();
    let mut stages: Stages = walked
        .seconds
        .iter()
        .filter(|(n, _)| !matches!(*n, "CA" | "local contrast" | "dehaze" | "sharpen"))
        .copied()
        .collect();
    let (image, picture) = match walked.buf {
        Buf::Texture(texture) => (Developed::Texture(texture), None),
        Buf::Working(image) => {
            let halves = Arc::new(timed(&mut stages, "halves", || {
                Halves::from_image(&image, walked.sharpen_mask.as_deref())
            }));
            (Developed::Halves(halves), Some(image))
        }
        _ => {
            return (
                Outcome::Failed {
                    generation,
                    message: "the develop ended on the device with no texture".into(),
                },
                None,
            );
        }
    };
    let (width, height) = match &image {
        Developed::Texture(t) => (t.width(), t.height()),
        Developed::Halves(h) => (h.width, h.height),
    };
    let seconds = start.elapsed().as_secs_f64();
    note_developed(&DevelopLog {
        width,
        height,
        from,
        ca,
        learned: &learned,
        fills: &fills,
        stages: &stages,
        detail: detail_ran,
        dehazed: walked.dehaze.and(dehaze_ran),
        sharpened,
        stats: (walked.detail, walked.dehaze, walked.sharpen),
        card_free,
        total: seconds,
    });
    let outcome = Outcome::Developed {
        generation,
        turn,
        image,
        kind: walked.meta.source,
        guide: super::masks::guide_with(h, mask_picture),
        planes: h.planes.clone(),
        white: h.white(),
        seconds,
        #[cfg(test)]
        detail: walked.detail.zip(detail_ran),
        sharpen: walked.sharpen,
        #[cfg(test)]
        dehaze: walked.dehaze,
        sources: walked.sources,
        learned,
        fills,
        settles: round.quick.is_some(),
    };
    h.developed_as = Some((edit.clone(), turn));
    h.left_out = left_out_of(&outcome, Some(h));
    (outcome, picture)
}

/// Develop another `edit` of the open frame beside its own, as a before
/// and after shows it: a walk from a clone of what `held` keeps, which
/// shares its pictures, into a frame kept of its own that goes with
/// the walk. Nothing of the open frame's develop is written: its kept
/// points, its base and stamp, the edit its masks see, and the develop
/// it last delivered stay as they were, so its next develop goes on
/// from them. No fill is made.
#[allow(clippy::too_many_arguments)]
pub(super) fn develop_aside(
    input: &Input,
    edit: &Edit,
    turn: u8,
    generation: u64,
    held: Option<&FrameKept>,
    ai: &mut Ai,
    stores: (
        Option<&greycard_ai::DenoiseCache>,
        Option<&greycard_lens::Database>,
    ),
    gpu: &mut Option<greycard_gpu::Context>,
    deliver: &Deliver,
) -> Outcome {
    let facts = input.walked().facts(turn);
    let mut aside = FrameKept::new(&facts);
    if let Some(h) = held.filter(|h| h.facts == aside.facts) {
        aside.kept = h.kept.clone();
        aside.stamp = h.stamp;
        aside.turn = h.turn;
        aside.image = h.image.clone();
    }
    // No round comes after an aside: its highlight fill is made in it.
    let mut round = Round {
        aside: true,
        highlight: Highlight::Inline,
        ..Round::first()
    };
    develop_round(
        input,
        edit,
        turn,
        generation,
        &mut Some(aside),
        ai,
        stores,
        gpu,
        deliver,
        &mut round,
        &NoHold,
    )
    .0
}

/// What of the edit the frame could not take, by name and why, other
/// than the learned denoiser, which [`learned_report`] says: a
/// panorama's lens profile, standing down on a canvas no one frame's
/// profile fits. For the log.
pub(super) fn stood_down(
    steps: &crate::stages::Steps,
) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
    steps.skipped.iter().filter_map(|(name, why)| match why {
        Skip::Unavailable(why) if *name != LEARNED_DENOISER => Some((*name, *why)),
        _ => None,
    })
}

/// What the learned denoiser did for a walk of `edit`'s `steps`, as
/// the window says it: its answer kept when the walk started past it,
/// none on a mosaic it has no version for, why the engine's own stood
/// in, or how `ran` had the network's answer.
fn learned_report(
    edit: &Edit,
    steps: &crate::stages::Steps,
    walked: &crate::walk::Walked,
    ran: LearnedReport,
) -> LearnedReport {
    if edit.noise.tier().is_none() {
        return LearnedReport::Off;
    }
    if walked.from.is_some() {
        return LearnedReport::Kept;
    }
    let unavailable = steps.skipped.iter().find_map(|(name, why)| match why {
        Skip::Unavailable(why) if *name == LEARNED_DENOISER => Some(*why),
        _ => None,
    });
    match (&walked.meta.stood_in, unavailable) {
        (_, Some(why)) => LearnedReport::Unavailable(why),
        (Some(no), _) => stood_in_report(no),
        (None, None) => ran,
    }
}

/// What the window and an export are told of the learned denoiser's
/// standing down: its model missing, or its failure, which the engine's
/// denoise stood in for; or the card it lost, where nothing stood in, so
/// it is said as the no denoise it is (`NoLearned::off_card`), never as
/// a failure with the engine's denoise in its place.
pub(super) fn stood_in_report(no: &NoLearned) -> LearnedReport {
    match no {
        no if no.off_card() => LearnedReport::Unavailable(crate::stages::OFF_CARD),
        NoLearned::Missing(model) => LearnedReport::Missing(model),
        NoLearned::Failed(why) | NoLearned::Short(why) => LearnedReport::Failed(why.clone()),
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

/// What a develop's log line reads: the picture's size, what was made
/// anew and what was kept, and the seconds in all.
struct DevelopLog<'a> {
    width: u32,
    height: u32,
    from: BaseFrom,
    ca: Option<(CaRan, f64)>,
    learned: &'a LearnedReport,
    fills: &'a FillReport,
    stages: &'a Stages,
    detail: Option<DetailRan>,
    dehazed: Option<DehazeRan>,
    sharpened: Option<Sharpened>,
    /// What the local contrast, the dehaze, and the sharpen measured.
    stats: (
        Option<LocalContrastStats>,
        Option<DehazeStats>,
        Option<SharpenStats>,
    ),
    /// The card's free memory before the develop and after, bytes, when
    /// it ran on the device and the card says.
    card_free: Option<(u64, u64)>,
    total: f64,
}

/// One line for a develop: what was made anew and what was kept,
/// and the seconds, the same the outcome carries. A model that is
/// missing is a state, named here and offered by the UI, not warned
/// on every develop; one that would not load is warned where the
/// load fails. A fill that failed is warned here, with its name.
fn note_developed(log: &DevelopLog<'_>) {
    warn_failed_fills(log.fills);
    tracing::info!("{}", developed_line(log));
}

/// The learned denoiser's part of a develop's line: how its answer
/// was had, and on which provider when it ran.
pub(super) fn learned_part(learned: &LearnedReport) -> Option<String> {
    Some(match learned {
        LearnedReport::Off => return None,
        LearnedReport::Ran {
            version,
            provider,
            seconds,
        } => format!("denoiser {version} on {provider} {seconds:.2} s"),
        LearnedReport::Cached { seconds } => format!("denoiser cached {seconds:.2} s"),
        LearnedReport::Kept => "denoiser kept".into(),
        LearnedReport::Missing(_) => "denoiser missing".into(),
        LearnedReport::Failed(_) => "denoiser failed".into(),
        LearnedReport::Unavailable(why) => format!("no learned denoiser: {why}"),
    })
}

/// The fills' parts of a develop's line: those made, by name, and
/// their seconds, those whose model is missing, and those that wait
/// on a GPU.
pub(super) fn fill_parts(fills: &FillReport) -> Vec<String> {
    let mut parts = Vec::new();
    if !fills.made.is_empty() {
        parts.push(format!(
            "{} fill(s) {:.2} s ({})",
            fills.made.len(),
            fills.seconds,
            fills.made.join(", ")
        ));
    }
    if !fills.missing.is_empty() {
        let names: Vec<&str> = fills.missing.iter().map(|(n, _)| n.as_str()).collect();
        parts.push(format!(
            "{} fill(s) missing ({})",
            fills.missing.len(),
            names.join(", ")
        ));
    }
    if !fills.no_gpu.is_empty() {
        parts.push(format!(
            "{} fill(s) need a GPU ({})",
            fills.no_gpu.len(),
            fills.no_gpu.join(", ")
        ));
    }
    if !fills.pending.is_empty() {
        parts.push(format!("{} fill(s) to make after", fills.pending.len()));
    }
    parts
}

/// Each fill the model failed, warned: the line counts none of them.
pub(super) fn warn_failed_fills(fills: &FillReport) {
    for (name, why) in &fills.failed {
        tracing::warn!("fill {name}: {why}");
    }
}

/// [`note_developed`]'s line: where the base came from, then the CA
/// and the denoiser, then each stage this develop ran and its seconds
/// (a fresh base's own, the retouch, and the picture's way to the
/// viewport: its upload to the device or its half floats), then the
/// fills and the ops after the base, and the total.
fn developed_line(log: &DevelopLog<'_>) -> String {
    let &DevelopLog {
        width,
        height,
        from,
        ca,
        learned,
        fills,
        stages,
        detail,
        dehazed,
        sharpened,
        stats,
        card_free,
        total,
    } = log;
    let (contrast, dehaze, sharpen) = stats;
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
    parts.extend(learned_part(learned));
    for (name, s) in stages {
        parts.push(format!("{name} {s:.2} s"));
    }
    parts.extend(fill_parts(fills));
    // Each op after the base with what it measured, where known: the
    // radii, the transmission, and the sharpen's radius and coverage.
    let lc = contrast
        .map(|s| {
            format!(
                "local contrast at {} and {} px",
                s.texture_radius, s.clarity_radius
            )
        })
        .unwrap_or_else(|| "local contrast".into());
    match detail {
        Some(DetailRan::Gpu) => parts.push(format!("{lc} on the GPU")),
        Some(DetailRan::Cpu(s)) => parts.push(format!("{lc} {s:.2} s")),
        Some(DetailRan::Kept) if contrast.is_some() => parts.push(format!("{lc} kept")),
        Some(DetailRan::Kept) | None => {}
    }
    let dh = dehaze
        .map(|d| {
            format!(
                "dehaze {:+.0} to a mean transmission of {:.2}",
                d.amount * 100.0,
                d.transmission_mean
            )
        })
        .unwrap_or_else(|| "dehaze".into());
    match dehazed {
        Some(DehazeRan::Gpu) => parts.push(format!("{dh} on the GPU")),
        Some(DehazeRan::Cpu) => parts.push(dh),
        Some(DehazeRan::Kept) if dehaze.is_some() => parts.push(format!("{dh} kept")),
        Some(DehazeRan::Kept) | None => {}
    }
    let sh = sharpen
        .map(|s| {
            format!(
                "sharpen at radius {:.2} over {:.0}%",
                s.radius,
                s.blend_mean * 100.0
            )
        })
        .unwrap_or_else(|| "sharpen".into());
    match sharpened {
        Some(Sharpened::Cpu) => parts.push(sh),
        Some(Sharpened::Gpu) => parts.push(format!("{sh} on the GPU")),
        None => {}
    }
    if let Some((before, after)) = card_free {
        let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
        parts.push(format!(
            "the card {:.2} GiB free before, {:.2} after",
            gib(before),
            gib(after)
        ));
    }
    format!(
        "developed {width}x{height}: {}, total {total:.2} s",
        parts.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The card the learned denoiser lost is said as no denoise, in the
    /// window and an export, never as a failure the engine's denoise
    /// stood in for (nothing ran); every other standing down is said as
    /// it always was. A merge's warning keeps its old wording but for
    /// this one, which it says in words.
    #[test]
    fn the_card_it_lost_is_said_as_no_denoise() {
        let off = NoLearned::Failed(crate::stages::OFF_CARD.to_string());
        let report = stood_in_report(&off);
        assert!(matches!(
            report,
            LearnedReport::Unavailable(why) if why == crate::stages::OFF_CARD
        ));
        let said = super::super::engine::left_out_words(&report, None, &FillReport::default());
        assert_eq!(
            said,
            vec!["the denoise: the learned denoiser is not on this graphics card; none ran"]
        );
        assert!(matches!(
            stood_in_report(&NoLearned::Failed("bad input".into())),
            LearnedReport::Failed(why) if why == "bad input"
        ));
        assert!(matches!(
            stood_in_report(&NoLearned::Short("1.0 GiB free".into())),
            LearnedReport::Failed(why) if why == "1.0 GiB free"
        ));
        assert!(matches!(
            stood_in_report(&NoLearned::Missing(&greycard_ai::DENOISE_BEST)),
            LearnedReport::Missing(_)
        ));
        assert!(!NoLearned::Failed("bad input".into()).off_card());
        assert!(!NoLearned::Short(crate::stages::OFF_CARD.into()).off_card());
    }

    /// What the frame could not take is read for the log: a panorama's
    /// lens profile, not the learned denoiser (its own report says it)
    /// nor what a purpose leaves out.
    #[test]
    fn what_stood_down_is_read_for_the_log() {
        let steps = crate::stages::Steps {
            steps: Vec::new(),
            skipped: vec![
                (LEARNED_DENOISER, Skip::Unavailable("no version for it")),
                (
                    "the lens profile",
                    Skip::Unavailable("a panorama's lens was corrected when it was merged"),
                ),
                ("the retouch", Skip::ModelsSee),
            ],
        };
        assert_eq!(
            stood_down(&steps).collect::<Vec<_>>(),
            [(
                "the lens profile",
                "a panorama's lens was corrected when it was merged"
            )]
        );
    }

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
        let line = developed_line(&DevelopLog {
            width: 8192,
            height: 5464,
            from: BaseFrom::Made,
            ca: Some((CaRan::Gpu, 0.25)),
            learned: &LearnedReport::Off,
            fills: &FillReport::default(),
            stages: &stages,
            detail: None,
            dehazed: None,
            sharpened: Some(Sharpened::Gpu),
            stats: (None, None, None),
            card_free: None,
            total: 2.5,
        });
        assert_eq!(
            line,
            "developed 8192x5464: base made, CA on the GPU 0.25 s, prepare 0.31 s, \
             demosaic 0.92 s, finish 0.20 s, guide 0.02 s, upload 0.12 s, \
             sharpen on the GPU, total 2.50 s"
        );

        let line = developed_line(&DevelopLog {
            width: 8192,
            height: 5464,
            from: BaseFrom::Shelf,
            ca: None,
            learned: &LearnedReport::Kept,
            fills: &FillReport::default(),
            stages: &vec![("upload", 0.118)],
            detail: None,
            dehazed: None,
            sharpened: Some(Sharpened::Gpu),
            stats: (None, None, None),
            card_free: None,
            total: 0.4,
        });
        assert_eq!(
            line,
            "developed 8192x5464: base from the shelf, denoiser kept, upload 0.12 s, \
             sharpen on the GPU, total 0.40 s"
        );
        // The card's free memory around a develop on it, where known.
        let line = developed_line(&DevelopLog {
            width: 8192,
            height: 5464,
            from: BaseFrom::Shelf,
            ca: None,
            learned: &LearnedReport::Kept,
            fills: &FillReport::default(),
            stages: &vec![("upload", 0.118)],
            detail: None,
            dehazed: None,
            sharpened: Some(Sharpened::Gpu),
            stats: (None, None, None),
            card_free: Some((9 << 29, 3 << 30)),
            total: 0.4,
        });
        assert!(
            line.ends_with(
                "sharpen on the GPU, the card 4.50 GiB free before, 3.00 after, total 0.40 s"
            ),
            "{line}"
        );

        // The demosaic on the device carries its seconds, as the CPU's
        // does, so a tester's log shows it.
        let line = developed_line(&DevelopLog {
            width: 8192,
            height: 5464,
            from: BaseFrom::Made,
            ca: Some((CaRan::Gpu, 0.05)),
            learned: &LearnedReport::Off,
            fills: &FillReport::default(),
            stages: &vec![
                ("prepare", 0.31),
                ("demosaic on the GPU", 0.094),
                ("finish", 0.2),
            ],
            detail: None,
            dehazed: None,
            sharpened: Some(Sharpened::Gpu),
            stats: (None, None, None),
            card_free: None,
            total: 0.9,
        });
        assert_eq!(
            line,
            "developed 8192x5464: base made, CA on the GPU 0.05 s, prepare 0.31 s, \
             demosaic on the GPU 0.09 s, finish 0.20 s, sharpen on the GPU, total 0.90 s"
        );

        let line = developed_line(&DevelopLog {
            width: 300,
            height: 200,
            from: BaseFrom::Kept,
            ca: None,
            learned: &LearnedReport::Off,
            fills: &FillReport::default(),
            stages: &Stages::new(),
            detail: Some(DetailRan::Cpu(0.5)),
            dehazed: Some(DehazeRan::Cpu),
            sharpened: Some(Sharpened::Cpu),
            stats: (None, None, None),
            card_free: None,
            total: 0.75,
        });
        assert_eq!(
            line,
            "developed 300x200: base kept, local contrast 0.50 s, dehaze, sharpen, total 0.75 s"
        );
    }

    /// What the window's status line once said after a develop is the
    /// log's: the radii, the dehaze's transmission, the sharpen's
    /// radius and coverage, why the learned denoiser stood down, and
    /// each fill by name.
    #[test]
    fn the_develop_s_line_carries_what_the_ops_measured() {
        let fills = FillReport {
            made: vec!["Fill 1".into()],
            seconds: 1.5,
            no_gpu: vec!["Fill 2".into()],
            ..FillReport::default()
        };
        let stats = (
            Some(LocalContrastStats {
                texture_radius: 3,
                clarity_radius: 139,
                structure_radius: 400,
            }),
            Some(DehazeStats {
                airlight: [1.0; 3],
                amount: 0.28,
                strength: 0.0,
                factor: 4,
                transmission_mean: 0.95,
                transmission_min: 0.5,
            }),
            Some(SharpenStats {
                radius: 0.77,
                threshold: 0.0,
                blend_mean: 0.48,
                clipped: 0.0,
                iterations: 10,
            }),
        );
        let line = developed_line(&DevelopLog {
            width: 5568,
            height: 3712,
            from: BaseFrom::Kept,
            ca: None,
            learned: &LearnedReport::Unavailable("no version for it"),
            fills: &fills,
            stages: &Stages::new(),
            detail: Some(DetailRan::Gpu),
            dehazed: Some(DehazeRan::Kept),
            sharpened: Some(Sharpened::Gpu),
            stats,
            card_free: None,
            total: 1.18,
        });
        assert_eq!(
            line,
            "developed 5568x3712: base kept, no learned denoiser: no version for it, \
             1 fill(s) 1.50 s (Fill 1), 1 fill(s) need a GPU (Fill 2), \
             local contrast at 3 and 139 px on the GPU, \
             dehaze +28 to a mean transmission of 0.95 kept, \
             sharpen at radius 0.77 over 48% on the GPU, total 1.18 s"
        );

        // On the CPU: the local contrast with its seconds, the dehaze
        // and the sharpen with no place named.
        let line = developed_line(&DevelopLog {
            width: 300,
            height: 200,
            from: BaseFrom::Kept,
            ca: None,
            learned: &LearnedReport::Off,
            fills: &FillReport::default(),
            stages: &Stages::new(),
            detail: Some(DetailRan::Cpu(0.5)),
            dehazed: Some(DehazeRan::Cpu),
            sharpened: Some(Sharpened::Cpu),
            stats,
            card_free: None,
            total: 0.75,
        });
        assert_eq!(
            line,
            "developed 300x200: base kept, local contrast at 3 and 139 px 0.50 s, \
             dehaze +28 to a mean transmission of 0.95, \
             sharpen at radius 0.77 over 48%, total 0.75 s"
        );
    }
}
