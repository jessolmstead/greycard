//! A frame's develop as its walks kept it, and the models those walks
//! run.

use greycard_core::develop::retouch::Region;
use greycard_edit::retouch::Patch;

use super::*;
use crate::stages::{Facts, Keep, Purpose, Step, Steps, stages};
use crate::walk::{Kept, Meta, NoLearned, Place};

/// The seconds each stage of a develop took, in the order they ran,
/// for its log line ([`super::develop`]'s `note_developed`).
pub(super) type Stages = Vec<(&'static str, f64)>;

/// `run`, its seconds noted in `stages` as `name`.
pub(super) fn timed<T>(stages: &mut Stages, name: &'static str, run: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let out = run();
    stages.push((name, started.elapsed().as_secs_f64()));
    out
}

/// A frame's develop as its walks kept it: the points they kept, and
/// what the window's asks (its masks, its people, its pictures, its
/// export) read of the last one. The open frame's, and each frame's
/// left behind on the shelf.
pub(super) struct FrameKept {
    pub kept: Kept,
    /// What the frame's decode said, which its lists are derived from,
    /// at no turn.
    pub facts: Facts,
    /// The edit the base was made under, which the masks' models see
    /// it through, and the quarter turns of the last walk.
    pub edit: Edit,
    pub turn: u8,
    /// Counts the base pictures made of the frame, for the learned
    /// masks to know theirs.
    pub stamp: u64,
    /// The print of the base under the last develop's edit, which the
    /// learned masks' rasters are kept by ([`crate::mask_base::print`]).
    pub print: crate::ai::Print,
    /// Taken back from the shelf and not walked from since: the next
    /// develop's line says where its base came from.
    pub from_shelf: bool,
    /// The base picture of the last walk.
    pub image: Arc<WorkingImage>,
    /// What the last walk measured of the picture.
    pub meta: Meta,
    /// The last walk's guide plane, and the masks' bands it made.
    pub guide: Arc<greycard_render::finish::Guide>,
    pub planes: Option<Arc<greycard_render::detail::DetailPlanes>>,
    /// Whether the CA, the demosaic, or the lens in the base ran on the
    /// device.
    pub on_gpu: bool,
    /// The picture the refined masks are made against, with what it
    /// was made from: made only for an edit that refines a mask
    /// ([`super::masks::mask_picture`]), kept for the next develop of
    /// it, and shelved with the base.
    pub mask_picture: Option<KeptMaskPicture>,
    /// The edit and the quarter turns of the last develop, and what it
    /// left out of what the edit asked for ([`left_out_of`]): a save of
    /// the frame makes its pictures from that develop when it is of the
    /// edit saved, and says nothing kept when it went without.
    pub developed_as: Option<(Edit, u8)>,
    pub left_out: Vec<String>,
    /// The last develop left fills for the worker to make after it: its
    /// picture is not yet the edit's.
    pub filling: bool,
}

/// The refined masks' picture ([`greycard_render::refine::MaskPicture`])
/// with what it was made from: the patched picture.
pub(super) struct KeptMaskPicture {
    pub patched: Arc<WorkingImage>,
    pub picture: Arc<greycard_render::refine::MaskPicture>,
}

impl FrameKept {
    /// The base the learned masks are made from or read for.
    pub(super) fn base(&self) -> crate::ai::Base {
        crate::ai::Base {
            stamp: self.stamp,
            print: self.print,
        }
    }

    /// Nothing kept yet of a frame `facts` describes.
    pub(super) fn new(facts: &Facts) -> Self {
        Self {
            kept: Kept::new(),
            facts: Facts {
                turn: 0,
                ..facts.clone()
            },
            edit: Edit::default(),
            turn: 0,
            stamp: 0,
            print: crate::ai::Print {
                hash: 0,
                size: crate::mask_base::Size::Full,
            },
            from_shelf: false,
            image: Arc::new(WorkingImage::new(0, 0)),
            meta: Meta::empty(),
            guide: Arc::new(greycard_render::finish::Guide::NONE),
            planes: None,
            on_gpu: false,
            mask_picture: None,
            developed_as: None,
            left_out: Vec::new(),
            filling: false,
        }
    }

    /// The picture the retouch made, or the base where there is none:
    /// what the Detail section and the sharpen run on.
    pub(super) fn patched_picture(&self) -> Arc<WorkingImage> {
        self.kept
            .patched()
            .map_or_else(|| self.image.clone(), |p| p.image.clone())
    }

    /// The bytes of the guide plane and the refined masks' picture,
    /// unless that is the one the walk's own count has.
    fn beside_bytes(&self) -> u64 {
        let picture = self
            .mask_picture
            .as_ref()
            .filter(|k| {
                !self
                    .kept
                    .mask_picture()
                    .is_some_and(|p| Arc::ptr_eq(p, &k.picture))
            })
            .map_or(0, |k| k.picture.bytes());
        (self.guide.data.len() * std::mem::size_of::<f32>()) as u64 + picture
    }

    /// The frame's facts at `turn`.
    pub(super) fn facts_at(&self, turn: u8) -> Facts {
        Facts {
            turn,
            ..self.facts.clone()
        }
    }

    /// The white balance the last walk used, as the window reads it.
    pub(super) fn white(&self) -> WhiteBase {
        match &self.meta.white {
            Some(wb) => WhiteBase::from(wb, self.meta.clip_level),
            None => WhiteBase {
                clip: self.meta.clip_level,
                picture: self.meta.picture,
                ..WhiteBase::IDENTITY
            },
        }
    }

    /// What the masks' white balances are made matrices from.
    pub(super) fn white_shift(&self) -> Option<greycard_edit::WhiteShift> {
        self.meta.white_shift()
    }

    /// The masks' bands kept, when they hold what `edit`'s masks read.
    pub(super) fn planes_for(
        &self,
        edit: &Edit,
    ) -> Option<Arc<greycard_render::detail::DetailPlanes>> {
        let wants = edit.detail_wants();
        self.kept.planes_for(
            &wants,
            edit.detail.options().filter(|_| wants.haze),
            &edit.sharpen.settings(),
        )
    }

    /// The bytes the frame's develop holds in memory: its pictures, the
    /// guide plane, and the refined masks' picture.
    pub(super) fn bytes(&self) -> u64 {
        self.kept.bytes() + self.beside_bytes()
    }

    /// The bytes of the base and the patched picture alone: what a
    /// prefetched base of a frame this size puts on the shelf, with no
    /// guide plane and no masks' picture beside it.
    pub(super) fn picture_bytes(&self) -> u64 {
        let base = self.kept.picture(Keep::Base);
        let patched = self.kept.patched().map(|p| p.image);
        let size =
            |image: &Arc<WorkingImage>| (image.data.len() * std::mem::size_of::<f32>()) as u64;
        let mut total = base.map_or(0, size);
        if let Some(p) = patched
            && !base.is_some_and(|b| Arc::ptr_eq(b, p))
        {
            total += size(p);
        }
        total
    }

    /// Whether the base kept here serves a develop of `edit` at `turn`,
    /// with a device `lent` or not, as the window's walk would resume
    /// from it. With `stands_in`, the learned denoiser the edit asks for
    /// is not to be had as the walk starts (its model is not in the
    /// store, or there is no runtime), so the walk's list is the one the
    /// engine's own path stands in with, and only a base made by that
    /// path serves; without, a base made by it never does, since the
    /// walk would run the model.
    pub(super) fn serves(
        &self,
        edit: &Edit,
        turn: u8,
        lenses: Option<&greycard_lens::Database>,
        lent: bool,
        stands_in: bool,
    ) -> bool {
        let (mut steps, mut plan) = window_steps(edit, &self.facts_at(turn), lenses, lent);
        if stands_in {
            plan = crate::walk::replan(&steps, &plan);
            steps = steps.without_learned();
        } else if self.meta.stood_in.is_some() {
            return false;
        }
        self.kept.serves(&steps, &plan, Keep::Base)
    }
}

/// The window's list for `edit` on the frame `facts` describes, and
/// where each step runs with a device `lent` or not. On the CPU the
/// window keeps its developed picture, which an export of the open
/// frame and a save of its edit start from, and not its picture before
/// the sharpen, one more picture the size of the frame held for a
/// slider the CPU's sharpen is quick at. On the device it keeps the
/// picture before the sharpen, which a sharpen move starts from, and
/// its developed picture is the viewport's texture. The refined masks'
/// picture is tapped from the patched picture, in memory either way.
pub(super) fn window_steps(
    edit: &Edit,
    facts: &Facts,
    lenses: Option<&greycard_lens::Database>,
    lent: bool,
) -> (Steps, Vec<Place>) {
    let purpose = Purpose::Viewport;
    let mut steps = stages(edit, &purpose, facts, lenses);
    steps.steps.retain(|s| match s {
        Step::Keep(Keep::Developed) => !lent,
        Step::Keep(Keep::PreSharpen) => lent,
        _ => true,
    });
    let plan = crate::walk::plan(&steps, &purpose, lent);
    (steps, plan)
}

/// Why the learned denoiser is not to be had, as the walk says it.
fn no_learned(e: NoDenoiser, model: &'static greycard_ai::Model) -> NoLearned {
    match e {
        NoDenoiser::Missing => NoLearned::Missing(model),
        NoDenoiser::Failed(why) => NoLearned::Failed(why),
    }
}

/// The models a walk of the worker's runs, lent from the `Ai` it is
/// given: the learned denoiser through the disk cache, the fills, and
/// the learned masks an export makes, each one that could not be had
/// noted in the words the window and the export say it in.
pub(super) struct WorkerModels<'a> {
    ai: &'a mut Ai,
    cache: Option<&'a greycard_ai::DenoiseCache>,
    deliver: &'a Deliver,
    /// The job's generation, which its fills are said under.
    generation: u64,
    /// How many fills the retouch had to make when the develop
    /// started, and how many it has made since: the status says where
    /// the develop is. A later round of the same develop carries it on.
    pub ahead: (usize, usize),
    /// How many fills the model may make in this walk, none for no
    /// limit: one it may not make now is left for later
    /// ([`FillReport::pending`]).
    pub budget: Option<usize>,
    /// The walk ran the retouch, and so asked for each of its fills.
    retouched: bool,
    /// The edit and the turn the walk is of.
    pub edit: Edit,
    pub turn: u8,
    /// How the learned denoiser's answer was had, for the log line.
    pub learned: LearnedReport,
    pub fills: FillReport,
    /// Each learned mask not made, in words.
    pub masks: Vec<String>,
    /// Run the retouch again over fills a kept point left unmade rather
    /// than go on past them: an export says what each fill went
    /// without, which only a run of the fill finds out.
    pub run_unmade: bool,
    /// Ask the card again, a few seconds apart, for a model it was
    /// short of, and fail on it rather than stand in: an export's.
    pub patient: bool,
    /// A model run the card stayed short of, in a patient walk
    /// ([`crate::ai::CardShort`]): the export fails its item on it.
    pub short: Option<String>,
}

impl<'a> WorkerModels<'a> {
    pub(super) fn new(
        ai: &'a mut Ai,
        cache: Option<&'a greycard_ai::DenoiseCache>,
        deliver: &'a Deliver,
        generation: u64,
    ) -> Self {
        Self {
            ai,
            cache,
            deliver,
            generation,
            ahead: (0, 0),
            budget: None,
            retouched: false,
            edit: Edit::default(),
            turn: 0,
            learned: LearnedReport::Off,
            fills: FillReport::default(),
            masks: Vec::new(),
            run_unmade: false,
            patient: false,
            short: None,
        }
    }
}

impl WorkerModels<'_> {
    /// What the card keeps from the picture shown ([`Ai::card_shorts`]).
    pub(super) fn card_shorts(
        &self,
        retouch: &Retouch,
        unmade: Option<&[u64]>,
        pending: &[u64],
        failed: &[String],
    ) -> Vec<(String, u64, u64)> {
        self.ai.card_shorts(retouch, unmade, pending, failed)
    }

    /// The fills still to make after a walk that did not run the
    /// retouch but started past it: those its patched picture left
    /// unmade that could be made on the base, by patch id. A walk that
    /// ran it said each one as it was asked for.
    pub(super) fn pending_after(&mut self, kept: &crate::walk::Kept) {
        if self.retouched || self.budget.is_none() {
            return;
        }
        let (Some(patched), Some(base)) = (kept.patched(), kept.picture(Keep::Base)) else {
            return;
        };
        if patched.unmade.is_empty() {
            return;
        }
        let left = Retouch {
            patches: patched
                .retouch
                .patches
                .iter()
                .filter(|p| patched.unmade.contains(&p.id))
                .cloned()
                .collect(),
            ..patched.retouch.clone()
        };
        self.fills.pending = self.ai.fills_unmade_ids(&left, base);
    }

    /// The learned denoiser `model`, loaded now if it is not yet; why
    /// not when it cannot be.
    fn denoiser(
        &mut self,
        model: &'static greycard_ai::Model,
    ) -> Result<&mut greycard_ai::Denoiser, NoLearned> {
        self.ai.denoiser(model).map_err(|e| no_learned(e, model))
    }
}

/// What the models' cache knows an export's picture by: each frame of
/// a set is the first picture made of it.
const EXPORT_STAMP: u64 = 1;

impl crate::walk::Models for WorkerModels<'_> {
    fn can_denoise(&mut self, model: &'static greycard_ai::Model) -> Result<(), NoLearned> {
        self.ai
            .denoiser_there(model)
            .map_err(|e| no_learned(e, model))
    }

    fn denoise(
        &mut self,
        model: &'static greycard_ai::Model,
        frame: &RawFrame,
        prepared: &Prepared,
    ) -> Result<Vec<f32>, NoLearned> {
        // A patient walk stops asking at its first short.
        if let Some(why) = &self.short {
            return Err(NoLearned::Short(why.clone()));
        }
        let cache = self.cache;
        // The disk cache by the tier's tiling, kept when its session was
        // dropped to give the card back: a hit loads nothing.
        if self.ai.denoiser_tiling(model).is_none() {
            self.denoiser(model)?;
        }
        if let (Some(c), Some(tiling)) = (cache, self.ai.denoiser_tiling(model)) {
            let start = Instant::now();
            if let Some(rgb) = cached(c, model, tiling, prepared) {
                self.learned = LearnedReport::Cached {
                    seconds: start.elapsed().as_secs_f64(),
                };
                return Ok(rgb);
            }
        }
        self.denoiser(model)?;
        let on = match self.ai.denoise_room_for(model) {
            Ok(on) => on,
            Err(short) => {
                if self.patient {
                    self.short = Some(format!("the learned denoiser: {short}"));
                }
                return Err(NoLearned::Short(short.0));
            }
        };
        // Loaded again when the card was given back on the way.
        let denoiser = self.denoiser(model)?;
        let tiling = run_tiling(on, denoiser.tiling());
        let (rgb, report) = network(frame, prepared, model, denoiser, cache, tiling)
            .map_err(|e| learned_failure(&e))?;
        self.learned = report;
        Ok(rgb)
    }

    fn fill(&mut self, patch: &Patch, region: &Region, picture: &WorkingImage) -> Option<Vec<f32>> {
        // A fill the model has to make is said so before it starts,
        // since it takes a while, unless the walk's budget is spent,
        // when it is left for later. The count is what the retouch had
        // ahead when the develop began; a fill kept for the picture an
        // earlier patch left is not in it, so the fill made is never
        // past it.
        // A patient walk stops asking at its first short: the export
        // fails on it.
        if let Some(why) = &self.short {
            self.fills.failed.push((patch.name(), why.clone()));
            return None;
        }
        self.ai.take_short();
        let mut kept = true;
        let started = Instant::now();
        let (deliver, generation, (of, made)) = (self.deliver, self.generation, self.ahead);
        let budget = self.budget;
        let fill = self.ai.fill_or_run(
            patch,
            region,
            picture,
            |_| {
                if budget == Some(0) {
                    return false;
                }
                kept = false;
                deliver(Outcome::Filling {
                    generation,
                    name: patch.name(),
                    made,
                    of: of.max(made + 1),
                });
                true
            },
            &mut |step, of| {
                deliver(Outcome::FillStep {
                    generation,
                    step,
                    of,
                })
            },
        );
        // A fill refused for the card's memory ran nothing: its place in
        // the round goes to the next, which may fit.
        let short = matches!(fill, Err(NoFill::Failed(_)))
            .then(|| self.ai.take_short())
            .flatten();
        if !kept
            && short.is_none()
            && let Some(b) = self.budget.as_mut()
        {
            *b = b.saturating_sub(1);
        }
        match fill {
            Ok((data, engine)) => {
                if !kept {
                    self.ahead.1 += 1;
                    self.fills.made.push(patch.name());
                    self.fills.seconds += started.elapsed().as_secs_f64();
                }
                self.fills.engines.push((patch.id, engine));
                Some(data)
            }
            Err(NoFill::Missing(model)) => {
                self.fills.missing.push((patch.name(), model));
                None
            }
            Err(NoFill::NoGpu) => {
                self.fills.no_gpu.push(patch.name());
                None
            }
            // Never on this card: said once, not asked again, and an
            // export of it fails.
            Err(NoFill::Card { need, have }) => {
                let why = crate::ai::FillNotHere::Card { need, have }.words();
                tracing::info!("fill {}: {why}", patch.name());
                if self.patient {
                    self.short = Some(format!("fill {}: {why}", patch.name()));
                }
                self.fills.short.push((patch.name(), need, have));
                self.fills.failed.push((patch.name(), why));
                None
            }
            Err(NoFill::Failed(why)) => {
                if let Some(short) = &short
                    && self.patient
                {
                    self.short = Some(format!("fill {}: {short}", patch.name()));
                }
                // Refused for the card's memory, a develop's fill stays
                // ahead: the worker gives the card back across its
                // models and asks for it again, once (`ask_fills_again`).
                if short.is_some() && !self.patient {
                    self.fills.pending.push(patch.id);
                    // A removal refused at 1024 is asked again, at 512
                    // where 1024 is still short: still to come, said
                    // refused only if it ends so.
                    if self.ai.asks_again_at_512(patch) {
                        self.ai.take_short_figures();
                        return None;
                    }
                }
                // What the window says of a removal the card was short
                // of, now or still: its need and the card's free memory.
                if let Some((need, free)) = self.ai.take_short_figures() {
                    self.fills.short.push((patch.name(), need, free));
                }
                self.fills.failed.push((patch.name(), why));
                None
            }
            Err(NoFill::Later) => {
                self.fills.pending.push(patch.id);
                None
            }
        }
    }

    /// A walk that may make a fill goes back over the retouch for the
    /// fills a kept point left unmade: those left for later have no
    /// fill in memory for the picture they are on, which a kept
    /// picture past the retouch cannot tell.
    fn could_fill_more(&mut self, retouch: &Retouch, picture: &WorkingImage) -> bool {
        self.run_unmade
            || self.budget.is_some_and(|b| b > 0)
            || self.ai.could_fill_more(retouch, picture)
    }

    fn fills_ahead(&mut self, retouch: &Retouch, picture: &WorkingImage) {
        self.retouched = true;
        let unmade = self.ai.fills_unmade(retouch, picture);
        self.ahead.0 = self.ahead.0.max(self.ahead.1 + unmade);
    }

    fn rasters(
        &mut self,
        how: crate::stages::Masks,
        base: &Arc<WorkingImage>,
        patched: Option<crate::walk::Patched<'_>>,
        meta: &crate::walk::Meta,
    ) -> crate::walk::Rasters {
        // The window asks for its masks on their own job, when it
        // shows them; only an export makes them beside the develop.
        if how != crate::stages::Masks::Make {
            return crate::walk::Rasters::new();
        }
        // The print the viewport keeps its rasters by for this edit: an
        // export of the open frame reads the viewport's, and one of
        // another frame or with no window reads or makes the same.
        let print = self
            .ai
            .base_print(&self.edit, self.turn, crate::mask_base::Size::Full);
        let on = super::masks::On {
            base: crate::ai::Base {
                stamp: EXPORT_STAMP,
                print,
            },
            image: base,
            retouched: patched.map(|p| crate::ai::Retouched {
                retouch: p.retouch,
                image: p.image,
                unmade: p.unmade,
            }),
            edit: &self.edit,
            source: meta.source,
            turn: self.turn,
        };
        // A patient walk stops asking at its first short.
        if self.short.is_some() {
            return crate::walk::Rasters::new();
        }
        // A mask the card is short of waits as the Ai's patience says
        // (an export's); still short, the frame fails on it, and is
        // tried again with the card given back across the worker's
        // models, when the mask may run on the CPU instead
        // (`Ai::set_cpu_fallback`).
        self.ai.take_short();
        let (rasters, left_out) = super::masks::learned_rasters(&self.edit, self.ai, &on);
        if let Some(why) = self.ai.take_short()
            && self.patient
        {
            self.short = Some(format!("a learned mask: {why}"));
        }
        self.masks = left_out;
        rasters
    }
}

/// The learned denoiser's network takes a packed 2x2 Bayer mosaic.
fn bayer_only(frame: &RawFrame) -> anyhow::Result<()> {
    if let greycard_core::raw::SensorLayout::Cfa(p) = &frame.layout
        && !greycard_core::develop::rcd::is_bayer(p)
    {
        anyhow::bail!("the learned denoiser needs a 2x2 Bayer mosaic, not a {p} one");
    }
    Ok(())
}

/// Why the learned denoiser failed, as the walk takes it: refused for
/// the card's memory ([`crate::ai::CardShort`]) or otherwise.
pub(crate) fn learned_failure(e: &anyhow::Error) -> NoLearned {
    match e.downcast_ref::<crate::ai::CardShort>() {
        Some(short) => NoLearned::Short(short.0.clone()),
        None => NoLearned::Failed(e.to_string()),
    }
}

/// The tiling the learned denoiser runs at, as the card has the room
/// for it ([`DenoiseOn`]): its own, `held`, or the smaller tile where
/// the card is short of its own.
pub(crate) fn run_tiling(
    on: DenoiseOn,
    held: greycard_ai::denoise::Tiling,
) -> greycard_ai::denoise::Tiling {
    match on {
        DenoiseOn::Tile(tile) => greycard_ai::denoise::Tiling { tile, ..held },
        DenoiseOn::Card => held,
    }
}

/// The learned denoiser's answer for `prepared` kept in `cache` at
/// `tiling`, or at one of the smaller tiles a card short of it runs at
/// ([`crate::ai::DENOISE_SMALLER_TILES`]): the same answer, kept under
/// the tiling it ran at.
pub(crate) fn cached(
    cache: &greycard_ai::DenoiseCache,
    model: &'static greycard_ai::Model,
    tiling: greycard_ai::denoise::Tiling,
    prepared: &Prepared,
) -> Option<Vec<f32>> {
    let smaller = crate::ai::DENOISE_SMALLER_TILES
        .into_iter()
        .filter(|&t| t < tiling.tile);
    std::iter::once(tiling.tile)
        .chain(smaller)
        .find_map(|tile| {
            let at = greycard_ai::denoise::Tiling { tile, ..tiling };
            cache.read(
                greycard_ai::DenoiseCache::key(model, at, prepared),
                prepared,
            )
        })
}

/// The network's camera RGB of `prepared` at `tiling`: from the disk
/// cache when it is there ([`cached`]), else run and put there under
/// `tiling`, once the card has the memory for it
/// ([`crate::ai::denoise_room`]).
pub(crate) fn network(
    frame: &RawFrame,
    prepared: &Prepared,
    model: &'static greycard_ai::Model,
    denoiser: &mut greycard_ai::Denoiser,
    cache: Option<&greycard_ai::DenoiseCache>,
    tiling: greycard_ai::denoise::Tiling,
) -> anyhow::Result<(Vec<f32>, LearnedReport)> {
    use anyhow::Context;
    bayer_only(frame)?;
    let pattern = prepared
        .pattern
        .clone()
        .context("the learned denoiser needs a Bayer mosaic")?;
    let noise = prepared
        .noise_model()
        .context("the learned denoiser needs the frame's noise model")?;
    let start = Instant::now();
    let key = cache.map(|c| (c, greycard_ai::DenoiseCache::key(model, tiling, prepared)));
    let (rgb, report) = match cache.and_then(|c| cached(c, model, tiling, prepared)) {
        Some(rgb) => (
            rgb,
            LearnedReport::Cached {
                seconds: start.elapsed().as_secs_f64(),
            },
        ),
        None => {
            // Not begun on a card short of the memory for it. Held
            // loaded between develops, the denoiser's run, not its
            // load, is what asks for the most.
            if denoiser.provider() == greycard_ai::Provider::WebGpu {
                crate::ai::denoise_room(tiling.tile)?;
            }
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
                provider: denoiser.label(),
                seconds: start.elapsed().as_secs_f64(),
            };
            if let Some((c, k)) = key
                && let Err(e) = c.write(k, frame, prepared, &rgb)
            {
                tracing::warn!("denoise cache {}: {e}", c.path(k).display());
            }
            (rgb, report)
        }
    };
    Ok((rgb, report))
}

#[cfg(test)]
impl FrameKept {
    /// What the window's walk of `edit` at `turn` on `input` keeps, on
    /// the CPU, with `models`: for the tests of what holds it.
    pub(super) fn walked_with(
        input: &crate::walk::Input,
        edit: &Edit,
        turn: u8,
        models: &mut dyn crate::walk::Models,
    ) -> FrameKept {
        let facts = input.facts(turn);
        let (steps, plan) = window_steps(edit, &facts, None, false);
        let mut held = FrameKept::new(&facts);
        let walked = crate::walk::walk(
            &steps,
            &plan,
            input,
            &mut held.kept,
            models,
            &mut None,
            &crate::walk::NoHold,
        )
        .expect("the walk");
        held.edit = edit.clone();
        held.turn = turn;
        held.stamp = 1;
        held.print = crate::mask_base::print(None, edit, turn, crate::mask_base::Size::Full);
        held.meta = walked.meta;
        held.guide = walked
            .taps
            .guide
            .unwrap_or_else(|| Arc::new(greycard_render::finish::Guide::NONE));
        held.planes = walked.taps.planes;
        if let Some(image) = held.kept.picture(Keep::Base) {
            held.image = image.clone();
        }
        held
    }

    /// [`FrameKept::walked_with`] a raw, with no models.
    pub(super) fn walked(frame: &Arc<RawFrame>, edit: &Edit, turn: u8) -> FrameKept {
        Self::walked_with(
            &crate::walk::Input::Raw(frame.clone()),
            edit,
            turn,
            &mut crate::walk::NoModels,
        )
    }
}

#[cfg(test)]
mod learned_tests {
    use greycard_ai::denoise::Tiling;
    use greycard_core::develop::{DevelopSettings, prepare};
    use greycard_core::raw::{
        Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Orientation, Samples, SensorLayout,
    };

    use super::*;

    /// A flat RGGB frame of `side` square.
    fn frame(side: usize) -> RawFrame {
        let pattern = CfaPattern::rggb();
        let mut samples = Vec::new();
        for y in 0..side {
            for x in 0..side {
                samples.push(match pattern.color_at(y, x) {
                    CfaColor::Red => 600u16,
                    CfaColor::Green => 900,
                    CfaColor::Blue => 350,
                    CfaColor::Other(_) => unreachable!(),
                });
            }
        }
        RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            other_names: Vec::new(),
            width: side,
            height: side,
            channels: 1,
            layout: SensorLayout::Cfa(pattern),
            samples: Samples::U16(samples),
            levels: Levels {
                black: LevelPattern::uniform(100.0, 1),
                white: LevelPattern::uniform(1100.0, 1),
                black_deltas: None,
                saturation: None,
            },
            as_shot_coefficients: Some([2.0, 1.0, 4.0]),
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix: vec![
                    1.7167, -0.3557, -0.2534, -0.6667, 1.6165, 0.0158, 0.0176, -0.0428, 0.9421,
                ],
                forward_matrix: None,
            }],
            crop: None,
            orientation: Orientation::Normal,
            shot: Default::default(),
            dng: Default::default(),
        }
    }

    fn at(tile: usize) -> Tiling {
        Tiling {
            tile,
            ..Tiling::default()
        }
    }

    /// The learned denoiser runs at its own tiling on a card with the
    /// room, the smaller tile, its margin kept, where the card is short.
    #[test]
    fn the_denoiser_runs_at_the_tile_the_card_has_room_for() {
        assert_eq!(run_tiling(DenoiseOn::Card, at(512)), at(512));
        assert_eq!(run_tiling(DenoiseOn::Tile(384), at(512)), at(384));
        assert_eq!(run_tiling(DenoiseOn::Tile(512), at(1024)).margin, 96);
    }

    /// An answer a short card kept at a smaller tile is read back for
    /// the tier's own: 384 and 256 under 512, and 512 under 1024; never
    /// one at a larger tile.
    #[test]
    fn the_answer_at_a_smaller_tile_is_read_back() {
        let frame = frame(8);
        let prepared = prepare(&frame, &DevelopSettings::default(), false).unwrap();
        let model = &greycard_ai::registry::DENOISE_FAST;
        let dir = std::env::temp_dir().join(format!(
            "greycard-learned-tiles-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        crate::testing::remove_dir_retry(&dir);
        let cache = greycard_ai::DenoiseCache::at(dir.clone());
        let rgb = vec![0.25f32; prepared.width * prepared.height * 3];
        let put = |tile: usize| {
            let key = greycard_ai::DenoiseCache::key(model, at(tile), &prepared);
            cache.write(key, &frame, &prepared, &rgb).unwrap();
        };
        assert!(cached(&cache, model, at(512), &prepared).is_none());
        put(256);
        assert!(cached(&cache, model, at(512), &prepared).is_some());
        assert!(cached(&cache, model, at(1024), &prepared).is_some());
        assert!(cached(&cache, model, at(256), &prepared).is_some());
        crate::testing::remove_dir_retry(&dir);
        put(512);
        assert!(cached(&cache, model, at(1024), &prepared).is_some());
        assert!(
            cached(&cache, model, at(384), &prepared).is_none(),
            "never a larger tile"
        );
        crate::testing::remove_dir_retry(&dir);
        put(1024);
        assert!(cached(&cache, model, at(512), &prepared).is_none());
        crate::testing::remove_dir_retry(&dir);
    }
}
