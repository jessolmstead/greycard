//! The walk: one function that runs a list of steps, from the last
//! point a walk before it kept, with each stage on the CPU or the
//! device as a plan places it.
//!
//! What a walk keeps is data: at each [`Keep`] it passes it stores the
//! picture, what the develop measured of it, the taps so far, the
//! steps up to there, and where each ran. The next walk starts from
//! the last kept point whose steps are its own and whose places its
//! plan allows, so an export with no device, whose plan is all CPU,
//! never starts from a picture the device touched.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use greycard_core::develop::ca::{self, CaFit, CaStats};
use greycard_core::develop::dehaze::{self, DehazeStats};
use greycard_core::develop::denoise;
use greycard_core::develop::local_contrast::{LocalContrastStats, local_contrast};
use greycard_core::develop::retouch::Region;
use greycard_core::develop::sharpen::{
    SharpenOptions, SharpenStats, sharpen_delta, sharpen_with_delta, sharpen_with_mask,
};
use greycard_core::develop::warp::{WarpPlacement, apply_warps};
use greycard_core::develop::{
    self as core_develop, DemosaicMethod, DevelopSettings, Prepared, demosaic_prepared,
    prepare_with,
};
use greycard_core::image::WorkingImage;
use greycard_core::picture::Picture;
use greycard_core::raw::{Orientation, RawFrame, SensorLayout};
use greycard_core::{CameraProfile, TempTint, WhiteBalance};
use greycard_edit::retouch::{Patch, Retouch};
use greycard_edit::{DetailWants, WhiteShift};
use greycard_gpu::wgpu;
use greycard_render::detail::DetailPlanes;
use greycard_render::finish::{Guide, Source, guide_plane};

use crate::stages::{Delta, Facts, Fills, Keep, Masks, Purpose, Stage, Step, Steps, Tap, quarters};

/// The open file: a raw to develop, or a picture already in the
/// working space, its image shared rather than copied.
#[derive(Clone)]
pub enum Input {
    Raw(Arc<RawFrame>),
    Picture {
        image: Arc<WorkingImage>,
        meta: Arc<Picture>,
    },
}

impl Input {
    /// What the list is derived from, at the frame's `turn`.
    pub fn facts(&self, turn: u8) -> Facts {
        match self {
            Input::Raw(frame) => Facts::raw(frame, turn),
            Input::Picture { image, meta } => Facts::picture(
                (image.width, image.height),
                meta.own_white(),
                &meta.make,
                &meta.model,
                &meta.shot,
                turn,
            ),
        }
    }
}

/// Where a step runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Cpu,
    Gpu,
}

/// Where each step of `steps` runs for `purpose`, with a device `lent`
/// or not. Every walk with a device runs there the stages that work a
/// picture in memory and hand it back (the CA, the demosaic, and the
/// lens: [`Stage::in_memory_on_device`]): the viewport, the prefetch,
/// and the export, so a base one makes is the base the others would,
/// and the editor's export and the command line's are one picture on
/// one machine. The viewport and the export also run there the Detail
/// section, the dehaze, and the sharpen ([`Stage::device`]): the
/// viewport's run ends at its half-float texture and its shader, the
/// export's in full floats read back for the CPU's finish. The reduced
/// pictures, the models' pictures, and a merge's frames are all the
/// CPU's.
pub fn plan(steps: &Steps, purpose: &Purpose, lent: bool) -> Vec<Place> {
    let shader = lent && matches!(purpose, Purpose::Viewport | Purpose::Prefetch);
    let runs = shader || (lent && matches!(purpose, Purpose::Export(_)));
    places(steps, shader, runs, runs)
}

/// The places of `steps` with the shader on the device where `shader`
/// says, the stages on pictures in memory where `in_memory` does, and
/// the rest with a device version where `runs` does. The
/// learned denoiser runs on its own providers, never as a device step;
/// its place is the one the demosaic standing in for it would take, so
/// a walk that drops it ([`replan`]) keeps the plan.
fn places(steps: &Steps, shader: bool, in_memory: bool, runs: bool) -> Vec<Place> {
    let stand_in = steps.ops().find_map(|s| match s {
        Stage::Prepare(p) => Some(
            Stage::Demosaic(p.settings.demosaic, p.settings.dual_contrast).in_memory_on_device(),
        ),
        _ => None,
    });
    steps
        .steps
        .iter()
        .map(|step| match step {
            Step::Op(Stage::Finish(crate::stages::Finish::Shader)) if shader => Place::Gpu,
            Step::Op(stage) if in_memory && stage.in_memory_on_device() => Place::Gpu,
            Step::Op(Stage::Learned(_)) if in_memory && stand_in == Some(true) => Place::Gpu,
            Step::Op(stage) if runs && stage.device() => Place::Gpu,
            _ => Place::Cpu,
        })
        .collect()
}

/// What the picture is between two steps.
pub enum Buf {
    /// One sample a site, balanced, in camera space.
    Mosaic(Prepared),
    /// Camera RGB, with what the move into the working space reads of
    /// the prepare.
    Camera {
        prepared: Prepared,
        rgb: Vec<f32>,
    },
    /// The learned denoiser's camera RGB and the plain demosaic's.
    CameraPair {
        prepared: Prepared,
        network: Vec<f32>,
        plain: Vec<f32>,
    },
    /// The pair in the working space.
    Pair {
        model: Arc<WorkingImage>,
        plain: Arc<WorkingImage>,
    },
    Working(Arc<WorkingImage>),
    /// A picture on the device.
    Device(Arc<greycard_gpu::Image>),
    /// The viewport's texture, which a run ending at the shader wrote.
    Texture(wgpu::Texture),
}

/// What the develop measured of the picture, carried beside it.
#[derive(Debug, Clone)]
pub struct Meta {
    /// A raw's white; none for a picture already rendered.
    pub white: Option<WhiteBalance>,
    /// The working-space value a channel clips at.
    pub clip_level: f32,
    /// The point spread the mosaic suggests, for an automatic sharpen.
    pub radius: Option<f32>,
    /// A raw's scene or a picture already rendered, for the finish.
    pub source: Source,
    /// The camera profile a raw's white was resolved through.
    pub profile: Option<Arc<CameraProfile>>,
    /// The full develop's size as it stands, before any bin.
    pub full: (u32, u32),
    /// The bin the picture was made at.
    pub bin: usize,
    /// Why the demosaic the edit chose did not run, when one of the
    /// frame's own stood in.
    pub gave_way: Option<String>,
    /// Why the learned denoiser did not run, when the engine's own
    /// path stood in for it: carried with the picture, so a walk that
    /// starts past the stand-in still says so.
    pub stood_in: Option<NoLearned>,
    /// A picture's own white and the light its white balance shows it
    /// at, the same at As shot; none for a raw.
    pub picture: Option<(TempTint, TempTint)>,
    /// A monochrome sensor's raw: no white moves it, a mask's neither.
    pub monochrome: bool,
}

impl Meta {
    /// Nothing measured yet.
    pub fn empty() -> Self {
        Self {
            white: None,
            clip_level: f32::INFINITY,
            radius: None,
            source: Source::default(),
            profile: None,
            full: (0, 0),
            bin: 1,
            gave_way: None,
            stood_in: None,
            picture: None,
            monochrome: false,
        }
    }

    /// The masks' white balances are made from this develop's white
    /// and the profile it was resolved through.
    /// A picture's are made from the light its own white balance
    /// shows it at.
    pub fn white_shift(&self) -> Option<WhiteShift> {
        if let Some((_, light)) = self.picture {
            return Some(WhiteShift::picture(light));
        }
        let (profile, wb) = (self.profile.as_ref()?, self.white.as_ref()?);
        if self.monochrome {
            return Some(WhiteShift::neutral(wb.temp_tint));
        }
        let cols = wb.matrix_f32();
        let rows = std::array::from_fn(|r| std::array::from_fn(|c| cols[c][r]));
        Some(WhiteShift::from_parts(
            (**profile).clone(),
            wb.coefficients_f32(),
            rows,
            wb.temp_tint,
        ))
    }
}

/// The products read off the picture so far.
#[derive(Clone, Default)]
pub struct Taps {
    pub guide: Option<Arc<Guide>>,
    pub planes: Option<Arc<DetailPlanes>>,
    /// The sharpen's delta for the masks' Sharpen, made by the sharpen
    /// and put with the planes at the walk's end.
    pub sharpen: Option<SharpenTap>,
    /// The picture the learned masks are made on, and how.
    pub masks: Option<(Masks, Arc<WorkingImage>)>,
    /// The picture the refined masks are made against.
    pub picture: Option<Arc<greycard_render::refine::MaskPicture>>,
    /// The picture the range masks read, from before the Detail
    /// section, put with the planes at the walk's end.
    pub sample: Option<Arc<WorkingImage>>,
    /// The picture the sharpen reads what is flat from
    /// ([`Tap::SharpenGuide`]), until the sharpen has read it.
    pub sharpen_guide: Option<SharpenGuide>,
}

/// The sharpen's guide where it stands: the picture tapped, or one
/// the device dehazed.
#[derive(Clone)]
pub enum SharpenGuide {
    Working(Arc<WorkingImage>),
    Device(Arc<greycard_gpu::Image>),
}

impl SharpenGuide {
    /// The guide in memory, for a sharpen on the CPU. One the device
    /// made is only ever read by the device: a point kept past it
    /// serves no walk that sharpens on the CPU ([`allowed`]), and a
    /// run that fails goes again from before the tap. Should one reach
    /// the CPU all the same, the sharpen reads its own input, as it
    /// did before the guide.
    fn working(guide: &Option<SharpenGuide>) -> Option<&WorkingImage> {
        match guide {
            Some(SharpenGuide::Working(g)) => Some(g),
            Some(SharpenGuide::Device(_)) => {
                debug_assert!(false, "the device's sharpen guide on the CPU");
                log::warn!("the sharpen's guide is on the device; the sharpen reads its input");
                None
            }
            None => None,
        }
    }
}

impl Taps {
    /// The guide plane, with the refined masks' picture beside it when
    /// one was made: what every finish of this walk reads.
    pub fn guide_for_finish(&self) -> Option<Arc<Guide>> {
        match (&self.guide, &self.picture) {
            (Some(g), Some(p)) => Some(Arc::new(g.with_picture(Some(p.clone())))),
            (g, _) => g.clone(),
        }
    }
}

/// The sharpen's delta as the sharpen made it: the picture's size, the
/// options it was made by, whether the picture was sharpened by it,
/// and the plane in stops.
#[derive(Clone)]
pub struct SharpenTap {
    pub width: usize,
    pub height: usize,
    pub options: SharpenOptions,
    pub sharpened: bool,
    pub delta: Arc<[half::f16]>,
}

impl SharpenTap {
    fn of(image: &WorkingImage, options: &SharpenOptions, sharpened: bool, delta: &[f32]) -> Self {
        Self {
            width: image.width,
            height: image.height,
            options: *options,
            sharpened,
            delta: greycard_render::detail::sharpen_delta(delta),
        }
    }
}

/// `planes` with the sharpen's delta `sharpen` beside them, or the
/// delta alone when no band was asked for; as they were without one.
fn with_sharpen(
    planes: Option<Arc<DetailPlanes>>,
    sharpen: Option<&SharpenTap>,
    clip_level: f32,
) -> Option<Arc<DetailPlanes>> {
    let Some(s) = sharpen else {
        return planes;
    };
    let mark = |mut p: DetailPlanes| {
        p.meta.sharpened = s.sharpened;
        Arc::new(p)
    };
    match planes {
        Some(p) if (p.width, p.height) == (s.width, s.height) => {
            Some(mark(p.with_sharpen(s.delta.clone())))
        }
        Some(p) => {
            log::warn!(
                "the sharpen's delta is {}x{}, the masks' bands {}x{}; the delta is left off",
                s.width,
                s.height,
                p.width,
                p.height
            );
            Some(p)
        }
        None => Some(mark(DetailPlanes::sharpen_only(
            s.width,
            s.height,
            clip_level,
            s.delta.clone(),
        ))),
    }
}

/// The rasters of the learned masks, by adjustment id and component.
pub type Rasters = std::collections::HashMap<(u64, usize), Arc<greycard_edit::brush::Raster>>;

/// Why the learned denoiser did not run.
#[derive(Debug, Clone)]
pub enum NoLearned {
    /// Its model is not in the store.
    Missing(&'static greycard_ai::Model),
    /// It could not run.
    Failed(String),
    /// The card had not the memory free for it
    /// ([`crate::ai::CardShort`]): a develop stands in, an export or a
    /// merge fails.
    Short(String),
}

/// The picture with its retouch applied, beside the base the learned
/// masks are made on.
#[derive(Clone, Copy)]
pub struct Patched<'a> {
    pub image: &'a Arc<WorkingImage>,
    /// The retouch as it was applied, its sources chosen.
    pub retouch: &'a Retouch,
    /// The Fill patches left as they were, by id: the picture there is
    /// the base's.
    pub unmade: &'a [u64],
}

/// The models a walk runs, lent by whoever holds them.
pub trait Models {
    /// Whether the learned denoiser `model` is to be had, and why not
    /// when it is not: asked before the walk starts, so a list that
    /// cannot run it is the engine's own from its first step, and
    /// resumes from what a walk of the same stand-in kept.
    fn can_denoise(&mut self, model: &'static greycard_ai::Model) -> Result<(), NoLearned>;

    /// The learned denoiser's camera RGB of `prepared`.
    fn denoise(
        &mut self,
        model: &'static greycard_ai::Model,
        frame: &RawFrame,
        prepared: &Prepared,
    ) -> Result<Vec<f32>, NoLearned>;

    /// A fill's pixels for `patch` over `region` of `picture`; none
    /// leaves the patch as it was.
    fn fill(&mut self, patch: &Patch, region: &Region, picture: &WorkingImage) -> Option<Vec<f32>>;

    /// A fill kept for `patch` at the full develop's size, `full`, read
    /// and taken down by `factor` to `region` of `picture`, that
    /// develop's picture reduced: none where none is kept for this
    /// picture. No model runs.
    fn fill_read(
        &mut self,
        _patch: &Patch,
        _region: &Region,
        _picture: &WorkingImage,
        _factor: usize,
        _full: (usize, usize),
    ) -> Option<Vec<f32>> {
        None
    }

    /// Whether a fill left unmade in `retouch` could be made now.
    fn could_fill_more(&mut self, retouch: &Retouch, picture: &WorkingImage) -> bool;

    /// The retouch is about to ask for its fills, over `picture`: a
    /// chance to count what is ahead before the first is asked.
    fn fills_ahead(&mut self, _retouch: &Retouch, _picture: &WorkingImage) {}

    /// The learned masks' rasters, made on `base` (and on `patched`
    /// where a patch meets a shape) as `how` says.
    fn rasters(
        &mut self,
        how: Masks,
        base: &Arc<WorkingImage>,
        patched: Option<Patched<'_>>,
        meta: &Meta,
    ) -> Rasters;
}

/// No models at all: the command line's, and a test's.
pub struct NoModels;

impl Models for NoModels {
    fn can_denoise(&mut self, model: &'static greycard_ai::Model) -> Result<(), NoLearned> {
        Err(NoLearned::Missing(model))
    }

    fn denoise(
        &mut self,
        model: &'static greycard_ai::Model,
        _: &RawFrame,
        _: &Prepared,
    ) -> Result<Vec<f32>, NoLearned> {
        Err(NoLearned::Missing(model))
    }

    fn fill(&mut self, _: &Patch, _: &Region, _: &WorkingImage) -> Option<Vec<f32>> {
        None
    }

    fn could_fill_more(&mut self, _: &Retouch, _: &WorkingImage) -> bool {
        false
    }

    fn rasters(
        &mut self,
        _: Masks,
        _: &Arc<WorkingImage>,
        _: Option<Patched<'_>>,
        _: &Meta,
    ) -> Rasters {
        Rasters::new()
    }
}

/// The background work a develop holds, and the give-way the prefetch
/// asks between its steps.
pub trait Hold {
    /// Hold the background work while a walk runs, or let it go.
    fn hold(&self, _on: bool) {}

    /// Asked before each stage: whether to stop the walk there.
    fn give_way(&self, _next: &Stage) -> bool {
        false
    }

    /// Whether a device error short of a lost device leaves the
    /// session its context, this walk going on on the CPU: a walk
    /// nobody waits on (a prefetch) or a file's (an export) is not
    /// worth the session's device over a failure that may be its own,
    /// such as running out of memory on a large frame.
    fn keeps_device(&self) -> bool {
        false
    }
}

/// Whether `e`, met by a walk under `hold`, leaves the session its
/// context.
fn keeps_device(hold: &dyn Hold, e: &greycard_gpu::Error) -> bool {
    hold.keeps_device() && !matches!(e, greycard_gpu::Error::Lost(_))
}

/// Nothing to hold and never a give-way: the viewport's walk, and the
/// tests'.
pub struct NoHold;

impl Hold for NoHold {}

/// An export's walk: nothing to hold and never a give-way, and a device
/// error on its frame, short of a lost device, runs that frame on the
/// CPU and leaves the session its context, so one large file never puts
/// the viewport on the CPU.
pub struct ExportHold;

impl Hold for ExportHold {
    fn keeps_device(&self) -> bool {
        true
    }
}

/// What a kept point holds of the picture.
#[derive(Clone)]
enum KeptBuf {
    Pair {
        model: Arc<WorkingImage>,
        plain: Arc<WorkingImage>,
    },
    Working(Arc<WorkingImage>),
    Device(Arc<greycard_gpu::Image>),
}

impl KeptBuf {
    fn buf(&self) -> Buf {
        match self {
            KeptBuf::Pair { model, plain } => Buf::Pair {
                model: model.clone(),
                plain: plain.clone(),
            },
            KeptBuf::Working(image) => Buf::Working(image.clone()),
            KeptBuf::Device(image) => Buf::Device(image.clone()),
        }
    }
}

/// A retouch as a walk applied it, and the Fill patches it left as
/// they were, by id: a walk that starts past it never asks for its
/// fills, and the masks must still know where the picture is the
/// base's.
#[derive(Clone)]
struct Applied {
    retouch: Retouch,
    unmade: Vec<u64>,
}

/// What the stages up to a kept point measured, and the sources the
/// retouch chose: a walk that starts from the point reports them as
/// its own, since its picture is theirs.
#[derive(Clone, Default)]
struct Ran {
    ca: Option<CaStats>,
    detail: Option<LocalContrastStats>,
    dehaze: Option<DehazeStats>,
    sources: Vec<Patch>,
}

impl Ran {
    fn of(walked: &Walked) -> Self {
        Self {
            ca: walked.ca,
            detail: walked.detail,
            dehaze: walked.dehaze,
            sources: walked.sources.clone(),
        }
    }
}

/// One point a walk kept.
#[derive(Clone)]
struct Point {
    keep: Keep,
    steps: Vec<Step>,
    places: Vec<Place>,
    buf: KeptBuf,
    meta: Meta,
    taps: Taps,
    /// The retouch this point's picture carries; none before the
    /// retouch.
    retouch: Option<Applied>,
    ran: Ran,
}

/// What walks keep for the walks after them. A clone shares every
/// picture it holds, and is a walk's own to resume from and keep in
/// without touching the original's.
#[derive(Clone, Default)]
pub struct Kept {
    /// The input every picture here was made from; none before a walk.
    from: Option<MadeFrom>,
    points: Vec<Point>,
    /// The masks' bands, with the steps to the picture they were made
    /// from and what was asked of them.
    planes: Option<KeptPlanes>,
    /// The sharpen's delta the last walk that made one made.
    sharpen: Option<SharpenTap>,
    /// A picture's upload, with the picture it was made from: the very
    /// one, not one of the same steps, since a picture made again with
    /// a step in another place (the CA on the CPU for an export) is not
    /// the same to the bit.
    uploaded: Option<(Arc<WorkingImage>, Arc<greycard_gpu::Image>)>,
    /// The dehaze's reduced copy on the device's picture, with the
    /// steps to the picture it was read off.
    haze: Option<(Vec<Step>, Arc<dehaze::Reduced>)>,
    /// The refined masks' picture, with the steps to the picture it
    /// was tapped from.
    mask_picture: Option<(Vec<Step>, Arc<greycard_render::refine::MaskPicture>)>,
}

/// An input as a [`Kept`] knows it: its own allocation, held weakly so
/// no other input can take its address while the `Kept` lives. Two
/// frames of a burst under one synced edit have equal steps, and are
/// never taken for one another.
#[derive(Clone)]
enum MadeFrom {
    Raw(std::sync::Weak<RawFrame>),
    Picture(std::sync::Weak<WorkingImage>),
}

impl MadeFrom {
    fn of(input: &Input) -> Self {
        match input {
            Input::Raw(frame) => MadeFrom::Raw(Arc::downgrade(frame)),
            Input::Picture { image, .. } => MadeFrom::Picture(Arc::downgrade(image)),
        }
    }

    fn is(&self, input: &Input) -> bool {
        match (self, input) {
            (MadeFrom::Raw(w), Input::Raw(frame)) => std::ptr::eq(w.as_ptr(), Arc::as_ptr(frame)),
            (MadeFrom::Picture(w), Input::Picture { image, .. }) => {
                std::ptr::eq(w.as_ptr(), Arc::as_ptr(image))
            }
            _ => false,
        }
    }
}

#[derive(Clone)]
struct KeptPlanes {
    steps: Vec<Step>,
    wants: DetailWants,
    haze_after: Option<greycard_core::develop::local_contrast::LocalContrastOptions>,
    planes: Arc<DetailPlanes>,
}

impl KeptPlanes {
    /// Whether these are the bands of the picture `to` makes, with
    /// every band `wants` asks for and a haze read after `haze_after`.
    fn serves(
        &self,
        to: &[Step],
        wants: &DetailWants,
        haze_after: Option<greycard_core::develop::local_contrast::LocalContrastOptions>,
    ) -> bool {
        same_picture(&self.steps, to)
            && self.wants.covers(wants)
            && (!wants.haze || self.haze_after == haze_after)
    }
}

/// Whether a point that ran at `kept` may serve a plan of `plan`: a
/// step the device ran serves only a plan that puts it there.
fn allowed(kept: &[Place], plan: &[Place]) -> bool {
    kept.iter()
        .zip(plan)
        .all(|(k, p)| *k == Place::Cpu || *k == *p)
}

/// [`allowed`], for a point whose steps `map` puts at those indices of
/// the walk's ([`align`]).
fn allowed_at(kept: &[Place], map: &[Option<usize>], plan: &[Place]) -> bool {
    kept.iter()
        .zip(map)
        .all(|(k, j)| *k == Place::Cpu || j.is_none_or(|j| plan.get(j) == Some(k)))
}

/// A step as the picture sees it: none for a keep, which changes
/// nothing, and the learned masks' tap whatever its purpose makes of
/// it, since the picture it reads is the same. The lists of two
/// purposes are one list so far as these agree.
fn keyed(step: &Step) -> Option<Step> {
    match step {
        Step::Keep(_) => None,
        Step::Tap(Tap::Masks(_)) => Some(Step::Tap(Tap::Masks(Masks::OnAsk))),
        step => Some(step.clone()),
    }
}

/// Whether two lists of steps make the same picture ([`keyed`]).
fn same_picture(a: &[Step], b: &[Step]) -> bool {
    a.iter().filter_map(keyed).eq(b.iter().filter_map(keyed))
}

/// Where a walk of `steps` goes on from a point kept after `kept`, when
/// those make the same picture as the start of `steps` ([`keyed`]):
/// the index in `steps` past the last of them, and the index in
/// `steps` of each of `kept` (none for a keep).
fn align(kept: &[Step], steps: &[Step]) -> Option<(usize, Vec<Option<usize>>)> {
    let mut j = 0;
    let mut map = Vec::with_capacity(kept.len());
    for step in kept {
        let Some(want) = keyed(step) else {
            map.push(None);
            continue;
        };
        while matches!(steps.get(j), Some(Step::Keep(_))) {
            j += 1;
        }
        if keyed(steps.get(j)?) != Some(want) {
            return None;
        }
        map.push(Some(j));
        j += 1;
    }
    Some((j, map))
}

impl Kept {
    /// Nothing kept.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind what is kept to `input`: kept for another input, all of it
    /// is let go first, so a walk never resumes from another frame's
    /// pictures however equal the steps.
    fn bind(&mut self, input: &Input) {
        if self.from.as_ref().is_some_and(|f| f.is(input)) {
            return;
        }
        if !self.points.is_empty() {
            log::debug!("the kept points were another input's; let go");
        }
        *self = Self {
            from: Some(MadeFrom::of(input)),
            ..Self::default()
        };
    }

    /// Take what is kept as made from `input`, as it is: for a caller
    /// that knows `input` is the very frame these were made from,
    /// decoded again (the shelf, by the file's path, its stat, and what
    /// its decode said).
    pub fn adopt(&mut self, input: &Input) {
        self.from = Some(MadeFrom::of(input));
    }

    /// Whether anything is kept at `keep`.
    pub fn holds(&self, keep: Keep) -> bool {
        self.points.iter().any(|p| p.keep == keep)
    }

    /// Whether a point kept for `steps` serves `plan` at `keep`.
    pub fn serves(&self, steps: &Steps, plan: &[Place], keep: Keep) -> bool {
        let Some(at) = steps.at(keep) else {
            return false;
        };
        self.points.iter().any(|p| {
            p.keep == keep
                && same_picture(&p.steps, &steps.steps[..=at])
                && align(&p.steps, &steps.steps)
                    .is_some_and(|(_, map)| allowed_at(&p.places, &map, plan))
        })
    }

    /// The point a walk of `steps` under `plan` starts from: the last
    /// one whose steps make the picture the start of its own make
    /// ([`align`]), whatever purpose kept it, and whose places the plan
    /// allows, its patched picture only while no fill left unmade in it
    /// could be made now. The points past it are let go, as they were
    /// another list's.
    fn resume(&mut self, steps: &Steps, plan: &[Place], models: &mut dyn Models) -> Option<usize> {
        let found = self
            .points
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let (at, map) = align(&p.steps, &steps.steps)?;
                // The walk's next stage, past its own keeps there.
                let next = (at..steps.steps.len())
                    .find(|&k| !matches!(steps.steps[k], Step::Keep(_)))
                    .unwrap_or(steps.steps.len());
                let serves = allowed_at(&p.places, &map, plan)
                    // A picture on the device serves only a walk that
                    // goes on there: the viewport's texture is made by
                    // a run, not from a picture left on the device.
                    && (!matches!(p.buf, KeptBuf::Device(_))
                        || (plan.get(next) == Some(&Place::Gpu)
                            && !matches!(steps.steps.get(next), Some(Step::Op(Stage::Finish(_))))))
                    && match (&p.retouch, &p.buf) {
                        (Some(a), KeptBuf::Working(image)) if !a.unmade.is_empty() => {
                            !models.could_fill_more(&a.retouch, image)
                        }
                        _ => true,
                    };
                serves.then_some((i, (at, p.steps.len())))
            })
            .max_by_key(|(_, key)| *key)?;
        let (best, (at, _)) = found;
        let keep: Vec<bool> = self
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| i == best || align(&p.steps, &steps.steps).is_some_and(|(a, _)| a <= at))
            .collect();
        let index = keep[..best].iter().filter(|k| **k).count();
        let mut flags = keep.into_iter();
        self.points.retain(|_| flags.next().unwrap_or(false));
        self.drop_orphaned_upload();
        Some(index)
    }

    /// The kept base and the learned pair made at another turn, each
    /// turned to this one where nothing after the turn minds which way
    /// up it is: the index of the later of them, now turned. A base
    /// blended at an end of the strength shares the pair's picture, and
    /// still does turned. The masks' tap reads the base, so it reads the
    /// turned one.
    fn turned(&mut self, steps: &Steps, plan: &[Place]) -> Option<usize> {
        let want = turnless(&steps.steps);
        let to = turn_of(&steps.steps[..])?;
        let fits = |p: &Point| {
            matches!(p.keep, Keep::Base | Keep::Pair)
                && !matches!(p.buf, KeptBuf::Device(_))
                && allowed(&p.places, plan)
                && turnless(&p.steps) == want[..turnless(&p.steps).len().min(want.len())]
                && commutes_after_turn(&p.steps)
                && steps.at(p.keep).is_some()
                && turn_of(&p.steps)
                    .and_then(|from| relative(from, to))
                    .is_some()
        };
        if !self.points.iter().any(fits) {
            return None;
        }
        let mut points: Vec<Point> = std::mem::take(&mut self.points)
            .into_iter()
            .filter(fits)
            .collect();
        points.sort_by_key(|p| p.steps.len());
        // Each picture turned once, and shared on where it was shared.
        let mut done: Vec<(*const WorkingImage, Arc<WorkingImage>)> = Vec::new();
        for p in &mut points {
            let from = turn_of(&p.steps).expect("fits");
            let q = relative(from, to).expect("fits");
            let o = quarters(q).expect("a turn that moves");
            let mut turn = |image: &Arc<WorkingImage>| {
                let at = Arc::as_ptr(image);
                if let Some((_, t)) = done.iter().find(|(p, _)| *p == at) {
                    return t.clone();
                }
                let t = Arc::new(core_develop::orient((**image).clone(), o));
                done.push((at, t.clone()));
                t
            };
            p.buf = match &p.buf {
                KeptBuf::Pair { model, plain } => KeptBuf::Pair {
                    model: turn(model),
                    plain: turn(plain),
                },
                KeptBuf::Working(image) => KeptBuf::Working(turn(image)),
                KeptBuf::Device(_) => unreachable!("fits"),
            };
            if let (KeptBuf::Working(image), Some((_, tapped))) = (&p.buf, p.taps.masks.as_mut()) {
                *tapped = image.clone();
            }
            if q % 2 == 1 {
                p.meta.full = (p.meta.full.1, p.meta.full.0);
            }
            let n = steps.at(p.keep).expect("fits");
            p.steps = steps.steps[..=n].to_vec();
            p.places = plan[..=n].to_vec();
        }
        self.planes = None;
        self.sharpen = None;
        self.uploaded = None;
        self.haze = None;
        self.points = points;
        Some(self.points.len() - 1)
    }

    fn put(&mut self, point: Point) {
        self.points.retain(|p| p.keep != point.keep);
        self.points.push(point);
        self.drop_orphaned_upload();
    }

    /// The upload, and the dehaze's copy read off its run, let go once
    /// no point holds the picture it was made from: that picture was
    /// made again (one with no device, its CA on the CPU), and the upload
    /// would hold the old one for nothing until the next run.
    fn drop_orphaned_upload(&mut self) {
        let held = |from: &Arc<WorkingImage>| {
            self.points
                .iter()
                .any(|p| matches!(&p.buf, KeptBuf::Working(image) if Arc::ptr_eq(image, from)))
        };
        if self.uploaded.as_ref().is_some_and(|(from, _)| !held(from)) {
            self.uploaded = None;
            self.haze = None;
        }
    }

    /// Take the picture kept in memory at `keep` out of what is kept.
    pub fn take_working(&mut self, keep: Keep) -> Option<Arc<WorkingImage>> {
        let i = self
            .points
            .iter()
            .position(|p| p.keep == keep && matches!(p.buf, KeptBuf::Working(_)))?;
        let KeptBuf::Working(image) = self.points.remove(i).buf else {
            unreachable!("found in memory")
        };
        self.drop_orphaned_upload();
        Some(image)
    }

    /// Let go of every point past a retouch that left a fill unmade,
    /// on the device or not: the walk after starts before the retouch
    /// and asks for those fills again.
    pub fn drop_unmade(&mut self) {
        self.points
            .retain(|p| p.retouch.as_ref().is_none_or(|a| a.unmade.is_empty()));
        self.drop_orphaned_upload();
    }

    /// Let go of everything kept on the device: its context is gone.
    pub fn forget_device(&mut self) {
        self.points.retain(|p| !matches!(p.buf, KeptBuf::Device(_)));
        self.uploaded = None;
        self.haze = None;
    }

    /// What a frame left behind keeps: the learned pair, the base, and
    /// the patched picture, in memory. Everything on the device goes,
    /// which it would hold for a frame not on screen, and the masks'
    /// bands, which are made again beside the develop and cost as much
    /// as the base.
    pub fn shelve(&mut self) {
        self.forget_device();
        self.points
            .retain(|p| matches!(p.keep, Keep::Pair | Keep::Base | Keep::Patched));
        self.planes = None;
        self.sharpen = None;
        self.mask_picture = None;
    }

    /// Nothing kept at all.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The picture kept at `keep`, when it is one in memory.
    pub fn picture(&self, keep: Keep) -> Option<&Arc<WorkingImage>> {
        self.points
            .iter()
            .find(|p| p.keep == keep)
            .and_then(|p| match &p.buf {
                KeptBuf::Working(image) => Some(image),
                _ => None,
            })
    }

    /// What the develop measured of the picture kept at `keep`.
    pub fn meta(&self, keep: Keep) -> Option<&Meta> {
        self.points.iter().find(|p| p.keep == keep).map(|p| &p.meta)
    }

    /// The patched picture, when a retouch made it, with the retouch as
    /// it was applied and the Fill patches it left as they were.
    pub fn patched(&self) -> Option<Patched<'_>> {
        let p = self.points.iter().find(|p| p.keep == Keep::Patched)?;
        match (&p.buf, &p.retouch) {
            (KeptBuf::Working(image), Some(applied)) => Some(Patched {
                image,
                retouch: &applied.retouch,
                unmade: &applied.unmade,
            }),
            _ => None,
        }
    }

    /// The masks' bands kept, when they hold every band `wants` asks
    /// for, with the haze read after `haze_after` when it asks for one,
    /// and the sharpen's delta made by `sharpen` when it asks for that.
    pub fn planes_for(
        &self,
        wants: &DetailWants,
        haze_after: Option<greycard_core::develop::local_contrast::LocalContrastOptions>,
        sharpen: &SharpenOptions,
    ) -> Option<Arc<DetailPlanes>> {
        let delta = match &self.sharpen {
            Some(s) if wants.sharpen && s.options == *sharpen => Some(s),
            _ if wants.sharpen => return None,
            _ => None,
        };
        let bands = DetailWants {
            sharpen: false,
            sample: false,
            ..*wants
        };
        let planes = if bands.bands() {
            Some(
                self.planes
                    .as_ref()
                    .filter(|k| {
                        k.wants.covers(&bands) && (!wants.haze || k.haze_after == haze_after)
                    })
                    .map(|k| k.planes.clone())?,
            )
        } else {
            None
        };
        let clip = self
            .points
            .last()
            .map_or(f32::INFINITY, |p| p.meta.clip_level);
        with_sharpen(planes, delta, clip)
    }

    /// Whether the base kept here and the one `other` keeps are the
    /// same picture: made by the same steps.
    pub fn same_base(&self, other: &Kept) -> bool {
        let steps = |k: &Kept| {
            k.points
                .iter()
                .find(|p| p.keep == Keep::Base)
                .map(|p| p.steps.clone())
        };
        match (steps(self), steps(other)) {
            (Some(a), Some(b)) => same_picture(&a, &b),
            _ => false,
        }
    }

    /// Let go of the refined masks' picture: the edit refines no mask.
    pub fn forget_mask_picture(&mut self) {
        self.mask_picture = None;
    }

    /// The refined masks' picture a walk on the CPU tapped.
    pub fn mask_picture(&self) -> Option<&Arc<greycard_render::refine::MaskPicture>> {
        self.mask_picture.as_ref().map(|(_, picture)| picture)
    }

    /// The bytes of the pictures kept in memory, each counted once
    /// however many points share it, the sharpen's guide a point holds
    /// in memory among them, and the refined masks' picture.
    pub fn bytes(&self) -> u64 {
        let mut seen: Vec<*const WorkingImage> = Vec::new();
        let mut total = 0u64;
        let mut count = |image: &Arc<WorkingImage>| {
            let at = Arc::as_ptr(image);
            if !seen.contains(&at) {
                seen.push(at);
                total += (image.data.len() * std::mem::size_of::<f32>()) as u64;
            }
        };
        for p in &self.points {
            match &p.buf {
                KeptBuf::Pair { model, plain } => {
                    count(model);
                    count(plain);
                }
                KeptBuf::Working(image) => count(image),
                KeptBuf::Device(_) => {}
            }
            // Dehazed, the guide is a picture of its own.
            if let Some(SharpenGuide::Working(guide)) = &p.taps.sharpen_guide {
                count(guide);
            }
        }
        total + self.mask_picture().map_or(0, |p| p.bytes())
    }

    /// Whether the point at `keep` is a picture on the device.
    pub fn on_device(&self, keep: Keep) -> bool {
        self.points
            .iter()
            .any(|p| p.keep == keep && matches!(p.buf, KeptBuf::Device(_)))
    }

    /// The steps for which `stage` holds marked as run on the device,
    /// as a run there would have left them: for the tests of what a
    /// device's points serve.
    #[cfg(test)]
    pub(crate) fn as_if_on_device(&mut self, stage: impl Fn(&Stage) -> bool) {
        for p in &mut self.points {
            for (s, place) in p.steps.iter().zip(p.places.iter_mut()) {
                if matches!(s, Step::Op(op) if stage(op)) {
                    *place = Place::Gpu;
                }
            }
        }
    }

    /// The patched picture's upload, for the tests of what is kept.
    #[cfg(test)]
    pub(crate) fn upload(&self) -> Option<&Arc<greycard_gpu::Image>> {
        self.uploaded.as_ref().map(|(_, image)| image)
    }

    /// The dehaze's reduced copy, for the tests of what is kept.
    #[cfg(test)]
    pub(crate) fn haze_copy(&self) -> Option<&Arc<dehaze::Reduced>> {
        self.haze.as_ref().map(|(_, reduced)| reduced)
    }

    /// Whether the patched picture's upload is kept on the device.
    pub fn has_upload(&self) -> bool {
        self.uploaded.is_some()
    }

    /// Whether the dehaze's reduced copy is kept.
    pub fn has_haze(&self) -> bool {
        self.haze.is_some()
    }

    /// Whether the step `stage` of the point kept at `keep` ran on the
    /// device.
    pub fn ran_on_device(&self, keep: Keep, stage: impl Fn(&Stage) -> bool) -> bool {
        self.points.iter().any(|p| {
            p.keep == keep
                && p.steps.iter().zip(&p.places).any(|(s, place)| {
                    matches!(s, Step::Op(op) if stage(op)) && *place == Place::Gpu
                })
        })
    }
}

/// The steps with every turn left out: what two lists that differ only
/// in the frame's turn share.
fn turnless(steps: &[Step]) -> Vec<Step> {
    steps
        .iter()
        .filter(|s| !matches!(s, Step::Op(Stage::Orient(_))))
        .map(|s| match s {
            Step::Op(Stage::IntoWorking {
                profile,
                profile_look,
                look_baseline,
                ..
            }) => Step::Op(Stage::IntoWorking {
                profile: profile.clone(),
                profile_look: *profile_look,
                look_baseline: *look_baseline,
                orientation: Orientation::Normal,
            }),
            s => s.clone(),
        })
        .collect()
}

/// Whether everything after the turn in `steps` gives the same picture
/// turned, to the bit, and no tap made data on the unturned picture.
fn commutes_after_turn(steps: &[Step]) -> bool {
    steps.iter().all(|s| match s {
        Step::Op(stage) => stage.commutes_with_turn(),
        Step::Tap(Tap::Masks(m)) => *m == Masks::OnAsk,
        Step::Tap(_) => false,
        Step::Keep(_) => true,
    })
}

/// The orientation a list shows its frame in: a raw's move into the
/// working space, or a picture's turn (none for none).
fn turn_of(steps: &[Step]) -> Option<Option<Orientation>> {
    for s in steps {
        match s {
            Step::Op(Stage::IntoWorking { orientation, .. }) => return Some(Some(*orientation)),
            Step::Op(Stage::Orient(o)) => return Some(Some(*o)),
            _ => {}
        }
    }
    // A picture at no turn has no step for it.
    Some(None)
}

/// The quarter turns that take a picture shown at `from` to `to`.
fn relative(from: Option<Orientation>, to: Option<Orientation>) -> Option<u8> {
    let from = from.unwrap_or(Orientation::Normal);
    let to = to.unwrap_or(Orientation::Normal);
    (1..4u8).find(|q| from.turned(i32::from(*q)) == to)
}

/// How a walk did not end at its last step.
#[derive(Debug)]
pub enum Stopped {
    /// It gave way before this stage.
    GaveWay(&'static str),
    /// A stage failed.
    Failed(String),
}

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stopped::GaveWay(stage) => write!(f, "gave way before the {stage}"),
            Stopped::Failed(why) => f.write_str(why),
        }
    }
}

/// What a walk made.
pub struct Walked {
    /// The picture where the walk stopped: before the finish, or the
    /// viewport's texture a run on the device wrote.
    pub buf: Buf,
    pub meta: Meta,
    pub taps: Taps,
    /// The learned masks' rasters, made at the end from the tap.
    pub rasters: Rasters,
    /// The sharpen's blend, of the CPU's sharpen, for the viewport to
    /// paint where it acted.
    pub sharpen_mask: Option<Vec<f32>>,
    pub ca: Option<CaStats>,
    /// The CA's fit, when the list asked for it to be handed back.
    pub ca_fit: Option<Arc<CaFit>>,
    pub detail: Option<LocalContrastStats>,
    pub dehaze: Option<DehazeStats>,
    pub sharpen: Option<SharpenStats>,
    /// The patches with the sources the retouch chose for them.
    pub sources: Vec<Patch>,
    /// The Fill patches the picture left as they were, by id, whether
    /// this walk's retouch or the kept point's it started from made it;
    /// none where no retouch made the picture.
    pub unmade: Option<Vec<u64>>,
    /// What was left of the edit: a fill refused, by name.
    pub without: Vec<String>,
    /// The kept point the walk started from; none for the decode.
    pub from: Option<Keep>,
    /// The step the walk started at: the ones before it are the kept
    /// point's, and their stats are what it measured then.
    pub begin: usize,
    /// Where each step ran, the kept ones as they ran then.
    pub places: Vec<Place>,
    /// The seconds each stage took, in order, for the log.
    pub seconds: Vec<(&'static str, f64)>,
}

/// The state a walk carries between its steps.
struct Carried {
    buf: Buf,
    meta: Meta,
    taps: Taps,
    retouch: Option<Applied>,
    /// What the stages before the walk's start measured.
    ran: Ran,
    /// The prepare's settings, which the demosaic, the denoise, and
    /// the move into the working space read.
    settings: DevelopSettings,
}

/// An attempt's walk, and the planes still being made beside it.
struct Out {
    walked: Walked,
    planes: Option<PlanesJob>,
}

/// Run `steps` under `plan` on `input`, from what `kept` holds,
/// keeping at each keep; the models are `models`'s, the device `gpu`'s,
/// asked of `hold` between steps whether to give way. A device op the
/// picture is too much for (`Unsupported`) runs its run again on the
/// CPU, the context kept; any other device error lets the context go
/// and runs the rest on the CPU. A learned denoiser that is not to be
/// had stands in the engine's own path.
pub fn walk(
    steps: &Steps,
    plan: &[Place],
    input: &Input,
    kept: &mut Kept,
    models: &mut dyn Models,
    gpu: &mut Option<greycard_gpu::Context>,
    hold: &dyn Hold,
) -> Result<Walked, Stopped> {
    hold.hold(true);
    let out = walk_held(steps, plan, input, kept, models, gpu, hold);
    hold.hold(false);
    out
}

fn walk_held(
    steps: &Steps,
    plan: &[Place],
    input: &Input,
    kept: &mut Kept,
    models: &mut dyn Models,
    gpu: &mut Option<greycard_gpu::Context>,
    hold: &dyn Hold,
) -> Result<Walked, Stopped> {
    kept.bind(input);
    let mut steps = steps.clone();
    let mut plan = plan.to_vec();
    if gpu.is_none() {
        plan.iter_mut().for_each(|p| *p = Place::Cpu);
    }
    let mut stood_in = None;
    let learned = steps.ops().find_map(|s| match s {
        Stage::Learned(model) => Some(*model),
        _ => None,
    });
    if let Some(model) = learned
        && let Err(why) = models.can_denoise(model)
    {
        log::info!("the learned denoiser: {why:?}; the engine's own path stands in");
        plan = replan(&steps, &plan);
        steps = steps.without_learned();
        stood_in = Some(why);
    }
    loop {
        match attempt(&steps, &mut plan, input, kept, models, gpu, hold, &stood_in) {
            Ok(mut out) => {
                let mut walked = out.walked;
                if let Some(job) = out.planes.take() {
                    walked.taps.planes = job.land(kept);
                }
                // A walk to the finish, past the sharpen, leaves the
                // delta it made or carried, or none: a delta kept from an
                // earlier picture is not this one's.
                if steps.ops().any(|s| matches!(s, Stage::Finish(_))) {
                    kept.sharpen = walked.taps.sharpen.clone();
                }
                walked.taps.planes = with_sharpen(
                    walked.taps.planes.take(),
                    walked.taps.sharpen.as_ref(),
                    walked.meta.clip_level,
                );
                walked.taps.planes = DetailPlanes::with_sample(
                    walked.taps.planes.take(),
                    walked.taps.sample.take(),
                    walked.meta.clip_level,
                );
                if let Some((how, base)) = walked.taps.masks.clone() {
                    // The patched picture, only where a retouch made it.
                    let point = kept.points.iter().find(|p| p.keep == Keep::Patched);
                    debug_assert!(
                        !point.is_some_and(|p| matches!(p.buf, KeptBuf::Device(_))),
                        "the patched picture is kept in memory, before any device run"
                    );
                    let patched = point.and_then(|p| match (&p.buf, &p.retouch) {
                        (KeptBuf::Working(image), Some(applied)) => Some(Patched {
                            image,
                            retouch: &applied.retouch,
                            unmade: &applied.unmade,
                        }),
                        _ => None,
                    });
                    walked.rasters = models.rasters(how, &base, patched, &walked.meta);
                }
                return Ok(walked);
            }
            Err(Retry::Learned(why)) => {
                log::info!("the learned denoiser: {why:?}; the engine's own path stands in");
                plan = replan(&steps, &plan);
                steps = steps.without_learned();
                stood_in = Some(why);
            }
            Err(Retry::Cpu { from, keep_context }) => {
                if !keep_context && let Some(ctx) = gpu.take() {
                    ctx.release();
                    kept.forget_device();
                }
                let rest = if keep_context { from.1 } else { plan.len() };
                plan[from.0..rest].iter_mut().for_each(|p| *p = Place::Cpu);
            }
            Err(Retry::Stop(stopped)) => return Err(stopped),
        }
    }
}

/// `plan`, the places of `steps`, carried over to
/// [`Steps::without_learned`]'s list: the steps it drops dropped, the
/// demosaic standing in for the learned denoiser at the place the plan
/// gave the denoiser ([`places`]).
fn replan(steps: &Steps, plan: &[Place]) -> Vec<Place> {
    steps
        .steps
        .iter()
        .zip(plan)
        .filter(|(s, _)| !matches!(s, Step::Keep(Keep::Pair) | Step::Op(Stage::Blend { .. })))
        .map(|(_, p)| *p)
        .collect()
}

/// Why an attempt at a walk is to be made again, or not.
enum Retry {
    /// The learned denoiser is not to be had.
    Learned(NoLearned),
    /// A run on the device declined or failed: the steps `from` (a
    /// range) go to the CPU, and the context stays when the decline
    /// was the picture's.
    Cpu {
        from: (usize, usize),
        keep_context: bool,
    },
    Stop(Stopped),
}

fn failed(e: impl std::fmt::Display) -> Retry {
    Retry::Stop(Stopped::Failed(e.to_string()))
}

/// The start of a walk: a kept point, a kept base turned, or the
/// decode.
fn start(
    steps: &Steps,
    plan: &[Place],
    input: &Input,
    kept: &mut Kept,
    models: &mut dyn Models,
) -> Result<(usize, Carried, Option<Keep>, Vec<Place>), Retry> {
    let point = kept
        .resume(steps, plan, models)
        .or_else(|| kept.turned(steps, plan));
    if let Some(i) = point {
        let p = &kept.points[i];
        let (at, map) = align(&p.steps, &steps.steps).expect("a point resumed is the walk's");
        let mut places = vec![Place::Cpu; at];
        for (place, j) in p.places.iter().zip(&map) {
            if let Some(j) = j {
                places[*j] = *place;
            }
        }
        // The masks are made as this walk's purpose makes them.
        let mut taps = p.taps.clone();
        let how = steps.steps[..at].iter().find_map(|s| match s {
            Step::Tap(Tap::Masks(how)) => Some(*how),
            _ => None,
        });
        if let (Some(how), Some((made, _))) = (how, taps.masks.as_mut()) {
            *made = how;
        }
        // The masks' bands are made beside the run on the device and land
        // after its keeps are put: a point kept there holds none, and the
        // walk resumed from it does not pass their tap again.
        if taps.planes.is_none() {
            taps.planes = steps.steps[..at]
                .iter()
                .enumerate()
                .find_map(|(t, s)| match s {
                    Step::Tap(Tap::Planes {
                        wants, haze_after, ..
                    }) => kept
                        .planes
                        .as_ref()
                        .filter(|k| k.serves(&steps.steps[..t], wants, *haze_after))
                        .map(|k| k.planes.clone()),
                    _ => None,
                });
        }
        let settings = steps
            .ops()
            .find_map(|s| match s {
                Stage::Prepare(p) => Some(p.settings.clone()),
                _ => None,
            })
            .unwrap_or_default();
        return Ok((
            at,
            Carried {
                buf: p.buf.buf(),
                meta: p.meta.clone(),
                taps,
                retouch: p.retouch.clone(),
                ran: p.ran.clone(),
                settings,
            },
            Some(p.keep),
            places,
        ));
    }
    kept.points.clear();
    let (buf, meta) = match input {
        Input::Raw(frame) => (
            // Made by the prepare, which reads the frame itself.
            Buf::Working(Arc::new(WorkingImage::new(0, 0))),
            Meta {
                white: None,
                clip_level: f32::INFINITY,
                radius: None,
                source: Source::of_frame(frame),
                profile: None,
                full: (0, 0),
                bin: 1,
                gave_way: None,
                stood_in: None,
                picture: None,
                monochrome: frame.layout == SensorLayout::Monochrome,
            },
        ),
        Input::Picture { image, meta } => (
            Buf::Working(image.clone()),
            Meta {
                white: None,
                clip_level: meta.clip_level(),
                radius: None,
                source: Source::Display,
                profile: None,
                full: (image.width as u32, image.height as u32),
                bin: 1,
                gave_way: None,
                stood_in: None,
                picture: Some((meta.own_white(), meta.own_white())),
                monochrome: false,
            },
        ),
    };
    Ok((
        0,
        Carried {
            buf,
            meta,
            taps: Taps::default(),
            retouch: None,
            ran: Ran::default(),
            settings: DevelopSettings::default(),
        },
        None,
        Vec::new(),
    ))
}

// The walk's seven, as `walk` takes them, and the tier that stood in.
#[allow(clippy::too_many_arguments)]
fn attempt(
    steps: &Steps,
    plan: &mut [Place],
    input: &Input,
    kept: &mut Kept,
    models: &mut dyn Models,
    gpu: &mut Option<greycard_gpu::Context>,
    hold: &dyn Hold,
    stood_in: &Option<NoLearned>,
) -> Result<Out, Retry> {
    let (begin, mut st, from, mut places) = start(steps, plan, input, kept, models)?;
    // Said afresh on every walk: the stand-in's list is the one an
    // edit without the learned denoiser makes, so a point kept by the
    // one serves the other, and must not say a stand-in it never had.
    st.meta.stood_in = stood_in.clone();
    let ran = std::mem::take(&mut st.ran);
    let mut walked = Walked {
        buf: Buf::Working(Arc::new(WorkingImage::new(0, 0))),
        meta: st.meta.clone(),
        taps: Taps::default(),
        rasters: Rasters::new(),
        sharpen_mask: None,
        ca: ran.ca,
        ca_fit: None,
        detail: ran.detail,
        dehaze: ran.dehaze,
        sharpen: None,
        sources: ran.sources,
        unmade: None,
        without: Vec::new(),
        from,
        begin,
        places: Vec::new(),
        seconds: Vec::new(),
    };
    let mut planes = None;
    let mut i = begin;
    while i < steps.steps.len() {
        let step = &steps.steps[i];
        if let Step::Op(stage) = step
            && !matches!(stage, Stage::Finish(_))
            && hold.give_way(stage)
        {
            return Err(Retry::Stop(Stopped::GaveWay(stage.name())));
        }
        match step {
            Step::Keep(keep) => {
                places.push(Place::Cpu);
                let buf = match &st.buf {
                    Buf::Pair { model, plain } => KeptBuf::Pair {
                        model: model.clone(),
                        plain: plain.clone(),
                    },
                    Buf::Working(image) => KeptBuf::Working(image.clone()),
                    Buf::Device(image) => KeptBuf::Device(image.clone()),
                    _ => unreachable!("a keep follows a picture"),
                };
                kept.put(Point {
                    keep: *keep,
                    steps: steps.steps[..=i].to_vec(),
                    places: places.clone(),
                    buf,
                    meta: st.meta.clone(),
                    taps: st.taps.clone(),
                    retouch: st.retouch.clone(),
                    ran: Ran::of(&walked),
                });
                i += 1;
            }
            Step::Tap(tap) => {
                places.push(Place::Cpu);
                let Buf::Working(image) = &st.buf else {
                    unreachable!("a tap reads a picture in memory")
                };
                match tap {
                    Tap::Guide => {
                        let started = Instant::now();
                        st.taps.guide = Some(Arc::new(guide_plane(image)));
                        walked
                            .seconds
                            .push(("guide", started.elapsed().as_secs_f64()));
                    }
                    Tap::Masks(how) => st.taps.masks = Some((*how, image.clone())),
                    Tap::Sample => st.taps.sample = Some(image.clone()),
                    Tap::SharpenGuide => {
                        st.taps.sharpen_guide = Some(SharpenGuide::Working(image.clone()));
                    }
                    Tap::MaskPicture => {
                        let to = &steps.steps[..i];
                        match &kept.mask_picture {
                            Some((s, picture)) if same_picture(s, to) => {
                                st.taps.picture = Some(picture.clone());
                            }
                            _ => {
                                let started = Instant::now();
                                let picture =
                                    Arc::new(greycard_render::refine::mask_picture(image));
                                kept.mask_picture = Some((to.to_vec(), picture.clone()));
                                st.taps.picture = Some(picture);
                                walked
                                    .seconds
                                    .push(("mask picture", started.elapsed().as_secs_f64()));
                            }
                        }
                    }
                    Tap::Planes {
                        wants,
                        haze_after,
                        beside,
                    } => {
                        let to = steps.steps[..i].to_vec();
                        let kept_planes =
                            kept.planes.as_ref().filter(|k| same_picture(&k.steps, &to));
                        match kept_planes {
                            Some(k) if k.serves(&to, wants, *haze_after) => {
                                st.taps.planes = Some(k.planes.clone());
                            }
                            k => {
                                let reuse = k.map(|k| k.planes.clone());
                                kept.planes = None;
                                if *beside {
                                    planes = PlanesJob::start(
                                        image.clone(),
                                        to,
                                        *haze_after,
                                        st.meta.clip_level,
                                        *wants,
                                        reuse,
                                    );
                                } else {
                                    st.taps.planes = DetailPlanes::make(
                                        image,
                                        *haze_after,
                                        st.meta.clip_level,
                                        *wants,
                                    )
                                    .map(Arc::new);
                                }
                            }
                        }
                    }
                }
                i += 1;
            }
            Step::Op(Stage::Finish(_)) => {
                // The finish is the caller's: the shader's, or the
                // render crate's on what the walk yields.
                places.push(plan[i]);
                i += 1;
            }
            Step::Op(stage) if plan[i] == Place::Gpu && !matches!(stage, Stage::Learned(_)) => {
                let end = run_end(steps, plan, i);
                let ctx = gpu.as_ref().expect("a plan on the device has a context");
                let inline = matches!(
                    step,
                    Step::Op(Stage::Ca(_) | Stage::Demosaic(..) | Stage::Lens(_))
                );
                match device_run(steps, i..end, ctx, &mut st, kept, &mut places, &mut walked) {
                    Ok(()) => i = end,
                    // The CA, the demosaic, and the lens read a picture
                    // still in hand: their run again on the CPU is the
                    // stage itself.
                    Err(e) if inline => {
                        let Step::Op(stage) = step else {
                            unreachable!()
                        };
                        match e {
                            greycard_gpu::Error::Unsupported(why) => {
                                log::info!("the {} on the CPU: {why}", stage.name());
                            }
                            // The demosaic's working set is made for
                            // its run and is the largest of any op's: a
                            // card too small for it says so with an
                            // error of its own, which is the picture's
                            // and not the device's.
                            e @ greycard_gpu::Error::Gpu { .. }
                                if matches!(stage, Stage::Demosaic(..)) =>
                            {
                                log::warn!("the GPU's demosaic: {e}; the CPU's for this frame");
                                ctx.release();
                            }
                            e if keeps_device(hold, &e) => {
                                log::warn!(
                                    "the GPU's {}: {e}; this frame on the CPU",
                                    stage.name()
                                );
                                ctx.release();
                                plan[i..].iter_mut().for_each(|p| *p = Place::Cpu);
                            }
                            e => {
                                log::warn!(
                                    "the GPU's {}: {e}; the CPU's from now on",
                                    stage.name()
                                );
                                if let Some(ctx) = gpu.take() {
                                    ctx.release();
                                }
                                kept.forget_device();
                                plan[i..].iter_mut().for_each(|p| *p = Place::Cpu);
                            }
                        }
                        places.push(Place::Cpu);
                        let started = Instant::now();
                        cpu_op(stage, input, &mut st, models, &mut walked)?;
                        walked
                            .seconds
                            .push((stage.name(), started.elapsed().as_secs_f64()));
                        i += 1;
                    }
                    Err(greycard_gpu::Error::Unsupported(why)) => {
                        log::info!("the {} on the CPU: {why}", steps_name(steps, i));
                        ctx.release();
                        return Err(Retry::Cpu {
                            from: (i, end),
                            keep_context: true,
                        });
                    }
                    Err(e) if keeps_device(hold, &e) => {
                        log::warn!(
                            "the GPU's {}: {e}; this frame on the CPU",
                            steps_name(steps, i)
                        );
                        ctx.release();
                        return Err(Retry::Cpu {
                            from: (i, plan.len()),
                            keep_context: true,
                        });
                    }
                    Err(e) => {
                        log::warn!(
                            "the GPU's {}: {e}; the CPU's from now on",
                            steps_name(steps, i)
                        );
                        return Err(Retry::Cpu {
                            from: (i, end),
                            keep_context: false,
                        });
                    }
                }
            }
            Step::Op(stage) => {
                places.push(Place::Cpu);
                let started = Instant::now();
                cpu_op(stage, input, &mut st, models, &mut walked)?;
                walked
                    .seconds
                    .push((stage.name(), started.elapsed().as_secs_f64()));
                i += 1;
            }
        }
    }
    walked.unmade = st.retouch.as_ref().map(|a| a.unmade.clone());
    walked.buf = st.buf;
    walked.meta = st.meta;
    walked.taps = st.taps;
    walked.places = places;
    Ok(Out { walked, planes })
}

fn steps_name(steps: &Steps, i: usize) -> &'static str {
    match &steps.steps[i] {
        Step::Op(stage) => stage.name(),
        _ => "step",
    }
}

/// The end of the run on the device that begins at `i`: the steps
/// after it the plan puts there, and the keeps between them.
fn run_end(steps: &Steps, plan: &[Place], i: usize) -> usize {
    let mut end = i + 1;
    // The CA, the demosaic, and the lens run on pictures in memory,
    // one at a time.
    if matches!(
        steps.steps[i],
        Step::Op(Stage::Ca(_) | Stage::Demosaic(..) | Stage::Lens(_))
    ) {
        return end;
    }
    while end < steps.steps.len() {
        match &steps.steps[end] {
            Step::Keep(_) => end += 1,
            Step::Op(Stage::Ca(_) | Stage::Demosaic(..) | Stage::Lens(_) | Stage::Finish(_)) => {
                break;
            }
            Step::Op(_) if plan[end] == Place::Gpu => end += 1,
            _ => break,
        }
    }
    end
}

/// A stage on the CPU.
fn cpu_op(
    stage: &Stage,
    input: &Input,
    st: &mut Carried,
    models: &mut dyn Models,
    walked: &mut Walked,
) -> Result<(), Retry> {
    let take =
        |buf: &mut Buf| std::mem::replace(buf, Buf::Working(Arc::new(WorkingImage::new(0, 0))));
    match stage {
        Stage::Prepare(options) => {
            let Input::Raw(frame) = input else {
                unreachable!("a prepare is a raw's")
            };
            st.settings = options.settings.clone();
            let prepared = if options.clip {
                prepare_with(frame, &options.settings, options.measure_noise, None)
            } else {
                core_develop::prepare_unclipped(frame, &options.settings, options.measure_noise)
            }
            .map_err(failed)?;
            st.meta.white = Some(prepared.white_balance);
            st.meta.radius = prepared.sharpen_radius;
            st.meta.profile = options
                .settings
                .profile
                .as_ref()
                .map(|p| p.camera.clone())
                .or_else(|| greycard_core::color::profile_from_frame(frame).ok())
                .map(Arc::new);
            st.meta.full = (prepared.width as u32, prepared.height as u32);
            st.meta.gave_way = Some(prepared.demosaic)
                .filter(|ran| {
                    *ran != options.settings.demosaic
                        && options.settings.demosaic != DemosaicMethod::default()
                        && prepared.pattern.is_some()
                })
                .map(|ran| {
                    format!(
                        "the demosaic: {} is not one for this raw's mosaic; {} instead",
                        options.settings.demosaic.name(),
                        ran.name()
                    )
                });
            st.buf = Buf::Mosaic(prepared);
        }
        Stage::Ca(options) => {
            let Buf::Mosaic(prepared) = &mut st.buf else {
                unreachable!("the CA is the mosaic's")
            };
            let pattern = prepared.pattern.as_ref().expect("a CA is a mosaic's");
            let (samples, stats) = ca::correct_ca(
                &prepared.samples,
                prepared.width,
                prepared.height,
                pattern,
                options,
            )
            .map_err(failed)?;
            prepared.samples = samples;
            prepared.chromatic_aberration = Some(stats);
            walked.ca = Some(stats);
        }
        Stage::CaFit(options) => {
            let Buf::Mosaic(prepared) = &mut st.buf else {
                unreachable!("the CA is the mosaic's")
            };
            let pattern = prepared.pattern.as_ref().expect("a CA is a mosaic's");
            let (samples, fit) = ca::fit_ca(
                &prepared.samples,
                prepared.width,
                prepared.height,
                pattern,
                options,
            )
            .map_err(failed)?;
            prepared.samples = samples;
            prepared.chromatic_aberration = Some(fit.stats);
            walked.ca = Some(fit.stats);
            walked.ca_fit = Some(Arc::new(fit));
        }
        Stage::CaApply(fit) => {
            let Buf::Mosaic(prepared) = &mut st.buf else {
                unreachable!("the CA is the mosaic's")
            };
            let pattern = prepared.pattern.as_ref().expect("a CA is a mosaic's");
            prepared.samples = ca::apply_ca(
                &prepared.samples,
                prepared.width,
                prepared.height,
                pattern,
                &fit.0,
            )
            .map_err(failed)?;
            prepared.chromatic_aberration = Some(fit.0.stats);
            walked.ca = Some(fit.0.stats);
        }
        Stage::Demosaic(..) => {
            let Buf::Mosaic(prepared) = take(&mut st.buf) else {
                unreachable!("the demosaic is the mosaic's")
            };
            let settings = DevelopSettings {
                denoise: None,
                ..st.settings.clone()
            };
            let (rgb, _, _) = demosaic_prepared(&prepared, &settings).map_err(failed)?;
            st.buf = Buf::Camera { prepared, rgb };
        }
        Stage::Bin(bin) => {
            let Buf::Mosaic(prepared) = take(&mut st.buf) else {
                unreachable!("the bin is the mosaic's")
            };
            let pattern = prepared.pattern.as_ref().expect("a bin is a mosaic's");
            let (rgb, width, height) = core_develop::reduced::bin_mosaic(
                &prepared.samples,
                prepared.width,
                prepared.height,
                pattern,
                *bin,
            );
            st.meta.bin = *bin;
            st.buf = Buf::Camera {
                prepared: Prepared {
                    samples: Vec::new(),
                    width,
                    height,
                    pattern: None,
                    ..prepared
                },
                rgb,
            };
        }
        Stage::Learned(model) => {
            let Input::Raw(frame) = input else {
                unreachable!("the learned denoiser is a raw's")
            };
            let Buf::Mosaic(prepared) = take(&mut st.buf) else {
                unreachable!("the learned denoiser is the mosaic's")
            };
            let network = models
                .denoise(model, frame, &prepared)
                .map_err(Retry::Learned)?;
            // The network is the demosaic: the one the edit chose gave
            // way to nothing.
            st.meta.gave_way = None;
            let (plain, _, _) = demosaic_prepared(&prepared, &st.settings).map_err(failed)?;
            st.buf = Buf::CameraPair {
                prepared,
                network,
                plain,
            };
        }
        Stage::Denoise(options) => {
            let Buf::Camera { prepared, rgb } = &mut st.buf else {
                unreachable!("the denoise is camera RGB's")
            };
            if let Some(model) = prepared.noise_model() {
                denoise::denoise_picture(
                    rgb,
                    prepared.width,
                    prepared.height,
                    &model,
                    options,
                    prepared.monochrome,
                )
                .map_err(failed)?;
            }
        }
        Stage::Warp(options) => {
            let at = WarpPlacement::binned(options.area, st.meta.bin);
            match &mut st.buf {
                Buf::Camera { prepared, rgb } => {
                    apply_warps(rgb, prepared.width, prepared.height, at, &options.warps);
                }
                Buf::CameraPair {
                    prepared,
                    network,
                    plain,
                } => {
                    apply_warps(network, prepared.width, prepared.height, at, &options.warps);
                    apply_warps(plain, prepared.width, prepared.height, at, &options.warps);
                }
                _ => unreachable!("the file's warp is camera RGB's"),
            }
        }
        Stage::IntoWorking { orientation, .. } => {
            let swapped = |o: Orientation| {
                matches!(
                    o,
                    Orientation::Transpose
                        | Orientation::Rotate90
                        | Orientation::Transverse
                        | Orientation::Rotate270
                )
            };
            if swapped(*orientation) {
                st.meta.full = (st.meta.full.1, st.meta.full.0);
            }
            match take(&mut st.buf) {
                Buf::Camera { mut prepared, rgb } => {
                    prepared.orientation = *orientation;
                    let d = core_develop::finish(prepared, rgb, None, None, &st.settings)
                        .map_err(failed)?;
                    st.meta.clip_level = d.clip_level;
                    st.buf = Buf::Working(Arc::new(d.image));
                }
                Buf::CameraPair {
                    mut prepared,
                    network,
                    plain,
                } => {
                    prepared.orientation = *orientation;
                    let p = core_develop::finish(prepared.clone(), plain, None, None, &st.settings)
                        .map_err(failed)?;
                    let m = core_develop::finish(prepared, network, None, None, &st.settings)
                        .map_err(failed)?;
                    st.meta.clip_level = m.clip_level;
                    st.buf = Buf::Pair {
                        model: Arc::new(m.image),
                        plain: Arc::new(p.image),
                    };
                }
                _ => unreachable!("the move into the working space is camera RGB's"),
            }
        }
        Stage::Blend { luma, chroma } => {
            let Buf::Pair { model, plain } = take(&mut st.buf) else {
                unreachable!("the blend is the pair's")
            };
            st.buf = Buf::Working(blend(&model, &plain, *luma, *chroma));
        }
        Stage::White { own, light } => {
            let image = working(&mut st.buf);
            let m = greycard_edit::white::adaptation(*own, *light).ok_or_else(|| {
                failed(format!(
                    "no white balance from {:.0} K {:+.4} to {:.0} K {:+.4}",
                    own.cct, own.duv, light.cct, light.duv
                ))
            })?;
            st.buf = Buf::Working(Arc::new(white_matrix(owned(image), &m)));
            st.meta.picture = Some((*own, *light));
        }
        Stage::Orient(o) => {
            let image = working(&mut st.buf);
            st.buf = Buf::Working(Arc::new(core_develop::orient(owned(image), *o)));
            if swaps(*o) {
                st.meta.full = (st.meta.full.1, st.meta.full.0);
            }
        }
        Stage::Reduce(bin) => {
            let image = working(&mut st.buf);
            st.meta.bin = *bin;
            st.buf = Buf::Working(Arc::new(greycard_render::reduce::reduce(&image, *bin)));
        }
        Stage::Lens(corrections) => match &mut st.buf {
            // In camera space, before the move into the working space:
            // a merge's frame. The corrections are linear in the
            // samples and carry no colorimetry, so they run on camera
            // RGB as they run on the working picture.
            Buf::Camera { prepared, rgb } => {
                lens_in_camera(rgb, prepared.width, prepared.height, corrections);
            }
            Buf::CameraPair {
                prepared,
                network,
                plain,
            } => {
                lens_in_camera(network, prepared.width, prepared.height, corrections);
                lens_in_camera(plain, prepared.width, prepared.height, corrections);
            }
            _ => {
                let mut image = owned(working(&mut st.buf));
                for c in corrections {
                    image = greycard_core::develop::lens::correct(&image, c).0;
                }
                st.buf = Buf::Working(Arc::new(image));
            }
        },
        Stage::Defringe(options) => {
            let mut image = owned(working(&mut st.buf));
            greycard_core::develop::defringe::defringe(&mut image, options);
            st.buf = Buf::Working(Arc::new(image));
        }
        Stage::Retouch(retouch, fills) => {
            let image = working(&mut st.buf);
            let chosen = retouch.choose_sources(&image);
            let applied = retouch.with_sources(&chosen);
            walked.sources = applied.chosen_patches(&chosen);
            let mut picture = (*image).clone();
            let mut unmade = Vec::new();
            models.fills_ahead(&applied, &picture);
            applied.apply_with(&mut picture, |patch, region, before| {
                let made = match fills {
                    Fills::Run => models.fill(patch, region, before),
                    Fills::Read { factor, full } => {
                        let read = models.fill_read(patch, region, before, *factor, *full);
                        if read.is_none() {
                            walked.without.push(format!("the fill {}", patch.name()));
                        }
                        read
                    }
                };
                if made.is_none() {
                    unmade.push(patch.id);
                }
                made
            });
            st.retouch = Some(Applied {
                retouch: applied,
                unmade,
            });
            st.buf = Buf::Working(Arc::new(picture));
        }
        Stage::LocalContrast(options) => {
            let mut image = owned(working(&mut st.buf));
            walked.detail = Some(local_contrast(&mut image, options, st.meta.clip_level));
            st.buf = Buf::Working(Arc::new(image));
        }
        Stage::Dehaze(options) => {
            let mut image = owned(working(&mut st.buf));
            // The sharpen's guide is dehazed as the picture is, so the
            // dehaze moves what the sharpen calls flat as it did before
            // the guide.
            walked.dehaze = Some(match st.taps.sharpen_guide.take() {
                Some(SharpenGuide::Working(guide)) => {
                    let mut guide = owned(guide);
                    let stats = dehaze::dehaze_also(&mut image, &mut guide, options);
                    st.taps.sharpen_guide = Some(SharpenGuide::Working(Arc::new(guide)));
                    stats
                }
                guide => {
                    // One on the device cannot be dehazed here; the
                    // sharpen then reads its input ([`SharpenGuide::working`]).
                    st.taps.sharpen_guide = guide;
                    dehaze::dehaze(&mut image, options)
                }
            });
            st.buf = Buf::Working(Arc::new(image));
        }
        Stage::Sharpen(options, Delta::Only) => {
            // The picture as it was; the delta of what the sharpen
            // would have done, for the masks' Sharpen.
            let Buf::Working(image) = &st.buf else {
                unreachable!("a stage on the working picture")
            };
            let guide = SharpenGuide::working(&st.taps.sharpen_guide);
            let (_, _, delta) =
                sharpen_delta(image, guide, options, st.meta.radius, st.meta.clip_level);
            st.taps.sharpen = Some(SharpenTap::of(image, options, false, &delta));
            st.taps.sharpen_guide = None;
        }
        Stage::Sharpen(options, delta) => {
            let mut image = owned(working(&mut st.buf));
            let guide = SharpenGuide::working(&st.taps.sharpen_guide);
            let (radius, clip) = (st.meta.radius, st.meta.clip_level);
            let (stats, mask) = if delta.made() {
                let (stats, mask, delta) =
                    sharpen_with_delta(&mut image, guide, options, radius, clip);
                st.taps.sharpen = Some(SharpenTap::of(&image, options, true, &delta));
                (stats, mask)
            } else {
                sharpen_with_mask(&mut image, guide, options, radius, clip)
            };
            st.taps.sharpen_guide = None;
            walked.sharpen = Some(stats);
            walked.sharpen_mask = Some(mask);
            st.buf = Buf::Working(Arc::new(image));
        }
        Stage::Finish(_) => unreachable!("the finish is the caller's"),
    }
    Ok(())
}

/// `corrections` made of camera RGB `rgb`, `width` by `height`, in
/// place.
fn lens_in_camera(
    rgb: &mut Vec<f32>,
    width: usize,
    height: usize,
    corrections: &[greycard_core::develop::lens::LensCorrection],
) {
    for c in corrections {
        let image = WorkingImage {
            width,
            height,
            data: std::mem::take(rgb),
        };
        *rgb = greycard_core::develop::lens::correct(&image, c).0.data;
    }
}

/// A run of stages on the device, `i..end` of `steps`: the CA and the
/// lens one at a time on pictures in memory, the Detail section and the
/// sharpen as one run with one upload at its head and the device's
/// picture between, a run ending at the shader writing its texture and
/// one ending at the CPU's finish read back in full floats.
fn device_run(
    steps: &Steps,
    std::ops::Range { start: i, end }: std::ops::Range<usize>,
    ctx: &greycard_gpu::Context,
    st: &mut Carried,
    kept: &mut Kept,
    places: &mut Vec<Place>,
    walked: &mut Walked,
) -> greycard_gpu::Result<()> {
    match &steps.steps[i] {
        Step::Op(Stage::Ca(options)) => {
            let Buf::Mosaic(prepared) = &mut st.buf else {
                unreachable!("the CA is the mosaic's")
            };
            let started = Instant::now();
            let pattern = prepared.pattern.clone().expect("a CA is a mosaic's");
            let (samples, stats) = ctx.correct_ca(
                &prepared.samples,
                prepared.width,
                prepared.height,
                &pattern,
                options,
            )?;
            prepared.samples = samples;
            prepared.chromatic_aberration = Some(stats);
            walked.ca = Some(stats);
            walked.seconds.push(("CA", started.elapsed().as_secs_f64()));
            places.push(Place::Gpu);
            return Ok(());
        }
        Step::Op(Stage::Demosaic(method, _)) => {
            let Buf::Mosaic(prepared) = &st.buf else {
                unreachable!("the demosaic is the mosaic's")
            };
            let started = Instant::now();
            let Some(pattern) = prepared.pattern.as_ref() else {
                return Err(greycard_gpu::Error::Unsupported(
                    "a file with no mosaic".into(),
                ));
            };
            let (samples, w, h) = (&prepared.samples, prepared.width, prepared.height);
            // The dual's threshold as the reference's demosaic takes it:
            // the measured noise's, through the gains.
            let contrast = || {
                core_develop::dual_contrast(&st.settings, prepared.noise.as_ref(), prepared.gains)
            };
            let rgb = match method {
                DemosaicMethod::Rcd => ctx.demosaic_rcd(samples, w, h, pattern)?,
                DemosaicMethod::Vng4 => ctx.demosaic_vng4(samples, w, h, pattern)?,
                DemosaicMethod::Amaze => ctx.demosaic_amaze(samples, w, h, pattern)?,
                DemosaicMethod::RcdVng4 => {
                    ctx.demosaic_rcd_dual(samples, w, h, pattern, contrast())?
                }
                DemosaicMethod::AmazeVng4 => {
                    ctx.demosaic_amaze_dual(samples, w, h, pattern, contrast())?
                }
                m => {
                    return Err(greycard_gpu::Error::Unsupported(format!(
                        "{} has no device version",
                        m.name()
                    )));
                }
            };
            let Buf::Mosaic(prepared) =
                std::mem::replace(&mut st.buf, Buf::Working(Arc::new(WorkingImage::new(0, 0))))
            else {
                unreachable!("checked above")
            };
            st.buf = Buf::Camera { prepared, rgb };
            walked
                .seconds
                .push(("demosaic on the GPU", started.elapsed().as_secs_f64()));
            places.push(Place::Gpu);
            return Ok(());
        }
        Step::Op(Stage::Lens(corrections)) => {
            let started = Instant::now();
            let Buf::Working(input) = &st.buf else {
                unreachable!("the lens is the working picture's")
            };
            let mut image: Option<WorkingImage> = None;
            for c in corrections {
                let from = image.as_ref().unwrap_or(input);
                // A correction that moves nothing is a multiply a
                // pixel, quicker on the CPU than the trip to the device.
                image = Some(if c.moves() {
                    ctx.correct_lens(from, c)?.0
                } else {
                    greycard_core::develop::lens::correct(from, c).0
                });
            }
            if let Some(image) = image {
                st.buf = Buf::Working(Arc::new(image));
            }
            walked
                .seconds
                .push(("lens", started.elapsed().as_secs_f64()));
            places.push(Place::Gpu);
            return Ok(());
        }
        _ => {}
    }
    let shader = matches!(
        steps.steps.get(end),
        Some(Step::Op(Stage::Finish(crate::stages::Finish::Shader)))
    );
    // An export's run: its picture is read back for the CPU's finish,
    // and nothing of it is kept on the device past the run: no upload,
    // no point, and no working set of a size the context did not hold.
    let download = matches!(
        steps.steps.get(end),
        Some(Step::Op(Stage::Finish(crate::stages::Finish::Cpu { .. })))
    );
    // The last op of the run writes the viewport's texture when the
    // shader is next.
    let last = (i..end)
        .rev()
        .find(|&j| matches!(steps.steps[j], Step::Op(_)))
        .expect("a run holds an op");
    // The picture in memory the run began at, while nothing in the
    // run has changed it: an export's run that only reads it (the
    // sharpen's delta alone) gives it back rather than its copy.
    let mut unchanged = match &st.buf {
        Buf::Working(picture) => Some(picture.clone()),
        _ => None,
    };
    let sets = ctx.kept_sizes();
    // The picture the run began at and its upload, while the sharpen's
    // guide is that picture: the guide on the device without a second
    // upload.
    let mut began = None;
    let mut image = match &st.buf {
        Buf::Device(image) => image.clone(),
        Buf::Working(picture) => match &kept.uploaded {
            Some((from, image)) if Arc::ptr_eq(from, picture) => image.clone(),
            _ => {
                // The dehaze's copy was read off the old upload's run.
                kept.uploaded = None;
                kept.haze = None;
                let started = Instant::now();
                let image = Arc::new(ctx.upload(picture)?);
                walked
                    .seconds
                    .push(("upload", started.elapsed().as_secs_f64()));
                // Held past the first op it would hold a frame's
                // picture on the card for nothing.
                if !download {
                    kept.uploaded = Some((picture.clone(), image.clone()));
                }
                image
            }
        },
        _ => unreachable!("a run on the device begins at a picture"),
    };
    if let (Buf::Working(picture), Some(SharpenGuide::Working(guide))) =
        (&st.buf, &st.taps.sharpen_guide)
        && Arc::ptr_eq(picture, guide)
    {
        began = Some((picture.clone(), image.clone()));
    }
    let texture = shader.then(|| ctx.viewport_texture(image.width(), image.height()));
    // Once the texture is written there is no picture to keep.
    let mut written = None;
    let run_started = Instant::now();
    for j in i..end {
        match &steps.steps[j] {
            Step::Keep(_) if written.is_some() || download => places.push(Place::Cpu),
            Step::Keep(keep) => {
                places.push(Place::Cpu);
                kept.put(Point {
                    keep: *keep,
                    steps: steps.steps[..=j].to_vec(),
                    places: places.clone(),
                    buf: KeptBuf::Device(image.clone()),
                    meta: st.meta.clone(),
                    taps: st.taps.clone(),
                    retouch: st.retouch.clone(),
                    ran: Ran::of(walked),
                });
            }
            Step::Op(Stage::LocalContrast(options)) => {
                match texture.as_ref().filter(|_| j == last) {
                    Some(out) => {
                        walked.detail = Some(ctx.local_contrast_to_viewport(
                            &image,
                            options,
                            st.meta.clip_level,
                            out,
                        )?);
                        written = Some(out.clone());
                    }
                    None => {
                        let (out, stats) =
                            ctx.local_contrast(&image, options, st.meta.clip_level)?;
                        walked.detail = Some(stats);
                        image = Arc::new(out);
                        unchanged = None;
                    }
                }
                places.push(Place::Gpu);
            }
            Step::Op(Stage::Dehaze(options)) => {
                let to = steps.steps[..j].to_vec();
                let reduced = match &kept.haze {
                    Some((s, reduced)) if same_picture(s, &to) => reduced.clone(),
                    _ => {
                        kept.haze = None;
                        let reduced = Arc::new(ctx.dehaze_reduced(&image)?);
                        kept.haze = Some((to, reduced.clone()));
                        reduced
                    }
                };
                let Some(model) = reduced.model(options) else {
                    return Err(greycard_gpu::Error::Unsupported(format!(
                        "a strength of zero at {}",
                        options.amount
                    )));
                };
                // The sharpen's guide dehazed as the picture is, by the
                // picture's model.
                if let Some(guide) = st.taps.sharpen_guide.take() {
                    let guide = guide_on_device(&guide, ctx, &began, kept)?;
                    let (dehazed, _) = ctx.dehaze(&guide, &model)?;
                    st.taps.sharpen_guide = Some(SharpenGuide::Device(Arc::new(dehazed)));
                    began = None;
                }
                match texture.as_ref().filter(|_| j == last) {
                    Some(out) => {
                        walked.dehaze = Some(ctx.dehaze_to_viewport(&image, &model, out)?);
                        written = Some(out.clone());
                    }
                    None => {
                        let (out, stats) = ctx.dehaze(&image, &model)?;
                        walked.dehaze = Some(stats);
                        image = Arc::new(out);
                        unchanged = None;
                    }
                }
                places.push(Place::Gpu);
            }
            Step::Op(Stage::Sharpen(options, delta)) if texture.is_none() && download => {
                let (radius, clip) = (st.meta.radius, st.meta.clip_level);
                let guide = match st.taps.sharpen_guide.take() {
                    Some(g) => Some(guide_on_device(&g, ctx, &began, kept)?),
                    None => None,
                };
                began = None;
                let (out, stats, d) = ctx.sharpen_to_image(
                    &image,
                    guide.as_deref(),
                    options,
                    radius,
                    clip,
                    delta.sharpens(),
                    delta.made(),
                )?;
                if let Some(d) = d {
                    st.taps.sharpen = Some(SharpenTap {
                        width: image.width() as usize,
                        height: image.height() as usize,
                        options: *options,
                        sharpened: delta.sharpens(),
                        delta: greycard_render::detail::sharpen_delta(&d),
                    });
                }
                if let Some(out) = out {
                    walked.sharpen = Some(stats);
                    image = Arc::new(out);
                    unchanged = None;
                }
                places.push(Place::Gpu);
            }
            Step::Op(Stage::Sharpen(options, delta)) => {
                let out = match &texture {
                    Some(out) => out.clone(),
                    None => ctx.viewport_texture(image.width(), image.height()),
                };
                let (radius, clip) = (st.meta.radius, st.meta.clip_level);
                let guide = match st.taps.sharpen_guide.take() {
                    Some(g) => Some(guide_on_device(&g, ctx, &began, kept)?),
                    None => None,
                };
                began = None;
                if delta.made() {
                    let (stats, d) = ctx.sharpen_with_delta(
                        &image,
                        guide.as_deref(),
                        options,
                        radius,
                        clip,
                        &out,
                        delta.sharpens(),
                    )?;
                    let (w, h) = (image.width() as usize, image.height() as usize);
                    st.taps.sharpen = Some(SharpenTap {
                        width: w,
                        height: h,
                        options: *options,
                        sharpened: delta.sharpens(),
                        delta: greycard_render::detail::sharpen_delta(&d),
                    });
                    if delta.sharpens() {
                        walked.sharpen = Some(stats);
                    }
                } else {
                    walked.sharpen =
                        Some(ctx.sharpen(&image, guide.as_deref(), options, radius, clip, &out)?);
                }
                places.push(Place::Gpu);
                written = Some(out);
            }
            Step::Op(stage) => unreachable!("no device version of the {}", stage.name()),
            Step::Tap(_) => unreachable!("a run holds no tap"),
        }
    }
    // Timed whole: an op returns once its work is queued, and the next
    // one's read back waits for it. The viewport's run is timed by its
    // frame.
    if download {
        walked.seconds.push((
            "the local contrast, dehaze, and sharpen on the GPU",
            run_started.elapsed().as_secs_f64(),
        ));
        // A set made here for a picture of another size than the one
        // the context held (a frame not open, or a set's) goes with
        // the run, rather than stay on the card where the viewport's
        // was until its next develop. One of the size held stays: the
        // open frame's export shares it with the viewport.
        let size = Some((image.width(), image.height()));
        let ran = |f: &dyn Fn(&Stage) -> bool| {
            steps.steps[i..end]
                .iter()
                .any(|s| matches!(s, Step::Op(op) if f(op)))
        };
        if ran(&|s| matches!(s, Stage::Sharpen(..))) && sets.sharpen != size {
            ctx.release_sharpen();
        }
        if ran(&|s| matches!(s, Stage::LocalContrast(_))) && sets.local_contrast != size {
            ctx.release_local_contrast();
        }
    }
    st.buf = match (written, texture) {
        (Some(out), _) => Buf::Texture(out),
        // Neither the Detail section nor the sharpen wrote it: the
        // upload goes to the viewport as it is.
        (None, Some(_)) => {
            return Err(greycard_gpu::Error::Unsupported(
                "nothing on the device to write the viewport's texture".into(),
            ));
        }
        (None, None) if download && unchanged.is_some() => {
            Buf::Working(unchanged.expect("checked"))
        }
        (None, None) if download => {
            let started = Instant::now();
            let picture = ctx.download(&image)?;
            walked
                .seconds
                .push(("download", started.elapsed().as_secs_f64()));
            Buf::Working(Arc::new(picture))
        }
        (None, None) => Buf::Device(image),
    };
    Ok(())
}

/// The sharpen's guide on the device: the device's own; the run's
/// upload, or the one kept, when the guide is the picture it was made
/// from; or uploaded now.
fn guide_on_device(
    guide: &SharpenGuide,
    ctx: &greycard_gpu::Context,
    began: &Option<(Arc<WorkingImage>, Arc<greycard_gpu::Image>)>,
    kept: &Kept,
) -> greycard_gpu::Result<Arc<greycard_gpu::Image>> {
    let picture = match guide {
        SharpenGuide::Device(image) => return Ok(image.clone()),
        SharpenGuide::Working(picture) => picture,
    };
    let held = [began.as_ref(), kept.uploaded.as_ref()];
    match held
        .into_iter()
        .flatten()
        .find(|(from, _)| Arc::ptr_eq(from, picture))
    {
        Some((_, image)) => Ok(image.clone()),
        None => Ok(Arc::new(ctx.upload(picture)?)),
    }
}

/// The working picture out of `buf`.
fn working(buf: &mut Buf) -> Arc<WorkingImage> {
    match std::mem::replace(buf, Buf::Working(Arc::new(WorkingImage::new(0, 0)))) {
        Buf::Working(image) => image,
        _ => unreachable!("a stage on the working picture"),
    }
}

/// The picture to change in place: its own when nothing else holds
/// it, a copy otherwise.
fn owned(image: Arc<WorkingImage>) -> WorkingImage {
    Arc::try_unwrap(image).unwrap_or_else(|a| (*a).clone())
}

/// A picture's white balance: the matrix `m` (rows) on every pixel of
/// the linear picture. The CPU reference, and the only version: the
/// stage runs where the picture's decode to linear does.
pub fn white_matrix(mut image: WorkingImage, m: &greycard_core::color::Matrix3) -> WorkingImage {
    use rayon::prelude::*;
    image.data.par_chunks_mut(3).for_each(|px| {
        let out = greycard_core::color::apply3(m, [px[0], px[1], px[2]]);
        px.copy_from_slice(&out);
    });
    image
}

fn swaps(o: Orientation) -> bool {
    matches!(
        o,
        Orientation::Transpose
            | Orientation::Rotate90
            | Orientation::Transverse
            | Orientation::Rotate270
    )
}

/// Rec.2020's luminance weights, the working space's own: the
/// luminance a pixel's change has in that space.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// `plain` taken toward `model`, the luminance `luma` of the way and
/// the chroma `chroma` of the way: the two are one picture in one
/// space, so the mix is that much of the denoise.
///
/// A pixel's change `d = model - plain` is split into its luminance,
/// `Y = LUMA . d` laid on all three channels (a grey change, whose
/// luminance is `Y` again since the weights sum to 1), and the rest,
/// `d - Y`, which has no luminance and is the change's color. The
/// working space is linear, so this split is exact and the two parts
/// add back to `d`; it costs a dot product a pixel. The luminance
/// is the working space's, not a perceptual one: what is kept at
/// `chroma` 0 is the plain's color noise, measured where it is made.
///
/// With the two equal it is the per-sample mix it always was, bit for
/// bit, and at the ends no copy is made.
pub fn blend(
    model: &Arc<WorkingImage>,
    plain: &Arc<WorkingImage>,
    luma: f32,
    chroma: f32,
) -> Arc<WorkingImage> {
    let s = luma.clamp(0.0, 1.0);
    let c = chroma.clamp(0.0, 1.0);
    if s == c {
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
        return Arc::new(out);
    }
    // plain + c d + (s - c) Y: the chroma's share of the whole change,
    // and the luminance's difference on top.
    let k = s - c;
    let mut out = (**plain).clone();
    for (o, m) in out
        .data
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(model.data.as_chunks::<3>().0)
    {
        let d = [m[0] - o[0], m[1] - o[1], m[2] - o[2]];
        let y = k * (LUMA[0] * d[0] + LUMA[1] * d[1] + LUMA[2] * d[2]);
        for (o, d) in o.iter_mut().zip(d) {
            *o += c * d + y;
        }
    }
    Arc::new(out)
}

/// The masks' bands being made on a thread of their own beside the
/// walk: on the device's path the CPU is otherwise idle.
struct PlanesJob {
    made: std::sync::mpsc::Receiver<Option<DetailPlanes>>,
    cancel: Arc<AtomicBool>,
    steps: Vec<Step>,
    wants: DetailWants,
    haze_after: Option<greycard_core::develop::local_contrast::LocalContrastOptions>,
    started: Instant,
}

impl Drop for PlanesJob {
    /// A walk that ends without taking the planes up stops them.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl PlanesJob {
    /// A thread and not the pool's, whose panic would end the process;
    /// a panic here drops the sender, and the walk goes on without the
    /// planes.
    fn start(
        patched: Arc<WorkingImage>,
        steps: Vec<Step>,
        haze_after: Option<greycard_core::develop::local_contrast::LocalContrastOptions>,
        clip_level: f32,
        wants: DetailWants,
        reuse: Option<Arc<DetailPlanes>>,
    ) -> Option<Self> {
        let (send, made) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("detail planes".into())
            .spawn(move || {
                let planes = DetailPlanes::make_with(
                    &patched,
                    haze_after,
                    clip_level,
                    wants,
                    reuse.as_deref(),
                    &stop,
                );
                if !stop.load(Ordering::Relaxed) {
                    let _ = send.send(planes);
                }
            });
        match spawned {
            Ok(_) => Some(Self {
                made,
                cancel,
                steps,
                wants,
                haze_after,
                started: Instant::now(),
            }),
            Err(e) => {
                log::warn!("the masks' detail bands: no thread to make them on: {e}");
                None
            }
        }
    }

    /// The planes, waited for, and kept for the next walk.
    fn land(self, kept: &mut Kept) -> Option<Arc<DetailPlanes>> {
        let waited = Instant::now();
        let planes = match self.made.recv() {
            Ok(planes) => planes.map(Arc::new),
            Err(_) => {
                log::warn!(
                    "the masks' detail bands: the thread making them died; developing without them"
                );
                None
            }
        };
        log::info!(
            "the masks' detail bands in {:.3} s, {:.3} s of it waited for",
            self.started.elapsed().as_secs_f64(),
            waited.elapsed().as_secs_f64()
        );
        kept.planes = planes.clone().map(|planes| KeptPlanes {
            steps: self.steps.clone(),
            wants: self.wants,
            haze_after: self.haze_after,
            planes,
        });
        planes
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use greycard_core::develop::defringe::defringe;
    use greycard_core::raw::{
        Calibration, CfaColor, CfaPattern, LevelPattern, Levels, Samples, SensorLayout,
    };
    use greycard_edit::Edit;

    use super::*;
    use crate::stages::{self, Purpose};

    /// A small Bayer raw with edges and texture for every stage to
    /// bite on: a ramp, a bright disk, and a fine checker.
    fn frame() -> Arc<RawFrame> {
        frame_of(160, 120, [1.0, 1.0])
    }

    /// A Bayer raw `w` by `h`, its red and blue drawn from the scene at
    /// `scale` times their radius from the center: a lateral chromatic
    /// aberration for the CA to find.
    fn frame_of(w: usize, h: usize, scale: [f32; 2]) -> Arc<RawFrame> {
        let pattern = CfaPattern::rggb();
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let scene = |x: f32, y: f32| {
            let ramp = 200.0 + 2400.0 * x / w as f32;
            let (dx, dy) = (x - w as f32 * 0.6, y - h as f32 * 0.4);
            let r = (w.min(h) as f32 / 6.0).powi(2);
            let disk = if dx * dx + dy * dy < r { 1200.0 } else { 0.0 };
            let checker = if ((x / 3.0).floor() + (y / 3.0).floor()) as i64 % 2 == 0 {
                150.0
            } else {
                0.0
            };
            ramp + disk + checker
        };
        let mut samples = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let (gain, k) = match pattern.color_at(y, x) {
                    CfaColor::Red => (0.55, scale[0]),
                    CfaColor::Green => (1.0, 1.0),
                    CfaColor::Blue => (0.4, scale[1]),
                    CfaColor::Other(_) => unreachable!(),
                };
                let (fx, fy) = (cx + (x as f32 - cx) * k, cy + (y as f32 - cy) * k);
                samples.push((scene(fx, fy) * gain).min(4000.0) as u16);
            }
        }
        Arc::new(RawFrame {
            make: "Test".into(),
            model: "Cam".into(),
            other_names: Vec::new(),
            width: w,
            height: h,
            channels: 1,
            layout: SensorLayout::Cfa(pattern),
            samples: Samples::U16(samples),
            levels: Levels {
                black: LevelPattern::uniform(64.0, 1),
                white: LevelPattern::uniform(4095.0, 1),
                black_deltas: None,
            },
            as_shot_coefficients: Some([2.0, 1.0, 1.6]),
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix: vec![
                    0.9766, -0.2953, -0.1254, -0.4276, 1.2116, 0.2433, -0.0437, 0.1336, 0.5131,
                ],
                forward_matrix: None,
            }],
            crop: None,
            orientation: Orientation::Normal,
            shot: Default::default(),
            dng: Default::default(),
        })
    }

    /// An edit with every CPU stage after the base on.
    fn edit() -> Edit {
        let mut edit = Edit::default();
        edit.noise.profiled = true;
        edit.lens.defringe = true;
        edit.detail.texture = 0.3;
        edit.detail.clarity = 0.4;
        edit.detail.dehaze = 0.25;
        edit
    }

    fn walk_cpu(
        edit: &Edit,
        purpose: &Purpose,
        input: &Input,
        turn: u8,
        kept: &mut Kept,
    ) -> Walked {
        let steps = stages::stages(edit, purpose, &input.facts(turn), None);
        let plan = plan(&steps, purpose, false);
        walk(
            &steps,
            &plan,
            input,
            kept,
            &mut NoModels,
            &mut None,
            &NoHold,
        )
        .expect("walks")
    }

    /// A device of the tests' own, none where there is no adapter;
    /// under CI (GREYCARD_REQUIRE_GPU) a missing one fails the test
    /// rather than pass it.
    fn device() -> Option<Option<greycard_gpu::Context>> {
        match greycard_gpu::Context::own() {
            Ok(ctx) => Some(Some(ctx)),
            Err(e) => {
                assert!(
                    std::env::var_os("GREYCARD_REQUIRE_GPU").is_none_or(|v| v.is_empty()),
                    "no device for the walk ({e}) and GREYCARD_REQUIRE_GPU is set"
                );
                eprintln!("SKIPPED: no device for the walk ({e})");
                None
            }
        }
    }

    fn picture(w: &Walked) -> Arc<WorkingImage> {
        match &w.buf {
            Buf::Working(image) => image.clone(),
            _ => panic!("a CPU walk ends at a picture in memory"),
        }
    }

    /// The walk on the CPU is the stages run by hand in their order:
    /// the engine's develop with its CA inside the prepare and its
    /// denoise inside the demosaic, then the defringe, the local
    /// contrast, the dehaze, and the sharpen, to the bit.
    #[test]
    fn a_walk_on_the_cpu_is_the_stages_run_by_hand() {
        // Past the CA's smallest side, with an aberration for it to
        // correct, so its half of the split is held too.
        let raw = frame_of(640, 480, [1.004, 0.996]);
        let edit = edit();
        let walked = walk_cpu(
            &edit,
            &Purpose::Viewport,
            &Input::Raw(raw.clone()),
            0,
            &mut Kept::new(),
        );

        let settings = DevelopSettings {
            orientation: Some(raw.orientation),
            ..edit.settings(&greycard_edit_io::Stores::user())
        };
        let prepared = prepare_with(&raw, &settings, false, None).unwrap();
        let radius = prepared.sharpen_radius;
        let (rgb, dual, denoised) = demosaic_prepared(&prepared, &settings).unwrap();
        assert!(denoised.is_some(), "the profiled denoise ran");
        let d = core_develop::finish(prepared, rgb, dual, denoised, &settings).unwrap();
        let mut want = d.image;
        defringe(&mut want, &edit.lens.defringe().unwrap());
        // The sharpen's guide: the picture before the local contrast,
        // dehazed with the picture.
        let mut guide = want.clone();
        local_contrast(&mut want, &edit.detail.options().unwrap(), d.clip_level);
        dehaze::dehaze_also(
            &mut want,
            &mut guide,
            &edit.detail.dehaze_options().unwrap(),
        );
        let (_, mask) = sharpen_with_mask(
            &mut want,
            Some(&guide),
            &edit.sharpen.options().unwrap(),
            radius,
            d.clip_level,
        );

        let got = picture(&walked);
        assert_eq!((got.width, got.height), (want.width, want.height));
        assert!(got.data == want.data, "the walk is the stages by hand");
        assert_eq!(walked.sharpen_mask.as_deref(), Some(&mask[..]));
        assert_eq!(walked.meta.clip_level, d.clip_level);
        assert!(walked.taps.guide.is_some());
        assert!(walked.ca.is_some_and(|c| c.corrected), "the CA corrected");
        assert_eq!(walked.from, None);
        assert!(walked.places.iter().all(|p| *p == Place::Cpu));
    }

    /// With Texture, Clarity, and Structure at zero there is no guide:
    /// the dehazed picture's own sharpen, as before the guide, to the
    /// bit. And a sharpen moved after a walk with the guide reads the
    /// guide kept with the picture before the sharpen, so the picture
    /// it makes is a walk from nothing's.
    #[test]
    fn detail_at_zero_sharpens_as_before_and_a_kept_guide_serves() {
        let raw = frame_of(640, 480, [1.0, 1.0]);
        let mut edit = edit();
        edit.detail.texture = 0.0;
        edit.detail.clarity = 0.0;
        let input = Input::Raw(raw.clone());
        let walked = walk_cpu(&edit, &Purpose::Viewport, &input, 0, &mut Kept::new());
        let settings = DevelopSettings {
            orientation: Some(raw.orientation),
            ..edit.settings(&greycard_edit_io::Stores::user())
        };
        let prepared = prepare_with(&raw, &settings, false, None).unwrap();
        let radius = prepared.sharpen_radius;
        let (rgb, dual, denoised) = demosaic_prepared(&prepared, &settings).unwrap();
        let d = core_develop::finish(prepared, rgb, dual, denoised, &settings).unwrap();
        let mut want = d.image;
        defringe(&mut want, &edit.lens.defringe().unwrap());
        dehaze::dehaze(&mut want, &edit.detail.dehaze_options().unwrap());
        let (_, mask) = sharpen_with_mask(
            &mut want,
            None,
            &edit.sharpen.options().unwrap(),
            radius,
            d.clip_level,
        );
        assert!(picture(&walked).data == want.data, "the unguided sharpen");
        assert_eq!(walked.sharpen_mask.as_deref(), Some(&mask[..]));

        // Texture on, then the sharpen moved: from the kept point.
        edit.detail.texture = 0.8;
        let mut kept = Kept::new();
        walk_cpu(&edit, &Purpose::Viewport, &input, 0, &mut kept);
        edit.sharpen.iterations = 7;
        let resumed = walk_cpu(&edit, &Purpose::Viewport, &input, 0, &mut kept);
        assert_eq!(resumed.from, Some(Keep::PreSharpen));
        let fresh = walk_cpu(&edit, &Purpose::Viewport, &input, 0, &mut Kept::new());
        assert!(picture(&resumed).data == picture(&fresh).data);
        assert_eq!(resumed.sharpen, fresh.sharpen);
        assert!(resumed.taps.sharpen_guide.is_none(), "read and let go");
    }

    /// The dehazed guide a point holds is a picture of its own, and
    /// the kept bytes count it, once.
    #[test]
    fn the_kept_bytes_count_a_dehazed_guide() {
        let mut kept = Kept::new();
        walk_cpu(
            &edit(),
            &Purpose::Viewport,
            &Input::Raw(frame()),
            0,
            &mut kept,
        );
        let point = kept
            .points
            .iter()
            .find(|p| p.keep == Keep::PreSharpen)
            .expect("the point before the sharpen");
        let Some(SharpenGuide::Working(guide)) = &point.taps.sharpen_guide else {
            panic!("the guide is kept with the point");
        };
        let size = |i: &WorkingImage| (i.data.len() * std::mem::size_of::<f32>()) as u64;
        let mut pictures: Vec<&Arc<WorkingImage>> = Vec::new();
        for p in &kept.points {
            if let KeptBuf::Working(image) = &p.buf
                && !pictures.iter().any(|q| Arc::ptr_eq(q, image))
            {
                pictures.push(image);
            }
        }
        assert!(
            !pictures.iter().any(|q| Arc::ptr_eq(q, guide)),
            "dehazed, the guide is no point's picture"
        );
        let held: u64 = pictures.iter().map(|p| size(p)).sum();
        let masks = kept.mask_picture().map_or(0, |p| p.bytes());
        assert_eq!(kept.bytes(), held + size(guide) + masks);
    }

    /// A walk after another starts from the last point they share: a
    /// sharpen moved starts at the picture before the sharpen, a
    /// Detail move (Structure's as Clarity's) at the patched picture,
    /// a white balance at the decode; and what each makes is what a
    /// walk from nothing makes.
    #[test]
    fn a_kept_prefix_is_reused_and_a_changed_stage_past_it_is_not() {
        let input = Input::Raw(frame());
        let mut kept = Kept::new();
        let first = walk_cpu(&edit(), &Purpose::Viewport, &input, 0, &mut kept);
        assert_eq!(first.from, None);

        let again = walk_cpu(&edit(), &Purpose::Viewport, &input, 0, &mut kept);
        assert_eq!(again.from, Some(Keep::Developed));
        assert!(Arc::ptr_eq(&picture(&again), &picture(&first)));

        // An edit's change, and the point the walk after it starts at.
        type Case<'a> = (&'a dyn Fn(&mut Edit), Option<Keep>);
        let cases: [Case; 4] = [
            (&|e| e.sharpen.iterations = 5, Some(Keep::PreSharpen)),
            (&|e| e.detail.clarity = -0.2, Some(Keep::Patched)),
            (&|e| e.detail.structure = 0.6, Some(Keep::Patched)),
            (
                &|e| {
                    e.white_balance = greycard_edit::WhiteBalance::Custom {
                        temperature: 4300.0,
                        tint: 0.0,
                    }
                },
                None,
            ),
        ];
        for (change, from) in cases {
            let mut e = edit();
            change(&mut e);
            let walked = walk_cpu(&e, &Purpose::Viewport, &input, 0, &mut kept);
            assert_eq!(walked.from, from);
            let fresh = walk_cpu(&e, &Purpose::Viewport, &input, 0, &mut Kept::new());
            assert!(picture(&walked).data == picture(&fresh).data);
            assert!(picture(&walked).data != picture(&first).data);
        }

        // An export of the same edit takes the developed picture: the
        // list is the viewport's up to the finish, and all of it ran on
        // the CPU.
        let mut kept = Kept::new();
        let viewport = walk_cpu(&edit(), &Purpose::Viewport, &input, 0, &mut kept);
        let export = walk_cpu(
            &edit(),
            &Purpose::Export(Box::default()),
            &input,
            0,
            &mut kept,
        );
        assert_eq!(export.from, Some(Keep::Developed));
        assert!(Arc::ptr_eq(&picture(&export), &picture(&viewport)));
    }

    /// Two frames of a burst under one synced edit have the same steps,
    /// and what one kept never serves the other: the walk of B from A's
    /// points is a walk of B from nothing, to the bit, and lets A's go.
    /// The same frame read again resumes only once adopted.
    #[test]
    fn a_kept_point_never_serves_another_frame_of_the_same_steps() {
        let a = Input::Raw(frame_of(160, 120, [1.0, 1.0]));
        let b = Input::Raw(frame_of(160, 120, [1.004, 0.996]));
        let e = edit();
        let steps = |input: &Input| stages::stages(&e, &Purpose::Viewport, &input.facts(0), None);
        assert!(same_picture(&steps(&a).steps, &steps(&b).steps));

        let mut kept = Kept::new();
        let first = walk_cpu(&e, &Purpose::Viewport, &a, 0, &mut kept);
        let crossed = walk_cpu(&e, &Purpose::Viewport, &b, 0, &mut kept);
        assert_eq!(crossed.from, None, "A's points are not B's");
        let fresh = walk_cpu(&e, &Purpose::Viewport, &b, 0, &mut Kept::new());
        assert!(picture(&crossed).data == picture(&fresh).data);
        assert!(picture(&crossed).data != picture(&first).data);
        // B's own points now, and B resumes from them.
        let again = walk_cpu(&e, &Purpose::Viewport, &b, 0, &mut kept);
        assert_eq!(again.from, Some(Keep::Developed));

        // A read again is another frame in memory until adopted.
        let Input::Raw(frame) = &a else {
            unreachable!()
        };
        let mut kept = Kept::new();
        walk_cpu(&e, &Purpose::Viewport, &a, 0, &mut kept);
        let reread = Input::Raw(Arc::new((**frame).clone()));
        let mut unadopted = kept.clone();
        let walked = walk_cpu(&e, &Purpose::Viewport, &reread, 0, &mut unadopted);
        assert_eq!(walked.from, None);
        kept.adopt(&reread);
        let walked = walk_cpu(&e, &Purpose::Viewport, &reread, 0, &mut kept);
        assert_eq!(walked.from, Some(Keep::Developed));
        assert!(picture(&walked).data == picture(&first).data);

        // A picture's points are bound to its image in the same way.
        let picture_of = |input: &Input| Input::Picture {
            image: picture(&walk_cpu(
                &e,
                &Purpose::Viewport,
                input,
                0,
                &mut Kept::new(),
            )),
            meta: Arc::new(Picture {
                image: WorkingImage::new(0, 0),
                make: String::new(),
                model: String::new(),
                shot: Default::default(),
                metadata: Default::default(),
                space: "sRGB".into(),
                bits: 16,
                white: greycard_core::illuminant::D65,
            }),
        };
        let (pa, pb) = (picture_of(&a), picture_of(&b));
        let plain = Edit::default();
        let mut kept = Kept::new();
        walk_cpu(&plain, &Purpose::Viewport, &pa, 0, &mut kept);
        let crossed = walk_cpu(&plain, &Purpose::Viewport, &pb, 0, &mut kept);
        assert_eq!(crossed.from, None);
        let fresh = walk_cpu(&plain, &Purpose::Viewport, &pb, 0, &mut Kept::new());
        assert!(picture(&crossed).data == picture(&fresh).data);
    }

    /// [`a_kept_point_never_serves_another_frame_of_the_same_steps`] on
    /// a device: B's walk from A's points, some of them on the device,
    /// is B's walk from nothing there.
    #[test]
    fn a_device_s_kept_point_never_serves_another_frame() {
        let Some(mut gpu) = device() else {
            return;
        };
        let a = Input::Raw(frame_of(160, 120, [1.0, 1.0]));
        let b = Input::Raw(frame_of(160, 120, [1.004, 0.996]));
        let mut e = edit();
        e.lens.defringe = false;
        let purpose = Purpose::Export(Box::default());
        let go = |input: &Input, kept: &mut Kept, gpu: &mut Option<greycard_gpu::Context>| {
            let steps = stages::stages(&e, &purpose, &input.facts(0), None);
            let plan = plan(&steps, &purpose, gpu.is_some());
            walk(&steps, &plan, input, kept, &mut NoModels, gpu, &NoHold).unwrap()
        };
        let mut kept = Kept::new();
        go(&a, &mut kept, &mut gpu);
        assert!(
            go(&a, &mut kept.clone(), &mut gpu).from.is_some(),
            "A resumes"
        );
        let crossed = go(&b, &mut kept, &mut gpu);
        assert_eq!(crossed.from, None);
        let fresh = go(&b, &mut Kept::new(), &mut gpu);
        assert!(picture(&crossed).data == picture(&fresh).data);
    }

    /// A plan all on the CPU never starts from a point the device
    /// touched, however much of its list it shares; a plan that puts
    /// the same step on the device does.
    #[test]
    fn the_all_cpu_plan_never_matches_a_kept_buffer_the_device_touched() {
        let input = Input::Raw(frame());
        let edit = edit();
        let mut kept = Kept::new();
        let first = walk_cpu(&edit, &Purpose::Viewport, &input, 0, &mut kept);
        let steps = stages::stages(&edit, &Purpose::Viewport, &input.facts(0), None);
        let ca = steps
            .steps
            .iter()
            .position(|s| matches!(s, Step::Op(Stage::Ca(_))))
            .unwrap();
        // As the viewport's CA would have left them, on the device.
        for p in &mut kept.points {
            p.places[ca] = Place::Gpu;
        }
        let cpu = plan(&steps, &Purpose::Viewport, false);
        let device = plan(&steps, &Purpose::Viewport, true);
        assert_eq!(device[ca], Place::Gpu);
        assert!(!kept.serves(&steps, &cpu, Keep::Base));
        assert!(kept.serves(&steps, &device, Keep::Base));
        let export = stages::stages(
            &edit,
            &Purpose::Export(Box::default()),
            &input.facts(0),
            None,
        );
        // An export with no device (`--cpu`) is all the CPU's, and takes
        // none of it; one lent a device takes the base.
        let lent = plan(&export, &Purpose::Export(Box::default()), true);
        assert!(kept.serves(&export, &lent, Keep::Base));
        let export_plan = plan(&export, &Purpose::Export(Box::default()), false);
        assert!(export_plan.iter().all(|p| *p == Place::Cpu));
        assert!(!kept.serves(&export, &export_plan, Keep::Base));
        let walked = walk(
            &export,
            &export_plan,
            &input,
            &mut kept,
            &mut NoModels,
            &mut None,
            &NoHold,
        )
        .unwrap();
        assert_eq!(walked.from, None);
        assert!(picture(&walked).data == picture(&first).data);
    }

    /// A turn of the frame with nothing after it that minds which way
    /// up the picture is permutes the kept base rather than develop it
    /// again, and the picture is the one a develop at that turn makes,
    /// to the bit. With a defringe on, it develops again.
    #[test]
    fn a_turn_permutes_the_kept_base() {
        let input = Input::Raw(frame());
        let mut e = edit();
        e.lens.defringe = false;
        let mut kept = Kept::new();
        walk_cpu(&e, &Purpose::Viewport, &input, 0, &mut kept);
        let turned = walk_cpu(&e, &Purpose::Viewport, &input, 1, &mut kept);
        assert_eq!(turned.from, Some(Keep::Base));
        let fresh = walk_cpu(&e, &Purpose::Viewport, &input, 1, &mut Kept::new());
        let (a, b) = (picture(&turned), picture(&fresh));
        assert_eq!((a.width, a.height), (120, 160));
        assert!(a.data == b.data);
        assert_eq!(turned.meta.full, fresh.meta.full);

        let mut kept = Kept::new();
        walk_cpu(&edit(), &Purpose::Viewport, &input, 0, &mut kept);
        let turned = walk_cpu(&edit(), &Purpose::Viewport, &input, 1, &mut kept);
        assert_eq!(turned.from, None);
    }

    /// A DNG's warp is walked on camera RGB as core's develop runs it,
    /// to the bit, and moves the picture; the reduced walk warps its
    /// binned picture as well.
    #[test]
    fn a_files_warp_is_walked_as_cores_develop_runs_it() {
        let mut frame = (*frame_of(96, 64, [1.0, 1.0])).clone();
        let mut lens = greycard_core::opcodes::WarpPlane::identity();
        (lens.radial[0], lens.radial[2]) = (0.97, 0.06);
        frame.dng.warps = vec![greycard_core::opcodes::Warp {
            planes: vec![lens],
            center_x: 0.48,
            center_y: 0.53,
            reciprocal: false,
        }];
        let warped = Arc::new(frame.clone());
        frame.dng.warps.clear();
        let plain = Arc::new(frame);
        let mut edit = Edit::default();
        edit.sharpen.enabled = false;
        let steps = stages::stages(
            &edit,
            &Purpose::Viewport,
            &Input::Raw(warped.clone()).facts(0),
            None,
        );
        assert_eq!(
            steps.ops().map(Stage::name).collect::<Vec<_>>(),
            [
                "prepare",
                "CA",
                "demosaic",
                "the file's warp",
                "move into the working space",
                "finish"
            ]
        );
        let walked = walk_cpu(
            &edit,
            &Purpose::Viewport,
            &Input::Raw(warped.clone()),
            0,
            &mut Kept::new(),
        );
        let Some(Stage::Prepare(prepare)) = steps.ops().next() else {
            unreachable!("a raw's list starts at its prepare")
        };
        let ca = steps.ops().find_map(|s| match s {
            Stage::Ca(o) => Some(*o),
            _ => None,
        });
        // Core's develop runs the CA inside its prepare.
        let settings = DevelopSettings {
            chromatic_aberration: ca,
            ..prepare.settings.clone()
        };
        let want = core_develop::develop(&warped, &settings).unwrap().image;
        let got = picture(&walked);
        assert!(got.data == want.data, "the walk is core's develop");
        let unwarped = walk_cpu(
            &edit,
            &Purpose::Viewport,
            &Input::Raw(plain),
            0,
            &mut Kept::new(),
        );
        assert!(
            picture(&unwarped).data != got.data,
            "the warp moved nothing"
        );

        let reduced = walk_cpu(
            &edit,
            &Purpose::Reduced,
            &Input::Raw(warped),
            0,
            &mut Kept::new(),
        );
        assert!(picture(&reduced).data.iter().all(|v| v.is_finite()));
    }

    /// The give-way is asked before each stage, and a walk told to
    /// stop says before which.
    #[test]
    fn a_walk_gives_way_between_its_stages() {
        struct Before(&'static str, Cell<usize>);
        impl Hold for Before {
            fn give_way(&self, next: &Stage) -> bool {
                self.1.set(self.1.get() + 1);
                next.name() == self.0
            }
        }
        let input = Input::Raw(frame());
        let steps = stages::stages(&edit(), &Purpose::Prefetch, &input.facts(0), None);
        let plan = plan(&steps, &Purpose::Prefetch, false);
        let hold = Before("demosaic", Cell::new(0));
        let mut kept = Kept::new();
        let stopped = walk(
            &steps,
            &plan,
            &input,
            &mut kept,
            &mut NoModels,
            &mut None,
            &hold,
        );
        assert!(matches!(stopped, Err(Stopped::GaveWay("demosaic"))));
        assert_eq!(
            hold.1.get(),
            3,
            "asked before the prepare, the CA, and the demosaic"
        );
        assert!(!kept.holds(Keep::Base));
        let walked = walk(
            &steps,
            &plan,
            &input,
            &mut kept,
            &mut NoModels,
            &mut None,
            &NoHold,
        )
        .unwrap();
        assert_eq!(walked.from, None);
        assert!(kept.holds(Keep::Base));
    }

    /// A learned tier whose model is not to be had stands the engine's
    /// own path in, and says why.
    #[test]
    fn a_missing_learned_model_stands_the_engine_in() {
        let input = Input::Raw(frame());
        let mut learned = edit();
        learned.noise.learned = greycard_edit::Learned::Fast;
        let walked = walk_cpu(&learned, &Purpose::Viewport, &input, 0, &mut Kept::new());
        assert!(matches!(walked.meta.stood_in, Some(NoLearned::Missing(_))));
        let mut plain = learned.clone();
        plain.noise.learned = greycard_edit::Learned::Off;
        plain.noise.profiled = false;
        let engine = walk_cpu(&plain, &Purpose::Viewport, &input, 0, &mut Kept::new());
        assert!(picture(&walked).data == picture(&engine).data);
    }

    /// The reduced develop is the same walk under its purpose: the
    /// mosaic binned, the defringe's radius over the bin, no sharpen.
    #[test]
    fn the_reduced_walk_bins_and_leaves_the_sharpen_out() {
        let raw = frame();
        let input = Input::Raw(raw.clone());
        // Small enough that no bin is taken; a bin of two by hand.
        let walked = walk_cpu(&edit(), &Purpose::Reduced, &input, 0, &mut Kept::new());
        assert_eq!(walked.meta.bin, 1);
        assert!(walked.sharpen.is_none());
        assert!(walked.sharpen_mask.is_none());

        let mut steps = stages::stages(&edit(), &Purpose::Reduced, &input.facts(0), None);
        for s in &mut steps.steps {
            if let Step::Op(stage @ Stage::Demosaic(..)) = s {
                *stage = Stage::Bin(2);
            }
        }
        let plan = plan(&steps, &Purpose::Reduced, false);
        let binned = walk(
            &steps,
            &plan,
            &input,
            &mut Kept::new(),
            &mut NoModels,
            &mut None,
            &NoHold,
        )
        .unwrap();
        let got = picture(&binned);
        assert_eq!((got.width, got.height), (80, 60));
        assert_eq!(binned.meta.full, (160, 120));
        assert_eq!(binned.meta.bin, 2);
    }

    /// On a device, the viewport's walk runs the CA, the Detail
    /// section, and the sharpen there and ends at the viewport's
    /// texture; a sharpen moved starts from the picture before it, kept
    /// on the device; and an export after it with no device (`--cpu`)
    /// takes none of it and makes the reference's picture. Skipped where no
    /// adapter is, unless GREYCARD_REQUIRE_GPU says one must be.
    #[test]
    fn on_a_device_the_viewport_s_run_ends_at_its_texture() {
        let Some(mut gpu) = device() else {
            return;
        };
        let input = Input::Raw(frame());
        let mut e = edit();
        e.lens.defringe = false;
        let mut kept = Kept::new();
        let go = |e: &Edit,
                  purpose: &Purpose,
                  kept: &mut Kept,
                  gpu: &mut Option<greycard_gpu::Context>| {
            let steps = stages::stages(e, purpose, &input.facts(0), None);
            let plan = plan(&steps, purpose, gpu.is_some());
            walk(&steps, &plan, &input, kept, &mut NoModels, gpu, &NoHold).unwrap()
        };
        let first = go(&e, &Purpose::Viewport, &mut kept, &mut gpu);
        assert!(matches!(first.buf, Buf::Texture(_)));
        assert!(first.places.contains(&Place::Gpu));
        assert!(first.sharpen.is_some() && first.sharpen_mask.is_none());

        e.sharpen.iterations = 5;
        let sharper = go(&e, &Purpose::Viewport, &mut kept, &mut gpu);
        assert_eq!(sharper.from, Some(Keep::PreSharpen));
        assert!(matches!(sharper.buf, Buf::Texture(_)));

        let export = go(&e, &Purpose::Export(Box::default()), &mut kept, &mut None);
        assert_eq!(export.from, None);
        assert!(export.places.iter().all(|p| *p == Place::Cpu));
        let reference = go(
            &e,
            &Purpose::Export(Box::default()),
            &mut Kept::new(),
            &mut None,
        );
        assert!(picture(&export).data == picture(&reference).data);
    }

    /// A demosaic the device declines falls back to the CPU's for the
    /// frame, the develop going on and the device kept: a frame too
    /// small for the device's bands, here, as a card too small for its
    /// working set would decline it.
    #[test]
    fn a_demosaic_the_device_declines_is_the_cpu_s() {
        let Some(mut gpu) = device() else {
            return;
        };
        let input = Input::Raw(frame_of(14, 12, [1.0, 1.0]));
        let mut e = Edit::default();
        e.lens.chromatic_aberration = false;
        let steps = stages::stages(&e, &Purpose::Viewport, &input.facts(0), None);
        let i = steps
            .steps
            .iter()
            .position(|s| matches!(s, Step::Op(Stage::Demosaic(..))))
            .unwrap();
        let places = plan(&steps, &Purpose::Viewport, true);
        assert_eq!(places[i], Place::Gpu);
        let mut kept = Kept::new();
        let walked = walk(
            &steps,
            &places,
            &input,
            &mut kept,
            &mut NoModels,
            &mut gpu,
            &NoHold,
        )
        .expect("the develop goes on");
        assert_eq!(walked.places[i], Place::Cpu);
        assert!(gpu.is_some(), "the device is kept");
        let mut reference = Kept::new();
        walk_cpu(&e, &Purpose::Viewport, &input, 0, &mut reference);
        assert!(
            kept.picture(Keep::Base).unwrap().data == reference.picture(Keep::Base).unwrap().data
        );
    }

    /// The viewport, the prefetch, and the export make the same
    /// demosaic on a device: the prefetch's base is the viewport's to
    /// the bit, and the viewport's points serve the prefetch's list and
    /// the export's, whose picture is the one an export from nothing on
    /// the device makes; the export with no device takes none of them.
    #[test]
    fn the_viewport_and_the_prefetch_make_the_same_demosaic_on_a_device() {
        let Some(mut gpu) = device() else {
            return;
        };
        let input = Input::Raw(frame());
        let mut e = edit();
        e.lens.defringe = false;
        let go = |purpose: &Purpose, kept: &mut Kept, gpu: &mut Option<greycard_gpu::Context>| {
            let steps = stages::stages(&e, purpose, &input.facts(0), None);
            let plan = plan(&steps, purpose, gpu.is_some());
            walk(&steps, &plan, &input, kept, &mut NoModels, gpu, &NoHold).unwrap()
        };
        let mut viewport_kept = Kept::new();
        let viewport = go(&Purpose::Viewport, &mut viewport_kept, &mut gpu);
        assert!(
            viewport
                .seconds
                .iter()
                .any(|(name, _)| *name == "demosaic on the GPU"),
            "{:?}",
            viewport.seconds
        );
        assert!(viewport_kept.ran_on_device(Keep::Base, |s| matches!(s, Stage::Demosaic(..))));
        let mut prefetch_kept = Kept::new();
        go(&Purpose::Prefetch, &mut prefetch_kept, &mut gpu);
        let base = |k: &Kept| k.picture(Keep::Base).cloned().unwrap();
        assert!(
            base(&viewport_kept).data == base(&prefetch_kept).data,
            "the prefetch's base is the viewport's to the bit"
        );
        let steps = stages::stages(&e, &Purpose::Prefetch, &input.facts(0), None);
        let prefetch_plan = plan(&steps, &Purpose::Prefetch, true);
        assert!(viewport_kept.serves(&steps, &prefetch_plan, Keep::Base));
        let export = go(
            &Purpose::Export(Box::default()),
            &mut viewport_kept.clone(),
            &mut gpu,
        );
        // The viewport's picture before the sharpen, on the device,
        // serves an export whose sharpen runs there too.
        assert_eq!(
            export.from,
            Some(Keep::PreSharpen),
            "the viewport's points serve"
        );
        let sharpen = |w: &Walked| {
            let steps = stages::stages(&e, &Purpose::Export(Box::default()), &input.facts(0), None);
            let i = steps
                .steps
                .iter()
                .position(|s| matches!(s, Step::Op(Stage::Sharpen(..))))
                .unwrap();
            w.places[i]
        };
        assert_eq!(sharpen(&export), Place::Gpu);
        let fresh = go(&Purpose::Export(Box::default()), &mut Kept::new(), &mut gpu);
        assert_eq!(fresh.from, None);
        assert!(
            picture(&export).data == picture(&fresh).data,
            "the export from the viewport's points is the export from nothing"
        );
        let cpu = go(
            &Purpose::Export(Box::default()),
            &mut viewport_kept.clone(),
            &mut None,
        );
        assert_eq!(cpu.from, None, "with no device the export makes its own");
    }

    /// [`an_export_on_a_device_runs_the_detail_and_the_sharpen_there`]
    /// on every raw in the folder `GREYCARD_SAMPLES` names: the device's
    /// run of the Detail section, the dehaze, and the sharpen against
    /// the CPU's on the same base, so the difference is theirs alone,
    /// and not the demosaic's carried through them.
    #[test]
    #[ignore = "reads the folder named by GREYCARD_SAMPLES"]
    fn the_detail_and_the_sharpen_on_a_device_on_the_samples() {
        let Some(dir) = std::env::var_os("GREYCARD_SAMPLES") else {
            eprintln!("set GREYCARD_SAMPLES to a folder of raws");
            return;
        };
        let Some(mut gpu) = device() else {
            return;
        };
        let mut frames: Vec<_> = std::fs::read_dir(&dir)
            .expect("read GREYCARD_SAMPLES")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .filter(|p| greycard_core::decode::decode_path(p).is_ok())
            .collect();
        frames.sort();
        let mut e = Edit::default();
        e.detail.texture = 0.3;
        e.detail.clarity = 0.4;
        e.detail.structure = 0.2;
        e.detail.dehaze = 0.25;
        e.sharpen.iterations = 30;
        let purpose = Purpose::Export(Box::default());
        for path in &frames {
            let input = Input::Raw(Arc::new(greycard_core::decode::decode_path(path).unwrap()));
            let mut steps = stages::stages(&e, &purpose, &input.facts(0), None);
            steps.steps.retain(|s| !matches!(s, Step::Keep(_)));
            let lent = plan(&steps, &purpose, true);
            let mut cpu_plan = lent.clone();
            for (s, p) in steps.steps.iter().zip(&mut cpu_plan) {
                if matches!(
                    s,
                    Step::Op(Stage::LocalContrast(_) | Stage::Dehaze(_) | Stage::Sharpen(..))
                ) {
                    *p = Place::Cpu;
                }
            }
            let mut go = |plan: &[Place]| {
                let w = walk(
                    &steps,
                    plan,
                    &input,
                    &mut Kept::new(),
                    &mut NoModels,
                    &mut gpu,
                    &NoHold,
                )
                .unwrap();
                picture(&w)
            };
            let (device, cpu) = (go(&lent), go(&cpu_plan));
            let mut worst = 0f32;
            let mut past = 0usize;
            for (a, b) in cpu.data.iter().zip(&device.data) {
                let r = (a - b).abs() / a.abs().max(1e-2);
                worst = worst.max(r);
                past += usize::from(r > 1e-4);
            }
            println!(
                "{}: {worst:.3e} relative at worst, {past} of {} past 1e-4",
                path.file_name().unwrap().to_string_lossy(),
                cpu.data.len()
            );
            assert!(worst <= 1e-4, "{worst:.3e}");
        }
    }

    /// An export lent a device runs the Detail section, the dehaze, and
    /// the sharpen there and reads the picture back in full floats for
    /// the CPU's finish. It keeps nothing of the run on the device: no
    /// upload, no point (its list's keeps included), and no working set
    /// of a size the context did not hold, while one of the size the
    /// viewport left stays. Against the same run on the CPU from the
    /// same base it is within the sharpen's GPU test tolerance.
    #[test]
    fn an_export_on_a_device_runs_the_detail_and_the_sharpen_there() {
        let Some(mut gpu) = device() else {
            return;
        };
        let input = Input::Raw(frame());
        let mut e = edit();
        e.lens.defringe = false;
        e.detail.structure = 0.2;
        let purpose = Purpose::Export(Box::default());
        let steps = stages::stages(&e, &purpose, &input.facts(0), None);
        let at = |f: &dyn Fn(&Stage) -> bool| {
            steps
                .steps
                .iter()
                .position(|s| matches!(s, Step::Op(op) if f(op)))
                .unwrap()
        };
        let ops = [
            at(&|s| matches!(s, Stage::LocalContrast(_))),
            at(&|s| matches!(s, Stage::Dehaze(_))),
            at(&|s| matches!(s, Stage::Sharpen(..))),
        ];
        let lent = plan(&steps, &purpose, true);
        assert!(ops.iter().all(|&i| lent[i] == Place::Gpu));
        let mut kept = Kept::new();
        let walked = walk(
            &steps,
            &lent,
            &input,
            &mut kept,
            &mut NoModels,
            &mut gpu,
            &NoHold,
        )
        .expect("the export walks");
        assert!(gpu.is_some());
        assert!(ops.iter().all(|&i| walked.places[i] == Place::Gpu));
        assert!(walked.sharpen.is_some() && walked.detail.is_some() && walked.dehaze.is_some());
        assert!(
            walked.seconds.iter().any(|(name, _)| *name == "download"),
            "{:?}",
            walked.seconds
        );
        assert!(kept.uploaded.is_none(), "no upload stays");
        assert!(
            !kept
                .points
                .iter()
                .any(|p| matches!(p.buf, KeptBuf::Device(_))),
            "no point on the device"
        );
        let none = greycard_gpu::KeptSizes {
            sharpen: None,
            local_contrast: None,
        };
        assert_eq!(
            gpu.as_ref().unwrap().kept_sizes(),
            none,
            "no set the context did not hold"
        );
        let device = picture(&walked);

        // The viewport's sets at this size stay through an export.
        let viewport = stages::stages(&e, &Purpose::Viewport, &input.facts(0), None);
        let viewport_plan = plan(&viewport, &Purpose::Viewport, true);
        let ctx_keeps = |gpu: &Option<greycard_gpu::Context>| gpu.as_ref().unwrap().kept_sizes();
        gpu.as_ref().unwrap().keep_local_contrast_planes(true);
        walk(
            &viewport,
            &viewport_plan,
            &input,
            &mut Kept::new(),
            &mut NoModels,
            &mut gpu,
            &NoHold,
        )
        .unwrap();
        let held = ctx_keeps(&gpu);
        assert!(held.sharpen.is_some() && held.local_contrast.is_some());
        walk(
            &steps,
            &lent,
            &input,
            &mut Kept::new(),
            &mut NoModels,
            &mut gpu,
            &NoHold,
        )
        .unwrap();
        assert_eq!(ctx_keeps(&gpu), held, "the viewport's sets stay");
        // An export of a frame of another size makes its own sets and
        // lets them go: the viewport's were evicted either way, and its
        // next develop makes them again.
        let other = Input::Raw(frame_of(200, 140, [1.0, 1.0]));
        let other_steps = stages::stages(&e, &purpose, &other.facts(0), None);
        let other_plan = plan(&other_steps, &purpose, true);
        walk(
            &other_steps,
            &other_plan,
            &other,
            &mut Kept::new(),
            &mut NoModels,
            &mut gpu,
            &NoHold,
        )
        .unwrap();
        assert_eq!(ctx_keeps(&gpu), none, "a set of another size goes");
        gpu.as_ref().unwrap().keep_local_contrast_planes(false);

        // The stages before the run as the device ran them, the run on
        // the CPU: the ops' rounding alone.
        let mut cpu_plan = lent.clone();
        ops.iter().for_each(|&i| cpu_plan[i] = Place::Cpu);
        let cpu = walk(
            &steps,
            &cpu_plan,
            &input,
            &mut Kept::new(),
            &mut NoModels,
            &mut gpu,
            &NoHold,
        )
        .unwrap();
        let cpu = picture(&cpu);
        assert_eq!(cpu.data.len(), device.data.len());
        let worst = cpu
            .data
            .iter()
            .zip(&device.data)
            .map(|(a, b)| (a - b).abs() / a.abs().max(1e-2))
            .fold(0f32, f32::max);
        println!("the device's run against the CPU's: {worst:.3e} relative at worst");
        assert!(worst <= 1e-4, "{worst:.3e}");
        assert!(device.data != cpu.data, "did the device run?");
    }

    /// No white tints a monochrome raw, on a device or on the CPU: the
    /// edit's 3000 K light with a tint develops grey, the edit keeps
    /// it, and a mask's white from the walk's shift moves nothing.
    #[test]
    fn no_white_tints_a_monochrome_raw_on_either_path() {
        let input = Input::Raw(Arc::new(RawFrame {
            layout: SensorLayout::Monochrome,
            samples: Samples::U16(
                (0..160 * 120)
                    .map(|i| 300 + (i % 160) as u16 * 20)
                    .collect(),
            ),
            as_shot_coefficients: Some([1.0; 3]),
            calibrations: Vec::new(),
            ..(*frame()).clone()
        }));
        let mut e = edit();
        e.lens.defringe = false;
        e.white_balance = greycard_edit::WhiteBalance::Custom {
            temperature: 3000.0,
            tint: 0.01,
        };
        let held = e.white_balance.clone();
        let purpose = Purpose::Export(Box::default());
        let steps = stages::stages(&e, &purpose, &input.facts(0), None);
        let mut runs = vec![(plan(&steps, &purpose, false), None)];
        if let Some(gpu) = device() {
            runs.push((plan(&steps, &purpose, true), gpu));
        }
        for (plan, mut gpu) in runs {
            let walked = walk(
                &steps,
                &plan,
                &input,
                &mut Kept::new(),
                &mut NoModels,
                &mut gpu,
                &NoHold,
            )
            .unwrap();
            for px in picture(&walked).pixels() {
                assert!((px[0] - px[1]).abs() < 1e-5 && (px[2] - px[1]).abs() < 1e-5);
            }
            let local = greycard_edit::white::LocalWhite::Absolute {
                temperature: 2500.0,
                tint: -0.01,
            };
            let m = walked.meta.white_shift().unwrap().local(&local).unwrap();
            assert_eq!(m, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        }
        assert_eq!(e.white_balance, held);
    }

    /// A monochrome raw on a device: no mosaic for the device's
    /// demosaic, so the base is the CPU's, and the Detail section, the
    /// dehaze, and the sharpen run there; the picture stays grey and is
    /// the CPU's run within the sharpen's tolerance.
    #[test]
    fn a_monochrome_raw_on_a_device_is_the_cpu_s_in_grey() {
        let Some(mut gpu) = device() else {
            return;
        };
        let (w, h) = (160, 120);
        let samples = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let checker = if (x / 3 + y / 3) % 2 == 0 { 150 } else { 0 };
                (200 + 2400 * x / w + checker) as u16
            })
            .collect();
        let input = Input::Raw(Arc::new(RawFrame {
            layout: SensorLayout::Monochrome,
            samples: Samples::U16(samples),
            as_shot_coefficients: Some([1.0; 3]),
            calibrations: Vec::new(),
            ..(*frame()).clone()
        }));
        let mut e = edit();
        e.lens.defringe = false;
        let purpose = Purpose::Export(Box::default());
        let steps = stages::stages(&e, &purpose, &input.facts(0), None);
        let go = |plan: &[Place], gpu: &mut Option<greycard_gpu::Context>| {
            let walked = walk(
                &steps,
                plan,
                &input,
                &mut Kept::new(),
                &mut NoModels,
                gpu,
                &NoHold,
            )
            .unwrap();
            (picture(&walked), walked.places)
        };
        let (device, places) = go(&plan(&steps, &purpose, true), &mut gpu);
        assert!(places.contains(&Place::Gpu));
        let (cpu, _) = go(&plan(&steps, &purpose, false), &mut None);
        for image in [&device, &cpu] {
            for px in image.pixels() {
                assert!((px[0] - px[1]).abs() < 1e-5 && (px[2] - px[1]).abs() < 1e-5);
            }
        }
        let worst = cpu
            .data
            .iter()
            .zip(&device.data)
            .map(|(a, b)| (a - b).abs() / a.abs().max(1e-2))
            .fold(0f32, f32::max);
        println!("a monochrome raw on the device against the CPU: {worst:.3e} relative at worst");
        assert!(worst <= 1e-4, "{worst:.3e}");
    }

    /// An export lent a device whose ops fail goes on on the CPU, as
    /// the viewport does: a lost device lets the context go and the
    /// export is the CPU's to the bit; a demosaic the device declines
    /// (a frame too small for its bands) is the CPU's for the frame
    /// with the context kept.
    #[test]
    fn an_export_on_a_failing_device_falls_back_to_the_cpu() {
        let Some(mut gpu) = device() else {
            return;
        };
        let purpose = Purpose::Export(Box::default());
        let input = Input::Raw(frame());
        let mut e = edit();
        e.lens.defringe = false;
        let steps = stages::stages(&e, &purpose, &input.facts(0), None);
        let lent = plan(&steps, &purpose, true);
        assert!(lent.contains(&Place::Gpu));
        let reference = walk_cpu(&e, &purpose, &input, 0, &mut Kept::new());
        gpu.as_ref().unwrap().mark_lost();
        let walked = walk(
            &steps,
            &lent,
            &input,
            &mut Kept::new(),
            &mut NoModels,
            &mut gpu,
            &NoHold,
        )
        .expect("the export goes on");
        assert!(gpu.is_none(), "a lost device's context goes");
        let demosaic = steps
            .steps
            .iter()
            .position(|s| matches!(s, Step::Op(Stage::Demosaic(..))))
            .unwrap();
        assert_eq!(lent[demosaic], Place::Gpu);
        // A CA on a frame below its smallest side runs nowhere, so only
        // the steps past the demosaic say where the rest went.
        assert!(walked.places[demosaic..].iter().all(|p| *p == Place::Cpu));
        assert!(picture(&walked).data == picture(&reference).data);

        let Some(mut gpu) = device() else {
            return;
        };
        let small = Input::Raw(frame_of(14, 12, [1.0, 1.0]));
        let mut e = Edit::default();
        e.lens.chromatic_aberration = false;
        // The sharpen would run on the device and differ from the
        // reference by its rounding; what is checked here is the
        // demosaic's fall to the CPU, so the picture must be exact.
        e.sharpen.enabled = false;
        let steps = stages::stages(&e, &purpose, &small.facts(0), None);
        let i = steps
            .steps
            .iter()
            .position(|s| matches!(s, Step::Op(Stage::Demosaic(..))))
            .unwrap();
        let lent = plan(&steps, &purpose, true);
        assert_eq!(lent[i], Place::Gpu);
        let walked = walk(
            &steps,
            &lent,
            &small,
            &mut Kept::new(),
            &mut NoModels,
            &mut gpu,
            &NoHold,
        )
        .expect("the export goes on");
        assert_eq!(walked.places[i], Place::Cpu);
        assert!(gpu.is_some(), "the device is kept");
        let reference = walk_cpu(&e, &purpose, &small, 0, &mut Kept::new());
        assert!(picture(&walked).data == picture(&reference).data);
    }

    /// A grey picture, `white` its color space's.
    fn grey_picture(white: greycard_core::Chromaticity) -> Input {
        let mut image = WorkingImage::new(8, 6);
        image.data.fill(0.18);
        Input::Picture {
            image: Arc::new(image),
            meta: Arc::new(Picture {
                image: WorkingImage::new(0, 0),
                make: String::new(),
                model: String::new(),
                shot: Default::default(),
                metadata: Default::default(),
                space: "sRGB".into(),
                bits: 16,
                white,
            }),
        }
    }

    /// A picture's white balance on the CPU, the reference: an offset of
    /// none is the picture at As shot to the bit, there being no step;
    /// a warmer one makes a grey warm by the adaptation's own amount,
    /// and the masks' white balances are made from the light it names.
    #[test]
    fn a_pictures_offset_warms_its_grey_by_the_adaptation() {
        let input = grey_picture(greycard_core::illuminant::D65);
        let go = |wb: greycard_edit::WhiteBalance| {
            let mut edit = Edit::for_picture();
            edit.white_balance = wb;
            let steps = stages::stages(&edit, &Purpose::Reduced, &input.facts(0), None);
            let plan = plan(&steps, &Purpose::Reduced, false);
            walk(
                &steps,
                &plan,
                &input,
                &mut Kept::new(),
                &mut NoModels,
                &mut None,
                &NoHold,
            )
            .unwrap()
        };
        let shot = go(greycard_edit::WhiteBalance::AsShot);
        let none = go(greycard_edit::WhiteBalance::Offset {
            mired: 0.0,
            tint: 0.0,
        });
        assert!(picture(&shot).data == picture(&none).data);
        assert!(picture(&shot).data.iter().all(|&v| v == 0.18));
        let warm = go(greycard_edit::WhiteBalance::Offset {
            mired: 25.0,
            tint: 0.0,
        });
        let own = greycard_core::picture::white_temp_tint(greycard_core::illuminant::D65);
        let light = greycard_edit::white::picture_light(
            &greycard_edit::WhiteBalance::Offset {
                mired: 25.0,
                tint: 0.0,
            },
            own,
        )
        .unwrap();
        let want = greycard_core::color::apply3(
            &greycard_edit::white::adaptation(own, light).unwrap(),
            [0.18; 3],
        );
        let px = &picture(&warm).data[..3];
        assert_eq!(px, want);
        assert!(px[0] > 0.18 && px[2] < 0.18, "{px:?}");
        assert_eq!(warm.meta.white_shift().unwrap().global, light);
        assert_eq!(shot.meta.white_shift().unwrap().global, own);
    }

    /// A reduce drops the pixels past its last whole block, which a
    /// turn puts on another edge: an odd-sized picture's kept base is
    /// developed again at a new turn, not permuted, and is the picture
    /// a walk from nothing makes.
    #[test]
    fn a_reduced_picture_of_an_odd_size_is_not_permuted() {
        let mut image = WorkingImage::new(9, 7);
        for (i, v) in image.data.iter_mut().enumerate() {
            *v = (i % 23) as f32 / 23.0;
        }
        let input = Input::Picture {
            image: Arc::new(image),
            meta: Arc::new(Picture {
                image: WorkingImage::new(0, 0),
                make: String::new(),
                model: String::new(),
                shot: Default::default(),
                metadata: Default::default(),
                space: "sRGB".into(),
                bits: 16,
                white: greycard_core::illuminant::D65,
            }),
        };
        let edit = Edit::default();
        let reduced = |turn: u8| {
            let mut steps = stages::stages(&edit, &Purpose::Reduced, &input.facts(turn), None);
            // Too small for the reduced develop's own bin: one by hand.
            let at = steps
                .steps
                .iter()
                .position(|s| matches!(s, Step::Keep(Keep::Base)))
                .unwrap();
            steps.steps.insert(at, Step::Op(Stage::Reduce(2)));
            steps
        };
        let go = |turn: u8, kept: &mut Kept| {
            let steps = reduced(turn);
            let plan = plan(&steps, &Purpose::Reduced, false);
            walk(
                &steps,
                &plan,
                &input,
                kept,
                &mut NoModels,
                &mut None,
                &NoHold,
            )
            .unwrap()
        };
        let mut kept = Kept::new();
        go(0, &mut kept);
        let turned = go(1, &mut kept);
        assert_eq!(turned.from, None);
        let fresh = go(1, &mut Kept::new());
        let (a, b) = (picture(&turned), picture(&fresh));
        assert_eq!((a.width, a.height), (3, 4));
        assert!(a.data == b.data);
    }

    /// A device op that declines the picture (a dehaze whose strength
    /// comes to zero has no model to apply) runs its whole run again on
    /// the CPU, the context kept, and the picture is the CPU's.
    #[test]
    fn a_declined_run_runs_again_on_the_cpu_with_the_context_kept() {
        let Some(mut gpu) = device() else {
            return;
        };
        let input = Input::Raw(frame());
        let mut e = edit();
        e.lens.defringe = false;
        let mut steps = stages::stages(&e, &Purpose::Viewport, &input.facts(0), None);
        for s in &mut steps.steps {
            if let Step::Op(Stage::Dehaze(o)) = s {
                o.amount = 0.0;
            }
        }
        let device_plan = plan(&steps, &Purpose::Viewport, true);
        let walked = walk(
            &steps,
            &device_plan,
            &input,
            &mut Kept::new(),
            &mut NoModels,
            &mut gpu,
            &NoHold,
        )
        .unwrap();
        assert!(gpu.is_some(), "the context is kept");
        let dehaze = steps
            .steps
            .iter()
            .position(|s| matches!(s, Step::Op(Stage::Dehaze(_))))
            .unwrap();
        assert_eq!(walked.places[dehaze], Place::Cpu);
        assert!(
            walked.sharpen_mask.is_some(),
            "the sharpen ran on the CPU too"
        );
        // Everything on the CPU but the demosaic, which is the
        // device's picture whatever runs after it.
        let cpu_plan = places(&steps, false, true, false);
        let cpu = walk(
            &steps,
            &cpu_plan,
            &input,
            &mut Kept::new(),
            &mut NoModels,
            &mut gpu,
            &NoHold,
        )
        .unwrap();
        assert!(picture(&walked).data == picture(&cpu).data);
    }

    /// The sharpen switched off after it ran on the device: the Detail
    /// section's run writes the viewport's texture itself, from the
    /// patched picture, and not from the picture before the sharpen
    /// left on the device.
    #[test]
    fn the_sharpen_switched_off_on_a_device_ends_at_the_detail_s_texture() {
        let Some(mut gpu) = device() else {
            return;
        };
        let input = Input::Raw(frame());
        let mut e = edit();
        e.lens.defringe = false;
        let mut kept = Kept::new();
        let mut go = |e: &Edit, kept: &mut Kept| {
            let steps = stages::stages(e, &Purpose::Viewport, &input.facts(0), None);
            let plan = plan(&steps, &Purpose::Viewport, true);
            walk(
                &steps,
                &plan,
                &input,
                kept,
                &mut NoModels,
                &mut gpu,
                &NoHold,
            )
            .unwrap()
        };
        let on = go(&e, &mut kept);
        assert!(on.sharpen.is_some());
        e.sharpen.enabled = false;
        let off = go(&e, &mut kept);
        assert_eq!(off.from, Some(Keep::Patched));
        assert!(matches!(off.buf, Buf::Texture(_)));
        assert!(off.sharpen.is_none());
        assert!(off.dehaze.is_some());
    }

    /// Models of the tests' own: a network that is the plain demosaic
    /// when it is `there`, no fills, and the unmade Fill patches each
    /// raster was made beside.
    #[derive(Default)]
    struct Fake {
        there: bool,
        unmade: Vec<Option<Vec<u64>>>,
    }

    impl Models for Fake {
        fn can_denoise(&mut self, model: &'static greycard_ai::Model) -> Result<(), NoLearned> {
            if self.there {
                Ok(())
            } else {
                Err(NoLearned::Missing(model))
            }
        }

        fn denoise(
            &mut self,
            _: &'static greycard_ai::Model,
            _: &RawFrame,
            prepared: &Prepared,
        ) -> Result<Vec<f32>, NoLearned> {
            let (rgb, _, _) = demosaic_prepared(prepared, &DevelopSettings::default())
                .map_err(|e| NoLearned::Failed(e.to_string()))?;
            Ok(rgb)
        }

        fn fill(&mut self, _: &Patch, _: &Region, _: &WorkingImage) -> Option<Vec<f32>> {
            None
        }

        fn could_fill_more(&mut self, _: &Retouch, _: &WorkingImage) -> bool {
            false
        }

        fn rasters(
            &mut self,
            _: Masks,
            _: &Arc<WorkingImage>,
            patched: Option<Patched<'_>>,
            _: &Meta,
        ) -> Rasters {
            self.unmade.push(patched.map(|p| p.unmade.to_vec()));
            Rasters::new()
        }
    }

    fn walk_with(
        edit: &Edit,
        purpose: &Purpose,
        input: &Input,
        kept: &mut Kept,
        models: &mut dyn Models,
    ) -> Walked {
        walk_at(edit, purpose, input, 0, kept, models)
    }

    fn walk_at(
        edit: &Edit,
        purpose: &Purpose,
        input: &Input,
        turn: u8,
        kept: &mut Kept,
        models: &mut dyn Models,
    ) -> Walked {
        let steps = stages::stages(edit, purpose, &input.facts(turn), None);
        let plan = plan(&steps, purpose, false);
        walk(&steps, &plan, input, kept, models, &mut None, &NoHold).expect("walks")
    }

    /// `edit` with a learned Subject mask, which puts the masks' tap
    /// in its list.
    fn with_subject(mut edit: Edit) -> Edit {
        edit.adjustments.push(greycard_edit::Adjustment {
            id: 11,
            mask: greycard_edit::mask::Mask {
                components: vec![greycard_edit::mask::Component {
                    shape: greycard_edit::mask::Shape::Subject {},
                    ..Default::default()
                }],
                invert: false,
                refine: 0.0,
            },
            ..Default::default()
        });
        edit
    }

    /// The export's list, its keeps left out as an export leaves them,
    /// starts from the last point the viewport's walk kept on the CPU:
    /// the two lists make the same picture to there, whatever each
    /// purpose makes of the masks. The picture is the one a fresh
    /// export walk makes, and the masks are made as the export makes
    /// them.
    #[test]
    fn an_export_list_resumes_from_the_viewport_s_kept_points_on_the_cpu() {
        let input = Input::Raw(frame());
        let edit = with_subject(edit());
        let mut kept = Kept::new();
        walk_cpu(&edit, &Purpose::Viewport, &input, 0, &mut kept);
        let purpose = Purpose::Export(Box::default());
        let mut export = stages::stages(&edit, &purpose, &input.facts(0), None);
        export.steps.retain(|s| !matches!(s, Step::Keep(_)));
        let export_plan = plan(&export, &purpose, false);
        let resumed = walk(
            &export,
            &export_plan,
            &input,
            &mut kept,
            &mut NoModels,
            &mut None,
            &NoHold,
        )
        .unwrap();
        assert_eq!(resumed.from, Some(Keep::Developed));
        assert!(matches!(resumed.taps.masks, Some((Masks::Make, _))));
        assert_eq!(resumed.places.len(), export.steps.len());
        let fresh = walk(
            &export,
            &export_plan,
            &input,
            &mut Kept::new(),
            &mut NoModels,
            &mut None,
            &NoHold,
        )
        .unwrap();
        assert_eq!(fresh.from, None);
        assert!(picture(&resumed).data == picture(&fresh).data);
        assert!(kept.holds(Keep::Developed), "the viewport's points stay");
    }

    /// A learned denoiser not to be had is known before the walk: the
    /// mosaic is prepared once, and the walk after it, of the same
    /// stand-in, starts from its points and still says why.
    #[test]
    fn a_stand_in_is_known_before_the_walk_and_kept_with_its_points() {
        let input = Input::Raw(frame());
        let mut learned = edit();
        learned.noise.learned = greycard_edit::Learned::Fast;
        let mut kept = Kept::new();
        let first = walk_cpu(&learned, &Purpose::Viewport, &input, 0, &mut kept);
        let prepares = first
            .seconds
            .iter()
            .filter(|(s, _)| *s == "prepare")
            .count();
        assert_eq!(prepares, 1);
        assert!(matches!(first.meta.stood_in, Some(NoLearned::Missing(_))));
        let again = walk_cpu(&learned, &Purpose::Viewport, &input, 0, &mut kept);
        assert_eq!(again.from, Some(Keep::Developed));
        assert!(matches!(again.meta.stood_in, Some(NoLearned::Missing(_))));
    }

    /// An edit without the learned denoiser makes the stand-in's list,
    /// so it starts from the stand-in's points; it says no stand-in.
    #[test]
    fn a_plain_edit_resumed_from_a_stand_in_s_points_says_none() {
        let input = Input::Raw(frame());
        let mut e = edit();
        e.noise.profiled = false;
        e.noise.learned = greycard_edit::Learned::Fast;
        let mut kept = Kept::new();
        let stood = walk_cpu(&e, &Purpose::Viewport, &input, 0, &mut kept);
        assert!(matches!(stood.meta.stood_in, Some(NoLearned::Missing(_))));
        e.noise.learned = greycard_edit::Learned::Off;
        let plain = walk_cpu(&e, &Purpose::Viewport, &input, 0, &mut kept);
        assert_eq!(plain.from, Some(Keep::Developed));
        assert!(plain.meta.stood_in.is_none());
    }

    /// A walk that starts past the retouch never asks for its fills,
    /// and the masks are still told which were left as they were.
    #[test]
    fn a_resumed_patched_picture_carries_its_unmade_fills() {
        use greycard_edit::retouch::Method;
        let input = Input::Raw(frame());
        let mut e = with_subject(edit());
        e.retouch.patches.push(Patch {
            id: 21,
            method: Method::Fill,
            points: vec![[0.5, 0.5]],
            radius: 0.08,
            ..Patch::default()
        });
        let mut kept = Kept::new();
        let mut models = Fake::default();
        walk_with(&e, &Purpose::Viewport, &input, &mut kept, &mut models);
        e.detail.clarity = 0.1;
        let again = walk_with(&e, &Purpose::Viewport, &input, &mut kept, &mut models);
        assert_eq!(again.from, Some(Keep::Patched));
        assert_eq!(models.unmade, [Some(vec![21]), Some(vec![21])]);
    }

    /// The learned denoiser is the demosaic: a method of the other
    /// mosaic chosen in the edit gives way to nothing when it runs,
    /// and to the frame's own when it does not.
    #[test]
    fn the_learned_denoiser_clears_a_demosaic_that_gave_way() {
        let input = Input::Raw(frame());
        let mut e = edit();
        e.demosaic = greycard_edit::Demosaic::Markesteijn3;
        let plain = walk_cpu(&e, &Purpose::Viewport, &input, 0, &mut Kept::new());
        assert!(plain.meta.gave_way.is_some());
        e.noise.learned = greycard_edit::Learned::Fast;
        let mut models = Fake {
            there: true,
            ..Fake::default()
        };
        let learned = walk_with(
            &e,
            &Purpose::Viewport,
            &input,
            &mut Kept::new(),
            &mut models,
        );
        assert!(learned.meta.stood_in.is_none());
        assert_eq!(learned.meta.gave_way, None);
    }

    /// The learned pair turns with the base: at either end of the
    /// strength the base is one of the pair itself and stays shared
    /// with it, turned once; in between it is a blend of its own,
    /// turned. Each is the picture a walk at the new turn makes.
    #[test]
    fn a_turned_base_takes_the_learned_pair_with_it() {
        let input = Input::Raw(frame());
        let mut e = edit();
        e.lens.defringe = false;
        e.noise.learned = greycard_edit::Learned::Fast;
        let pair = |kept: &Kept| match &kept
            .points
            .iter()
            .find(|p| p.keep == Keep::Pair)
            .expect("a pair")
            .buf
        {
            KeptBuf::Pair { model, plain } => (model.clone(), plain.clone()),
            _ => panic!("a pair in memory"),
        };
        // The two strengths as one, as an edit from before the chroma's
        // is brought up: the ends share as they did.
        for strength in [0.0f32, 0.5, 1.0] {
            e.noise.learned_strength = strength;
            e.noise.learned_chroma_strength = strength;
            let mut models = Fake {
                there: true,
                ..Fake::default()
            };
            let mut kept = Kept::new();
            walk_at(&e, &Purpose::Viewport, &input, 0, &mut kept, &mut models);
            let turned = walk_at(&e, &Purpose::Viewport, &input, 1, &mut kept, &mut models);
            assert_eq!(turned.from, Some(Keep::Base), "strength {strength}");
            let mut fresh = Kept::new();
            let made = walk_at(&e, &Purpose::Viewport, &input, 1, &mut fresh, &mut models);
            assert!(
                picture(&turned).data == picture(&made).data,
                "strength {strength}"
            );
            let ((model, plain), (fresh_model, fresh_plain)) = (pair(&kept), pair(&fresh));
            assert!(model.data == fresh_model.data && plain.data == fresh_plain.data);
            let base = kept.picture(Keep::Base).expect("a base");
            assert!(base.data == fresh.picture(Keep::Base).unwrap().data);
            match strength {
                1.0 => assert!(Arc::ptr_eq(base, &model), "the model, shared"),
                0.0 => assert!(Arc::ptr_eq(base, &plain), "the plain, shared"),
                _ => assert!(!Arc::ptr_eq(base, &model) && !Arc::ptr_eq(base, &plain)),
            }
        }
    }

    /// A walk resumed from a turned base still makes its masks: the
    /// masks' tap is turned with the base, not left behind.
    #[test]
    fn a_turned_base_still_makes_the_masks() {
        let input = Input::Raw(frame());
        let mut e = with_subject(edit());
        e.lens.defringe = false;
        let mut models = Fake::default();
        let mut kept = Kept::new();
        walk_at(&e, &Purpose::Viewport, &input, 0, &mut kept, &mut models);
        let turned = walk_at(&e, &Purpose::Viewport, &input, 1, &mut kept, &mut models);
        assert_eq!(turned.from, Some(Keep::Base));
        assert_eq!(models.unmade.len(), 2, "the masks made at each turn");
    }

    #[test]
    fn the_blend_is_linear_and_shares_at_the_ends() {
        let model = Arc::new(WorkingImage::from_data(2, 1, vec![1.0; 6]).unwrap());
        let plain = Arc::new(WorkingImage::from_data(2, 1, vec![0.0; 6]).unwrap());
        assert!(Arc::ptr_eq(&blend(&model, &plain, 1.0, 1.0), &model));
        assert!(Arc::ptr_eq(&blend(&model, &plain, 0.0, 0.0), &plain));
        assert!(Arc::ptr_eq(&blend(&model, &plain, 2.0, 2.0), &model));
        let half = blend(&model, &plain, 0.25, 0.25);
        assert!(half.data.iter().all(|v| (*v - 0.25).abs() < 1e-6));
    }

    /// The per-sample mix the blend was before the chroma had a
    /// strength of its own, its ends included.
    fn one_strength(model: &WorkingImage, plain: &WorkingImage, s: f32) -> Vec<f32> {
        if s >= 1.0 {
            return model.data.clone();
        }
        let mut out = plain.data.clone();
        for (o, m) in out.iter_mut().zip(&model.data) {
            *o += (m - *o) * s;
        }
        out
    }

    /// A pair of noisy pictures, the same scene apart.
    fn noisy_pair() -> (Arc<WorkingImage>, Arc<WorkingImage>) {
        let mut seed = 0x9e37_79b9_u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let (w, h) = (16, 8);
        let plain: Vec<f32> = (0..w * h * 3).map(|_| next()).collect();
        let model: Vec<f32> = (0..w * h * 3).map(|_| next()).collect();
        (
            Arc::new(WorkingImage::from_data(w, h, model).unwrap()),
            Arc::new(WorkingImage::from_data(w, h, plain).unwrap()),
        )
    }

    #[test]
    fn equal_strengths_blend_as_one_did_bit_for_bit() {
        let (model, plain) = noisy_pair();
        for s in [0.0f32, 0.05, 0.25, 0.41, 0.5, 0.9, 1.0] {
            let both = blend(&model, &plain, s, s);
            assert!(both.data == one_strength(&model, &plain, s), "strength {s}");
        }
    }

    #[test]
    fn the_chroma_and_the_luminance_blend_apart() {
        let lum = |px: &[f32; 3]| LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2];
        let (model, plain) = noisy_pair();
        // Chroma whole, luminance at 0.41: each pixel's luminance is
        // the plain's taken 0.41 of the way, its color the model's.
        let out = blend(&model, &plain, 0.41, 1.0);
        for ((o, m), p) in out
            .data
            .as_chunks::<3>()
            .0
            .iter()
            .zip(model.data.as_chunks::<3>().0)
            .zip(plain.data.as_chunks::<3>().0)
        {
            let y = lum(p) + (lum(m) - lum(p)) * 0.41;
            assert!((lum(o) - y).abs() < 1e-5);
            for i in 0..3 {
                assert!(((o[i] - lum(o)) - (m[i] - lum(m))).abs() < 1e-5);
            }
        }
        // And the other way: the luminance whole, the plain's color.
        let out = blend(&model, &plain, 1.0, 0.0);
        for ((o, m), p) in out
            .data
            .as_chunks::<3>()
            .0
            .iter()
            .zip(model.data.as_chunks::<3>().0)
            .zip(plain.data.as_chunks::<3>().0)
        {
            assert!((lum(o) - lum(m)).abs() < 1e-5);
            for i in 0..3 {
                assert!(((o[i] - lum(o)) - (p[i] - lum(p))).abs() < 1e-5);
            }
        }
        // A grey change has no chroma: the chroma's strength leaves it.
        let grey = Arc::new(WorkingImage::from_data(1, 1, vec![0.6; 3]).unwrap());
        let dark = Arc::new(WorkingImage::from_data(1, 1, vec![0.2; 3]).unwrap());
        let out = blend(&grey, &dark, 0.5, 0.0);
        assert!(out.data.iter().all(|v| (*v - 0.4).abs() < 1e-6));
    }

    /// The fill print's tolerances, measured: the largest relative move
    /// of any block's mean ([`crate::ai::print_distance`]) under five
    /// fills' windows, between the CPU's base and the device's (the
    /// rounding a fill must survive), between the base and the base
    /// under the smallest edits a user makes (one mired of white
    /// balance, one step of the lens's distortion, one of exposure),
    /// and between the full develop's and the reduced develop's.
    /// `GREYCARD_PRINT_RAW` names a raw to measure on; the engine's
    /// test frame otherwise. Run it on lavapipe.
    #[test]
    #[ignore = "a measurement, printed: run on lavapipe, GREYCARD_PRINT_RAW naming a raw"]
    fn the_fill_print_measured() {
        use greycard_edit::retouch::{Method, Patch};
        type Prints = Vec<Vec<[u16; 3]>>;
        /// Each fill's print of the picture it would be made from;
        /// none made, so every one sees the base.
        struct Printer(Prints);
        impl Models for Printer {
            fn can_denoise(&mut self, m: &'static greycard_ai::Model) -> Result<(), NoLearned> {
                NoModels.can_denoise(m)
            }
            fn denoise(
                &mut self,
                m: &'static greycard_ai::Model,
                f: &RawFrame,
                p: &Prepared,
            ) -> Result<Vec<f32>, NoLearned> {
                NoModels.denoise(m, f, p)
            }
            fn fill(
                &mut self,
                _: &Patch,
                region: &Region,
                before: &WorkingImage,
            ) -> Option<Vec<f32>> {
                self.0.push(crate::ai::fill_print(
                    region,
                    before,
                    before.width,
                    before.height,
                    1,
                ));
                None
            }
            fn fill_read(
                &mut self,
                patch: &Patch,
                _: &Region,
                picture: &WorkingImage,
                factor: usize,
                full: (usize, usize),
            ) -> Option<Vec<f32>> {
                let region = patch.region(full.0, full.1)?;
                self.0.push(crate::ai::fill_print(
                    &region, picture, full.0, full.1, factor,
                ));
                None
            }
            fn could_fill_more(&mut self, _: &Retouch, _: &WorkingImage) -> bool {
                false
            }
            fn rasters(
                &mut self,
                how: crate::stages::Masks,
                base: &Arc<WorkingImage>,
                patched: Option<Patched<'_>>,
                meta: &Meta,
            ) -> Rasters {
                NoModels.rasters(how, base, patched, meta)
            }
        }
        let Some(mut gpu) = device() else {
            return;
        };
        let input = match std::env::var_os("GREYCARD_PRINT_RAW") {
            Some(path) => Input::Raw(Arc::new(
                greycard_core::decode::decode_path_unmapped(std::path::Path::new(&path))
                    .expect("the raw decodes"),
            )),
            None => Input::Raw(frame_of(640, 480, [1.0, 1.0])),
        };
        let mut base = Edit {
            white_balance: greycard_edit::WhiteBalance::Custom {
                temperature: 5000.0,
                tint: 0.0,
            },
            ..Edit::default()
        };
        base.retouch.enabled = true;
        for (i, at) in [[0.3, 0.3], [0.7, 0.3], [0.5, 0.5], [0.3, 0.7], [0.7, 0.7]]
            .into_iter()
            .enumerate()
        {
            base.retouch.patches.push(Patch {
                id: i as u64 + 1,
                method: Method::Fill,
                points: vec![at],
                radius: 0.03,
                remove: true,
                ..Default::default()
            });
        }
        let prints = |edit: &Edit, purpose: &Purpose, gpu: &mut Option<greycard_gpu::Context>| {
            let steps = stages::stages(edit, purpose, &input.facts(0), None);
            let plan = plan(&steps, purpose, gpu.is_some());
            let mut printer = Printer(Vec::new());
            let walked = walk(
                &steps,
                &plan,
                &input,
                &mut Kept::new(),
                &mut printer,
                gpu,
                &NoHold,
            )
            .expect("walks");
            (printer.0, walked.places.contains(&Place::Gpu))
        };
        let export = Purpose::Export(Box::default());
        let distance = |a: &Prints, b: &Prints| {
            assert_eq!(a.len(), b.len());
            a.iter()
                .zip(b)
                .map(|(a, b)| crate::ai::print_distance(a, b))
                .fold(0.0f32, f32::max)
        };
        let (cpu, _) = prints(&base, &export, &mut None);
        let (device, on_device) = prints(&base, &export, &mut gpu);
        assert!(on_device, "the device ran some of the walk");
        let (again, _) = prints(&base, &export, &mut None);
        let edited = |change: &dyn Fn(&mut Edit)| {
            let mut e = base.clone();
            change(&mut e);
            prints(&e, &export, &mut None).0
        };
        let mired = edited(&|e| {
            e.white_balance = greycard_edit::WhiteBalance::Custom {
                temperature: 1.0 / (1.0 / 5000.0 - 1e-6),
                tint: 0.0,
            }
        });
        let lens = edited(&|e| e.lens.manual = 0.005);
        let exposure = edited(&|e| e.light.exposure = 0.1);
        // Where the device walk ran each stage, and how far its
        // pictures under the windows are from the CPU's, pixel by pixel.
        {
            let steps = stages::stages(&base, &export, &input.facts(0), None);
            let plan = plan(&steps, &export, true);
            let on: Vec<&str> = steps
                .steps
                .iter()
                .zip(&plan)
                .filter_map(|(s, p)| match s {
                    Step::Op(stage) if *p == Place::Gpu => Some(stage.name()),
                    _ => None,
                })
                .collect();
            eprintln!("on the device: {on:?}");
        }
        let (reduced, _) = prints(&base, &Purpose::Reduced, &mut None);
        let spread = |a: &Prints, b: &Prints| {
            let mut all: Vec<f32> = a
                .iter()
                .zip(b)
                .flat_map(|(a, b)| a.iter().zip(b))
                .flat_map(|(a, b)| a.iter().zip(b))
                .filter(|(a, b)| **a != u16::MAX && **b != u16::MAX)
                .map(|(&a, &b)| crate::ai::print_distance(&[[a; 3]], &[[b; 3]]))
                .collect();
            all.sort_by(f32::total_cmp);
            let q = |f: f64| all[((all.len() - 1) as f64 * f) as usize];
            format!(
                "{} values: median {:.5}, 90% {:.5}, 99% {:.5}, max {:.5}",
                all.len(),
                q(0.5),
                q(0.9),
                q(0.99),
                q(1.0)
            )
        };
        let reduced_distance = |full: &Prints| {
            full.iter()
                .zip(&reduced)
                .map(|(a, b)| crate::ai::print_distance(a, b))
                .fold(0.0f32, f32::max)
        };
        eprintln!("CPU against CPU: {:.5}", distance(&cpu, &again));
        eprintln!("CPU against the device: {:.5}", distance(&cpu, &device));
        eprintln!("one mired: {:.5}", distance(&cpu, &mired));
        eprintln!("one lens step: {:.5}", distance(&cpu, &lens));
        eprintln!("an exposure step: {:.5}", distance(&cpu, &exposure));
        eprintln!("reduced against the CPU: {:.5}", reduced_distance(&cpu));
        eprintln!(
            "reduced against the device: {:.5}",
            reduced_distance(&device)
        );
        let reduced_mired = {
            let mut e = base.clone();
            e.white_balance = greycard_edit::WhiteBalance::Custom {
                temperature: 1.0 / (1.0 / 5000.0 - 1e-6),
                tint: 0.0,
            };
            prints(&e, &Purpose::Reduced, &mut None).0
        };
        eprintln!(
            "reduced, one mired against none: {:.5}",
            distance(&reduced, &reduced_mired)
        );
        eprintln!("one mired, spread: {}", spread(&cpu, &mired));
        eprintln!("one lens step, spread: {}", spread(&cpu, &lens));
        let three = edited(&|e| {
            e.white_balance = greycard_edit::WhiteBalance::Custom {
                temperature: 1.0 / (1.0 / 5000.0 - 3e-6),
                tint: 0.0,
            }
        });
        eprintln!("three mired, spread: {}", spread(&cpu, &three));
        eprintln!(
            "reduced against the CPU, spread: {}",
            spread(&cpu, &reduced)
        );
        eprintln!(
            "reduced, one mired, spread: {}",
            spread(&reduced, &reduced_mired)
        );
    }
}
