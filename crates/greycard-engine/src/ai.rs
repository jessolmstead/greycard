//! The learned masks, on the worker: the models, the preview they see,
//! and the rasters they made. A mask is made once for a shape and kept
//! until the shape or the base it was made from changes, or another
//! file is opened. Made rasters are also kept on disk, keyed by the
//! base's print (`mask_base`: the file, what made the base, the turn,
//! and the size), the model, and the shape, so opening a file again or
//! exporting it does not run the model again, and the viewport and an
//! export read one raster; so are made fills, beside them.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use greycard_ai::fill::Window;
use greycard_ai::landscape::{self, Offer, Scene};
use greycard_ai::sam::Embedding;
use greycard_ai::sam3::{self, Choice, Encodings, Picture, Presets, Sam3};
use greycard_ai::sky;
#[cfg(any(test, feature = "testing"))]
use greycard_ai::sky::Prior;
use greycard_ai::{
    Denoiser, Fill, Model, Prompt, Provider, REMOVE, REMOVE_FAST, Remove, Rgb8, Rgbf, SAM, SAM3,
    SKY, Sam, Sky, Store, Subject, refine,
};
use greycard_core::develop::noise;
use greycard_core::develop::retouch::Region;
use greycard_core::image::WorkingImage;
use greycard_edit::Edit;
use greycard_edit::retouch::FillChoice;
// The learned rasters' stages, by version, in the disk cache's keys
// below: the develop key takes the same in for an edit with such a
// mask, so the pictures kept of it are made again too.
use greycard_edit::brush::{RASTER_WIDTH, Raster, WIDE_RASTER_WIDTH, raster_height, raster_width};
use greycard_edit::key::{LANDSCAPE_PIPELINE, PART_PIPELINE, PERSON_EDGE, SKY_PIPELINE};
use greycard_edit::mask::Shape;
use greycard_edit::retouch::{Patch, Retouch};

pub use crate::mask_base::{Base, Print};

/// A learned component: its adjustment's id and its index in the mask.
pub type Key = (u64, usize);

/// The providers this build offers on this machine, asked once they
/// are known: none while ONNX Runtime cannot be had, which is not kept,
/// as Microsoft's CPU-only runtime fetched meanwhile brings the CPU.
pub fn providers() -> &'static [Provider] {
    static PROVIDERS: std::sync::OnceLock<Vec<Provider>> = std::sync::OnceLock::new();
    if greycard_ai::runtime::status().is_err() {
        return &[];
    }
    PROVIDERS.get_or_init(Provider::available)
}

/// Whether the providers here include WebGPU, which the generative
/// remove runs on and nothing else: [`providers`], or what a test set
/// with [`set_test_webgpu`].
pub fn on_webgpu() -> bool {
    #[cfg(any(test, feature = "testing"))]
    if let Some(on) = TEST_WEBGPU.with(|t| t.get()) {
        return on;
    }
    providers().contains(&Provider::WebGpu)
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static TEST_WEBGPU: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// [`on_webgpu`]'s answer on this thread, for a test; `None` asks the
/// providers again.
#[cfg(any(test, feature = "testing"))]
pub fn set_test_webgpu(on: Option<bool>) {
    TEST_WEBGPU.with(|t| t.set(on));
}

/// Whether the GPU the models run on shares the machine's memory
/// (`runtime::webgpu_unified`), false when it is not known.
pub fn unified() -> bool {
    #[cfg(any(test, feature = "testing"))]
    if let Some(on) = TEST_UNIFIED.with(|t| t.get()) {
        return on;
    }
    greycard_ai::runtime::webgpu_unified().unwrap_or(false)
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static TEST_UNIFIED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// [`unified`]'s answer on this thread, for a test; `None` asks the
/// adapter.
#[cfg(any(test, feature = "testing"))]
pub fn set_test_unified(on: Option<bool>) {
    TEST_UNIFIED.with(|t| t.set(on));
}

/// The bytes of the card the generative remove would run on, where it
/// is a card of its own whose memory is known
/// (`runtime::webgpu_memory`): none on unified memory, which keeps its
/// own bounds (§421), without WebGPU, on a CPU adapter or one of no
/// known type, or where the backend will not say; and none until the
/// adapter has been asked for elsewhere (the models probe, a model's
/// load), so the window's thread never waits on its probe: not known is
/// not short. In a test, what it set with [`set_test_card`], else none,
/// never the machine's own card.
pub fn card_memory() -> Option<u64> {
    #[cfg(any(test, feature = "testing"))]
    return TEST_CARD.with(|t| t.get()).flatten();
    #[allow(unreachable_code)]
    if !on_webgpu() {
        return None;
    }
    greycard_ai::runtime::webgpu_card_memory_if_probed()
}

/// [`card_memory`] for a thread that may wait on the adapter's probe
/// (the worker's): asked, not only read once asked elsewhere, so the
/// first removal of a launch, or of a headless export that never
/// probes, is said for the card it is.
fn card_memory_probed() -> Option<u64> {
    if !on_webgpu() {
        return None;
    }
    greycard_ai::runtime::webgpu_card_memory()
}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    static TEST_CARD: std::cell::Cell<Option<Option<u64>>> = const { std::cell::Cell::new(None) };
}

/// [`card_memory`]'s answer on this thread, for a test; `None` is
/// none.
#[cfg(any(test, feature = "testing"))]
pub fn set_test_card(card: Option<Option<u64>>) {
    TEST_CARD.with(|t| t.set(card));
}

/// Whether the Subject original is on record as having failed on
/// WebGPU, for the adapter and build this launch would use — asked,
/// and cached, once: the first call opens a `wgpu::Instance` and
/// blocks on `request_adapter` to name it, and every one after reads
/// and parses `providers.json`, neither of which belongs on the
/// render path, where a Subject want still waiting asks this every
/// frame. The record cannot change under a launch already up:
/// `runtime::open` is its only writer, and it never turns a WebGPU
/// failure for this file back into a success mid-launch.
pub fn subject_original_failed_on_webgpu(store: &Store) -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| greycard_ai::subject::original_failed_on_webgpu(store, providers()))
}

/// The model a shape needs, if any. For Subject that depends on the
/// providers, on what `store` already has, on which models cannot be
/// had this session (`unavailable`: declined, or their fetch failed),
/// and on whether the original is on record as having failed on
/// WebGPU (`greycard_ai::subject::model_for`); so the store the worker
/// loads from is the one to pass.
pub fn model_for(
    shape: &Shape,
    store: Option<&Store>,
    unavailable: &[&str],
) -> Option<&'static Model> {
    model_with(
        shape,
        |m| store.is_some_and(|s| s.have(m)),
        providers(),
        unavailable,
        |_| store.is_some_and(subject_original_failed_on_webgpu),
    )
}

/// `model_for` with what the store has, the providers, and whether the
/// original is on record as failing on WebGPU, given rather than read
/// from a real store: for a test.
pub fn model_with(
    shape: &Shape,
    have: impl Fn(&Model) -> bool,
    providers: &[Provider],
    unavailable: &[&str],
    original_failed_on_webgpu: impl Fn(&Model) -> bool,
) -> Option<&'static Model> {
    match shape {
        // A Background runs the same Subject model and takes one
        // minus its matte; it waits on, and is offered, the same file.
        Shape::Subject {} | Shape::Background {} => Some(greycard_ai::subject::pick(
            providers.contains(&Provider::WebGpu),
            have,
            |m| unavailable.contains(&m.id),
            original_failed_on_webgpu,
        )),
        Shape::Sky { .. } => Some(sky_model(have, unavailable)),
        // The sky's prior alone: its labels are the outline.
        Shape::Landscape { .. } if !shape.is_unknown() => Some(&SKY),
        Shape::Object { .. } => Some(&SAM),
        Shape::Part { .. } => Some(&SAM3),
        _ => None,
    }
}

/// What to do for a learned shape wanted: ask the worker for it, offer
/// its model, or wait (the sheet is busy, or every model it could use
/// was declined).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Ask(&'static greycard_ai::Model),
    /// A model to offer, and whether it is the Subject rewrite offered
    /// in place of an original already in the store — a card that
    /// original falls back to the CPU on — rather than a first-ever
    /// Subject offer, which the sheet's note reads differently.
    Offer(&'static greycard_ai::Model, bool),
    Wait,
}

/// The step for `shape`, given what the store has, the providers, the
/// models declined this session, those whose fetch failed, whether the
/// sheet is free for an offer, and whether the Subject original is on
/// record as having failed on WebGPU. A Subject whose WebGPU file was
/// declined or could not be fetched is offered the original. A model
/// whose fetch failed is not offered again this session, as one
/// declined is not: offline, the sheet would come straight back.
pub fn step(
    shape: &Shape,
    have: impl Fn(&greycard_ai::Model) -> bool + Copy,
    providers: &[greycard_ai::Provider],
    declined: &[&'static str],
    failed: &[&'static str],
    sheet_free: bool,
    original_failed_on_webgpu: impl Fn(&greycard_ai::Model) -> bool,
) -> Option<Step> {
    let unavailable: Vec<&str> = declined.iter().chain(failed).copied().collect();
    let model = model_with(
        shape,
        have,
        providers,
        &unavailable,
        original_failed_on_webgpu,
    )?;
    Some(if have(model) {
        Step::Ask(model)
    } else if sheet_free && !declined.contains(&model.id) && !failed.contains(&model.id) {
        // The store already has the Subject original: this offer is
        // the rewrite standing in for it, not the first Subject offer.
        Step::Offer(
            model,
            model.id == greycard_ai::SUBJECT_WEBGPU.id && have(&greycard_ai::SUBJECT),
        )
    } else if matches!(shape, Shape::Subject {} | Shape::Background {})
        && have(&greycard_ai::SUBJECT)
    {
        // Nothing to offer right now (another sheet is up, a fetch is
        // running, or this model was already declined or failed this
        // session), but the store's own original runs: use it rather
        // than wait on an offer that is not coming this round. A
        // Background wants exactly what a Subject want does — the
        // same model, the same fallback — so it is treated as one
        // here too. For either want alone: an Object or a Sky asked
        // for with the Subject model would load the wrong file.
        Step::Ask(&greycard_ai::SUBJECT)
    } else {
        Step::Wait
    })
}

/// The Subject model to offer beside a Whole person being asked for:
/// its edge is Subject's matte (`sam3::find`), but the People run never
/// waits on it. `may_offer`: the sheet is free, the Whole person was
/// just asked for from the People menu (not loaded or pasted), and the
/// edge offer was not declined this session. None for any other shape,
/// without `may_offer`, with either Subject file in the store, or with
/// the file `step` would pick for a Subject declined or failed to fetch
/// this session.
pub fn edge_offer(
    shape: &Shape,
    have: impl Fn(&greycard_ai::Model) -> bool + Copy,
    providers: &[greycard_ai::Provider],
    declined: &[&'static str],
    failed: &[&'static str],
    may_offer: bool,
    original_failed_on_webgpu: impl Fn(&greycard_ai::Model) -> bool,
) -> Option<&'static greycard_ai::Model> {
    let sheet_free = may_offer;
    if !takes_subject_edge(shape)
        || !may_offer
        || have(&greycard_ai::SUBJECT)
        || have(&greycard_ai::SUBJECT_WEBGPU)
    {
        return None;
    }
    match step(
        &Shape::Subject {},
        have,
        providers,
        declined,
        failed,
        sheet_free,
        original_failed_on_webgpu,
    )? {
        Step::Offer(model, _) => Some(model),
        _ => None,
    }
}

/// The model a run with nobody to offer a fetch to uses for `shape`:
/// the file [`step`] asks for with the session's `declined` and no
/// sheet free, so an export and the window pick the same; else the one
/// [`model_for`] names, for the report of what the frame went without.
pub fn unattended(
    shape: &Shape,
    store: Option<&Store>,
    declined: &[&'static str],
) -> Option<&'static Model> {
    unattended_with(
        shape,
        |m| store.is_some_and(|s| s.have(m)),
        providers(),
        declined,
        |_| store.is_some_and(subject_original_failed_on_webgpu),
    )
}

/// [`unattended`] with what the store has, the providers, and whether
/// the original is on record as failing on WebGPU given rather than
/// read from a real store: for a test.
pub fn unattended_with(
    shape: &Shape,
    have: impl Fn(&Model) -> bool + Copy,
    providers: &[Provider],
    declined: &[&'static str],
    original_failed_on_webgpu: impl Fn(&Model) -> bool + Copy,
) -> Option<&'static Model> {
    match step(
        shape,
        have,
        providers,
        declined,
        &[],
        false,
        original_failed_on_webgpu,
    )? {
        Step::Ask(model) => Some(model),
        Step::Offer(..) | Step::Wait => {
            model_with(shape, have, providers, declined, original_failed_on_webgpu)
        }
    }
}

/// The model a Sky shape waits on: the prior first, then SAM for the
/// outline; with both in the store, the prior (asked for by name, the
/// worker loads SAM beside it). SAM declined or not to be fetched this
/// session leaves the prior alone, whose own labels are then the
/// outline: a coarser sky, never a false one, since the gate is the
/// prior's.
fn sky_model(have: impl Fn(&Model) -> bool, unavailable: &[&str]) -> &'static Model {
    if have(&SKY) && !have(&SAM) && !unavailable.contains(&SAM.id) {
        &SAM
    } else {
        &SKY
    }
}

/// The found sky Foliage is cut by, by base develop and raster size,
/// and what the status line said of it: a Sky with no picks too.
type SkyCut = (
    (u64, (usize, usize)),
    Option<greycard_ai::Mask>,
    Option<String>,
);

/// The crops' encodings kept beside the preview's, each about 117 MB:
/// two faces and their four eyes, so Iris then Eyebrows then Lips on a
/// couple costs only decodes.
const PART_CROPS: usize = 6;

/// A Background's own raster from the Subject matte it shares: one
/// minus every byte. Any other shape reads it unchanged; the CPU
/// reference the composed Background is checked against.
fn background_or_not(shape: &Shape, base: &[u8]) -> Vec<u8> {
    if matches!(shape, Shape::Background {}) {
        base.iter().map(|&v| 255 - v).collect()
    } else {
        base.to_vec()
    }
}

pub use crate::stages::prompted;

/// Fetch `model`'s file for Core ML in the background where it has one
/// the store lacks. A Mac that had a denoiser tier before its fp16 file
/// was published gets it with no sheet, the tier's license having been
/// shown when the tier came, from the same place under the same
/// license; [`Ai::denoiser`] loads the tier again once it is here.
/// Asked at every develop while the tier waits for it, so it starts at
/// most one fetch a tier at a time, leaves a file another thread is
/// fetching to that one, and gives up on a tier for the process only
/// once a fetch of its own has failed.
fn fetch_core_ml_quietly(store: &Store, model: &'static Model) {
    // The tiers being fetched here, and those whose fetch failed.
    static STATE: std::sync::Mutex<(Vec<&'static str>, Vec<&'static str>)> =
        std::sync::Mutex::new((Vec::new(), Vec::new()));
    let state = || STATE.lock().unwrap_or_else(|e| e.into_inner());
    if greycard_ai::registry::core_ml_file(model).is_none() || store.core_ml_path(model).is_some() {
        return;
    }
    {
        let mut s = state();
        if s.0.contains(&model.id) || s.1.contains(&model.id) {
            return;
        }
        s.0.push(model.id);
    }
    /// Takes the tier off the list under way however its fetch ends.
    struct Done(&'static str, bool);
    impl Drop for Done {
        fn drop(&mut self) {
            let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
            s.0.retain(|id| *id != self.0);
            // A panic is a failure too, or one that always happens
            // would start a thread at every develop.
            if self.1 || std::thread::panicking() {
                s.1.push(self.0);
            }
        }
    }
    let store = store.clone();
    let spawned = std::thread::Builder::new()
        .name("core-ml-fetch".into())
        .spawn(move || {
            let mut done = Done(model.id, false);
            let result = store.fetch_core_ml(model, |_| {});
            match (result, store.core_ml_path(model).is_some()) {
                (_, true) => log::info!(
                    "{}: its file for Core ML arrived; the tier loads it at its next develop",
                    model.name
                ),
                (Err(e), false) => {
                    log::info!("{}: its file for Core ML did not arrive: {e}", model.name);
                    done.1 = true;
                }
                // Another thread has it under way; asked again later.
                (Ok(()), false) => {}
            }
        });
    if let Err(e) = spawned {
        state().0.retain(|id| *id != model.id);
        log::info!(
            "{}: no thread to fetch its file for Core ML: {e}",
            model.name
        );
    }
}

/// The long side of the preview the models see.
const PREVIEW: u32 = 2048;

pub struct Ai {
    store: Option<Store>,
    /// The open file, for the disk cache's key.
    file: Option<PathBuf>,
    providers: Vec<Provider>,
    subject: Option<Subject>,
    sam: Option<Sam>,
    sky: Option<Sky>,
    /// The sky's prior and the landscape's groups of base develop
    /// `stamp`, from one model run and one labeling, kept like the
    /// embedding: a pick on a Sky shape decodes again without running
    /// it, and the Landscape button, a Sky and every Landscape shape on
    /// the picture share it.
    scene: Option<(u64, Scene, Provider)>,
    /// The found sky Foliage is cut by, of base develop `stamp` at a
    /// raster size, or none where the sky's gate finds no sky.
    sky_cut: Option<SkyCut>,
    /// What the last Sky raster took, stage by stage.
    pub last_sky: Option<SkyReport>,
    fill: Option<Fill>,
    /// The generative remove, loaded at the first Fill patch it makes.
    remove: Option<Remove>,
    /// More fills of the open picture are queued behind the next one:
    /// the remove keeps its UNet open across them ([`Ai::fills_queued`]).
    fills_queued: bool,
    /// The learned denoiser loaded, by its model's id: one at a time,
    /// since each holds the GPU's memory.
    denoiser: Option<(&'static str, Denoiser)>,
    /// The tier of that denoiser when it was loaded without its fp16
    /// file for Core ML, which the store may yet fetch: loaded again
    /// once the file is here ([`Ai::denoiser`]).
    awaiting_fp16: Option<&'static str>,
    /// Fills made, by patch id and seed, with the patch, the window,
    /// the picture's print, and the engine each was made for: a seed's
    /// fill is kept when "Another" moves the patch on to the next, so
    /// undo is free. Each
    /// is kept on disk too (`fill_path`), so opening the file again
    /// does not run the model again; a failure is kept here alone.
    fills: HashMap<(u64, u32), FillMade>,
    /// Fills whose model ran and failed or was refused, by patch id and
    /// seed, with the patch, the window and why: not run again for the
    /// open file, where each develop would wait on it again, until the
    /// patch or its seed changes.
    failed: HashMap<(u64, u32), (Patch, Window, String)>,
    /// Each Fill patch's window and depth ([`window_for`]), by patch
    /// id, with the patch and the picture's size they were found for:
    /// finding them reads the whole region, which a stroke across the
    /// frame makes about the frame.
    windows: HashMap<u64, (Patch, (usize, usize), Window, f32)>,
    /// Each fill refused for the card's memory, by patch id, with the
    /// patch, its window, the card's free memory then, and why: neither
    /// counted ahead nor asked of the card again by a develop
    /// ([`Ai::still_refused`]) until something changes, so a fill the
    /// card stays short of does not bring a fill round, and a
    /// give-back, on every develop.
    refused: HashMap<u64, (Patch, Window, u64, u64, String)>,
    /// The preview of base develop `stamp`, and its luma.
    preview: Option<(u64, Rgb8, Vec<f32>)>,
    embedding: Option<(u64, Embedding)>,
    /// The People model and its phrase table, loaded on the first part.
    sam3: Option<Sam3>,
    presets: Option<Presets>,
    /// The People model's encodings of base develop `stamp`: the
    /// preview's and its crops', dropped with the preview.
    encodings: Encodings,
    /// The people on the preview of base develop `stamp`, found once
    /// for every part and pick made on it.
    people: Option<(u64, Vec<sam3::Person>)>,
    /// What the People model sees of base develop `stamp` made at
    /// `turn`: the picture under no edit at all and turned back to the
    /// camera's own orientation, and its luma. Look-neutral and
    /// unturned, so a person's signature does not move with the
    /// exposure, the look or a turn (`PartsView`).
    parts_view: Option<PartsView>,
    /// The open file's content hash (greycard-library's), the name a
    /// Part's person records the picture they were picked on by.
    picture: Option<(PathBuf, Option<String>)>,
    /// What has been made, by component and the base's turn, with the
    /// shape it was made for and the print of the base it was made
    /// from ([`crate::mask_base::print`]).
    cache: HashMap<(Key, u8), (Shape, Print, Arc<Raster>)>,
    /// The print of the base the raster being made or read is of, set
    /// by each way in with its base: its size names the disk slot, and
    /// its hash the raster in it.
    print: Print,
    /// What has been made again on the retouched picture, by component
    /// and the base's turn, with the shape and the print of the patches
    /// it was made under ([`Retouched::print_over`]), and the base's
    /// print.
    retouched: HashMap<(Key, u8), Remade>,
    /// What the open frame's panel was last given for each component:
    /// the shape, the model asked with, and the print of the Fill
    /// patches over it, none when none is, and the print of the base it
    /// was made from. A develop whose retouch or base changes either
    /// print asks for it again ([`Ai::to_ask_again`]).
    given: HashMap<Key, Given>,
    /// The components whose second run on the retouched picture failed
    /// and were given the base's raster: tried again when a model
    /// arrives ([`Ai::retry_fallbacks`]), since the failure may be one.
    fell_back: std::collections::HashSet<Key>,
    /// The print a raster is being made again under: hashed into its
    /// disk slot, so it is kept apart from the base's.
    salt: Option<u64>,
    /// The retouched picture's kept work while the base's is in the
    /// fields, and the other way round while a shape is made again.
    other_side: Side,
    /// Whether made rasters and fills are kept on disk (`cached_path`,
    /// `fill_path`); never for a test that runs the models.
    disk: bool,
    /// Whether a denoiser tier's file for Core ML is fetched in the
    /// background when the store lacks it ([`fetch_core_ml_quietly`]);
    /// never in a test build.
    quiet_fetch: bool,
    /// The Subject matte at the raster's size, of base develop
    /// `stamp`, and the provider that made it: run once and read by a
    /// Background sharing it, so a mask with both costs the model one
    /// call, not two, whichever shape asks for it first. The provider
    /// is carried along so a hit here still says where its run
    /// happened and still turns Show mask on, as an actual run does.
    subject_matte: Option<(u64, Vec<u8>, Provider)>,
    /// A Whole person keeps SAM's edge whatever the store has: for a
    /// test to set the two edges side by side.
    #[cfg(any(test, feature = "testing"))]
    pub sams_edge_only: bool,
    /// How many times the sky model has run for a scene: for a test
    /// that the Landscape button, a Sky and the Landscape shapes share
    /// one run.
    #[cfg(any(test, feature = "testing"))]
    pub scene_runs: usize,
    /// A Fill made with no model, every value this one, by LaMa's
    /// name: for a test of when the fills are made. Below nothing, each
    /// fails as a model that will not load fails, not remembered.
    #[cfg(any(test, feature = "testing"))]
    pub fill_by_test: Option<f32>,
    /// The card's free memory as a test has it, in bytes, in place of
    /// the backend's answer; `None` leaves it not known.
    #[cfg(any(test, feature = "testing"))]
    pub free_memory_by_test: Option<u64>,
    /// What the card gets back when a test's sessions are dropped, as
    /// the test has it; counted as sessions held until then.
    #[cfg(any(test, feature = "testing"))]
    pub freed_by_test: u64,
    /// Whether the remove's UNet is held open, as a test has it.
    #[cfg(any(test, feature = "testing"))]
    pub unet_held_by_test: Option<bool>,
    /// The card's whole memory, as a test has it ([`Ai::card_whole`]).
    #[cfg(any(test, feature = "testing"))]
    pub card_by_test: Option<Option<u64>>,
    /// The last run refused for the card's memory since it was taken
    /// ([`Ai::take_short`]): an export asks again on it, and fails its
    /// item when it stays short.
    short: Option<String>,
    /// The need and the free figure, bytes, of the last removal refused
    /// for the card's memory, taken with the refusal: what the window
    /// says of it ([`Ai::take_short_figures`]).
    short_figures: Option<(u64, u64)>,
    /// A fill's model ran since the card was last given back
    /// ([`give_back_card`]).
    fills_ran: bool,
    /// A fill was refused for the card's memory since the shorts were
    /// cleared ([`Ai::take_fill_short`]).
    fill_short: bool,
    /// A learned mask was, likewise ([`Ai::take_mask_short`]), and ran
    /// on the CPU for it.
    mask_short: bool,
    /// What the worker lets go of the card when a learned mask or a
    /// removal is short of it ([`Ai::set_card_release`]).
    card_release: Option<CardRelease>,
    /// Told before a learned mask's run on the CPU for the card's
    /// memory ([`Ai::set_on_cpu`]).
    on_cpu: Option<OnCpu>,
    /// This job's learned masks run on the CPU: the card was short of
    /// them after it was given back and the engine let go of what it
    /// could ([`Ai::mask_room`]). Cleared with the shorts.
    mask_cpu: bool,
    /// Which of the learned masks' sessions were loaded on the CPU for
    /// the card's memory: dropped when the card has the room again, so
    /// the next load is the card's. Set at each load, cleared where the
    /// sessions go, and carried with a lent one.
    cpu_chosen: CpuChosen,
    /// A learned mask short of the card may run on the CPU rather than
    /// be refused ([`Ai::set_cpu_fallback`]).
    cpu_fallback: bool,
    /// How an export or a merge waits on a card that is short; none
    /// for the window's jobs ([`Ai::set_patience`]).
    patience: Option<Patience>,
    /// A patient walk met a short it gave up on: the rest of it does
    /// not wait again.
    hit_short: bool,
    /// A patient walk was refused a run for the card's memory, no
    /// fallback taking it ([`Ai::short_failed`]).
    short_failed: bool,
    /// The tiling the learned denoiser last loaded ran at, by tier
    /// ([`Ai::denoiser_tiling`]).
    denoiser_tiling: Option<(&'static str, greycard_ai::denoise::Tiling)>,
}

/// A raster made again on the retouched picture: the shape, the print
/// of the Fill patches it was made under, the base's print, and the
/// raster.
type Remade = (Shape, u64, Print, Arc<Raster>);

/// What the panel was given of a component: the shape, the model asked
/// with, the print of the Fill patches over it, and the base's print.
type Given = (Shape, Option<&'static Model>, Option<u64>, Print);

/// A base's picture with its retouch applied: the retouch as the
/// develop applied it (sources chosen), the picture, and the Fill
/// patches in it left as they were (a model missing, failed, or wanting
/// a GPU), by id.
#[derive(Clone, Copy)]
pub struct Retouched<'a> {
    pub retouch: &'a Retouch,
    pub image: &'a WorkingImage,
    pub unmade: &'a [u64],
}

/// How much of a learned raster counts as the shape where a patch
/// meets it: an eighth, above the faint haze a matte leaves around.
const MEETS: u8 = 32;

impl Retouched<'_> {
    /// The print of the Fill patches that meet `raster` and how many
    /// there are; none when none do, or when one of them is not made
    /// (the picture there is still the base's).
    pub fn print_over(&self, raster: &Raster) -> Option<(u64, usize)> {
        let under = overlapping(self.retouch, raster);
        if under.is_empty() || under.iter().any(|p| self.unmade.contains(&p.id)) {
            return None;
        }
        Some((print_of(&under), under.len()))
    }

    /// A print of the whole picture: every patch as applied and the
    /// fills left unmade, for the stamp the models' kept work on it
    /// goes by.
    fn picture_print(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let all: Vec<&Patch> = self.retouch.patches.iter().collect();
        (self.retouch.enabled, print_of(&all), self.unmade).hash(&mut h);
        h.finish()
    }
}

/// The Fill patches of `retouch` whose hole meets `raster`'s shape. A
/// Heal or a Clone keeps what is under it (a spot, a hair, a blemish
/// gone), so the shape stands; a Fill takes it out. A patch's points
/// are in units of the picture's width, as the raster's texels are, so
/// its region is found on the raster's own grid.
pub fn overlapping<'a>(retouch: &'a Retouch, raster: &Raster) -> Vec<&'a Patch> {
    if retouch.is_empty() {
        return Vec::new();
    }
    let data = raster.data();
    let w = raster.width;
    retouch
        .patches
        .iter()
        .filter(|p| p.method == greycard_edit::retouch::Method::Fill)
        .filter(|p| {
            let Some(r) = p.region(w, raster.height) else {
                return false;
            };
            (0..r.height).any(|y| {
                (0..r.width).any(|x| {
                    r.cover[y * r.width + x] > 0.0
                        && data
                            .get((r.y + y) * w + r.x + x)
                            .is_some_and(|&v| v >= MEETS)
                })
            })
        })
        .collect()
}

/// A hash of `patches` in every field: their places, sizes, sources,
/// methods, and fill choices and seeds.
pub fn print_of(patches: &[&Patch]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in patches {
        serde_json::to_string(p).unwrap_or_default().hash(&mut h);
    }
    h.finish()
}

/// What the models keep of one picture, by its stamp: the preview, the
/// Object model's embedding, the sky's scene and cut, the Subject
/// matte, and the people. The base's and the retouched picture's are
/// kept apart, so a shape asked on one does not throw the other's away.
#[derive(Default)]
struct Side {
    preview: Option<(u64, Rgb8, Vec<f32>)>,
    embedding: Option<(u64, Embedding)>,
    scene: Option<(u64, Scene, Provider)>,
    sky_cut: Option<SkyCut>,
    subject_matte: Option<(u64, Vec<u8>, Provider)>,
    people: Option<(u64, Vec<sam3::Person>)>,
    parts_view: Option<PartsView>,
}

/// Why the learned denoiser is not to be had.
#[derive(Debug)]
pub enum NoDenoiser {
    /// Its model is not in the store.
    Missing,
    /// It would not load.
    Failed(String),
}

/// A raster made, and how.
pub struct Made {
    pub raster: Arc<Raster>,
    /// None when it came from the cache.
    pub provider: Option<Provider>,
    pub seconds: f64,
    /// What the status line says in place of the usual: a Sky shape
    /// on a frame with no sky, whose raster is empty; a Part whose
    /// person is not settled.
    pub note: Option<String>,
    /// A Part made empty because its person is not settled on this
    /// picture: asked about, or not on it.
    pub part: Option<Unresolved>,
}

/// Why a Part's raster is empty.
#[derive(Debug, Clone)]
pub enum Unresolved {
    /// Which person is it? The people on the picture to pick from, the
    /// one to highlight, and why it asks.
    Ask(Ask),
    /// No one is on this picture.
    Nobody,
}

impl Unresolved {
    /// What an export's log says of it: a part not sure of its person
    /// needs one picked; one whose person is not found, whoever else
    /// is on the picture, has no one here.
    pub fn left_out(&self) -> &'static str {
        match self {
            Unresolved::Ask(Ask {
                why: Asking::Unsure,
                ..
            }) => PART_ASKS,
            Unresolved::Ask(Ask {
                why: Asking::SomeoneElse,
                ..
            })
            | Unresolved::Nobody => PART_NOBODY,
        }
    }
}

/// The people a Part could be of on a picture, for the panel to pick
/// one from, the guess to highlight (never a pick), and why it asks.
#[derive(Debug, Clone, Default)]
pub struct Ask {
    pub people: Vec<Candidate>,
    pub guess: Option<usize>,
    pub why: Asking,
}

/// Why a Part asks which person it is of.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Asking {
    /// More than one could be the one, or the one here could be: the
    /// likeliest the guess, when there is one.
    #[default]
    Unsure,
    /// Its person is not on this picture, and whoever is here is
    /// someone else: never a guess, but one of them can be picked to
    /// have it.
    SomeoneElse,
}

impl Ask {
    /// The status line while it asks.
    pub fn words(&self) -> &'static str {
        match self.why {
            Asking::Unsure => "which person? click one, or All people",
            Asking::SomeoneElse => {
                "this mask was made for someone else: click a person to use it for them, \
                 or All people"
            }
        }
    }
}

/// What became of a person picked on one frame, looked for on another
/// frame a paste or a sync took the same People shape to ("use this
/// person on all").
#[derive(Debug, Clone, PartialEq)]
pub enum PersonOn {
    /// The shape's own person is decided alone here already: it is
    /// left as it is.
    Settled,
    /// The person picked is found here: this frame's own pick of them,
    /// as a click on their face here would store it.
    Found(greycard_edit::mask::Person),
    /// Not found here, nor told apart from the others: left asking.
    NotFound,
}

/// A person on a picture as the panel shows and picks them.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// The face's box, x0, y0, x1, y1, in the masks' units.
    pub face: [f32; 4],
    /// The face's center, in the masks' units.
    pub at: [f32; 2],
    pub signature: greycard_edit::mask::Signature,
    /// Their body, over the picture in its fractions; none for a face
    /// no body went with.
    pub body: Option<Arc<greycard_ai::Mask>>,
    /// The picture's content hash, for the shape to record.
    pub picture: Option<String>,
}

impl Candidate {
    /// A person the model found on the unturned view, on the picture
    /// as it stands: turned `turn` quarters clockwise, `aspect` its
    /// height over its width.
    fn of(p: &sam3::Person, turn: u8, aspect: f32, picture: &Option<String>) -> Self {
        let a = turn_frac(turn, [p.face[0], p.face[1]]);
        let b = turn_frac(turn, [p.face[2], p.face[3]]);
        let c = turn_frac(
            turn,
            [(p.face[0] + p.face[2]) / 2.0, (p.face[1] + p.face[3]) / 2.0],
        );
        Self {
            face: [
                a[0].min(b[0]),
                a[1].min(b[1]) * aspect,
                a[0].max(b[0]),
                a[1].max(b[1]) * aspect,
            ],
            at: [c[0], c[1] * aspect],
            signature: greycard_edit::mask::Signature {
                kind: p.signature.kind().name().to_string(),
                values: p.signature.values().to_vec(),
            },
            body: p.body.as_ref().map(|m| Arc::new(rotate_mask(m, turn))),
            picture: picture.clone(),
        }
    }

    /// The shape's person, as picked here.
    pub fn person(&self) -> greycard_edit::mask::Person {
        greycard_edit::mask::Person {
            signature: self.signature.clone(),
            at: self.at,
            picture: self.picture.clone(),
        }
    }
}

/// Which of `people` is the face at `at` on the picture a person was
/// picked on: the nearest within about its own face's width, all in
/// the masks' units. A turn moves `at` with the shape, and nothing done
/// to the picture's colors moves it at all.
pub fn at_face(people: &[Candidate], at: [f32; 2]) -> Option<usize> {
    let gap = |p: &Candidate| (p.at[0] - at[0]).hypot(p.at[1] - at[1]);
    people
        .iter()
        .enumerate()
        .filter(|(_, p)| gap(p) <= (p.face[2] - p.face[0]).max(0.01))
        .min_by(|a, b| gap(a.1).total_cmp(&gap(b.1)))
        .map(|(i, _)| i)
}

/// A point in fractions of the camera's own picture, on the picture
/// turned `turn` quarters clockwise.
pub fn turn_frac(turn: u8, [x, y]: [f32; 2]) -> [f32; 2] {
    match turn % 4 {
        1 => [1.0 - y, x],
        2 => [1.0 - x, 1.0 - y],
        3 => [y, 1.0 - x],
        _ => [x, y],
    }
}

/// `data`, `w` by `h` of `ch` values a pixel, turned `turn` quarters
/// clockwise, and its size after.
fn rotate<T: Copy>(data: &[T], w: usize, h: usize, ch: usize, turn: u8) -> (Vec<T>, usize, usize) {
    let turn = turn % 4;
    if turn == 0 {
        return (data.to_vec(), w, h);
    }
    let (nw, nh) = if turn % 2 == 1 { (h, w) } else { (w, h) };
    let mut out = data.to_vec();
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match turn {
                1 => (h - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (y, w - 1 - x),
            };
            let (from, to) = ((y * w + x) * ch, (ny * nw + nx) * ch);
            out[to..to + ch].copy_from_slice(&data[from..from + ch]);
        }
    }
    (out, nw, nh)
}

fn rotate_rgb(img: &Rgb8, turn: u8) -> Rgb8 {
    let (data, w, h) = rotate(&img.data, img.width, img.height, 3, turn);
    Rgb8::new(w, h, data)
}

fn rotate_mask(m: &greycard_ai::Mask, turn: u8) -> greycard_ai::Mask {
    let (data, w, h) = rotate(&m.data, m.width, m.height, 1, turn);
    greycard_ai::Mask::new(w, h, data)
}

/// A rectangle of the camera's own picture, `size` its pixels, as the
/// same pixels on the picture turned `turn` quarters clockwise.
fn turn_rect(turn: u8, (w, h): (usize, usize), r: sam3::Rect) -> sam3::Rect {
    let (x0, y0, x1, y1) = (r.x0, r.y0, r.x1, r.y1);
    let (x0, y0, x1, y1) = match turn % 4 {
        1 => (h - y1, x0, h - y0, x1),
        2 => (w - x1, h - y1, w - x0, h - y0),
        3 => (y0, w - x1, y1, w - x0),
        _ => (x0, y0, x1, y1),
    };
    sam3::Rect { x0, y0, x1, y1 }
}

/// What the People model sees of a base: see `Ai::parts_view`.
struct PartsView {
    stamp: u64,
    turn: u8,
    preview: Rgb8,
    luma: Vec<f32>,
}

/// The edit the People model's pictures are rendered under: none, so
/// the exposure, the curves and the look a picture is given are not in
/// what a person is known by.
fn neutral() -> Edit {
    Edit::default()
}

/// Which of `people` a click at `at` (the masks' units, `aspect` the
/// picture's height over its width) picks: the one whose face box is
/// under it, else the one whose body is (the highest there, the
/// smaller body on a tie, so a child held up is the child), else none.
pub fn picked(people: &[Candidate], at: (f32, f32), aspect: f32) -> Option<usize> {
    let inside = |b: [f32; 4]| at.0 >= b[0] && at.0 <= b[2] && at.1 >= b[1] && at.1 <= b[3];
    if let Some(i) = people.iter().position(|p| inside(p.face)) {
        return Some(i);
    }
    let (fx, fy) = (at.0, at.1 / aspect.max(1e-6));
    if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
        return None;
    }
    let area = |m: &greycard_ai::Mask| m.data.iter().filter(|&&v| v > sam3::CUT).count();
    people
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let m = p.body.as_ref()?;
            let x = ((fx * m.width as f32) as usize).min(m.width - 1);
            let y = ((fy * m.height as f32) as usize).min(m.height - 1);
            let v = m.at(x, y);
            (v > sam3::CUT).then_some((i, v, area(m)))
        })
        .min_by(|a, b| {
            if (a.1 - b.1).abs() < 0.01 {
                a.2.cmp(&b.2)
            } else {
                b.1.total_cmp(&a.1)
            }
        })
        .map(|(i, _, _)| i)
}

/// What an export's log says of a Part asking which person it is of.
pub const PART_ASKS: &str = "a People mask needs a person picked";
/// And of a Part whose person is not on the picture.
pub const PART_NOBODY: &str = "a People mask's person isn't in this picture";

/// Which of a frame's left-out items (an export's) are
/// People shapes written empty: (asking, nobody).
pub fn parts_left_out(items: &[String]) -> (usize, usize) {
    let n = |what: &str| items.iter().filter(|i| i.contains(what)).count();
    (n(PART_ASKS), n(PART_NOBODY))
}

/// What a result line adds for a frame, or (`per_frame`) a set's frames, written
/// with a People mask empty: asking which person, or with nobody, as
/// `parts_left_out` counted them over the set; nothing for none.
pub fn parts_words(per_frame: bool, (asks, nobody): (usize, usize)) -> String {
    let lead = |n: usize| {
        if per_frame {
            format!(", {}: ", crate::queue::frames(n))
        } else {
            ", ".to_string()
        }
    };
    let mut s = String::new();
    if asks > 0 {
        s.push_str(&lead(asks));
        s.push_str(PART_ASKS);
    }
    if nobody > 0 {
        s.push_str(&lead(nobody));
        s.push_str(PART_NOBODY);
    }
    s
}

/// The status line while a part is made on the CPU, before the run:
/// a part takes seconds there, an eye's or a mouth's about thirty (an
/// Iris took 29.8 s and Lips 8.7 s in a release build).
pub fn part_on_cpu(route: &greycard_edit::mask::Route) -> &'static str {
    match route {
        greycard_edit::mask::Route::Eye | greycard_edit::mask::Route::Mouth => {
            "finding it on the CPU: an eye or mouth part takes about thirty seconds"
        }
        _ => "finding it on the CPU: this takes several seconds",
    }
}

/// Whether the People model will run on the CPU on this machine: no
/// WebGPU offered.
pub fn parts_on_cpu() -> bool {
    !providers().contains(&Provider::WebGpu)
}

fn route_of(route: &greycard_edit::mask::Route) -> Result<sam3::Route, String> {
    match route {
        greycard_edit::mask::Route::Whole => Ok(sam3::Route::Whole),
        greycard_edit::mask::Route::Eye => Ok(sam3::Route::Eye),
        greycard_edit::mask::Route::Mouth => Ok(sam3::Route::Mouth),
        greycard_edit::mask::Route::Other(name) => {
            Err(format!("the route \"{name}\" is not one this build knows"))
        }
    }
}

/// A Sky raster's making, stage by stage, in seconds.
#[derive(Debug, Clone, Default)]
pub struct SkyReport {
    /// The prior's provider, and SAM's (encoder) when it ran.
    pub prior_on: Option<Provider>,
    pub sam_on: Option<Provider>,
    /// Loading the prior, when this raster loaded it.
    pub load: f64,
    pub prior: f64,
    /// SAM's loading and embedding, when this raster paid for them.
    pub sam_load: f64,
    pub embed: f64,
    pub decodes: usize,
    pub stages: sky::Times,
    /// Bringing the matte to the raster's size.
    pub raster: f64,
    pub seeded: bool,
    pub no_sky: Option<sky::NoSky>,
    /// The outline the matte was made from, at the preview's size.
    pub outline: Option<greycard_ai::Mask>,
}

/// Whether a learned mask's raster can be had ([`Ai::raster_ready`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ready {
    /// In the disk cache.
    Cached,
    /// Not made yet; its model is in the store.
    ToMake,
    /// Not made, and its model, named, is not in the store.
    NoModel(&'static str),
}

/// Why a fill was not made: the patch is left as it was either way.
#[derive(Debug, Clone)]
pub enum NoFill {
    /// The model named is not in the store.
    Missing(&'static Model),
    /// A patch the generative remove made, on a machine without
    /// WebGPU, which the remove needs.
    NoGpu,
    /// It could not run.
    Failed(String),
    /// A removal on a card whose whole memory is under the remove's
    /// need ([`FillNotHere::Card`]): the need and the card, bytes.
    Card { need: u64, have: u64 },
    /// Its model was to run and the caller said not now: a develop
    /// shows its picture first and makes its fills after, one at a
    /// time.
    Later,
}

/// A fill made: the patch, window and engine it was made for, the
/// print of the picture under the window ([`window_print`]), and the
/// region's values.
type FillMade = (Patch, Window, WindowPrint, Engine, Vec<f32>);

/// Which model makes a Fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// LaMa, on any machine and any window, through its tiles.
    Lama,
    /// The generative remove, on WebGPU, a window up to
    /// [`greycard_ai::remove::FACTOR`] times
    /// [`greycard_ai::remove::MAX`] a side, taken down to it past it.
    Remove,
}

/// What a Fill's makers need, as this machine has it: LaMa, the
/// remove, the four-step remove's own files, WebGPU, and the card's
/// memory where it is a card of its own ([`card_memory`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FillHaves {
    pub fill: bool,
    pub remove: bool,
    pub fast: bool,
    pub webgpu: bool,
    pub card: Option<u64>,
}

/// Why a remove can never make a Fill on this machine
/// ([`FillHaves::why_not`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillNotHere {
    /// No WebGPU, which both removes run on.
    NoGpu,
    /// The whole card is under the remove's smallest need: the need and
    /// the card, bytes.
    Card { need: u64, have: u64 },
}

impl FillNotHere {
    /// In English, for the log and a test; the window says it in the
    /// user's language.
    pub fn words(self) -> String {
        match self {
            FillNotHere::NoGpu => "Needs a GPU with WebGPU".into(),
            FillNotHere::Card { need, have } => {
                let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
                format!(
                    "Needs {:.1} GiB VRAM, {:.1} available. Use Patch.",
                    gib(need),
                    gib(have)
                )
            }
        }
    }
}

impl FillHaves {
    /// The store's models and this machine's providers.
    pub fn of(store: Option<&Store>) -> Self {
        let has = |m: &Model| store.is_some_and(|s| s.have(m));
        Self {
            fill: has(&greycard_ai::FILL),
            remove: has(&REMOVE),
            fast: has(&REMOVE_FAST),
            webgpu: on_webgpu(),
            card: card_memory(),
        }
    }

    /// Why `choice` can never make a Fill on this machine, or None:
    /// both removes want WebGPU, and a card of its own at least the
    /// remove's smallest need ([`remove_needs`] at 512), its whole
    /// memory, read at start. A model not downloaded is not a reason
    /// ([`FillHaves::missing`]): it can be fetched.
    pub fn why_not(self, choice: FillChoice) -> Option<FillNotHere> {
        let need = remove_needs(512, false);
        match choice {
            FillChoice::Patch => None,
            _ if !self.webgpu => Some(FillNotHere::NoGpu),
            _ => match self.card {
                Some(have) if have < need => Some(FillNotHere::Card { need, have }),
                _ => None,
            },
        }
    }

    /// The model `choice` waits for, the first to fetch: Patch wants
    /// LaMa, Best the remove, Fast the remove and then its own files,
    /// which sit beside the remove's.
    pub fn missing(self, choice: FillChoice) -> Option<&'static Model> {
        match choice {
            FillChoice::Patch if !self.fill => Some(&greycard_ai::FILL),
            FillChoice::Best | FillChoice::Fast if !self.remove => Some(&REMOVE),
            FillChoice::Fast if !self.fast => Some(&REMOVE_FAST),
            _ => None,
        }
    }

    /// Whether `choice` can make a Fill here now.
    pub fn runs(self, choice: FillChoice) -> bool {
        self.why_not(choice).is_none() && self.missing(choice).is_none()
    }

    /// What a Fill drawn now is made by when `wanted` is asked: it, or
    /// where it cannot run the other remove, or LaMa. A patch drawn
    /// before a model came keeps what it was drawn with.
    pub fn drawn(self, wanted: FillChoice) -> FillChoice {
        let other = match wanted {
            FillChoice::Fast => FillChoice::Best,
            FillChoice::Best => FillChoice::Fast,
            FillChoice::Patch => FillChoice::Patch,
        };
        [wanted, other]
            .into_iter()
            .find(|&c| self.runs(c))
            .unwrap_or(FillChoice::Patch)
    }

    /// The choice before one is made: Fast where the GPU shares the
    /// machine's memory and Fast can run, since the full remove is a
    /// minute a fill there; Patch where neither remove can ever run
    /// (a card under the remove's need, or no WebGPU); Best otherwise.
    pub fn default_choice(self, unified: bool) -> FillChoice {
        if unified && self.runs(FillChoice::Fast) {
            FillChoice::Fast
        } else if self.why_not(FillChoice::Best).is_some() {
            FillChoice::Patch
        } else {
            FillChoice::Best
        }
    }
}

/// What this machine can run, as the refusals above read it: whether
/// ONNX Runtime can be had (why not, and the CPU-only runtime that
/// would bring it, where there is one), whether the models have WebGPU,
/// whether its shaders take 16-bit floats, and the card's memory. For a
/// list of every model, and a fetch of all of them, to say which would
/// never run here and why, in the words the refusals use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Here {
    /// Why the AI features are off, or None where they are on.
    pub off: Option<String>,
    /// The CPU-only runtime that would turn them on, where they are off
    /// and one can be fetched (`runtime::Unavailable::fetchable`).
    pub runtime: Option<&'static greycard_ai::Runtime>,
    pub webgpu: bool,
    /// As [`greycard_ai::runtime::webgpu_shader_f16`] answers.
    pub f16: Option<(bool, String)>,
    /// The card's memory and its name, as [`Ai::room`] reads the whole
    /// card (`runtime::webgpu_memory`); None where `room` never
    /// refuses, which is wherever the card's free memory cannot be read
    /// (`runtime::webgpu_free_memory`): shared memory, a CPU adapter, a
    /// backend that will not say. An integrated GPU's figure there is
    /// the firmware's carve-out, which is no reason to skip.
    pub card: Option<(u64, String)>,
}

/// Why a model would never run here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NeverHere {
    /// ONNX Runtime cannot be had: its reason.
    Off(String),
    /// The remove wants WebGPU.
    NoWebGpu,
    /// The remove wants 16-bit floats in shaders, which the card named
    /// has none of.
    NoF16(String),
    /// The remove's smallest run needs more than the whole card: the
    /// card, its bytes, and the need.
    ShortCard(String, u64, u64),
    /// The Subject file this machine does not use: the other runs, the
    /// GPU copy where there is WebGPU (true), else the original.
    OtherSubject(bool),
}

impl NeverHere {
    /// In the refusals' own words, for the log.
    pub fn words(&self) -> String {
        match self {
            NeverHere::Off(why) => why.clone(),
            NeverHere::NoWebGpu => FillNotHere::NoGpu.words(),
            NeverHere::NoF16(card) => {
                remove_f16_refusal(Some(&(false, card.clone()))).unwrap_or_default()
            }
            NeverHere::ShortCard(card, size, need) => {
                short_of(card, *size, *need, "a removal at 512").unwrap_or_default()
            }
            NeverHere::OtherSubject(true) => "its GPU copy runs here".into(),
            NeverHere::OtherSubject(false) => "the original runs here".into(),
        }
    }

    /// In a few words, for a line of a list: no card named, nothing
    /// longer than a line, and sizes in decimal GB as the list's are.
    pub fn short(&self) -> String {
        match self {
            NeverHere::Off(_) => "no ONNX Runtime here".into(),
            NeverHere::NoWebGpu => "needs a GPU with WebGPU".into(),
            NeverHere::NoF16(_) => "no 16-bit floats in shaders".into(),
            NeverHere::ShortCard(_, _, need) => {
                format!("needs {:.1} GB on the card", *need as f64 / 1e9)
            }
            other => other.words(),
        }
    }
}

/// The card's free memory now, bytes, for the log: none until the
/// WebGPU adapter has been asked for by something else ([`Here::now`]
/// on its own thread), so a develop never sets off the adapter's probe
/// on the worker, and none where the card does not say.
pub fn card_free_for_log() -> Option<u64> {
    greycard_ai::runtime::webgpu_free_memory_if_probed().map(|(free, _)| free)
}

impl Here {
    /// This machine. Blocks on the WebGPU adapter's probe the first
    /// time, so for a thread of its own, never the window's.
    pub fn now() -> Self {
        Self::probe()
    }

    fn probe() -> Self {
        let status = greycard_ai::runtime::status();
        let off = status.err().map(|why| why.reason());
        let runtime = status.err().and_then(|why| why.fetchable());
        let webgpu = off.is_none() && on_webgpu();
        let f16 = if webgpu {
            greycard_ai::runtime::webgpu_shader_f16()
        } else {
            None
        };
        let card = if webgpu && greycard_ai::runtime::webgpu_free_memory().is_some() {
            greycard_ai::runtime::webgpu_memory().map(|bytes| {
                let name = f16
                    .as_ref()
                    .map(|(_, n)| n.clone())
                    .or_else(greycard_ai::runtime::webgpu_name)
                    .unwrap_or_else(|| "the card".into());
                (bytes, name)
            })
        } else {
            None
        };
        Self {
            off,
            runtime,
            webgpu,
            f16,
            card,
        }
    }

    /// This machine once the CPU-only runtime it lacks has come: the
    /// CPU alone, as that runtime offers.
    pub fn with_runtime(&self) -> Self {
        Self::default()
    }

    /// Why `model` would never run here, or None. Every model wants
    /// ONNX Runtime; the generative remove and its four steps want
    /// WebGPU ([`FillHaves::why_not`]), 16-bit floats in shaders
    /// ([`remove_f16_refusal`]), and a card that holds their smallest
    /// run whole ([`Ai::room`] refuses one past the whole card); and of
    /// the two Subject files, only the one [`greycard_ai::subject::pick`]
    /// takes for a store with neither runs here. A model not
    /// downloaded is not a reason.
    pub fn why_not(&self, model: &Model) -> Option<NeverHere> {
        if let Some(off) = &self.off {
            return Some(NeverHere::Off(off.clone()));
        }
        if model.id == REMOVE.id || model.id == REMOVE_FAST.id {
            let haves = FillHaves {
                webgpu: self.webgpu,
                ..FillHaves::default()
            };
            if haves.why_not(FillChoice::Best).is_some() {
                return Some(NeverHere::NoWebGpu);
            }
            if let Some((false, card)) = &self.f16 {
                return Some(NeverHere::NoF16(card.clone()));
            }
            let need = remove_needs(512, false);
            if let Some((size, card)) = &self.card
                && need > *size
            {
                return Some(NeverHere::ShortCard(card.clone(), *size, need));
            }
            return None;
        }
        let subject = [&greycard_ai::SUBJECT, &greycard_ai::SUBJECT_WEBGPU];
        if subject.iter().any(|m| m.id == model.id) {
            let used = greycard_ai::subject::pick(self.webgpu, |_| false, |_| false, |_| false);
            if used.id != model.id {
                return Some(NeverHere::OtherSubject(self.webgpu));
            }
        }
        None
    }
}

/// The remove refused on a WebGPU adapter whose shaders take no 16-bit
/// floats (`f16` as [`greycard_ai::runtime::webgpu_shader_f16`]
/// answers), in words: the remove's models are fp16 through and
/// through. None where they take them, or no adapter answers.
pub fn remove_f16_refusal(f16: Option<&(bool, String)>) -> Option<String> {
    match f16 {
        Some((false, name)) => Some(format!(
            "the generative remove needs 16-bit floats in shaders, which {name} has none of"
        )),
        _ => None,
    }
}

/// The models Core ML has loaded in this process, Sky and the
/// denoiser's tiers, by id: their compile is in the cache, and
/// [`warm_core_ml`] has nothing to do for them.
static CORE_ML_LOADED: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

fn core_ml_loaded(id: &'static str) {
    let mut loaded = CORE_ML_LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if !loaded.contains(&id) {
        loaded.push(id);
    }
}

/// Core ML's compile of `model` made now, on a Mac, so the model's
/// first use is as quick as its second: Core ML compiles a graph at a
/// session's first load and keeps the compile in the user's cache.
/// Only the models Core ML runs, Sky and the denoiser's tiers (the
/// others stay off it, `Provider::without_core_ml`); a tier's fp16
/// file, which Core ML opens in its place, is fetched first. One warm
/// at a time, so four models landing do not compile at once, and none
/// for a model already loaded in this process, the worker's or a warm
/// before. Anywhere else, or without Core ML on offer, nothing (a
/// tier's fp16 file still comes, `fetch_core_ml_quietly`). Blocks for
/// the load, so for a thread of its own. A failure is logged and costs
/// nothing: the first use compiles, as it did.
pub fn warm_core_ml(store: &Store, model: &'static Model) {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if !cfg!(target_os = "macos") || !providers().contains(&Provider::CoreMl) {
        return;
    }
    let tier = greycard_ai::tier_of(model).is_some();
    if model.id != SKY.id && !tier {
        return;
    }
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if CORE_ML_LOADED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&model.id)
    {
        log::info!("{}: loaded already; no compile to make ahead", model.name);
        return;
    }
    let started = Instant::now();
    let loaded = if tier {
        if greycard_ai::registry::core_ml_file(model).is_some()
            && !greycard_ai::denoise::fp32_refused()
            && let Err(e) = store.fetch_core_ml(model, |_| {})
        {
            log::info!("{}: its file for Core ML did not arrive: {e}", model.name);
        }
        Denoiser::from_store(store, model, providers())
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else {
        Sky::load(store, providers())
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    match loaded {
        Ok(()) => {
            core_ml_loaded(model.id);
            log::info!(
                "{}: Core ML's compile made ahead, {:.1} s",
                model.name,
                started.elapsed().as_secs_f64()
            );
        }
        Err(e) => log::info!("{}: no compile made ahead: {e}", model.name),
    }
}

/// The sampler a remove's Fill runs: four steps for a Fast one.
pub fn steps_for(patch: &Patch) -> greycard_ai::remove::Steps {
    match patch.fill_choice() {
        FillChoice::Fast => greycard_ai::remove::Steps::Lcm4,
        _ => greycard_ai::remove::Steps::Euler25,
    }
}

/// Bytes in a tenth of a GiB, the precision the peaks below were read to.
const TENTH_GIB: u64 = (1 << 30) / 10;

/// What a removal asks of the card at its peak, at 512 and at 1024,
/// and the same with its UNet held open for a queue: measured with
/// `remove_bench` on an RTX 5070 Ti, three removals of one window, the
/// peak over the desktop, 5.8 and 6.6 GiB one at a time, 6.6 and 9.9
/// with the UNet held between them.
const REMOVE_PEAK_512: u64 = 58 * TENTH_GIB;
const REMOVE_PEAK_1024: u64 = 66 * TENTH_GIB;
const REMOVE_HELD_PEAK_512: u64 = 66 * TENTH_GIB;
const REMOVE_HELD_PEAK_1024: u64 = 99 * TENTH_GIB;

/// What the learned denoiser's run asks of the card, by its tile: the
/// best tier, the largest, held 3.86 GiB after a run at 1024 and 1.49
/// at 512, of which its weights are 0.05, already on the card when it
/// is loaded; measured on an RTX 5070 Ti under ONNX Runtime's WebGPU
/// provider, a second run asking nothing more. At 384 and 256 the
/// card's least free figure fell 0.58 and 0.49 GiB below the loaded
/// tier's (`denoise_tiles`, a 24 MP frame, one tile a process); a
/// margin of 96 on every side is most of a small tile's run.
const DENOISE_PEAK_1024: u64 = 39 * TENTH_GIB;
const DENOISE_PEAK_512: u64 = 15 * TENTH_GIB;
const DENOISE_PEAK_384: u64 = 6 * TENTH_GIB;
const DENOISE_PEAK_256: u64 = 5 * TENTH_GIB;
const DENOISE_WEIGHTS: u64 = TENTH_GIB / 2;

/// The smaller tiles the learned denoiser falls to, in order, on a card
/// short of its own tile, those under it: at 384 and 256 its answer was
/// the one at 512 (bitwise, on one card, the 5070 Ti, a 24 MP frame),
/// the network's reach inside the margin, at 1.15 and 1.5 times the
/// time of 512; 512 against 1024 likewise (§431).
pub const DENOISE_SMALLER_TILES: [usize; 3] = [512, 384, 256];

/// A learned mask's model, for what it asks of the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskModel {
    Sky,
    Subject,
    Sam,
    People,
}

/// What a learned mask's model asks of the card to load and run twice
/// on the editor's preview, as the guard reads the card: measured on
/// an RTX 5070 Ti under ONNX Runtime's WebGPU provider with
/// `mask_peak` (greycard-ai's example), each model in a process of
/// its own, and rounded up a tenth. The Subject model (BiRefNet at
/// fp16) is the largest at 5.9 GiB; People (SAM 3) 4.4, SAM 2.5, Sky
/// 0.8. One figure for all four would refuse the Sky on a card with
/// room for it, so each asks for its own.
const fn mask_peak(model: MaskModel) -> u64 {
    let tenths = match model {
        MaskModel::Sky => 9,
        MaskModel::Subject => 60,
        MaskModel::Sam => 25,
        MaskModel::People => 45,
    };
    tenths * TENTH_GIB
}

/// Room kept over a peak: the budget is the driver's estimate, and the
/// window's own textures can grow while a model runs (a develop
/// landing).
const CARD_MARGIN: u64 = greycard_ai::remove::CARD_MARGIN;

/// What the window's own develop may take of the card without asking,
/// over [`CARD_MARGIN`]: its frame's textures and the GPU ops' working
/// sets on the largest frame open. The models' sessions are kept after
/// a fill while the card has this free, and given back under it
/// ([`give_back_card`]); every model run asks for its own room.
/// Measured from the worker's log line of the card's free memory
/// before and after a develop lands, on an RTX 5070 Ti: 1.15 GiB at
/// 24 MP, 4.0 GiB at 102 MP, the largest sensor shipping. Half a GiB
/// over the 102 MP figure.
const KEEP_FLOOR: u64 = 45 * TENTH_GIB;

/// How many times an export or a merge asks the card for a model it is
/// short of, and how long it waits between: three asks, ten seconds in
/// all, the card given back once before them ([`Ai::room`]); another
/// program may let the memory go. A window's job does not wait.
pub const PATIENT_TRIES: u32 = 3;
pub const PATIENT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How an export or a merge waits on a card that is short: how many
/// asks in all, how long between, and whether the export was canceled
/// meanwhile.
#[derive(Clone)]
pub struct Patience {
    pub tries: u32,
    pub wait: std::time::Duration,
    pub canceled: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Patience {
    /// `tries` asks, [`PATIENT_WAIT`] apart, never canceled: a merge's,
    /// which has no cancel of its own to look at.
    pub fn of(tries: u32) -> Self {
        Self {
            tries,
            wait: PATIENT_WAIT,
            canceled: Arc::new(|| false),
        }
    }
}

/// Held for reading by each merge running on its own thread, with a
/// learned denoiser of its own on ONNX Runtime's one WebGPU context, and
/// for writing across a give-back. That context and its buffer cache
/// have no lock of their own (only the factory that hands it out does),
/// so a session dropped while a merge's run takes from the cache would
/// race it: no give-back while a merge runs, and none can begin one
/// halfway through a drop.
#[cfg_attr(test, allow(dead_code))]
static GIVE_BACK: std::sync::RwLock<()> = std::sync::RwLock::new(());

// A unit test's merges are its own thread's, so one test's merge does
// not hold another's give-back.
#[cfg(test)]
thread_local! {
    static MERGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A merge running, while this lives ([`GIVE_BACK`]).
pub struct MergeRunning {
    #[cfg(not(test))]
    _read: std::sync::RwLockReadGuard<'static, ()>,
}

impl MergeRunning {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        #[cfg(test)]
        MERGES.with(|m| m.set(m.get() + 1));
        Self {
            #[cfg(not(test))]
            _read: merge_hold(&GIVE_BACK),
        }
    }
}

#[cfg(test)]
impl Drop for MergeRunning {
    fn drop(&mut self) {
        MERGES.with(|m| m.set(m.get() - 1));
    }
}

/// The give-back's hold, kept across the drop: none while a merge runs.
fn give_back_lock() -> Option<Box<dyn std::any::Any>> {
    #[cfg(not(test))]
    {
        give_back_hold(&GIVE_BACK).map(|w| Box::new(w) as Box<dyn std::any::Any>)
    }
    #[cfg(test)]
    {
        (MERGES.with(|m| m.get()) == 0).then(|| Box::new(()) as Box<dyn std::any::Any>)
    }
}

/// A merge's hold on `lock`: shared, and taken whatever a panic left.
#[cfg_attr(test, allow(dead_code))]
fn merge_hold(lock: &'static std::sync::RwLock<()>) -> std::sync::RwLockReadGuard<'static, ()> {
    lock.read().unwrap_or_else(|e| e.into_inner())
}

/// A give-back's hold on `lock`: none while a merge holds it, and taken
/// whatever a panic left.
#[cfg_attr(test, allow(dead_code))]
fn give_back_hold(
    lock: &'static std::sync::RwLock<()>,
) -> Option<std::sync::RwLockWriteGuard<'static, ()>> {
    match lock.try_write() {
        Ok(w) => Some(w),
        Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

/// Whether a merge runs now, for [`give_back_due`]'s answer.
fn merge_running() -> bool {
    give_back_lock().is_none()
}

/// The card's free memory a removal at `size` needs before it is
/// started, `held` when its UNet is open on the card already.
pub fn remove_needs(size: usize, held: bool) -> u64 {
    let unet = greycard_ai::remove::UNET_ON_CARD;
    let peak = match (size > 512, held) {
        (false, false) => REMOVE_PEAK_512,
        (true, false) => REMOVE_PEAK_1024,
        (false, true) => REMOVE_HELD_PEAK_512 - unet,
        (true, true) => REMOVE_HELD_PEAK_1024 - unet,
    };
    peak + CARD_MARGIN
}

/// The card's free memory the learned denoiser's run at `tile` needs,
/// loaded.
pub fn denoise_needs(tile: usize) -> u64 {
    let peak = if tile <= 256 {
        DENOISE_PEAK_256
    } else if tile <= 384 {
        DENOISE_PEAK_384
    } else if tile <= 512 {
        DENOISE_PEAK_512
    } else {
        // Past 1024 (`GREYCARD_DENOISE_TILE`), by the tile's area.
        let area = (tile as f64 / 1024.0).powi(2).max(1.0);
        (DENOISE_PEAK_1024 as f64 * area) as u64
    };
    peak - DENOISE_WEIGHTS + CARD_MARGIN
}

/// The card's free memory a learned mask's model needs.
pub fn mask_needs(model: MaskModel) -> u64 {
    mask_peak(model) + CARD_MARGIN
}

/// Why `what` is not begun on `card` with `free` bytes free when it
/// needs `need`, or `None` when it fits: ONNX Runtime does not fail
/// cleanly when the card runs out of memory, the driver takes the
/// process down, so a run the card cannot hold is not begun.
pub fn short_of(card: &str, free: u64, need: u64, what: &str) -> Option<String> {
    let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
    (free < need).then(|| {
        format!(
            "{card} has {:.1} GiB free, short of the {:.1} GiB {what} needs; another program may be using it",
            gib(free),
            gib(need)
        )
    })
}

/// `bytes` in GiB for the log: to a tenth, or to a hundredth where a
/// tenth would round away what is compared (a need of 1.95 GiB is not
/// said as 1.9). The window's refusals keep the tenth ([`short_of`]).
pub fn gib_said(bytes: u64) -> String {
    let gib = bytes as f64 / (1u64 << 30) as f64;
    let tenth = format!("{gib:.1}");
    if (tenth.parse::<f64>().unwrap_or(gib) - gib).abs() < 0.005 {
        tenth
    } else {
        format!("{gib:.2}")
    }
}

/// [`short_of`] for a removal at `size`.
pub fn short_of_memory(card: &str, free: u64, size: usize, held: bool) -> Option<String> {
    short_of(
        card,
        free,
        remove_needs(size, held),
        &format!("a removal at {size}"),
    )
}

/// What the worker lets go of the card for a learned mask or a removal
/// short of it, each named with its bytes: the engine's working sets
/// that a develop makes again ([`Ai::set_card_release`]).
pub type CardRelease = Box<dyn FnMut() -> Vec<(&'static str, u64)> + Send>;

/// Told before a learned mask runs on the CPU for the card's memory
/// ([`Ai::set_on_cpu`]).
pub type OnCpu = Box<dyn Fn() + Send>;

/// Which of the learned masks' sessions were loaded on the CPU for the
/// card's memory ([`Ai::mask_room`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CpuChosen {
    sam: bool,
    sam3: bool,
    subject: bool,
    sky: bool,
}

/// What was let go for a learned mask, in words for the log: "the
/// sharpen's working set (0.8 GiB) and the local contrast's (0.9 GiB)".
fn freed_words(freed: &[(&'static str, u64)]) -> String {
    let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
    let items: Vec<String> = freed
        .iter()
        .map(|(what, bytes)| format!("{what} ({:.1} GiB)", gib(*bytes)))
        .collect();
    match items.as_slice() {
        [] => "nothing".into(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// What the log says of a learned mask's second ask of the card, after
/// the first found it short (`why`) and the engine let go of `freed`:
/// the free memory before and after, and where the mask runs.
fn second_ask_said(
    why: &str,
    freed: &[(&'static str, u64)],
    before: Option<u64>,
    after: Option<u64>,
    fits: bool,
) -> String {
    let gib = |b: Option<u64>| b.unwrap_or(0) as f64 / (1u64 << 30) as f64;
    match (fits, freed.is_empty()) {
        (true, _) => format!(
            "a learned mask: begun on the card once the engine let go of {}, {:.1} GiB free before and {:.1} after",
            freed_words(freed),
            gib(before),
            gib(after)
        ),
        (false, true) => {
            format!("{why}; nothing of the engine's to let go: the mask runs on the CPU")
        }
        (false, false) => format!(
            "{why}; the engine let go of {}, {:.1} GiB free before and {:.1} after, short still: the mask runs on the CPU",
            freed_words(freed),
            gib(before),
            gib(after)
        ),
    }
}

/// What the log says of a removal's or the learned denoiser's ask of
/// the card once the engine let go of `freed`, the ask before it short
/// (`why`): the free memory before and after, and whether it begins.
fn release_said(
    what: &str,
    why: &str,
    freed: &[(&'static str, u64)],
    before: Option<u64>,
    after: Option<u64>,
    fits: bool,
) -> String {
    let gib = |b: Option<u64>| b.unwrap_or(0) as f64 / (1u64 << 30) as f64;
    let (freed, before, after) = (freed_words(freed), gib(before), gib(after));
    if fits {
        format!(
            "{what}: begun once the engine let go of {freed}, {before:.1} GiB free before and {after:.1} after"
        )
    } else {
        format!(
            "{why}; the engine let go of {freed}, {before:.1} GiB free before and {after:.1} after, short still"
        )
    }
}

/// What the log and a refusal call the learned denoiser at `tile`.
fn what_at(tile: usize) -> String {
    format!("the learned denoiser at a tile of {tile}")
}

/// Where the learned denoiser runs, as the card has the memory for it
/// ([`Ai::denoise_room_for`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenoiseOn {
    /// Where it was loaded, at its own tiling.
    Card,
    /// On the card at this smaller tile.
    Tile(usize),
}

/// A model's run refused for the card's memory ([`short_of`]): an
/// export or a merge fails its item on it, where a develop stands in.
#[derive(Debug, Clone)]
pub struct CardShort(pub String);

impl std::fmt::Display for CardShort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CardShort {}

#[cfg(any(test, feature = "testing"))]
thread_local! {
    /// The card's free memory as a test on this thread has it, read in
    /// turn, the last read again; `None` leaves the card asked.
    static FREE_BY_TEST: std::cell::RefCell<Option<Vec<Option<u64>>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    /// How many patient waits on a short card this thread has slept, for
    /// a test that counts them where each is no time at all.
    static WAITS_BY_TEST: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many patient waits on a short card this thread has slept.
#[cfg(test)]
pub fn waits_by_test() -> usize {
    WAITS_BY_TEST.with(|w| w.get())
}

/// Sets the card's free memory this thread's reads by [`card_free_now`]
/// get, one a read, the last again: for a test.
#[cfg(any(test, feature = "testing"))]
pub fn set_free_by_test(figures: Option<Vec<Option<u64>>>) {
    FREE_BY_TEST.with(|f| *f.borrow_mut() = figures);
}

/// The card's free memory now, and its name
/// ([`greycard_ai::runtime::webgpu_free_memory`]), or as a test has it.
pub fn card_free_now() -> Option<(u64, String)> {
    #[cfg(any(test, feature = "testing"))]
    if let Some(next) = FREE_BY_TEST.with(|f| {
        let mut f = f.borrow_mut();
        let figures = f.as_mut()?;
        let next = if figures.len() > 1 {
            figures.remove(0)
        } else {
            figures.first().copied().flatten()
        };
        Some(next)
    }) {
        return next.map(|b| (b, "the card".into()));
    }
    let (free, _) = greycard_ai::runtime::webgpu_free_memory()?;
    let card = greycard_ai::runtime::webgpu_name().unwrap_or_else(|| "the card".into());
    Some((free, card))
}

/// The card given back once a fill's model has run in any of `ais`,
/// the worker's models, or a UNet was opened ahead, when the card has
/// less than [`KEEP_FLOOR`] and [`CARD_MARGIN`] free with them held, or
/// at once when `force`d after a short: every ONNX Runtime session they hold
/// dropped, the UNet held between removals let go, and the host's
/// heap trimmed. ONNX Runtime's WebGPU provider keeps one context a
/// process for all its sessions, and the buffers a session lets go
/// stay in that context's cache while any other session lives: beside
/// a kept denoiser, letting a held UNet go gave none of the process's
/// 7.75 GiB back, and the denoiser's next run made 2.9 GiB more beside
/// them. With no session left the context goes, and with it
/// everything. Called on the worker between jobs, off the queue's
/// lock, never mid-walk. Not while a merge runs (its turn comes after),
/// nor where the card's free memory is not known: on unified memory
/// the models stay as they always have. True when anything was let go.
pub fn give_back_card(ais: &mut [&mut Ai], force: bool) -> bool {
    let Some(_hold) = give_back_lock() else {
        return false;
    };
    let ran = ais
        .iter_mut()
        .fold(false, |ran, ai| ai.take_fills_ran() | ran)
        | greycard_ai::remove::take_prepared();
    if !(ran || force) {
        return false;
    }
    let Some((before, card)) = ais.first().and_then(|a| a.free_memory()) else {
        return false;
    };
    // With room for the window's develop, the sessions are kept: a
    // model loaded again costs the next fill or mask seconds.
    if !force && before >= KEEP_FLOOR + CARD_MARGIN {
        tracing::info!(
            "{card}: the models' sessions kept, {:.1} GiB free",
            before as f64 / (1u64 << 30) as f64
        );
        return false;
    }
    let held = ais.iter().any(|a| a.holds_sessions());
    for ai in ais.iter_mut() {
        ai.drop_sessions();
    }
    let unet = greycard_ai::remove::let_go_unet();
    if !(held || unet) {
        return false;
    }
    greycard_ai::runtime::trim_heap();
    for ai in ais.iter_mut() {
        ai.refused.clear();
    }
    if let Some((after, card)) = ais.first().and_then(|a| a.free_memory()) {
        tracing::info!(
            "{card}: the models' sessions let go, the card's {:.1} GiB given back",
            after.saturating_sub(before) as f64 / (1u64 << 30) as f64
        );
    }
    true
}

/// Whether [`give_back_card`] is to be asked now: a fill's model ran,
/// no merge runs, and the card's free memory is known. It reads the
/// card again and keeps the sessions while there is room. Asked under
/// the queue's lock, so the give-back itself waits until it is let go.
pub fn give_back_due(ais: &[&Ai]) -> bool {
    ais.iter().any(|a| a.fills_ran())
        && !merge_running()
        && ais.first().and_then(|a| a.free_memory()).is_some()
}

/// Whether the learned denoiser, loaded, may run at `tile` on the card
/// now, refused with why when it is short: the last word before its
/// run, after [`Ai::denoise_room_for`] has given the card back and
/// waited. Where the card's free memory is not known it may.
pub fn denoise_room(tile: usize) -> Result<(), CardShort> {
    let what = format!("the learned denoiser at a tile of {tile}");
    match card_free_now().and_then(|(free, card)| short_of(&card, free, denoise_needs(tile), &what))
    {
        Some(why) => Err(CardShort(why)),
        None => Ok(()),
    }
}

/// The engine a Fill patch's `window`, at the picture's own pixels, is
/// made by. A patch drawn for the remove ([`marks_remove`]) is made by
/// it when the window fits it taken down by no more than
/// [`greycard_ai::remove::FACTOR`] ([`greycard_ai::remove::factor_for`]);
/// a bigger one is LaMa's through its tiles. Where the remove cannot
/// run (its model missing or declined, no WebGPU), its patch is left
/// as it was rather than made by LaMa, so an export there is not
/// another picture than the window's.
pub fn engine_for(
    marked: bool,
    have_remove: bool,
    providers: &[Provider],
    window: Window,
) -> Result<Engine, NoFill> {
    if !marked || greycard_ai::remove::factor_for(window.width, window.height).is_none() {
        Ok(Engine::Lama)
    } else if !providers.contains(&Provider::WebGpu) {
        Err(NoFill::NoGpu)
    } else if have_remove {
        Ok(Engine::Remove)
    } else {
        Err(NoFill::Missing(&REMOVE))
    }
}

/// The noise seed of a patch's variation: its id and its seed, so two
/// patches at seed 0 do not share their noise.
fn remove_seed(patch: &Patch) -> u64 {
    (patch.id << 32) ^ u64::from(patch.seed)
}

/// Whether a kept fill was made for this patch as it is now: its
/// opacity is applied afterwards and does not count, and its seed is
/// the cache's key.
fn same_fill(a: &Patch, b: &Patch) -> bool {
    a.points == b.points
        && a.radius == b.radius
        && a.feather == b.feather
        && a.method == b.method
        && a.remove == b.remove
        && a.fast == b.fast
}

/// What [`regrain`] measured at the hole's mean level, in the frame
/// round it and in the fill before and after: the luma's and the
/// chroma's standard deviations; the grain's shape round the hole and
/// in the fill before, and the widths the grain added was drawn at.
struct Regrained {
    level: f32,
    frame: [f32; 2],
    before: [f32; 2],
    after: [f32; 2],
    shape: [noise::GrainShape; 2],
    widths: [f32; 2],
}

/// The frame's own noise put under a fill the model made smooth:
/// `out` is the region's replacement in the picture's values, filled
/// in `window` of `image`. The noise is measured on the window's known
/// pixels, in the picture's values after the develop's own denoise, so
/// a denoised frame is given back only what it kept, and on its luma
/// and chroma axes, so its grain keeps the frame's color
/// ([`noise::PictureNoise`]), and drawn to make up the frame's grain at
/// both distances ([`noise::GrainShape`]); it is added under the
/// region's coverage less what the fill carries already, and the
/// retouch's blend feathers it with the coverage's own edge. Seeded
/// by `seed` and the place in the frame, so a fill made again is the
/// same. `None` when the frame measures no noise.
fn regrain(
    out: &mut [f32],
    region: &Region,
    window: Window,
    image: &WorkingImage,
    seed: u64,
) -> Option<Regrained> {
    let Window {
        x: wx,
        y: wy,
        width: ww,
        height: wh,
    } = window;
    let covered = |px: usize, py: usize| {
        (region.x..region.x + region.width).contains(&px)
            && (region.y..region.y + region.height).contains(&py)
            && region.cover[(py - region.y) * region.width + (px - region.x)] > 0.0
    };
    let mut values = Vec::with_capacity(ww * wh * 3);
    let mut known = Vec::with_capacity(ww * wh);
    for py in wy..wy + wh {
        let row = (py * image.width + wx) * 3;
        values.extend_from_slice(&image.data[row..row + ww * 3]);
        known.extend((wx..wx + ww).map(|px| !covered(px, py)));
    }
    let frame = noise::estimate_picture_noise(&values, ww, wh, &known);
    if frame.is_silent() {
        return None;
    }
    let (rw, rh) = (region.width, region.height);
    let hole: Vec<bool> = (0..rw * rh)
        .map(|i| {
            let (px, py) = (region.x + i % rw, region.y + i / rw);
            region.cover[i] > 0.0 && (wx..wx + ww).contains(&px) && (wy..wy + wh).contains(&py)
        })
        .collect();
    let present = noise::estimate_picture_noise(out, rw, rh, &hole);
    let mask: Vec<f32> = hole.iter().map(|&h| f32::from(u8::from(h))).collect();
    let widths =
        noise::add_picture_noise(out, rw, (region.x, region.y), &mask, &frame, &present, seed);
    let after = noise::estimate_picture_noise(out, rw, rh, &hole);
    let inside = hole.iter().filter(|&&h| h).count().max(1);
    let level = (0..rw * rh)
        .filter(|&i| hole[i])
        .map(|i| (out[i * 3] + out[i * 3 + 1] + out[i * 3 + 2]) / 3.0)
        .sum::<f32>()
        / inside as f32;
    let at = |m: &noise::PictureNoise| {
        let l = level.max(0.0);
        let chroma = (m.axes.variance(1, l) + m.axes.variance(2, l)) / 2.0;
        [m.luma_sigma(l), chroma.sqrt()]
    };
    Some(Regrained {
        level,
        frame: at(&frame),
        before: at(&present),
        after: at(&after),
        shape: [frame.shape.at(level), present.shape.at(level)],
        widths: [widths[0], widths[1]],
    })
}

/// Whether a fill made by `engine` at `scale` has the frame's noise
/// put back ([`regrain`]): LaMa's when it was taken down, the remove's
/// always.
fn regrains(engine: Engine, scale: f32) -> bool {
    engine == Engine::Remove || scale < 1.0
}

/// The window a region's fill is made in, in a picture of `width` ×
/// `height`, and how deep the hole is: round the box of what the
/// region covers, at the picture's own pixels
/// ([`greycard_ai::fill::window`]).
pub(crate) fn window_for(region: &Region, width: usize, height: usize) -> (Window, f32) {
    let hole: Vec<f32> = region
        .cover
        .iter()
        .map(|&c| if c > 0.0 { 1.0 } else { 0.0 })
        .collect();
    let depth = greycard_ai::fill::depth(&hole, region.width, region.height);
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..region.height {
        for x in 0..region.width {
            if region.cover[y * region.width + x] > 0.0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    let hole = if x0 < x1 {
        Window {
            x: region.x + x0,
            y: region.y + y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    } else {
        Window {
            x: region.x,
            y: region.y,
            width: region.width,
            height: region.height,
        }
    };
    (greycard_ai::fill::window(hole, depth, width, height), depth)
}

impl Ai {
    // Not `Default`: it reads the model store, which a default should
    // not go to disk for.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            store: Store::user().ok(),
            file: None,
            providers: Vec::new(),
            subject: None,
            sam: None,
            sky: None,
            scene: None,
            sky_cut: None,
            last_sky: None,
            fill: None,
            remove: None,
            fills_queued: false,
            denoiser: None,
            fills: HashMap::new(),
            failed: HashMap::new(),
            windows: HashMap::new(),
            refused: HashMap::new(),
            preview: None,
            embedding: None,
            sam3: None,
            presets: None,
            encodings: Encodings::new(PART_CROPS),
            people: None,
            parts_view: None,
            picture: None,
            cache: HashMap::new(),
            print: crate::mask_base::Print {
                hash: 0,
                size: crate::mask_base::Size::Full,
            },
            retouched: HashMap::new(),
            given: HashMap::new(),
            fell_back: std::collections::HashSet::new(),
            salt: None,
            other_side: Side::default(),
            awaiting_fp16: None,
            disk: true,
            // Never in a test build, whichever constructor it uses.
            quiet_fetch: !cfg!(any(test, feature = "testing")),
            subject_matte: None,
            #[cfg(any(test, feature = "testing"))]
            sams_edge_only: false,
            #[cfg(any(test, feature = "testing"))]
            scene_runs: 0,
            #[cfg(any(test, feature = "testing"))]
            fill_by_test: None,
            #[cfg(any(test, feature = "testing"))]
            free_memory_by_test: None,
            #[cfg(any(test, feature = "testing"))]
            freed_by_test: 0,
            #[cfg(any(test, feature = "testing"))]
            unet_held_by_test: None,
            #[cfg(any(test, feature = "testing"))]
            card_by_test: None,
            short: None,
            fills_ran: false,
            fill_short: false,
            mask_short: false,
            card_release: None,
            on_cpu: None,
            mask_cpu: false,
            cpu_chosen: CpuChosen::default(),
            cpu_fallback: false,
            patience: None,
            hit_short: false,
            short_failed: false,
            short_figures: None,
            denoiser_tiling: None,
        }
    }

    /// The sky prior of the last develop a Sky raster was made on.
    #[cfg(any(test, feature = "testing"))]
    pub fn sky_prior(&self) -> Option<&Prior> {
        self.scene.as_ref().map(|s| &s.1.prior)
    }

    /// Models from `store` on `providers` alone, and no file, so no
    /// disk cache: for a test that runs the models.
    #[cfg(any(test, feature = "testing"))]
    pub fn for_test(store: Store, providers: Vec<Provider>) -> Self {
        Self {
            store: Some(store),
            providers,
            disk: false,
            quiet_fetch: false,
            ..Self::new()
        }
    }

    /// Whether the learned denoiser `model` is there to be loaded: the
    /// runtime and the model in the store. Asked without loading it, so
    /// a develop that keeps the network's answer never pays for a load.
    pub fn denoiser_there(&self, model: &'static Model) -> Result<(), NoDenoiser> {
        // No runtime here: fetching the model would not help.
        greycard_ai::runtime::status().map_err(|why| NoDenoiser::Failed(why.to_string()))?;
        let store = self.store.as_ref().ok_or(NoDenoiser::Missing)?;
        if !store.have(model) {
            return Err(NoDenoiser::Missing);
        }
        Ok(())
    }

    /// The learned denoiser `model`, loaded the first time it is asked
    /// for; another tier replaces the one loaded.
    pub fn denoiser(&mut self, model: &'static Model) -> Result<&mut Denoiser, NoDenoiser> {
        self.denoiser_there(model)?;
        let store = self.store.clone().ok_or(NoDenoiser::Missing)?;
        if self.providers.is_empty() {
            self.providers = Provider::available();
        }
        // Loaded without its fp16 file, which has come since (from the
        // sheet's fetch, or the background one): loaded again, once.
        let arrived = self.awaiting_fp16 == Some(model.id) && store.core_ml_path(model).is_some();
        if arrived {
            tracing::info!("{}: its file for Core ML is here; loading it", model.name);
        }
        if arrived || self.denoiser.as_ref().is_none_or(|(id, _)| *id != model.id) {
            self.denoiser = None;
            self.awaiting_fp16 = None;
            // Asked before the load, which asks for itself: a file that
            // lands while the tier loads is then still awaited, and the
            // tier loads again at the next develop.
            let had = store.core_ml_path(model).is_some();
            let denoiser = Denoiser::from_store(&store, model, &self.providers)
                .map_err(|e| NoDenoiser::Failed(e.to_string()))?;
            core_ml_loaded(model.id);
            self.denoiser_tiling = Some((model.id, denoiser.tiling()));
            self.denoiser = Some((model.id, denoiser));
            let wants = self.providers.contains(&Provider::CoreMl)
                && greycard_ai::registry::core_ml_file(model).is_some()
                && !greycard_ai::denoise::fp32_refused();
            if wants && !had {
                self.awaiting_fp16 = Some(model.id);
            }
        }
        if self.quiet_fetch && self.awaiting_fp16 == Some(model.id) {
            fetch_core_ml_quietly(&store, model);
        }
        Ok(&mut self
            .denoiser
            .as_mut()
            .expect("a denoiser was just loaded")
            .1)
    }

    /// The print of the open file's base under `edit` at `turn`, made
    /// at `size` ([`crate::mask_base::print`]).
    pub fn base_print(&self, edit: &Edit, turn: u8, size: crate::mask_base::Size) -> Print {
        crate::mask_base::print(self.file.as_deref(), edit, turn, size)
    }

    /// Another file: nothing made so far applies.
    pub fn forget(&mut self, file: Option<PathBuf>) {
        self.file = file;
        self.cache.clear();
        self.retouched.clear();
        self.given.clear();
        self.fell_back.clear();
        self.other_side = Side::default();
        self.fills.clear();
        self.failed.clear();
        self.windows.clear();
        self.refused.clear();
        self.preview = None;
        self.embedding = None;
        self.scene = None;
        self.sky_cut = None;
        self.subject_matte = None;
        self.encodings.clear();
        self.people = None;
        self.parts_view = None;
        // Its hash read again too: a file written again under its name
        // (a merge made again) is another picture.
        self.picture = None;
    }

    /// No Part is live on the open picture: the People model, its
    /// encodings and its people go, about 2 GB with a picture's crops;
    /// the next Part loads them again.
    pub fn release_parts(&mut self) {
        if self.sam3.is_some() {
            tracing::debug!("the People model let go: no People shape on the picture");
        }
        self.sam3 = None;
        self.cpu_chosen.sam3 = false;
        self.presets = None;
        self.encodings.clear();
        self.people = None;
        self.parts_view = None;
        self.other_side.people = None;
        self.other_side.parts_view = None;
    }

    /// Whether more fills are queued behind the next one the remove
    /// makes: while they are, it keeps its UNet open between them,
    /// whatever its memory mode, at the sizes the machine's memory
    /// allows ([`greycard_ai::remove::queued_up_to`]). The last of a
    /// queue is made with `more` false and lets the UNet go after it.
    pub fn fills_queued(&mut self, more: bool) {
        self.fills_queued = more;
    }

    /// Whether a test makes the fills with no model ([`Ai::fill_by_test`]).
    fn fills_by_test(&self) -> bool {
        #[cfg(any(test, feature = "testing"))]
        {
            self.fill_by_test.is_some()
        }
        #[cfg(not(any(test, feature = "testing")))]
        {
            false
        }
    }

    /// A UNet held for the queue goes now, the queue going on: a walk of
    /// it is about to load another model (the learned denoiser of a
    /// base made again). The next fill opens it again.
    pub fn let_go_held_unet(&mut self) {
        if let Some(r) = self.remove.as_mut() {
            r.release_unet();
        }
    }

    /// A queue of fills is over or given up, for another develop or
    /// another model: a UNet held for it goes now, unless the memory
    /// mode keeps it anyway.
    pub fn release_fills(&mut self) {
        self.fills_queued = false;
        if let Some(r) = self.remove.as_mut() {
            r.release_unet();
        }
    }

    /// The base's kept work for the retouched picture's, or back: the
    /// People model's encodings are not kept for both, at about 2 GB,
    /// and go at each change of side.
    fn swap_side(&mut self) {
        let o = &mut self.other_side;
        std::mem::swap(&mut self.preview, &mut o.preview);
        std::mem::swap(&mut self.embedding, &mut o.embedding);
        std::mem::swap(&mut self.scene, &mut o.scene);
        std::mem::swap(&mut self.sky_cut, &mut o.sky_cut);
        std::mem::swap(&mut self.subject_matte, &mut o.subject_matte);
        std::mem::swap(&mut self.people, &mut o.people);
        std::mem::swap(&mut self.parts_view, &mut o.parts_view);
        self.encodings.clear();
    }

    /// The People model lent to `other`, or back from it: the export of
    /// a set's other frames runs its own `Ai` on the same thread, and
    /// has the editor's model rather than a second copy of it.
    pub fn lend_parts(&mut self, other: &mut Ai) {
        if self.sam3.is_some() {
            other.sam3 = self.sam3.take();
            other.presets = self.presets.take();
            other.cpu_chosen.sam3 = std::mem::take(&mut self.cpu_chosen.sam3);
        }
        // And the Subject model a Whole person's edge runs.
        if self.subject.is_some() {
            other.subject = self.subject.take();
            other.cpu_chosen.subject = std::mem::take(&mut self.cpu_chosen.subject);
        }
    }

    /// The open file's content hash, read once.
    fn picture_hash(&mut self) -> Option<String> {
        let file = self.file.clone()?;
        if self.picture.as_ref().is_none_or(|(f, _)| *f != file) {
            let hash = greycard_library::hash_file(&file).ok();
            self.picture = Some((file, hash));
        }
        self.picture.as_ref().and_then(|(_, h)| h.clone())
    }

    /// A Subject model just arrived in the store: drop the one
    /// loaded, if any, so the next Subject mask picks up the new
    /// file rather than reusing the session's first choice forever.
    /// The shared matte is that model's; a different one makes a
    /// different matte, so a Background waiting on it must not read
    /// what the old model made.
    pub fn forget_subject(&mut self) {
        self.subject = None;
        self.cpu_chosen.subject = false;
        self.subject_matte = None;
        self.other_side.subject_matte = None;
        // A Whole person made without Subject's edge is made again.
        self.cache.retain(|_, (s, _, _)| !takes_subject_edge(s));
        self.retouched
            .retain(|_, (s, _, _, _)| !takes_subject_edge(s));
    }

    /// Whether the fill model is in the store, so a fill not kept
    /// will be made rather than left.
    pub fn fill_available(&self) -> bool {
        self.store
            .as_ref()
            .is_some_and(|s| s.have(&greycard_ai::FILL))
    }

    /// Whether a Fill of `retouch` on `image` that is not kept could be
    /// made now, by either engine: a picture kept with fills left out
    /// is made again only then.
    /// `image` is a kept picture past the retouch, not what any fill
    /// was made from, so a fill in memory for the patch and window
    /// counts as kept whatever picture it was made from.
    pub fn could_fill_more(&mut self, retouch: &Retouch, image: &WorkingImage) -> bool {
        !self.unmade(retouch, image, true).is_empty()
    }

    /// How many Fills of `retouch` on `image` are not kept and could be
    /// made now, by either engine: what a develop has ahead of it,
    /// asked on the picture before the retouch. A fill whose window
    /// takes in an earlier patch's region was kept for the picture
    /// that patch left, so it may be counted and then found kept; the
    /// count can stop short of its total; the status line holds it
    /// at least at the fill being made.
    pub fn fills_unmade(&mut self, retouch: &Retouch, image: &WorkingImage) -> usize {
        self.unmade(retouch, image, false).len()
    }

    /// [`Ai::fills_unmade`]'s Fills, by patch id.
    pub fn fills_unmade_ids(&mut self, retouch: &Retouch, image: &WorkingImage) -> Vec<u64> {
        self.unmade(retouch, image, false)
    }

    /// The ids of the Fills of `retouch` on `image` not kept that could
    /// be made; `any_print` takes one in memory for the patch and
    /// window as kept whatever picture it was made from.
    fn unmade(&mut self, retouch: &Retouch, image: &WorkingImage, any_print: bool) -> Vec<u64> {
        retouch
            .patches
            .iter()
            .filter(|p| p.method == greycard_edit::retouch::Method::Fill)
            .filter(|p| {
                p.region(image.width, image.height).is_some_and(|region| {
                    let (window, _) = self.window_of(p, &region, image);
                    let in_memory = any_print
                        && self
                            .fills
                            .get(&(p.id, p.seed))
                            .is_some_and(|(kept, w, _, _, _)| same_fill(kept, p) && *w == window);
                    !in_memory
                        && self.never_on_card(p, window).is_none()
                        && self.still_refused(p, window).is_none()
                        && !self.fill_kept(p, &region, image)
                        && self.failed_before(p, &region, image).is_none()
                        && self.engine(p, &region, image).is_ok()
                })
            })
            .map(|p| p.id)
            .collect()
    }

    /// Why the patch's fill was refused for the card's memory, when it
    /// was as it is, in `window`, with no more free since and no
    /// give-back: a give-back, a patch or window changed, or more memory
    /// free lets it be asked again. An export, patient, always asks.
    fn still_refused(&mut self, patch: &Patch, window: Window) -> Option<String> {
        if self.patience.is_some() {
            return None;
        }
        let (p, w, at, _, why) = self.refused.get(&patch.id)?;
        let still = same_fill(p, patch)
            && *w == window
            && self.free_memory().is_some_and(|(free, _)| free <= *at);
        let why = still.then(|| why.clone());
        if why.is_none() {
            self.refused.remove(&patch.id);
        }
        why
    }

    /// Why the patch's fill failed when its model last ran on it as it
    /// is now, if it did.
    fn failed_before(
        &mut self,
        patch: &Patch,
        region: &Region,
        image: &WorkingImage,
    ) -> Option<String> {
        let (window, _) = self.window_of(patch, region, image);
        self.failed
            .get(&(patch.id, patch.seed))
            .filter(|(p, w, _)| same_fill(p, patch) && *w == window)
            .map(|(_, _, why)| why.clone())
    }

    /// The patch's fill, as it is now, failed with `why`.
    fn remember_failure(&mut self, patch: &Patch, window: Window, why: String) -> NoFill {
        self.failed
            .insert((patch.id, patch.seed), (patch.clone(), window, why.clone()));
        NoFill::Failed(why)
    }

    /// Whether the patch's fill is made already and kept, so asking
    /// for it costs nothing.
    pub fn fill_kept(&mut self, patch: &Patch, region: &Region, image: &WorkingImage) -> bool {
        let (window, _) = self.window_of(patch, region, image);
        let print = window_print(image, region, window);
        let size = (image.width, image.height);
        self.fill_in_memory(patch, window, &print).is_some()
            || self.fill_from_disk(patch, region, size, window, &print)
    }

    /// The patch's fill, as it is now, kept in memory: made for this
    /// patch, this window, and this picture under it within
    /// [`PRINT_TOLERANCE`], so a fill made under another white balance
    /// or noise setting is not served after the base changes, as it is
    /// not from disk, and one made from the same picture rounded
    /// otherwise is.
    fn fill_in_memory(
        &self,
        patch: &Patch,
        window: Window,
        print: &[[u16; 3]],
    ) -> Option<(Vec<f32>, Engine)> {
        self.fills
            .get(&(patch.id, patch.seed))
            .filter(|(p, w, pr, _, _)| {
                same_fill(p, patch)
                    && *w == window
                    && prints_agree(pr, print, PRINT_TOLERANCE, Compare::Every)
            })
            .map(|(_, _, _, engine, data)| (data.clone(), *engine))
    }

    /// The patch's fill read from its slot on disk into memory, if it
    /// is there as the patch is now and its print is `print`'s within
    /// [`PRINT_TOLERANCE`]; else from its slot at the version before,
    /// once.
    fn fill_from_disk(
        &mut self,
        patch: &Patch,
        region: &Region,
        size: (usize, usize),
        window: Window,
        print: &[[u16; 3]],
    ) -> bool {
        let Some(path) = self.fill_path(patch, region, size, window) else {
            return false;
        };
        let dims = (region.width, region.height);
        let read = read_slot(&path, size, window, dims, |stored| {
            prints_agree(stored, print, PRINT_TOLERANCE, Compare::Every)
        })
        .or_else(|| self.fill_from_hashed_slot(patch, region, size, window, print, &path));
        let Some((engine, stored, data)) = read else {
            return false;
        };
        tracing::info!(
            "fill {}: read from the fill cache, {}x{} made by {engine:?}",
            patch.name(),
            region.width,
            region.height
        );
        self.fills.insert(
            (patch.id, patch.seed),
            (patch.clone(), window, stored, engine, data),
        );
        true
    }

    /// The patch's fill from its slot at [`FILL_VERSION_HASHED`], named
    /// with `print` hashed in, kept again in the slot `path` with the
    /// print, and the old file let go once it is: a machine whose base
    /// did not move keeps its fills across the change of version.
    fn fill_from_hashed_slot(
        &self,
        patch: &Patch,
        region: &Region,
        size: (usize, usize),
        window: Window,
        print: &[[u16; 3]],
        path: &Path,
    ) -> Option<(Engine, WindowPrint, Vec<f32>)> {
        let old = self.fill_path_hashed(patch, region, size, window, print)?;
        let dims = (region.width, region.height);
        let (engine, _, data) = read_fill(&old, FILL_VERSION_HASHED, size, window, dims, |_| true)?;
        if write_slot(path, engine, size, window, dims, print, &data) {
            // Read meanwhile by another run, or held open on Windows: left.
            let _ = std::fs::remove_file(&old);
        }
        tracing::info!(
            "fill {}: kept again from the fill cache's last version",
            patch.name()
        );
        Some((engine, print.to_vec(), data))
    }

    /// Where the fills on disk live: beside the made rasters.
    fn fills_root(&self) -> Option<PathBuf> {
        if !self.disk {
            return None;
        }
        Some(self.store.as_ref()?.root().parent()?.join("masks"))
    }

    /// What reads the fills kept on disk for `file` without this `Ai`:
    /// the reduced develop's. None off the disk.
    pub fn fill_reader(&self, file: &Path) -> Option<FillReader> {
        Some(FillReader {
            root: self.fills_root()?,
            file: file.to_path_buf(),
        })
    }

    /// The slot the patch's fill on the open file lives in on disk
    /// ([`fill_slot`], its files [`slot_files`]): a fill made under
    /// another turn or crop is another slot; one made under another
    /// white balance or lens, a virtual copy's among them, is the same
    /// slot, told apart by the print kept in each of its files, which
    /// hold [`BASES_A_SLOT`] bases. A virtual copy with the same base
    /// is the same fill.
    fn fill_path(
        &self,
        patch: &Patch,
        region: &Region,
        size: (usize, usize),
        window: Window,
    ) -> Option<PathBuf> {
        let file = self.file.as_ref()?;
        Some(fill_slot(
            &self.fills_root()?,
            file,
            patch,
            region,
            size,
            window,
            None,
        ))
    }

    /// [`Ai::fill_path`] as it was at [`FILL_VERSION_HASHED`], with the
    /// window's print hashed into the name.
    fn fill_path_hashed(
        &self,
        patch: &Patch,
        region: &Region,
        size: (usize, usize),
        window: Window,
        print: &[[u16; 3]],
    ) -> Option<PathBuf> {
        let file = self.file.as_ref()?;
        Some(fill_slot(
            &self.fills_root()?,
            file,
            patch,
            region,
            size,
            window,
            Some(print_hash(print)),
        ))
    }

    /// The patch's fill made: kept in memory and on disk, at half
    /// precision in both, so a fill read back is the one first shown.
    fn keep_fill(
        &mut self,
        patch: &Patch,
        region: &Region,
        image: &WorkingImage,
        window: Window,
        engine: Engine,
        mut data: Vec<f32>,
    ) -> Vec<f32> {
        for v in &mut data {
            *v = half::f16::from_f32(v.clamp(-65504.0, 65504.0)).to_f32();
        }
        let size = (image.width, image.height);
        let print = window_print(image, region, window);
        if let Some(path) = self.fill_path(patch, region, size, window) {
            write_slot(
                &path,
                engine,
                size,
                window,
                (region.width, region.height),
                &print,
                &data,
            );
        }
        self.fills.insert(
            (patch.id, patch.seed),
            (patch.clone(), window, print, engine, data.clone()),
        );
        data
    }

    /// The engine the patch's fill is made by here, or why it is left
    /// as it was ([`engine_for`]). The card's memory is not asked: the
    /// fills ahead are counted whatever it has free, and a fill asks it
    /// once it is sure to start ([`Ai::fill_or_run`]), so a count, or a
    /// fill left for later, gives nothing back and is never left out.
    pub fn engine(
        &mut self,
        patch: &Patch,
        region: &Region,
        image: &WorkingImage,
    ) -> Result<Engine, NoFill> {
        let (window, _) = self.window_of(patch, region, image);
        // Made with no model, by LaMa's name ([`Ai::held_to_card`]).
        #[cfg(any(test, feature = "testing"))]
        if self.fill_by_test.is_some() {
            return Ok(Engine::Lama);
        }
        let has = |m: &Model| self.store.as_ref().is_some_and(|s| s.have(m));
        // A Fast one wants its own files too, and names them missing.
        let (have, needed) = match patch.fill_choice() {
            FillChoice::Fast if has(&REMOVE) => (has(&REMOVE_FAST), &REMOVE_FAST),
            _ => (has(&REMOVE), &REMOVE),
        };
        if patch.remove && self.providers.is_empty() {
            self.providers = Provider::available();
        }
        let engine =
            engine_for(patch.remove, have, &self.providers, window).map_err(|e| match e {
                NoFill::Missing(_) => NoFill::Missing(needed),
                e => e,
            })?;
        if engine == Engine::Lama && !self.fill_available() {
            return Err(NoFill::Missing(&greycard_ai::FILL));
        }
        // Said plainly before any model is asked for: the chain's
        // first model fails on such a card with the shader compiler's
        // words, and a failure here is not counted as a fill ahead.
        if engine == Engine::Remove
            && let Some(why) =
                remove_f16_refusal(greycard_ai::runtime::webgpu_shader_f16().as_ref())
        {
            return Err(NoFill::Failed(why));
        }
        Ok(engine)
    }

    /// Whether the fill `engine` makes is held to the card's memory: a
    /// removal's; and in a test making fills with no model, one a
    /// removal would make here.
    fn held_to_card(&self, patch: &Patch, window: Window, engine: Engine) -> bool {
        #[cfg(any(test, feature = "testing"))]
        if self.fill_by_test.is_some() {
            return matches!(
                engine_for(patch.remove, true, &self.providers, window),
                Ok(Engine::Remove)
            );
        }
        let _ = (patch, window);
        engine == Engine::Remove
    }

    /// Whether the card has the memory free a removal of `window`
    /// about to run needs ([`short_of_memory`], [`Ai::room`]): the card
    /// given back, the engine's working sets let go, and an export's
    /// waits on the way. Refused, the fill is left as the base has
    /// it and is not remembered as failed, so it is asked for again;
    /// the worker is told ([`Ai::take_fill_short`]). A removal never
    /// falls to the CPU, which takes minutes a fill, nor to LaMa, which
    /// the user did not choose.
    fn room_for_remove(&mut self, patch: &Patch, window: Window) -> Result<(), NoFill> {
        let Some(size) = greycard_ai::remove::run_size(window.width, window.height) else {
            return Ok(());
        };
        let what = format!("a removal at {size}");
        let need = |ai: &Ai| remove_needs(size, ai.unet_held(patch, size));
        // A queue's UNet at the other size is kept: beside the others'
        // sessions, letting it go gives nothing back, and the queue's
        // next fill would open it again.
        let keep_unet = self
            .store
            .as_ref()
            .is_some_and(|s| greycard_ai::remove::unet_held_elsewhere(s, steps_for(patch), size));
        let refuse = |ai: &mut Ai, why: String| {
            ai.fill_short = true;
            ai.short_failed = true;
            ai.short_figures = Some((need(ai), ai.free_memory().map_or(0, |(f, _)| f)));
            ai.short = Some(why.clone());
            let free = ai.free_memory().map_or(0, |(f, _)| f);
            let needs = need(ai);
            ai.refused
                .insert(patch.id, (patch.clone(), window, free, needs, why.clone()));
            NoFill::Failed(why)
        };
        match self.room(&what, &need, keep_unet, true) {
            Ok(()) => Ok(()),
            Err(why) => Err(refuse(self, why)),
        }
    }

    /// Room on the card for a learned mask's model ([`mask_needs`],
    /// [`Ai::room`]), on a machine whose models run on WebGPU: asked
    /// before its load or a cold encoder run, and the model loaded
    /// after. Short after the card is given back, a mask that may fall
    /// to the CPU ([`Ai::set_cpu_fallback`]) has the engine let go of
    /// what a develop makes again ([`Ai::set_card_release`]) and asks
    /// once more; short still, this job's masks run on the CPU
    /// ([`Ai::mask_providers`]), the worker told before the run
    /// ([`Ai::set_on_cpu`]) and after it ([`Ai::take_mask_short`]).
    /// One that may not, or one whose export was canceled, is refused,
    /// left out as for a model that failed and asked for again; the
    /// worker is told ([`Ai::take_mask_short`]).
    pub(crate) fn mask_room(&mut self, model: MaskModel) -> Result<(), String> {
        if self.providers.is_empty() {
            self.providers = Provider::available();
        }
        if !self.providers.contains(&Provider::WebGpu) || self.mask_cpu {
            return Ok(());
        }
        let (short, hit_short) = (self.short.clone(), self.hit_short);
        let Err(why) = self.room("a learned mask", &move |_| mask_needs(model), false, false)
        else {
            self.leave_cpu();
            return Ok(());
        };
        self.mask_short = true;
        if !self.cpu_fallback || self.patience.as_ref().is_some_and(|p| (p.canceled)()) {
            self.short_failed = true;
            return Err(why);
        }
        let before = self.free_memory().map(|(b, _)| b);
        // An export's masks go to the CPU as they always did: the
        // engine's sets are let go for the denoiser and a removal alone.
        let freed = if self.patience.is_some() {
            Vec::new()
        } else {
            self.release_card()
        };
        let after = self.free_memory().map(|(b, _)| b);
        // A second read not known fits, as `room` has it.
        let fits = after.is_none_or(|b| b >= mask_needs(model));
        tracing::info!("{}", second_ask_said(&why, &freed, before, after, fits));
        // Not refused, the short is not the develop's or the export's
        // to fail on; a patient walk keeps it met, so its later frames
        // ask once.
        self.short = short;
        if fits || self.patience.is_none() {
            self.hit_short = hit_short;
        }
        if fits {
            self.mask_short = false;
            self.leave_cpu();
            return Ok(());
        }
        self.mask_cpu = true;
        // Sessions on the card the give-back left go now, so the run is
        // the CPU's whole.
        self.drop_card_masks();
        if let Some(told) = &self.on_cpu {
            told();
        }
        Ok(())
    }

    /// The card has the room for a learned mask: the masks' sessions
    /// loaded on the CPU for its memory go, so the next is the card's.
    /// A session loaded on the card since, or one on the CPU of its own
    /// accord, stays.
    fn leave_cpu(&mut self) {
        let chosen = std::mem::take(&mut self.cpu_chosen);
        let on_cpu = |p: Provider| p == Provider::Cpu;
        if chosen.sam && self.sam.as_ref().is_some_and(|s| on_cpu(s.providers().0)) {
            self.sam = None;
        }
        if chosen.sam3 && self.sam3.as_ref().is_some_and(|s| on_cpu(s.providers().0)) {
            self.sam3 = None;
        }
        if chosen.subject && self.subject.as_ref().is_some_and(|s| on_cpu(s.provider())) {
            self.subject = None;
        }
        if chosen.sky && self.sky.as_ref().is_some_and(|s| on_cpu(s.provider())) {
            self.sky = None;
        }
    }

    /// The learned masks' sessions on the card dropped, by each one's
    /// own provider: SAM's, the People model's, Subject's, and the sky
    /// model's. What they made is kept.
    fn drop_card_masks(&mut self) {
        let on_card = |p: Provider| p != Provider::Cpu;
        if self.sam.as_ref().is_some_and(|s| on_card(s.providers().0)) {
            self.sam = None;
        }
        if self.sam3.as_ref().is_some_and(|s| on_card(s.providers().0)) {
            self.sam3 = None;
        }
        if self.subject.as_ref().is_some_and(|s| on_card(s.provider())) {
            self.subject = None;
        }
        if self.sky.as_ref().is_some_and(|s| on_card(s.provider())) {
            self.sky = None;
        }
    }

    /// Whether a learned mask short of the card may run on the CPU
    /// rather than be refused: on for the open frame's mask, People and
    /// Landscape jobs and an export's frame tried again; off for the
    /// rest, which refuse as before. Off, this job's CPU choice goes.
    pub fn set_cpu_fallback(&mut self, on: bool) {
        self.cpu_fallback = on;
        if !on {
            self.mask_cpu = false;
        }
    }

    /// What the engine lets go of the card for a learned mask or a
    /// removal ([`Ai::set_card_release`]), each named with its bytes; a
    /// test's card gets them back.
    fn release_card(&mut self) -> Vec<(&'static str, u64)> {
        let Some(release) = self.card_release.as_mut() else {
            return Vec::new();
        };
        let freed: Vec<_> = release().into_iter().filter(|(_, b)| *b > 0).collect();
        #[cfg(any(test, feature = "testing"))]
        if let Some(free) = self.free_memory_by_test.as_mut() {
            *free += freed.iter().map(|(_, b)| b).sum::<u64>();
        }
        freed
    }

    /// What the worker lets go of the card when a learned mask or a
    /// removal is short of it, or none: the engine's working sets, for
    /// the open frame's masks and fills on the worker's thread.
    pub fn set_card_release(&mut self, release: Option<CardRelease>) {
        self.card_release = release;
    }

    /// Whether a release is set ([`Ai::set_card_release`]), for a test
    /// that it is cleared after its job.
    #[cfg(test)]
    pub(crate) fn holds_card_release(&self) -> bool {
        self.card_release.is_some()
    }

    /// Told before a learned mask runs on the CPU for the card's
    /// memory, or none.
    pub fn set_on_cpu(&mut self, told: Option<OnCpu>) {
        self.on_cpu = told;
    }

    /// The providers a learned mask's model is loaded on: the CPU alone
    /// when this job's masks run there for the card's memory.
    fn mask_providers(&self) -> Vec<Provider> {
        if self.mask_cpu {
            vec![Provider::Cpu]
        } else {
            self.providers.clone()
        }
    }

    /// Whether this job's learned masks run on the CPU for the card's
    /// memory.
    pub fn masks_on_cpu(&self) -> bool {
        self.mask_cpu
    }

    /// Where the learned denoiser `model`, loaded, runs, as the card has
    /// the memory for it ([`denoise_needs`], [`Ai::room`]): at its own
    /// tile, or a smaller one. Its session may be dropped on the way,
    /// when the card is given back: the caller loads it again. Where it
    /// does not run on WebGPU it runs as it is.
    pub fn denoise_room_for(&mut self, model: &'static Model) -> Result<DenoiseOn, CardShort> {
        let Some((_, d)) = self.denoiser.as_ref().filter(|(id, _)| *id == model.id) else {
            return Ok(DenoiseOn::Card);
        };
        if d.provider() != Provider::WebGpu {
            return Ok(DenoiseOn::Card);
        }
        let tile = d.tiling().tile;
        self.denoise_room_at(tile)
    }

    /// [`Ai::denoise_room_for`] for a denoiser on the card at `tile`.
    /// Short after its own give-back, the engine's working sets let go,
    /// and an export's waits ([`Ai::room`]): the largest of
    /// [`DENOISE_SMALLER_TILES`] under it the card has the room for.
    /// Short at every tile, it is refused in the words it always had,
    /// and the engine's own denoise stands in, or an export fails. The
    /// log says what was chosen and why. Where the card's memory is not
    /// known it runs as it is.
    pub(crate) fn denoise_room_at(&mut self, tile: usize) -> Result<DenoiseOn, CardShort> {
        let what = format!("the learned denoiser at a tile of {tile}");
        let short = self.short.clone();
        // A canceled export lets nothing go.
        let release =
            self.card_release.is_some() && !self.patience.as_ref().is_some_and(|p| (p.canceled)());
        // Asked at its own tile without an export's waits: a smaller tile
        // the card has the room for is no reason to wait.
        let patience = self.patience.take();
        let first = self.room(&what, &|_| denoise_needs(tile), false, release);
        self.patience = patience;
        let Err(mut why) = first else {
            return Ok(DenoiseOn::Card);
        };
        // Past the whole card at its own tile, `room` gave nothing back
        // and let nothing go: our own sessions and the engine's sets go
        // before the smaller tiles are judged, as an export's ask at the
        // smallest does.
        let past_card = self
            .card_size()
            .is_some_and(|size| denoise_needs(tile) > size);
        if past_card {
            self.give_back_own(false);
        }
        if release && past_card {
            let before = self.free_memory().map(|(b, _)| b);
            let freed = self.release_card();
            if !freed.is_empty() {
                let after = self.free_memory().map(|(b, _)| b);
                tracing::info!(
                    "{}",
                    release_said(&what, &why, &freed, before, after, false)
                );
            }
        }
        // The largest tile, its own or smaller, the card has the room for.
        let fitting = |ai: &Ai, from: usize| {
            let free = ai.free_memory().map(|(f, _)| f);
            std::iter::once(tile)
                .chain(DENOISE_SMALLER_TILES)
                .filter(|&t| t <= from)
                .find(|&t| free.is_none_or(|f| f >= denoise_needs(t)))
        };
        // The need as it is compared: the refusal's tenth can round it
        // away.
        let found = format!("{why} ({} GiB needed)", gib_said(denoise_needs(tile)));
        let at = |t: usize| {
            if t == tile {
                DenoiseOn::Card
            } else {
                DenoiseOn::Tile(t)
            }
        };
        let smaller = fitting(self, tile).filter(|&t| t < tile);
        if let Some(t) = smaller {
            tracing::info!(
                "{found}: the learned denoiser runs at a tile of {t}, which needs {} GiB",
                gib_said(denoise_needs(t))
            );
            // Handled, no refusal and no short met.
            self.short = short;
            return Ok(at(t));
        }
        // Short at every tile: an export waits its tries at the smallest,
        // then runs at the largest it has the room for.
        let least = DENOISE_SMALLER_TILES
            .into_iter()
            .filter(|&t| t < tile)
            .min()
            .unwrap_or(tile);
        if self.patience.is_some() {
            let what = format!("the learned denoiser at a tile of {least}");
            if self
                .room(&what, &|_| denoise_needs(least), false, false)
                .is_ok()
                && let Some(t) = fitting(self, tile)
            {
                tracing::info!("{found}: after the waits, it runs at a tile of {t}");
                self.short = short;
                return Ok(at(t));
            }
            // Said at its own tile, as it always was, the card as it is.
            if let Some(w) = self
                .free_memory()
                .and_then(|(free, card)| short_of(&card, free, denoise_needs(tile), &what_at(tile)))
            {
                why = w;
            }
        }
        tracing::info!("{found}, short at every smaller tile");
        self.short = Some(why.clone());
        self.short_failed = true;
        Err(CardShort(why))
    }

    /// Whether `what`, about to run and asking `need` of the card, may
    /// begin. The need is found first, since it may wait for a UNet
    /// being opened ahead, and the free figure read after it. Short,
    /// the card is given back first
    /// ([`Ai::give_back_own`]) and asked once more; then, with
    /// `release`, the engine lets go of what a develop makes again
    /// ([`Ai::set_card_release`]) and it is asked again: our own memory
    /// goes before we wait on another program's. Then, in an export or
    /// a merge ([`Patience`]), asked again its tries in all, its wait
    /// apart, unless the export is canceled or a short was met already
    /// in this walk. Short past all of them, the walk has met it. Where
    /// the free memory is not known it may.
    fn room(
        &mut self,
        what: &str,
        need: &dyn Fn(&Ai) -> u64,
        keep_unet: bool,
        release: bool,
    ) -> Result<(), String> {
        let short = |ai: &Ai| -> Result<(), String> {
            let need = need(ai);
            match ai.free_memory() {
                Some((free, card)) => short_of(&card, free, need, what).map_or(Ok(()), Err),
                None => Ok(()),
            }
        };
        let Err(mut why) = short(self) else {
            return Ok(());
        };
        // Past the whole card nothing given back or waited for helps.
        let past_card = self.card_size().is_some_and(|size| need(self) > size);
        let patience = self.patience.clone();
        let met = patience.is_some() && self.hit_short;
        if !past_card && !met && self.give_back_own(keep_unet) {
            match short(self) {
                Ok(()) => {
                    tracing::info!("{what}: begun once the card was given back");
                    return Ok(());
                }
                Err(w) => why = w,
            }
        }
        // Each frame of a set makes the sets again, so each lets go of
        // them, met or not; a canceled export lets nothing go.
        if release
            && !past_card
            && self.card_release.is_some()
            && !patience.as_ref().is_some_and(|p| (p.canceled)())
        {
            let before = self.free_memory().map(|(b, _)| b);
            let freed = self.release_card();
            if freed.is_empty() {
                tracing::info!("{why}; nothing of the engine's to let go");
            } else {
                let fits = short(self);
                let after = self.free_memory().map(|(b, _)| b);
                let said = release_said(what, &why, &freed, before, after, fits.is_ok());
                tracing::info!("{said}");
                match fits {
                    Ok(()) => return Ok(()),
                    Err(w) => why = w,
                }
            }
        }
        if !past_card
            && !met
            && let Some(p) = &patience
        {
            for _ in 1..p.tries {
                if (p.canceled)() {
                    break;
                }
                tracing::info!("{why}; asked again in {:?}", p.wait);
                #[cfg(test)]
                WAITS_BY_TEST.with(|w| w.set(w.get() + 1));
                std::thread::sleep(p.wait);
                match short(self) {
                    Ok(()) => return Ok(()),
                    Err(w) => why = w,
                }
            }
        }
        if patience.is_some() {
            self.hit_short = true;
        }
        tracing::info!("{why}");
        self.short = Some(why.clone());
        Err(why)
    }

    /// The card given back by this alone, right before a run it is
    /// short of: every session it holds dropped, the UNet held between
    /// removals let go, and the heap trimmed. The caller loads what it
    /// runs after. Not while a merge runs on its own thread (so a
    /// merge's own models never give back), nor where the card's free
    /// memory is not known (unified memory). True when anything went.
    fn give_back_own(&mut self, keep_unet: bool) -> bool {
        if self.free_memory().is_none() {
            return false;
        }
        let Some(_hold) = give_back_lock() else {
            return false;
        };
        let held = self.holds_sessions();
        self.drop_sessions();
        let unet = !keep_unet && greycard_ai::remove::let_go_unet();
        if held || unet {
            greycard_ai::runtime::trim_heap();
            self.refused.clear();
        }
        held || unet
    }

    /// The card's whole memory, read on the worker's thread, which may
    /// wait on the adapter's probe ([`card_memory_probed`]); in a test,
    /// what it set, else none, never the machine's own card.
    fn card_whole(&self) -> Option<u64> {
        #[cfg(any(test, feature = "testing"))]
        return self.card_by_test.flatten();
        #[allow(unreachable_code)]
        card_memory_probed()
    }

    /// The need and the card's whole memory where `patch`, a removal
    /// the remove would make in `window`, is one the card can never
    /// hold ([`FillHaves::why_not`]).
    fn never_on_card(&self, patch: &Patch, window: Window) -> Option<(u64, u64)> {
        if !patch.remove || greycard_ai::remove::factor_for(window.width, window.height).is_none() {
            return None;
        }
        let haves = FillHaves {
            webgpu: true,
            card: self.card_whole(),
            ..FillHaves::default()
        };
        match haves.why_not(patch.fill_choice()) {
            Some(FillNotHere::Card { need, have }) => Some((need, have)),
            _ => None,
        }
    }

    /// What a develop says of `retouch`'s removals the card keeps from
    /// being made, worked out afresh from now: by name, the need and the
    /// card's memory. Only a Fill the picture shown left as it was
    /// (`unmade`, none where no retouch made the picture) is said: with
    /// the card's whole where it can never hold the remove
    /// ([`Ai::never_on_card`]), or with the free figure of its refusal
    /// while that refusal holds ([`Ai::still_refused`]'s rule) and it is
    /// not still to be made this round (`pending`) unless this round's
    /// own ask was the refusal (`failed`, by name). A fill held,
    /// deleted, made Patch, or being made says nothing.
    pub fn card_shorts(
        &self,
        retouch: &Retouch,
        unmade: Option<&[u64]>,
        pending: &[u64],
        failed: &[String],
    ) -> Vec<(String, u64, u64)> {
        let Some(unmade) = unmade else {
            return Vec::new();
        };
        retouch
            .patches
            .iter()
            .filter(|p| p.method == greycard_edit::retouch::Method::Fill && p.remove)
            .filter(|p| unmade.contains(&p.id))
            .filter_map(|p| {
                // Its window as last found, else one the remove takes.
                let window = self
                    .windows
                    .get(&p.id)
                    .filter(|(kept, ..)| same_fill(kept, p))
                    .map_or(
                        Window {
                            x: 0,
                            y: 0,
                            width: 512,
                            height: 512,
                        },
                        |(_, _, w, _)| *w,
                    );
                if let Some((need, have)) = self.never_on_card(p, window) {
                    return Some((p.name(), need, have));
                }
                let (kept, _, at, need, _) = self.refused.get(&p.id)?;
                if !same_fill(kept, p) {
                    return None;
                }
                // Refused by this round's own ask: its figure is the one
                // to say, whatever the card frees after it.
                if failed.contains(&p.name()) {
                    return Some((p.name(), *need, *at));
                }
                // Else while the refusal holds and it is not asked again.
                let holds = self.free_memory().is_none_or(|(f, _)| f <= *at);
                (holds && !pending.contains(&p.id)).then(|| (p.name(), *need, *at))
            })
            .collect()
    }

    /// The bytes of the card's own memory, where known; in a test, what
    /// it set ([`Ai::card_by_test`]).
    fn card_size(&self) -> Option<u64> {
        #[cfg(any(test, feature = "testing"))]
        if self.free_memory_by_test.is_some() || self.fill_by_test.is_some() {
            return self.card_by_test.flatten();
        }
        greycard_ai::runtime::webgpu_memory()
    }

    /// Whether the UNet a removal of `patch` at `size` runs is open on
    /// the card ([`greycard_ai::remove::unet_held`]), waiting for one
    /// being opened ahead, or as a test has it. One held at the other
    /// size is not counted: with another model's session on ONNX
    /// Runtime's WebGPU context, letting it go gives the card nothing
    /// back.
    fn unet_held(&self, patch: &Patch, size: usize) -> bool {
        #[cfg(any(test, feature = "testing"))]
        if let Some(held) = self.unet_held_by_test {
            return held;
        }
        self.store
            .as_ref()
            .is_some_and(|s| greycard_ai::remove::unet_held(s, steps_for(patch), size))
    }

    /// Whether a fill's model ran since the card was last given back,
    /// taken.
    pub fn take_fills_ran(&mut self) -> bool {
        std::mem::take(&mut self.fills_ran)
    }

    /// Whether a fill's model ran since the card was last given back.
    pub fn fills_ran(&self) -> bool {
        self.fills_ran
    }

    /// Whether a fill was refused for the card's memory since the
    /// shorts were cleared, taken: the worker gives the card back across
    /// its models and asks for the fills again.
    pub fn take_fill_short(&mut self) -> bool {
        std::mem::take(&mut self.fill_short)
    }

    /// Whether a learned mask was refused for the card's memory since
    /// the shorts were cleared, taken: the worker gives the card back
    /// across its models.
    pub fn take_mask_short(&mut self) -> bool {
        std::mem::take(&mut self.mask_short)
    }

    /// The shorts met so far forgotten: before each develop round, so
    /// one met by another job does not move the next.
    pub fn clear_shorts(&mut self) {
        self.fill_short = false;
        self.mask_short = false;
        self.mask_cpu = false;
        self.short = None;
        self.short_figures = None;
    }

    /// How an export or a merge waits on a card that is short, or none
    /// for the window's jobs; and whether a walk of it met a short. A
    /// new patience clears what the last walk met.
    pub fn set_patience(&mut self, patience: Option<Patience>) {
        self.patience = patience;
        self.hit_short = false;
        self.short_failed = false;
        self.clear_shorts();
    }

    /// The walk met no short since its patience was set: a frame tried
    /// again after the card is given back starts afresh.
    pub fn retry_patiently(&mut self) {
        self.hit_short = false;
        self.short_failed = false;
        self.clear_shorts();
    }

    /// Whether a patient walk met a short it gave up on.
    pub fn hit_short(&self) -> bool {
        self.hit_short
    }

    /// Whether a patient walk was refused a run for the card's memory,
    /// no fallback taking it: the frame is tried again on it, and its
    /// set's later frames take a mask's CPU at once. A short a smaller
    /// tile handled is met ([`Ai::hit_short`]), not this.
    pub fn short_failed(&self) -> bool {
        self.short_failed
    }

    /// The need and the free figure of the last removal refused for the
    /// card's memory, taken.
    pub fn take_short_figures(&mut self) -> Option<(u64, u64)> {
        self.short_figures.take()
    }

    /// Every ONNX Runtime session this holds dropped: the remove's,
    /// LaMa's, the learned denoiser's, and the masks' models'. What they
    /// made is kept (SAM's embeddings, the scenes, the mattes, the
    /// people, and the disk caches), so only the sessions load again,
    /// at their next use. Every field named, so a new model's is not
    /// missed.
    pub fn drop_sessions(&mut self) {
        let Ai {
            store: _,
            file: _,
            providers: _,
            subject,
            sam,
            sky,
            scene: _,
            sky_cut: _,
            last_sky: _,
            fill,
            remove,
            fills_queued: _,
            denoiser,
            denoiser_tiling: _,
            awaiting_fp16: _,
            fills: _,
            failed: _,
            windows: _,
            refused: _,
            preview: _,
            embedding: _,
            sam3,
            presets: _,
            encodings: _,
            people: _,
            parts_view: _,
            picture: _,
            cache: _,
            print: _,
            retouched: _,
            given: _,
            fell_back: _,
            salt: _,
            other_side: _,
            disk: _,
            quiet_fetch: _,
            subject_matte: _,
            #[cfg(any(test, feature = "testing"))]
                sams_edge_only: _,
            #[cfg(any(test, feature = "testing"))]
                scene_runs: _,
            #[cfg(any(test, feature = "testing"))]
                fill_by_test: _,
            #[cfg(any(test, feature = "testing"))]
            free_memory_by_test,
            #[cfg(any(test, feature = "testing"))]
            freed_by_test,
            #[cfg(any(test, feature = "testing"))]
                unet_held_by_test: _,
            #[cfg(any(test, feature = "testing"))]
                card_by_test: _,
            short: _,
            fills_ran: _,
            fill_short: _,
            mask_short: _,
            card_release: _,
            on_cpu: _,
            mask_cpu: _,
            cpu_chosen,
            cpu_fallback: _,
            patience: _,
            hit_short: _,
            short_failed: _,
            short_figures: _,
        } = self;
        *subject = None;
        *sam = None;
        *sky = None;
        *cpu_chosen = CpuChosen::default();
        *fill = None;
        *remove = None;
        *denoiser = None;
        *sam3 = None;
        // A test's card has what its sessions held back.
        #[cfg(any(test, feature = "testing"))]
        if let Some(free) = free_memory_by_test.as_mut() {
            *free += std::mem::take(freed_by_test);
        }
    }

    /// Whether this holds any ONNX Runtime session, every field named
    /// as in [`Ai::drop_sessions`].
    pub fn holds_sessions(&self) -> bool {
        let Ai {
            store: _,
            file: _,
            providers: _,
            subject,
            sam,
            sky,
            scene: _,
            sky_cut: _,
            last_sky: _,
            fill,
            remove,
            fills_queued: _,
            denoiser,
            denoiser_tiling: _,
            awaiting_fp16: _,
            fills: _,
            failed: _,
            windows: _,
            refused: _,
            preview: _,
            embedding: _,
            sam3,
            presets: _,
            encodings: _,
            people: _,
            parts_view: _,
            picture: _,
            cache: _,
            print: _,
            retouched: _,
            given: _,
            fell_back: _,
            salt: _,
            other_side: _,
            disk: _,
            quiet_fetch: _,
            subject_matte: _,
            #[cfg(any(test, feature = "testing"))]
                sams_edge_only: _,
            #[cfg(any(test, feature = "testing"))]
                scene_runs: _,
            #[cfg(any(test, feature = "testing"))]
                fill_by_test: _,
            #[cfg(any(test, feature = "testing"))]
                free_memory_by_test: _,
            #[cfg(any(test, feature = "testing"))]
            freed_by_test,
            #[cfg(any(test, feature = "testing"))]
                unet_held_by_test: _,
            #[cfg(any(test, feature = "testing"))]
                card_by_test: _,
            short: _,
            fills_ran: _,
            fill_short: _,
            mask_short: _,
            card_release: _,
            on_cpu: _,
            mask_cpu: _,
            cpu_chosen: _,
            cpu_fallback: _,
            patience: _,
            hit_short: _,
            short_failed: _,
            short_figures: _,
        } = self;
        let held = subject.is_some()
            || sam.is_some()
            || sky.is_some()
            || fill.is_some()
            || remove.is_some()
            || denoiser.is_some()
            || sam3.is_some();
        // A test's sessions are what it says they hold back.
        #[cfg(any(test, feature = "testing"))]
        let held = held || *freed_by_test > 0;
        held
    }

    /// The tiling the learned denoiser `model` ran at, kept after its
    /// session is dropped: the disk cache's key, read without loading
    /// it again.
    pub fn denoiser_tiling(&self, model: &'static Model) -> Option<greycard_ai::denoise::Tiling> {
        match &self.denoiser {
            Some((id, d)) if *id == model.id => Some(d.tiling()),
            _ => self
                .denoiser_tiling
                .filter(|(id, _)| *id == model.id)
                .map(|(_, t)| t),
        }
    }

    /// The providers offered, as a test has them.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_providers_by_test(&mut self, providers: Vec<Provider>) {
        self.providers = providers;
    }

    /// The last run refused for the card's memory, taken.
    pub fn take_short(&mut self) -> Option<String> {
        self.short.take()
    }

    /// The bytes free on the card now ([`card_free_now`]), or as a test
    /// has it, and the card's name.
    fn free_memory(&self) -> Option<(u64, String)> {
        #[cfg(any(test, feature = "testing"))]
        if self.free_memory_by_test.is_some() || self.fill_by_test.is_some() {
            return self.free_memory_by_test.map(|b| (b, "the card".into()));
        }
        card_free_now()
    }

    /// The patch's window and depth in `image`, found once for the
    /// patch as it is and the picture's size.
    fn window_of(&mut self, patch: &Patch, region: &Region, image: &WorkingImage) -> (Window, f32) {
        let size = (image.width, image.height);
        if let Some((p, s, w, d)) = self.windows.get(&patch.id)
            && same_fill(p, patch)
            && *s == size
        {
            return (*w, *d);
        }
        let (window, depth) = window_for(region, image.width, image.height);
        self.windows
            .insert(patch.id, (patch.clone(), size, window, depth));
        (window, depth)
    }

    /// The region's window made up from what is around it, in the
    /// picture's own values, and the engine that made it ([`Engine`],
    /// [`engine_for`]); or why not. Kept by patch, seed, window, and
    /// the picture under the window, so a slider elsewhere does not run
    /// the model again.
    pub fn fill(
        &mut self,
        patch: &Patch,
        region: &Region,
        image: &WorkingImage,
    ) -> Result<(Vec<f32>, Engine), NoFill> {
        self.fill_or_run(patch, region, image, |_| true, &mut |_, _| {})
    }

    /// [`Ai::fill`], with `starting` told the engine when a fill not
    /// kept is about to be made, since that takes a while, and asked
    /// whether to make it now: [`NoFill::Later`] when not, nothing
    /// loaded. `told` is given the remove's UNet calls made, and of
    /// how many, as it makes them; LaMa's one call tells nothing.
    pub fn fill_or_run(
        &mut self,
        patch: &Patch,
        region: &Region,
        image: &WorkingImage,
        starting: impl FnOnce(Engine) -> bool,
        told: &mut dyn FnMut(usize, usize),
    ) -> Result<(Vec<f32>, Engine), NoFill> {
        let (window, depth) = self.window_of(patch, region, image);
        let print = window_print(image, region, window);
        let size = (image.width, image.height);
        if let Some(kept) = self.fill_in_memory(patch, window, &print) {
            return Ok(kept);
        }
        if self.fill_from_disk(patch, region, size, window, &print)
            && let Some(kept) = self.fill_in_memory(patch, window, &print)
        {
            return Ok(kept);
        }
        if let Some(why) = self.failed_before(patch, region, image) {
            return Err(NoFill::Failed(why));
        }
        // A card under the remove's need never holds it, as the panel
        // greys it: refused at once, before its model is asked for (it is
        // not offered) and with nothing given back or let go.
        if let Some((need, have)) = self.never_on_card(patch, window) {
            return Err(NoFill::Card { need, have });
        }
        if !self.fills_by_test() {
            greycard_ai::runtime::status().map_err(|why| NoFill::Failed(why.to_string()))?;
        }
        if self.store.is_none() {
            return Err(NoFill::Missing(&greycard_ai::FILL));
        }
        let engine = self.engine(patch, region, image)?;
        // Refused as it is, with nothing changed since: said so again,
        // and nothing asked of the card.
        if self.held_to_card(patch, window, engine)
            && let Some(why) = self.still_refused(patch, window)
        {
            // Its figures again, so the window says what it said the
            // first time.
            self.short_figures = self
                .refused
                .get(&patch.id)
                .map(|(_, _, at, need, _)| (*need, *at));
            return Err(NoFill::Failed(why));
        }
        if !starting(engine) {
            return Err(NoFill::Later);
        }
        // Sure to start, the card is asked: given back on a short, and
        // waited on as an export's [`Patience`] says ([`Ai::room`]).
        if self.held_to_card(patch, window, engine) {
            self.room_for_remove(patch, window)?;
        }
        self.refused.remove(&patch.id);
        self.fills_ran = true;
        #[cfg(any(test, feature = "testing"))]
        if let Some(v) = self.fill_by_test {
            if v < 0.0 {
                return Err(NoFill::Failed("the model would not load".into()));
            }
            let data = vec![v; region.width * region.height * 3];
            let data = self.keep_fill(patch, region, image, window, engine, data);
            return Ok((data, engine));
        }
        let store = self.store.clone().expect("a store, asked above");
        if self.providers.is_empty() {
            self.providers = Provider::available();
        }
        match engine {
            Engine::Lama if self.fill.is_none() => match Fill::load(&store, &self.providers) {
                Ok(f) => self.fill = Some(f),
                Err(e) => {
                    // The worker warns, with the patch's name.
                    tracing::debug!("fill model: {e}");
                    return Err(NoFill::Failed(e.to_string()));
                }
            },
            // Loaded again for a Fast fill when its files came after.
            Engine::Remove
                if self
                    .remove
                    .as_ref()
                    .is_none_or(|r| patch.fast && !r.has_fast()) =>
            {
                // The card's memory, or the machine's on unified
                // memory: the remove keeps more open on unified memory with more.
                let machine = greycard_ai::remove::Machine::here();
                match Remove::load_on(&store, &self.providers, Some(machine)) {
                    Ok(r) => self.remove = Some(r),
                    Err(e) => {
                        tracing::debug!("remove model: {e}");
                        return Err(NoFill::Failed(e.to_string()));
                    }
                }
            }
            _ => {}
        }
        let Window {
            x: wx,
            y: wy,
            width: ww,
            height: wh,
        } = window;
        let to_srgb = greycard_render::export::Space::Srgb.matrix();
        let from_srgb = invert(&to_srgb);
        // Coverage over the window, and the picture's values there.
        let mut hole = vec![0.0f32; ww * wh];
        let mut values = Vec::with_capacity(ww * wh * 3);
        let (mut luma_sum, mut luma_n) = (0.0f32, 0usize);
        for y in 0..wh {
            for x in 0..ww {
                let (px, py) = (wx + x, wy + y);
                let i = (py * image.width + px) * 3;
                let p = [image.data[i], image.data[i + 1], image.data[i + 2]];
                let s = mul(&to_srgb, p).map(|v| v.max(0.0));
                values.extend(s);
                let covered = px >= region.x
                    && px < region.x + region.width
                    && py >= region.y
                    && py < region.y + region.height
                    && region.cover[(py - region.y) * region.width + (px - region.x)] > 0.0;
                if covered {
                    hole[y * ww + x] = 1.0;
                } else {
                    luma_sum += 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2];
                    luma_n += 1;
                }
            }
        }
        // The window shown at a middle grey, so the model sees a picture.
        let mean = if luma_n > 0 {
            luma_sum / luma_n as f32
        } else {
            0.18
        };
        let gain = (0.18 / mean.max(1e-4)).clamp(0.25, 16.0);
        let encoded: Vec<f32> = values.iter().map(|v| encode(v * gain)).collect();
        let input = Rgbf::new(ww, wh, encoded);
        let started = Instant::now();
        // The scale LaMa made the window at; the remove's answer is
        // regrained at any factor it was taken down by, so it counts
        // as its own pixels here.
        let (filled, scale) = match engine {
            Engine::Lama => {
                if patch.remove {
                    tracing::info!(
                        "fill {}: a window of {ww}x{wh} is {:.2} times the remove's {}, past its {}; LaMa makes it",
                        patch.name(),
                        ww.max(wh) as f32 / greycard_ai::remove::MAX as f32,
                        greycard_ai::remove::MAX,
                        greycard_ai::remove::FACTOR
                    );
                }
                let fill = self.fill.as_mut().expect("LaMa, loaded above");
                let made = match fill.fill_window(&input, &hole, depth) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::debug!("fill: {e}");
                        return Err(self.remember_failure(patch, window, e.to_string()));
                    }
                };
                let fill = self.fill.as_ref().expect("LaMa, loaded above");
                // LaMa's answer sits a little off the picture's level
                // too, across a wide hole; matched across the edge as
                // the remove's is.
                let (image, under) = greycard_ai::remove::matched(&input, &hole, &made.image);
                let (image, evened) = greycard_ai::remove::evened(&input, &hole, &image);
                let made = greycard_ai::fill::Filled { image, ..made };
                tracing::info!(
                    "fill {}: LaMa, a window of {ww}x{wh} at {:.2} ({}x{}), {} tile(s) of up to {} on {}, {:.2}s",
                    patch.name(),
                    made.scale,
                    made.size.0,
                    made.size.1,
                    made.tiles,
                    greycard_ai::fill::MAX,
                    fill.provider().name(),
                    started.elapsed().as_secs_f64()
                );
                if let Some(g) = under {
                    tracing::info!(
                        "fill {}: level under the hole matched to the picture's across its edge, gain {:.3}, {:.3}, and {:.3} in R, G, and B",
                        patch.name(),
                        g.gain[0],
                        g.gain[1],
                        g.gain[2]
                    );
                }
                if let Some(m) = evened {
                    tracing::info!(
                        "fill {}: the fill's middle brought to the smooth surface round it, by up to {:.1}%, mean gains {:.3}, {:.3}, and {:.3} in R, G, and B",
                        patch.name(),
                        100.0 * m.largest,
                        m.mean[0],
                        m.mean[1],
                        m.mean[2]
                    );
                }
                (made.image, made.scale)
            }
            Engine::Remove => {
                // A queue of fills keeps the UNet open across them, at
                // the sizes the machine's memory allows.
                let remove = self.remove.as_mut().expect("the remove, loaded above");
                remove.hold_unet(self.fills_queued);
                if let Err(e) = remove.set_steps(steps_for(patch)) {
                    return Err(self.remember_failure(patch, window, e.to_string()));
                }
                let made = match remove.remove_window_told(&input, &hole, remove_seed(patch), told)
                {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::debug!("remove: {e}");
                        return Err(self.remember_failure(patch, window, e.to_string()));
                    }
                };
                let remove = self.remove.as_ref().expect("the remove, loaded above");
                let t = remove.times();
                tracing::info!(
                    "fill {}: the generative remove in {}, seed {}, a window of {ww}x{wh} taken down by {:.2} to run at {} on {}: loads {:.2}s, encode {:.2}s, {} UNet calls {:.2}s, decode {:.2}s, {:.2}s in all ({:?})",
                    patch.name(),
                    remove.steps().words(),
                    patch.seed,
                    t.factor,
                    t.size,
                    remove.provider().name(),
                    t.loads,
                    t.encode,
                    t.unet_calls,
                    t.steps,
                    t.decode,
                    t.total,
                    remove.memory()
                );
                if let Some(g) = t.under {
                    tracing::info!(
                        "fill {}: level under the hole matched to the picture's across its edge, gain {:.3}, {:.3}, and {:.3} in R, G, and B",
                        patch.name(),
                        g.gain[0],
                        g.gain[1],
                        g.gain[2]
                    );
                }
                (made, 1.0)
            }
        };
        if let Some(dir) = std::env::var_os("GREYCARD_AI_DUMP") {
            let dir = std::path::PathBuf::from(dir);
            let png = |name: &str, img: &Rgbf| {
                let data: Vec<u8> = img
                    .data
                    .iter()
                    .map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8)
                    .collect();
                if let Err(e) = image::save_buffer(
                    dir.join(name),
                    &data,
                    img.width as u32,
                    img.height as u32,
                    image::ExtendedColorType::Rgb8,
                ) {
                    tracing::warn!("dump {name}: {e}");
                }
            };
            png("fill_in.png", &input);
            png("fill_out.png", &filled);
            let hole8: Vec<u8> = hole.iter().map(|v| (v * 255.0) as u8).collect();
            if let Err(e) = image::save_buffer(
                dir.join("fill_hole.png"),
                &hole8,
                ww as u32,
                wh as u32,
                image::ExtendedColorType::L8,
            ) {
                tracing::warn!("dump fill_hole.png: {e}");
            }
        }
        // The region's window, decoded to the picture's values; where
        // the region reaches past the window (its ring, at the frame's
        // edge) the picture as it is, under no coverage.
        let mut out = Vec::with_capacity(region.width * region.height * 3);
        for y in 0..region.height {
            for x in 0..region.width {
                let (px, py) = (region.x + x, region.y + y);
                if (wx..wx + ww).contains(&px) && (wy..wy + wh).contains(&py) {
                    let s = filled.at(px - wx, py - wy).map(|v| decode(v) / gain);
                    out.extend(mul(&from_srgb, s));
                } else {
                    let i = (py * image.width + px) * 3;
                    out.extend_from_slice(&image.data[i..i + 3]);
                }
            }
        }
        // Taken down for LaMa, the hole comes back smooth where the
        // frame round it has grain; at its own pixels it keeps its
        // own. The remove's answer is smooth at any size, its VAE
        // giving back no grain: the frame's is put back always.
        if regrains(engine, scale)
            && let Some(r) = regrain(
                &mut out,
                region,
                window,
                image,
                patch.id ^ (u64::from(patch.seed) << 40),
            )
        {
            tracing::info!(
                "fill {}: the frame's noise put back, luma and chroma sigma at {:.2}: {:.5} and {:.5} round the hole, {:.5} and {:.5} in the fill before, {:.5} and {:.5} after; spread at one over two {:.2} and {:.2} round the hole, {:.2} and {:.2} in the fill before, drawn smoothed by {:.2} and {:.2} px",
                patch.name(),
                r.level,
                r.frame[0],
                r.frame[1],
                r.before[0],
                r.before[1],
                r.after[0],
                r.after[1],
                r.shape[0].ratio[0],
                r.shape[0].ratio[1],
                r.shape[1].ratio[0],
                r.shape[1].ratio[1],
                r.widths[0],
                r.widths[1]
            );
        }
        let out = self.keep_fill(patch, region, image, window, engine, out);
        Ok((out, engine))
    }

    /// Which Subject file to use: the one already loaded, once there
    /// is one, since the store may have gained the other file since
    /// and the raster in hand is the loaded one's; else `model` — the
    /// choice [`step`] already made for the window, with the session's
    /// declines and the providers record folded in; else, with
    /// neither, the same chooser with no declines and nobody to offer
    /// a fetch to ([`unattended`]), as the export path asks it.
    fn subject_model(&self, model: Option<&'static Model>) -> Option<&'static Model> {
        self.subject
            .as_ref()
            .map(|s| s.model())
            .or(model)
            .or_else(|| unattended(&Shape::Subject {}, self.store.as_ref(), &[]))
    }

    /// Where a made raster for `shape` on the open file lives on disk:
    /// under the models' cache, in a slot named by a hash of the file,
    /// the model's id and the shape, and in it the raster of the base
    /// of print `self.print`, the print in its name: a raster of
    /// another base is not read, and the slot keeps the last base's
    /// alone ([`write_raster`]). `model` is `raster`'s own, so the
    /// cache is keyed on the exact file it is about to load or has
    /// loaded, not a second, independent guess that can name a
    /// different file (notes, the Subject rewrite offer).
    fn cached_path(
        &self,
        shape: &Shape,
        model: Option<&'static Model>,
        turn: u8,
    ) -> Option<PathBuf> {
        self.cached_path_at(shape, model, turn, RASTER_WIDTH)
    }

    /// [`Ai::cached_path`] for a raster `width` texels wide
    /// (`brush::raster_width`): a raster of another width than
    /// `RASTER_WIDTH`, a Sky's on a frame past the rule, has a slot of
    /// its own, so a frame the rule moved to the wide raster makes it
    /// again rather than read the narrow one, and every narrow raster
    /// keeps the slot it had.
    fn cached_path_at(
        &self,
        shape: &Shape,
        model: Option<&'static Model>,
        turn: u8,
        width: usize,
    ) -> Option<PathBuf> {
        if !self.disk {
            return None;
        }
        let file = crate::mask_base::canonical(self.file.as_ref()?);
        let model = match shape {
            Shape::Subject {} | Shape::Background {} => self.subject_model(model)?,
            Shape::Sky { .. } | Shape::Landscape { .. } => &SKY,
            _ => model_for(shape, self.store.as_ref(), &[])?,
        };
        let prompt = serde_json::to_string(shape).ok()?;
        let store = self.store.as_ref()?;
        let root = store.root().parent()?.join("masks");
        let mut h = std::collections::hash_map::DefaultHasher::new();
        file.hash(&mut h);
        model.id.hash(&mut h);
        prompt.hash(&mut h);
        if matches!(shape, Shape::Sky { .. }) {
            SKY_PIPELINE.hash(&mut h);
            store.have(&SAM).hash(&mut h);
        }
        if matches!(shape, Shape::Part { .. }) {
            PART_PIPELINE.hash(&mut h);
        }
        if matches!(shape, Shape::Landscape { .. }) {
            LANDSCAPE_PIPELINE.hash(&mut h);
        }
        // Foliage is cut by the sky, whose outline is SAM's once SAM
        // is in, made by the sky's stages: made again when either
        // changes, as a sky is.
        if matches!(
            shape,
            Shape::Landscape {
                group: greycard_edit::mask::LandscapeGroup::Foliage
            }
        ) {
            SKY_PIPELINE.hash(&mut h);
            store.have(&SAM).hash(&mut h);
        }
        // Made again with Subject's edge once a Subject model is in.
        if takes_subject_edge(shape) {
            PERSON_EDGE.hash(&mut h);
            self.subject_in_store().is_some().hash(&mut h);
        }
        // A turned base is another picture to a raster: a half turn
        // keeps the height a cached one is checked by. Only hashed when
        // turned, so every raster made unturned keeps its slot.
        if !turn.is_multiple_of(4) {
            (turn % 4).hash(&mut h);
        }
        // Only hashed when not the narrow width, for the same reason.
        if width != RASTER_WIDTH {
            ("raster width", width).hash(&mut h);
        }
        // Made again on the retouched picture: a slot of its own for
        // the patches it was made under.
        if let Some(print) = self.salt {
            ("retouched", print).hash(&mut h);
        }
        // A proxy's raster, the reduced develop's, beside the full
        // develop's. Only hashed for the proxy, so the full develop's
        // slot is the name the cache gave it before rasters were kept
        // by their base, which the first write in it removes.
        if self.print.size == crate::mask_base::Size::Reduced {
            "reduced".hash(&mut h);
        }
        Some(root.join(format!("{:016x}-{:016x}.png", h.finish(), self.print.hash)))
    }

    /// Whether the raster for `shape` of the frame at `file` at `turn`
    /// can be had, asked before anything of the frame is decoded: from
    /// the disk cache (a file in its slot; its aspect is checked when it
    /// is read), or made by a model in the store, or not at all until a
    /// model is fetched. Nothing is loaded. `frame` is the frame's size
    /// as its file says it stands, turned (`decode::stance_path`): the
    /// raster in the slot of the width the frame takes
    /// (`brush::raster_width`) is the one that counts, so a frame past
    /// the rule with only its old narrow raster is not ready and its
    /// masks are made before a develop is spent on it. Where the file
    /// will not say, a Sky's raster in either width's slot counts, and
    /// one of the width the frame does not take is not read after the
    /// develop ([`Ai::cached_raster`]). `print` is the print of the
    /// base the raster would be read for ([`crate::mask_base::print`]).
    pub fn raster_ready(
        &mut self,
        file: &Path,
        shape: &Shape,
        turn: u8,
        frame: Option<(usize, usize)>,
        print: Print,
    ) -> Ready {
        if self.file.as_deref() != Some(file) {
            self.forget(Some(file.to_path_buf()));
        }
        self.print = print;
        let rule;
        let widths: &[usize] = match (shape, frame) {
            (_, Some(frame)) => {
                rule = [raster_width(shape, frame)];
                &rule
            }
            (Shape::Sky { .. }, None) => &[RASTER_WIDTH, WIDE_RASTER_WIDTH],
            _ => &[RASTER_WIDTH],
        };
        if widths.iter().any(|&w| {
            self.cached_path_at(shape, None, turn, w)
                .is_some_and(|p| p.is_file())
        }) {
            return Ready::Cached;
        }
        // A Sky is the prior's to make, SAM only sharpening its outline;
        // a Landscape shape the prior's alone.
        let model = match shape {
            Shape::Sky { .. } | Shape::Landscape { .. } => Some(&SKY),
            _ => model_for(shape, self.store.as_ref(), &[]),
        };
        match model {
            Some(m) if !self.store.as_ref().is_some_and(|s| s.have(m)) => Ready::NoModel(m.name),
            _ => Ready::ToMake,
        }
    }

    /// Where the raster for `shape` of `file` at `turn` is kept on disk:
    /// for a test that puts one there as the worker would.
    #[cfg(any(test, feature = "testing"))]
    pub fn cached_path_of(
        &mut self,
        file: &Path,
        shape: &Shape,
        turn: u8,
        print: Print,
    ) -> Option<PathBuf> {
        if self.file.as_deref() != Some(file) {
            self.forget(Some(file.to_path_buf()));
        }
        self.print = print;
        self.cached_path(shape, None, turn)
    }

    /// Models from `store`, with the disk cache of the rasters beside
    /// it: for a test of the reduced develop's masks.
    #[cfg(any(test, feature = "testing"))]
    pub fn for_test_on_disk(store: Store) -> Self {
        Self {
            store: Some(store),
            quiet_fetch: false,
            ..Self::new()
        }
    }

    /// The raster for `shape` of the frame at `file` from the disk cache
    /// alone, for a develop of the frame whose full develop is `full`
    /// (width, height) at `turn`, of the base `print` names: nothing is
    /// loaded and no model runs. The reduced develop's, which keeps no
    /// picture of a frame whose masks are not made yet. The raster's
    /// width is the frame's (`brush::raster_width`).
    pub fn cached_raster(
        &mut self,
        file: &Path,
        shape: &Shape,
        turn: u8,
        full: (u32, u32),
        print: Print,
    ) -> Option<Arc<Raster>> {
        if self.file.as_deref() != Some(file) {
            self.forget(Some(file.to_path_buf()));
        }
        self.print = print;
        let aspect = full.1 as f32 / full.0 as f32;
        let width = raster_width(shape, (full.0 as usize, full.1 as usize));
        let height = raster_height(width, aspect);
        let path = self.cached_path_at(shape, None, turn, width)?;
        // A row either way is the same aspect rounded at another size:
        // the open frame's develop and the reduced one's halved mosaic.
        let data = read_raster_near(&path, width, height)?;
        Some(Arc::new(Raster::from_data(aspect, width, data)))
    }

    /// The raster for `shape`, from the cache when it was made for this
    /// shape on a base of the same print, else from the model on the
    /// preview of `base` (`image`) under `edit`, finished as `kind` is. `model`
    /// is the file to load for a Subject shape when one is not loaded
    /// already — `panel::mask::step`'s own choice, passed down through
    /// `Job::Mask` so the worker never has to re-derive it (and
    /// possibly land on a different file than the one the panel
    /// offered or asked for); `None` from a caller with no session to
    /// ask, which falls back to the plain store-and-providers guess.
    ///
    /// What is made or kept already (a raster cached, a scene kept) is
    /// had without ONNX Runtime; what cannot be had where the runtime
    /// is off says so, rather than the store's or a load's own word
    /// for a model it could not have run anyway.
    // `base` to `turn` are `worker::masks::On`'s picture; moving it is its own change.
    #[allow(clippy::too_many_arguments)]
    pub fn raster(
        &mut self,
        base: Base,
        image: &WorkingImage,
        edit: &Edit,
        kind: greycard_render::finish::Source,
        turn: u8,
        key: Key,
        shape: &Shape,
        model: Option<&'static Model>,
    ) -> Result<Made, String> {
        let frame = (image.width, image.height);
        self.raster_for(frame, base, image, edit, kind, turn, key, shape, model)
    }

    /// [`Ai::raster`] on a picture of a frame whose full develop is
    /// `frame` (width, height): a reduced develop's, whose picture is
    /// smaller than the frame. The raster's width (`brush::raster_width`),
    /// its height and its aspect all follow the frame, not the picture,
    /// whose binning rounds its height: so the reduced develop, the
    /// index's masks and the open frame make the same raster, row for
    /// row, and keep it in the same slot.
    // `base` to `turn` are `worker::masks::On`'s picture; moving it is its own change.
    #[allow(clippy::too_many_arguments)]
    pub fn raster_for(
        &mut self,
        frame: (usize, usize),
        base: Base,
        image: &WorkingImage,
        edit: &Edit,
        kind: greycard_render::finish::Source,
        turn: u8,
        key: Key,
        shape: &Shape,
        model: Option<&'static Model>,
    ) -> Result<Made, String> {
        self.raster_of(frame, base, image, edit, kind, turn, key, shape, model)
            .map_err(|e| match greycard_ai::runtime::status() {
                Err(why) => why.to_string(),
                Ok(_) => e,
            })
    }

    /// [`Ai::raster`] on the base, and made again on the retouched
    /// picture when a Fill patch of `retouched` meets that raster: the
    /// base's raster is the one overlap is measured against, so a fill
    /// that takes a person out does not turn the question back. Beside
    /// the raster, the print of the Fill patches over it
    /// ([`Retouched::print_over`]), none when none is; a shape that
    /// could not be made again keeps the base's raster under that print,
    /// so it is not tried again until the patches change. A fill over it
    /// still unmade leaves the base's raster: the picture there is the
    /// base's.
    // `base` to `turn` are `worker::masks::On`'s picture; moving it is its own change.
    #[allow(clippy::too_many_arguments)]
    pub fn raster_retouched(
        &mut self,
        base: Base,
        image: &WorkingImage,
        retouched: Option<Retouched<'_>>,
        edit: &Edit,
        kind: greycard_render::finish::Source,
        turn: u8,
        key: Key,
        shape: &Shape,
        model: Option<&'static Model>,
    ) -> Result<(Made, Option<u64>), String> {
        let made = self.raster(base, image, edit, kind, turn, key, shape, model)?;
        if made.part.is_some() {
            return Ok((made, None));
        }
        let Some((r, (print, under))) =
            retouched.and_then(|r| Some((r, r.print_over(&made.raster)?)))
        else {
            return Ok((made, None));
        };
        if let Some((s, p, b, raster)) = self.retouched.get(&(key, turn))
            && s == shape
            && *p == print
            && *b == base.print
        {
            let raster = raster.clone();
            return Ok((
                Made {
                    note: self.empty_note(shape, &raster),
                    raster,
                    provider: None,
                    seconds: 0.0,
                    part: None,
                },
                Some(print),
            ));
        }
        // Made as the base's is, on the retouched picture under a stamp
        // of the whole picture's own, with its kept work, into a disk
        // slot of the print's own; the base's raster and kept work stay
        // where they were.
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (base.stamp, r.picture_print(), "retouched").hash(&mut h);
        let kept = self.cache.remove(&(key, turn));
        self.salt = Some(print);
        self.swap_side();
        let again = self.raster(
            Base {
                stamp: h.finish(),
                print: base.print,
            },
            r.image,
            edit,
            kind,
            turn,
            key,
            shape,
            model,
        );
        self.swap_side();
        self.salt = None;
        self.cache.remove(&(key, turn));
        if let Some(k) = kept {
            self.cache.insert((key, turn), k);
        }
        let name = self
            .file
            .as_deref()
            .and_then(|f| f.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let shape_name = shape.name();
        // A failure may pass (a model fetched, a device back): the base's
        // raster stands, given under the print so a develop does not ask
        // again, and is tried again when a model arrives.
        let mut again = match again {
            Ok(m) => m,
            Err(why) => {
                tracing::info!(
                    "the masks of {name}: {shape_name} not made again under its fills ({why}); the base's kept"
                );
                self.fell_back.insert(key);
                return Ok((made, Some(print)));
            }
        };
        self.fell_back.remove(&key);
        match again.part.take() {
            None => {}
            // The person is not on the retouched picture: taken out.
            // Kept in the print's slot, so a reopen does not run the
            // People model again for the same answer.
            Some(Unresolved::Nobody) => {
                let blank = vec![0u8; made.raster.width * made.raster.height];
                self.salt = Some(print);
                self.print = base.print;
                let slot = self.cached_path_at(shape, model, turn, made.raster.width);
                self.salt = None;
                if let Some(path) = &slot {
                    write_raster(path, made.raster.height, &blank);
                }
                again.raster = Arc::new(Raster::from_data(
                    made.raster.aspect,
                    made.raster.width,
                    blank,
                ));
                again.note = None;
            }
            Some(Unresolved::Ask(_)) => {
                tracing::info!(
                    "the masks of {name}: {shape_name} not settled on the retouched picture; the base's kept"
                );
                // Kept under the print: asking again would ask the same.
                self.retouched.insert(
                    (key, turn),
                    (shape.clone(), print, base.print, made.raster.clone()),
                );
                return Ok((made, Some(print)));
            }
        }
        if again.provider.is_some() {
            tracing::info!(
                "the masks of {name}: {shape_name} made again on the retouched picture, under {under} fill{}, in {:.2} s",
                if under == 1 { "" } else { "s" },
                again.seconds
            );
        }
        self.retouched.insert(
            (key, turn),
            (shape.clone(), print, base.print, again.raster.clone()),
        );
        Ok((again, Some(print)))
    }

    /// The open frame's panel was given the raster of `shape` at `key`,
    /// asked with `model`, under the Fill patches of `print`, made from
    /// the base of print `base`.
    pub fn given(
        &mut self,
        key: Key,
        shape: &Shape,
        model: Option<&'static Model>,
        print: Option<u64>,
        base: Print,
    ) {
        self.given.insert(key, (shape.clone(), model, print, base));
    }

    /// The model the component at `key` was recorded as given with:
    /// for a test.
    #[cfg(test)]
    pub(crate) fn given_model_by_test(&self, key: Key) -> Option<Option<&'static Model>> {
        self.given.get(&key).map(|(_, model, _, _)| *model)
    }

    /// A model arrived: the components given the base's raster for a
    /// failed second run, each with its shape and model, to be asked for
    /// again.
    pub fn retry_fallbacks(&mut self) -> Vec<(Key, Shape, Option<&'static Model>)> {
        let mut again: Vec<_> = self
            .fell_back
            .drain()
            .filter_map(|key| {
                let (shape, model, _, _) = self.given.get(&key)?;
                Some((key, shape.clone(), *model))
            })
            .collect();
        again.sort_by_key(|(k, _, _)| *k);
        again
    }

    /// The components given to the panel whose raster a develop of the
    /// base of print `base_print` under `retouched` at `turn` changed
    /// the patches of, still `live` with the same shape: each to be
    /// asked for again, with its model. One given from another base is
    /// [`Ai::of_another_base`]'s.
    pub fn to_ask_again(
        &self,
        retouched: Option<Retouched<'_>>,
        base_print: Print,
        turn: u8,
        live: impl Fn(Key, &Shape) -> bool,
    ) -> Vec<(Key, Shape, Option<&'static Model>)> {
        let mut again: Vec<_> = self
            .given
            .iter()
            .filter(|(key, (shape, _, _, made_from))| {
                *made_from == base_print && live(**key, shape)
            })
            .filter_map(|(key, (shape, model, print, _))| {
                let (s, _, base) = self.cache.get(&(*key, turn))?;
                if s != shape {
                    return None;
                }
                let now = retouched.and_then(|r| r.print_over(base)).map(|p| p.0);
                (now != *print).then(|| (*key, shape.clone(), *model))
            })
            .collect();
        again.sort_by_key(|(k, _, _)| *k);
        again
    }

    /// The components given to the panel from a base other than the one
    /// of print `base_print`, still `live` with the same shape: each to
    /// be made again from this one, with its model.
    pub fn of_another_base(
        &self,
        base_print: Print,
        live: impl Fn(Key, &Shape) -> bool,
    ) -> Vec<(Key, Shape, Option<&'static Model>)> {
        let mut again: Vec<_> = self
            .given
            .iter()
            .filter(|(key, (shape, _, _, made_from))| {
                *made_from != base_print && live(**key, shape)
            })
            .map(|(key, (shape, model, _, _))| (*key, shape.clone(), *model))
            .collect();
        again.sort_by_key(|(k, _, _)| *k);
        again
    }

    /// The raster for `shape` at `key` on a frame whose full develop is
    /// `frame` (width, height), from the cache alone: in memory when it
    /// was made for this shape on a base of `base`'s print, else read
    /// from its slot on disk (and kept in memory); None when neither
    /// has it, or nothing is picked yet. No model loads or runs: what
    /// the open frame's first develop is drawn with, before any ask.
    pub fn raster_cached(
        &mut self,
        frame: (usize, usize),
        base: Base,
        turn: u8,
        key: Key,
        shape: &Shape,
        model: Option<&'static Model>,
    ) -> Option<Made> {
        self.print = base.print;
        if let Some((s, p, r)) = self.cache.get(&(key, turn))
            && s == shape
            && *p == base.print
        {
            return Some(Made {
                note: self.empty_note(shape, r),
                raster: r.clone(),
                provider: None,
                seconds: 0.0,
                part: None,
            });
        }
        if !prompted(shape) {
            return None;
        }
        let width = raster_width(shape, frame);
        let aspect = frame.1 as f32 / frame.0.max(1) as f32;
        let height = raster_height(width, aspect);
        let path = self.cached_path_at(shape, model, turn, width)?;
        let data = read_raster(&path, width, height)?;
        let raster = Arc::new(Raster::from_data(aspect, width, data));
        self.cache
            .insert((key, turn), (shape.clone(), self.print, raster.clone()));
        Some(Made {
            note: self.empty_note(shape, &raster),
            raster,
            provider: None,
            seconds: 0.0,
            part: None,
        })
    }

    // `base` to `turn` are `worker::masks::On`'s picture; moving it is its own change.
    #[allow(clippy::too_many_arguments)]
    fn raster_of(
        &mut self,
        frame: (usize, usize),
        base: Base,
        image: &WorkingImage,
        edit: &Edit,
        kind: greycard_render::finish::Source,
        turn: u8,
        key: Key,
        shape: &Shape,
        model: Option<&'static Model>,
    ) -> Result<Made, String> {
        let stamp = base.stamp;
        if let Some(made) = self.raster_cached(frame, base, turn, key, shape, model) {
            return Ok(made);
        }
        if !prompted(shape) {
            return Err("nothing picked yet".into());
        }
        let width = raster_width(shape, frame);
        let aspect = frame.1 as f32 / frame.0.max(1) as f32;
        let height = raster_height(width, aspect);
        let cached = self.cached_path_at(shape, model, turn, width);
        // A Background reads the Subject matte, the model run once
        // for whichever of the two asks for it first; a hit here
        // needs no store and loads nothing. The provider that made
        // the shared matte is still returned, so the status line and
        // Show mask react as they do to an actual run rather than
        // going quiet the way a plain cache hit does.
        if matches!(shape, Shape::Subject {} | Shape::Background {})
            && let Some((s, base, provider)) = &self.subject_matte
            && *s == stamp
        {
            let data = background_or_not(shape, base);
            if let Some(path) = &cached {
                write_raster(path, height, &data);
            }
            let raster = Arc::new(Raster::from_data(aspect, width, data));
            self.cache
                .insert((key, turn), (shape.clone(), self.print, raster.clone()));
            return Ok(Made {
                raster,
                provider: Some(*provider),
                seconds: 0.0,
                part: None,
                note: None,
            });
        }
        let store = self
            .store
            .clone()
            .ok_or("no cache directory for the models")?;
        if self.providers.is_empty() {
            self.providers = Provider::available();
        }
        self.preview_of(stamp, image, edit, kind);
        if let Shape::Part {
            phrase,
            route,
            person,
        } = shape
        {
            let start = Instant::now();
            let PartMade {
                data,
                provider,
                part,
                edgeless,
            } = self.part(
                (stamp, turn),
                image,
                kind,
                (phrase, route, person.as_ref()),
                (width, height),
                &store,
            )?;
            let raster = Arc::new(Raster::from_data(aspect, width, data));
            let seconds = start.elapsed().as_secs_f64();
            // Only a settled person's part is kept: an ask is asked
            // again, so the panel can show it and an export can say it.
            // A Whole person whose Subject failed to run is kept only
            // here, its disk key being the one with Subject's edge.
            if part.is_none() {
                if let Some(path) = &cached
                    && !edgeless.as_ref().is_some_and(|e| e.failed)
                {
                    write_raster(path, height, raster.data());
                }
                self.cache
                    .insert((key, turn), (shape.clone(), self.print, raster.clone()));
            }
            let note = match (&part, &edgeless) {
                (Some(p), _) => Some(part_note(p, self.file.as_deref())),
                (None, Some(e)) => {
                    tracing::info!("Whole person keeps SAM's edge: {}", e.why);
                    Some(format!(
                        "whole person found on {} in {seconds:.2} s, with a coarser edge: {}",
                        provider.label(),
                        e.why
                    ))
                }
                (None, None) => None,
            };
            return Ok(Made {
                raster,
                provider: Some(provider),
                seconds,
                note,
                part,
            });
        }
        if let Shape::Sky { picks } = shape {
            let start = Instant::now();
            let size = (width, height);
            // With no picks, the sky a Foliage was cut by on this
            // develop, when one was: the same sky, made once.
            let kept = match (&self.sky_cut, &self.scene) {
                (Some((k, matte, note)), Some((s, _, provider)))
                    if picks.is_empty() && *k == (stamp, size) && *s == stamp =>
                {
                    Some((matte.clone(), *provider, note.clone()))
                }
                _ => None,
            };
            let (matte, provider, note) = match kept {
                Some(k) => k,
                None => self.sky_matte(stamp, image, picks, aspect, size, &store)?,
            };
            let t = Instant::now();
            let data = match &matte {
                // The matte is made at the raster's size: no resample,
                // which at the same size would only copy it.
                Some(m) if (m.width, m.height) == size => m.to_u8(),
                Some(m) => m.resampled(width, height).to_u8(),
                None => vec![0u8; width * height],
            };
            if let Some(report) = &mut self.last_sky {
                report.raster = t.elapsed().as_secs_f64();
            }
            if let Some(path) = &cached {
                write_raster(path, height, &data);
            }
            let raster = Arc::new(Raster::from_data(aspect, width, data));
            self.cache
                .insert((key, turn), (shape.clone(), self.print, raster.clone()));
            return Ok(Made {
                note,
                raster,
                provider: Some(provider),
                seconds: start.elapsed().as_secs_f64(),
                part: None,
            });
        }
        if let Shape::Landscape { group } = shape {
            let start = Instant::now();
            let (matte, provider, note) =
                self.landscape_matte(stamp, image, group, (width, height), &store)?;
            let data = match &matte {
                Some(m) => m.to_u8(),
                None => vec![0u8; width * height],
            };
            if let Some(path) = &cached {
                write_raster(path, height, &data);
            }
            let raster = Arc::new(Raster::from_data(aspect, width, data));
            self.cache
                .insert((key, turn), (shape.clone(), self.print, raster.clone()));
            return Ok(Made {
                note,
                raster,
                provider: Some(provider),
                seconds: start.elapsed().as_secs_f64(),
                part: None,
            });
        }
        let start = Instant::now();
        if matches!(shape, Shape::Subject {} | Shape::Background {}) {
            let chosen = self
                .subject_model(model)
                .ok_or("no Subject model to load")?;
            let (base, provider) = self.subject_base(stamp, (width, height), chosen, &store)?;
            let data = background_or_not(shape, &base);
            if let Some(path) = &cached {
                write_raster(path, height, &data);
            }
            let raster = Arc::new(Raster::from_data(aspect, width, data));
            self.cache
                .insert((key, turn), (shape.clone(), self.print, raster.clone()));
            return Ok(Made {
                raster,
                provider: Some(provider),
                seconds: start.elapsed().as_secs_f64(),
                part: None,
                note: None,
            });
        }
        // SAM's decoder alone runs on a kept embedding.
        if matches!(shape, Shape::Object { .. })
            && (self.sam.is_none() || self.embedding.as_ref().is_none_or(|e| e.0 != stamp))
        {
            self.mask_room(MaskModel::Sam)?;
        }
        let (_, rgb, luma) = self.preview.as_ref().expect("a preview was just made");
        let (mask, provider) = match shape {
            Shape::Object { picks, boxes } => {
                let on = self.mask_providers();
                if self.sam.is_none() {
                    self.cpu_chosen.sam = self.mask_cpu;
                }
                let sam = match &mut self.sam {
                    Some(s) => s,
                    None => self
                        .sam
                        .insert(Sam::load(&store, &on).map_err(|e| e.to_string())?),
                };
                if self.embedding.as_ref().is_none_or(|e| e.0 != stamp) {
                    let e = sam.embed(rgb).map_err(|e| e.to_string())?;
                    self.embedding = Some((stamp, e));
                }
                let (_, embedding) = self.embedding.as_ref().expect("just embedded");
                let aspect = image.height as f32 / image.width as f32;
                let mut prompts: Vec<Prompt> = picks
                    .iter()
                    .map(|p| Prompt::Point {
                        x: p.pos[0],
                        y: p.pos[1] / aspect,
                        positive: p.positive,
                    })
                    .collect();
                prompts.extend(boxes.iter().map(|b| Prompt::Box {
                    x0: b[0][0].min(b[1][0]),
                    y0: b[0][1].min(b[1][1]) / aspect,
                    x1: b[0][0].max(b[1][0]),
                    y1: b[0][1].max(b[1][1]) / aspect,
                }));
                let (mask, _) = sam.decode(embedding, &prompts).map_err(|e| e.to_string())?;
                (mask, sam.providers().1)
            }
            _ => return Err("not a learned shape".into()),
        };
        // Snapped to the preview's edges, then to the raster's size.
        let radius = (rgb.width / 256).max(2);
        let refined = refine(&mask, luma, rgb.width, rgb.height, radius, 1e-3);
        dump(rgb, &mask, &refined);
        let data = refined.resampled(width, height).to_u8();
        if let Some(path) = &cached {
            write_raster(path, height, &data);
        }
        let raster = Arc::new(Raster::from_data(aspect, width, data));
        self.cache
            .insert((key, turn), (shape.clone(), self.print, raster.clone()));
        Ok(Made {
            raster,
            provider: Some(provider),
            seconds: start.elapsed().as_secs_f64(),
            part: None,
            note: None,
        })
    }

    /// The preview of base develop `stamp`, made if it is not the one
    /// kept; everything made on the one before goes with it.
    fn preview_of(
        &mut self,
        stamp: u64,
        image: &WorkingImage,
        edit: &Edit,
        kind: greycard_render::finish::Source,
    ) {
        if self.preview.as_ref().is_none_or(|p| p.0 != stamp) {
            let rgb = preview(image, edit, kind);
            let luma = rgb.luma();
            self.preview = Some((stamp, rgb, luma));
            self.embedding = None;
            self.encodings.clear();
            self.people = None;
        }
    }

    /// The People model and its table, loaded the first time.
    fn parts_model(&mut self, store: &Store) -> Result<(), String> {
        if self.providers.is_empty() {
            self.providers = Provider::available();
        }
        if self.presets.is_none() {
            self.presets = Some(Presets::of(store).map_err(|e| e.to_string())?);
        }
        if self.sam3.is_none() {
            self.mask_room(MaskModel::People)?;
            self.sam3 = Some(Sam3::load(store, &self.mask_providers()).map_err(|e| e.to_string())?);
            self.cpu_chosen.sam3 = self.mask_cpu;
        }
        Ok(())
    }

    /// The people on the unturned, look-neutral view of base develop
    /// `stamp` of `image`, as the panel picks among them, the provider
    /// the model ran on and the seconds it took: for a Part chosen from
    /// the menu, which is of the person picked among them, or of the
    /// one person when there is one.
    pub fn people(
        &mut self,
        stamp: u64,
        image: &WorkingImage,
        kind: greycard_render::finish::Source,
        turn: u8,
    ) -> Result<(Vec<Candidate>, Provider, f64), String> {
        greycard_ai::runtime::status().map_err(|why| why.to_string())?;
        let store = self
            .store
            .clone()
            .ok_or("no cache directory for the models")?;
        let start = Instant::now();
        // The People model's encoder runs when the people of this
        // develop are not kept.
        if self.people.as_ref().is_none_or(|(s, _)| *s != stamp) {
            self.mask_room(MaskModel::People)?;
        }
        self.parts_model(&store)?;
        self.parts_view_of(stamp, turn, image, kind);
        let hash = self.picture_hash();
        let Ai {
            sam3,
            presets,
            encodings,
            parts_view,
            people,
            ..
        } = self;
        let sam = sam3.as_mut().expect("the People model was just loaded");
        let presets = presets.as_ref().expect("the table was just read");
        let view = parts_view.as_ref().expect("the view was just made");
        let size = unturned_size(image, turn);
        let mut region = |r: sam3::Rect| parts_region(image, kind, turn, size, r);
        let mut picture = Picture {
            preview: &view.preview,
            luma: &view.luma,
            size,
            key: stamp,
            region: &mut region,
            encodings,
            matte: None,
        };
        let found = people_of(sam, presets, &mut picture, people)?;
        let aspect = image.height as f32 / image.width as f32;
        let out = found
            .iter()
            .map(|p| Candidate::of(p, turn, aspect, &hash))
            .collect();
        Ok((out, sam.providers().0, start.elapsed().as_secs_f64()))
    }

    /// `wanted`, a person picked on another frame, on the look-neutral
    /// view of `image` (a frame's reduced develop, at `turn`), for a
    /// People shape that came here of `carried`: left as it is when
    /// `carried` is decided alone here, else this frame's own pick of
    /// `wanted` when they are found ([`same_person`]), else not found.
    pub fn person_on(
        &mut self,
        image: &WorkingImage,
        kind: greycard_render::finish::Source,
        turn: u8,
        carried: Option<&greycard_edit::mask::Person>,
        wanted: &Picked,
    ) -> Result<PersonOn, String> {
        greycard_ai::runtime::status().map_err(|why| why.to_string())?;
        let store = self
            .store
            .clone()
            .ok_or("no cache directory for the models")?;
        // Another frame: the People model's encoder runs on it.
        self.mask_room(MaskModel::People)?;
        self.parts_model(&store)?;
        // One frame's develop a call: the stamp says nothing more.
        let stamp = 1;
        self.parts_view_of(stamp, turn, image, kind);
        let hash = self.picture_hash();
        let Ai {
            sam3,
            presets,
            encodings,
            parts_view,
            people,
            ..
        } = self;
        let sam = sam3.as_mut().expect("the People model was just loaded");
        let presets = presets.as_ref().expect("the table was just read");
        let view = parts_view.as_ref().expect("the view was just made");
        let size = unturned_size(image, turn);
        let mut region = |r: sam3::Rect| parts_region(image, kind, turn, size, r);
        let mut picture = Picture {
            preview: &view.preview,
            luma: &view.luma,
            size,
            key: stamp,
            region: &mut region,
            encodings,
            matte: None,
        };
        let found = people_of(sam, presets, &mut picture, people)?;
        let aspect = image.height as f32 / image.width as f32;
        let vaspect = view.preview.height as f32 / view.preview.width as f32;
        let candidates: Vec<Candidate> = found
            .iter()
            .map(|q| Candidate::of(q, turn, aspect, &hash))
            .collect();
        if let Some(p) = carried {
            let own = hash.is_some() && p.picture == hash;
            if let Choice::Person(..) = settle(p, found, &candidates, own, turn, (aspect, vaspect))
            {
                return Ok(PersonOn::Settled);
            }
        }
        Ok(
            match same_person(wanted, found, &candidates, turn, (aspect, vaspect)) {
                Some(i) => PersonOn::Found(candidates[i].person()),
                None => PersonOn::NotFound,
            },
        )
    }

    /// The People model's view of base develop `stamp`, made at `turn`
    /// (`parts_view`), made if it is not the one kept; the encodings
    /// and the people made on the one before go with it.
    fn parts_view_of(
        &mut self,
        stamp: u64,
        turn: u8,
        image: &WorkingImage,
        kind: greycard_render::finish::Source,
    ) {
        if self
            .parts_view
            .as_ref()
            .is_none_or(|v| v.stamp != stamp || v.turn != turn)
        {
            let shown = preview(image, &neutral(), kind);
            let rgb = rotate_rgb(&shown, (4 - turn % 4) % 4);
            let luma = rgb.luma();
            self.parts_view = Some(PartsView {
                stamp,
                turn,
                preview: rgb,
                luma,
            });
            self.encodings.clear();
            self.people = None;
        }
    }

    /// A Part's raster at `raster` (width, height), on the preview of
    /// base develop `stamp`, and the provider: of the person the
    /// shape names, found again on this picture (`sam3::choose`), or
    /// of everyone. Empty, and why, where the person is not settled.
    /// A Whole person takes Subject's edge (`person_edge`), run only
    /// once the person is settled; where it cannot, why.
    fn part(
        &mut self,
        (stamp, turn): (u64, u8),
        image: &WorkingImage,
        kind: greycard_render::finish::Source,
        (phrase, route, person): PartAsked,
        raster: (usize, usize),
        store: &Store,
    ) -> Result<PartMade, String> {
        let route = route_of(route)?;
        // The People model's encoder runs on a picture it has not
        // encoded; a second Part on one it has decodes alone. Asked
        // before the model loads, since a give-back would drop it.
        if self.encodings.key() != Some(stamp) || !self.encodings.has_preview() {
            self.mask_room(MaskModel::People)?;
        }
        self.parts_model(store)?;
        self.parts_view_of(stamp, turn, image, kind);
        let hash = self.picture_hash();
        let who = match self.settled((stamp, turn), image, kind, phrase, person, &hash)? {
            Ok(who) => who,
            Err(why) => {
                return Ok(PartMade {
                    data: vec![0u8; raster.0 * raster.1],
                    provider: self.sam3.as_ref().expect("loaded").providers().0,
                    part: Some(why),
                    edgeless: None,
                });
            }
        };
        let (matte, edgeless) = if route == sam3::Route::Whole && phrase == sam3::PERSON {
            match self.person_edge(stamp, turn, raster, store) {
                Ok(m) => (Some(m), None),
                Err(e) => (None, Some(e)),
            }
        } else {
            (None, None)
        };
        // Subject's run may have given the card back, the People model
        // with it: loaded again, its encodings kept.
        self.parts_model(store)?;
        let Ai {
            sam3,
            presets,
            encodings,
            parts_view,
            ..
        } = self;
        let sam = sam3.as_mut().expect("the People model was just loaded");
        let presets = presets.as_ref().expect("the table was just read");
        let provider = sam.providers().0;
        let view = parts_view.as_ref().expect("the view was just made");
        let size = unturned_size(image, turn);
        let mut region = |r: sam3::Rect| parts_region(image, kind, turn, size, r);
        let mut picture = Picture {
            preview: &view.preview,
            luma: &view.luma,
            size,
            key: stamp,
            region: &mut region,
            encodings,
            matte: matte.as_ref(),
        };
        let pieces = sam3::find(sam, presets, &mut picture, route, phrase, who.as_ref())
            .map_err(|e| e.to_string())?;
        // Drawn on the unturned picture, then turned as the picture is.
        let unturned = if turn % 2 == 1 {
            (raster.1, raster.0)
        } else {
            raster
        };
        let drawn = sam3::draw(&pieces, picture.size, unturned);
        let data: Vec<u8> = drawn
            .iter()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect();
        let (data, _, _) = rotate(&data, unturned.0, unturned.1, 1, turn);
        Ok(PartMade {
            data,
            provider,
            part: None,
            edgeless,
        })
    }

    /// Which person on the People view of base develop `stamp` a
    /// part asked of `person` is of: `None` for everyone, or why it is
    /// not settled here.
    fn settled(
        &mut self,
        (stamp, turn): (u64, u8),
        image: &WorkingImage,
        kind: greycard_render::finish::Source,
        phrase: &str,
        person: Option<&greycard_edit::mask::Person>,
        hash: &Option<String>,
    ) -> Result<Result<Option<sam3::Person>, Unresolved>, String> {
        let Ai {
            sam3,
            presets,
            encodings,
            parts_view,
            people,
            ..
        } = self;
        let sam = sam3.as_mut().expect("the People model was just loaded");
        let presets = presets.as_ref().expect("the table was just read");
        // Said before any model runs: a phrase not in the table is
        // never asked of a text encoder.
        if presets.phrase(phrase).is_none() {
            return Err(format!("\"{phrase}\" is not in the People model's table"));
        }
        let Some(p) = person else {
            return Ok(Ok(None));
        };
        let view = parts_view.as_ref().expect("the view was just made");
        let size = unturned_size(image, turn);
        let mut region = |r: sam3::Rect| parts_region(image, kind, turn, size, r);
        let mut picture = Picture {
            preview: &view.preview,
            luma: &view.luma,
            size,
            key: stamp,
            region: &mut region,
            encodings,
            matte: None,
        };
        // The picture as it stands: its height over its width, the
        // masks' units' aspect.
        let aspect = image.height as f32 / image.width as f32;
        let found = people_of(sam, presets, &mut picture, people)?;
        let candidates: Vec<Candidate> = found
            .iter()
            .map(|q| Candidate::of(q, turn, aspect, hash))
            .collect();
        let own = hash.is_some() && p.picture == *hash;
        let vaspect = view.preview.height as f32 / view.preview.width as f32;
        let choice = settle(p, found, &candidates, own, turn, (aspect, vaspect));
        Ok(decided(choice, candidates).map(|i| Some(found[i].clone())))
    }

    /// The Subject model a Whole person's edge would run: the one
    /// loaded, else the file the store and the providers pick, else
    /// whichever of the two the store has; none with neither.
    fn subject_in_store(&self) -> Option<&'static Model> {
        if let Some(s) = &self.subject {
            return Some(s.model());
        }
        let store = self.store.as_ref()?;
        model_for(&Shape::Subject {}, Some(store), &[])
            .filter(|m| store.have(m))
            .or_else(|| {
                [&greycard_ai::SUBJECT, &greycard_ai::SUBJECT_WEBGPU]
                    .into_iter()
                    .find(|m| store.have(m))
            })
    }

    /// The Subject matte of base develop `stamp` at the raster's size
    /// (`raster`, as turned), as a Subject shape makes it, and the
    /// provider it ran on: the one kept in `subject_matte` when it is
    /// this develop's, else a run of `model`, kept there after. A
    /// Subject, a Background and a Whole person on one picture cost
    /// the model one run.
    fn subject_base(
        &mut self,
        stamp: u64,
        raster: (usize, usize),
        model: &'static Model,
        store: &Store,
    ) -> Result<(Vec<u8>, Provider), String> {
        if let Some((s, base, provider)) = &self.subject_matte
            && *s == stamp
        {
            return Ok((base.clone(), *provider));
        }
        self.mask_room(MaskModel::Subject)?;
        let providers = &self.mask_providers();
        if self.subject.is_none() {
            self.cpu_chosen.subject = self.mask_cpu;
        }
        let Ai {
            subject, preview, ..
        } = self;
        let (_, rgb, luma) = preview.as_ref().expect("a preview is made before a mask");
        let subject = match subject {
            Some(s) => s,
            None => subject
                .insert(Subject::load_model(store, model, providers).map_err(|e| e.to_string())?),
        };
        let mask = subject.mask(rgb).map_err(|e| e.to_string())?;
        let provider = subject.provider();
        let radius = (rgb.width / 256).max(2);
        let refined = refine(&mask, luma, rgb.width, rgb.height, radius, 1e-3);
        dump(rgb, &mask, &refined);
        let base = refined.resampled(raster.0, raster.1).to_u8();
        self.subject_matte = Some((stamp, base.clone(), provider));
        Ok((base, provider))
    }

    /// Subject's matte for a Whole person on base develop `stamp` made
    /// at `turn`, unturned as the People view is: the one a Subject or
    /// Background made on it, in memory or on disk, else a run of the
    /// Subject model, kept for them as theirs. Without the model in
    /// the store, or with a run that fails, why: the person then keeps
    /// SAM's edge, and the People run never waits on a download.
    fn person_edge(
        &mut self,
        stamp: u64,
        turn: u8,
        raster: (usize, usize),
        store: &Store,
    ) -> Result<greycard_ai::Mask, Edgeless> {
        #[cfg(any(test, feature = "testing"))]
        if self.sams_edge_only {
            return Err(Edgeless {
                why: "SAM's edge asked for by a test".into(),
                failed: false,
            });
        }
        let Some(model) = self.subject_in_store() else {
            return Err(Edgeless {
                why: "the Subject model is not downloaded".into(),
                failed: false,
            });
        };
        let kept = self
            .subject_matte
            .as_ref()
            .filter(|(s, _, _)| *s == stamp)
            .map(|(_, base, _)| base.clone());
        let on_disk = || {
            let path = self.cached_path(&Shape::Subject {}, Some(model), turn)?;
            read_raster(&path, raster.0, raster.1)
        };
        let base = match kept.or_else(on_disk) {
            Some(base) => base,
            None => {
                let (base, _) = self
                    .subject_base(stamp, raster, model, store)
                    .map_err(|e| Edgeless {
                        why: format!("Subject did not run ({e})"),
                        failed: true,
                    })?;
                if let Some(path) = self.cached_path(&Shape::Subject {}, Some(model), turn) {
                    write_raster(&path, raster.1, &base);
                }
                base
            }
        };
        let (data, w, h) = rotate(&base, raster.0, raster.1, 1, (4 - turn % 4) % 4);
        Ok(greycard_ai::Mask::new(
            w,
            h,
            data.into_iter().map(|v| v as f32 / 255.0).collect(),
        ))
    }

    /// What the status line says of a Sky or Landscape raster that
    /// comes back from a cache, in memory or on disk, with nothing in
    /// it: the gate found none. The raster is the only record a cache
    /// keeps, and an empty one means exactly that: once a gate passes,
    /// a share of the frame is labeled the shape (half a percent for a
    /// sky; a core of a quarter of one for a landscape group), the
    /// outline is never empty (the labels are its fallback, or are it),
    /// and so neither is the matte made from it. A raster made just now
    /// carries the gate's own word (`sky_matte`, `landscape_matte`).
    /// Nothing for any other shape.
    fn empty_note(&self, shape: &Shape, raster: &Raster) -> Option<String> {
        if raster.data().iter().any(|&v| v > 0) {
            return None;
        }
        match shape {
            Shape::Sky { .. } => Some(no_sky_note(self.file.as_deref())),
            Shape::Landscape { group } => Some(none_found_note(
                &group.label().to_lowercase(),
                self.file.as_deref(),
            )),
            _ => None,
        }
    }

    /// The scene of base develop `stamp`, made on its preview once for
    /// the develop whoever asks first (a Sky shape, the Landscape
    /// button or a Landscape shape), the sky model loaded for it then.
    /// The provider it ran on, the seconds loading took and the seconds
    /// the run and the labeling took, both none when it was kept.
    fn scene_of(&mut self, stamp: u64, store: &Store) -> Result<(Provider, f64, f64), String> {
        if let Some((s, _, provider)) = &self.scene
            && *s == stamp
        {
            return Ok((*provider, 0.0, 0.0));
        }
        self.mask_room(MaskModel::Sky)?;
        let providers = &self.mask_providers();
        if self.sky.is_none() {
            self.cpu_chosen.sky = self.mask_cpu;
        }
        let Ai {
            sky: loaded,
            scene,
            preview,
            ..
        } = self;
        let (_, rgb, _) = preview.as_ref().expect("a preview is made before a mask");
        let mut load = 0.0;
        if loaded.is_none() {
            let t = Instant::now();
            *loaded = Some(Sky::load(store, providers).map_err(|e| e.to_string())?);
            core_ml_loaded(SKY.id);
            load = t.elapsed().as_secs_f64();
        }
        let model = loaded.as_mut().expect("the Sky model was just loaded");
        let t = Instant::now();
        let made = model.scene(rgb).map_err(|e| e.to_string())?;
        let run = t.elapsed().as_secs_f64();
        *scene = Some((stamp, made, model.provider()));
        #[cfg(any(test, feature = "testing"))]
        {
            self.scene_runs += 1;
        }
        Ok((model.provider(), load, run))
    }

    /// What the Landscape button offers on the preview of base develop
    /// `stamp` of `image` under `edit`: the sky and the landscape's
    /// groups whose gates pass, the largest first; the provider the
    /// model ran on and the seconds the answer took. The model runs
    /// once a develop, for this and for every Sky and Landscape shape
    /// made on it.
    pub fn landscape(
        &mut self,
        stamp: u64,
        image: &WorkingImage,
        edit: &Edit,
        kind: greycard_render::finish::Source,
    ) -> Result<(Vec<Offer>, Provider, f64), String> {
        let store = self
            .store
            .clone()
            .ok_or("no cache directory for the models")?;
        if self.providers.is_empty() {
            self.providers = Provider::available();
        }
        let start = Instant::now();
        self.preview_of(stamp, image, edit, kind);
        let (provider, load, run) = self.scene_of(stamp, &store)?;
        let scene = &self.scene.as_ref().expect("the scene was just made").1;
        let offers = scene.offers();
        let name = self
            .file
            .as_deref()
            .and_then(|f| f.file_name())
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let groups: Vec<String> = landscape::Group::ALL
            .into_iter()
            .filter(|&g| scene.measure(g).area > 0.0)
            .map(|g| {
                let m = scene.measure(g);
                format!(
                    "{} {:.1}% at {:.3}, core {:.2}% ({})",
                    g.name(),
                    m.area * 100.0,
                    m.mean,
                    m.core * 100.0,
                    match scene.gate(g) {
                        Ok(()) => "offered".to_string(),
                        Err(why) => why.says(g),
                    }
                )
            })
            .collect();
        tracing::info!(
            "landscape of {name}: model and labels {run:.3}s on {} (load {load:.2}s); sky {}; {}",
            provider.name(),
            match sky::gate(&scene.prior) {
                Ok(()) => "offered".to_string(),
                Err(why) => why.to_string(),
            },
            groups.join("; ")
        );
        Ok((offers, provider, start.elapsed().as_secs_f64()))
    }

    /// A Landscape shape's matte for `group` on the preview of base
    /// develop `stamp`, at `size`, or none when its gate refuses it;
    /// the provider the prior ran on, and what the status line says
    /// instead of the usual.
    fn landscape_matte(
        &mut self,
        stamp: u64,
        image: &WorkingImage,
        group: &greycard_edit::mask::LandscapeGroup,
        size: (usize, usize),
        store: &Store,
    ) -> Result<(Option<greycard_ai::Mask>, Provider, Option<String>), String> {
        let label = group.label();
        let group = landscape::Group::of_name(group.name())
            .ok_or_else(|| format!("{label} is not a part of the landscape this build knows"))?;
        let (provider, load, run) = self.scene_of(stamp, store)?;
        // The sky a group is cut by: the Sky shape with no picks, as
        // it makes one (SAM's outline when SAM is in the store), kept
        // for the develop whichever asks first, a Sky or a Foliage.
        if group.capped_by_sky() && self.sky_cut.as_ref().is_none_or(|c| c.0 != (stamp, size)) {
            let t = Instant::now();
            let aspect = image.height as f32 / image.width.max(1) as f32;
            let (cut, _, _) = self.sky_matte(stamp, image, &[], aspect, size, store)?;
            tracing::info!(
                "the sky Foliage is cut by: {} in {:.3}s",
                if cut.is_some() { "found" } else { "none" },
                t.elapsed().as_secs_f64()
            );
        }
        let scene = &self.scene.as_ref().expect("the scene was just made").1;
        let (_, rgb, luma) = self
            .preview
            .as_ref()
            .expect("a preview is made before a mask");
        let frame = sky::Frame {
            display: rgb,
            luma,
            linear: image,
        };
        let name = self
            .file
            .as_deref()
            .and_then(|f| f.file_name())
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let t = Instant::now();
        let cut = self
            .sky_cut
            .as_ref()
            .filter(|_| group.capped_by_sky())
            .and_then(|c| c.1.as_ref());
        let found = landscape::find(scene, group, &frame, size, cut);
        let edge = t.elapsed().as_secs_f64();
        Ok(match found {
            landscape::Found::Shape { matte, matted } => {
                tracing::info!(
                    "{} of {name}: prior {run:.3}s on {} (load {load:.2}s), edge {edge:.3}s ({})",
                    group.name(),
                    provider.name(),
                    if matted { "matte" } else { "feathered" }
                );
                (Some(matte), provider, None)
            }
            landscape::Found::None(why) => {
                tracing::info!("no {} found in {name}: {}", group.name(), why.says(group));
                (
                    None,
                    provider,
                    Some(none_found_note(group.name(), self.file.as_deref())),
                )
            }
        })
    }

    /// A Sky shape's matte on the preview of base develop `stamp`, at
    /// the preview's size, or none when the gate finds no sky; and the
    /// provider the prior ran on. The prior is kept for the develop,
    /// as SAM's embedding is, so a pick decodes again and no more.
    /// SAM is loaded and the preview embedded only once the gate has
    /// passed, so a frame with no sky never pays for them; with SAM
    /// not in the store the prior's own labels are the outline.
    fn sky_matte(
        &mut self,
        stamp: u64,
        image: &WorkingImage,
        picks: &[greycard_edit::mask::Pick],
        aspect: f32,
        size: (usize, usize),
        store: &Store,
    ) -> Result<(Option<greycard_ai::Mask>, Provider, Option<String>), String> {
        let mut report = SkyReport::default();
        let any_positive = picks.iter().any(|p| p.positive);
        let (provider, load, run) = self.scene_of(stamp, store)?;
        report.prior_on = Some(provider);
        report.load = load;
        report.prior = run;
        // SAM's load or its encoder's run asks the card as the Object
        // shape's does, before the decode, and only when the gate
        // passes, so a frame with no sky never pays for it. Refused,
        // the prior's own labels are the outline, as for SAM failing.
        let cold = self.sam.is_none() || self.embedding.as_ref().is_none_or(|e| e.0 != stamp);
        let gated = self
            .scene
            .as_ref()
            .is_some_and(|(_, s, _)| sky::gate(&s.prior).is_ok());
        let refused = if cold && gated && store.have(&SAM) {
            self.mask_room(MaskModel::Sam).err()
        } else {
            None
        };
        let on = self.mask_providers();
        if self.sam.is_none() {
            self.cpu_chosen.sam = self.mask_cpu;
        }
        let Ai {
            scene,
            sam,
            embedding,
            preview,
            file,
            ..
        } = self;
        let (_, rgb, luma) = preview.as_ref().expect("a preview is made before a mask");
        let prior = &scene.as_ref().expect("the scene was just made").1.prior;
        let frame = sky::Frame {
            display: rgb,
            luma,
            linear: image,
        };
        // Picks are in the masks' units, the height in widths.
        let picks: Vec<([f32; 2], bool)> = picks
            .iter()
            .map(|p| ([p.pos[0], p.pos[1] / aspect], p.positive))
            .collect();
        let found = {
            let mut decode =
                |prompts: &[Prompt]| -> greycard_ai::runtime::Result<greycard_ai::Mask> {
                    if let Some(why) = &refused {
                        return Err(greycard_ai::runtime::Error::Short(why.clone()));
                    }
                    if sam.is_none() {
                        let t = Instant::now();
                        *sam = Some(Sam::load(store, &on)?);
                        report.sam_load = t.elapsed().as_secs_f64();
                    }
                    let s = sam.as_mut().expect("SAM was just loaded");
                    report.sam_on = Some(s.providers().0);
                    if embedding.as_ref().is_none_or(|e| e.0 != stamp) {
                        let t = Instant::now();
                        *embedding = Some((stamp, s.embed(rgb)?));
                        report.embed = t.elapsed().as_secs_f64();
                    }
                    let (_, e) = embedding.as_ref().expect("the preview was just embedded");
                    report.decodes += 1;
                    s.decode(e, prompts).map(|(m, _)| m)
                };
            let decode: Option<&mut sky::Decode> = if store.have(&SAM) {
                Some(&mut decode)
            } else {
                None
            };
            let mut times = sky::Times::default();
            let found = sky::find(prior, &frame, &picks, decode, size, &mut times);
            report.stages = times;
            found.map_err(|e| e.to_string())?
        };
        let name = file
            .as_deref()
            .and_then(|f| f.file_name())
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let have_sam = store.have(&SAM);
        let (matte, note) = match found {
            sky::Found::Sky {
                matte,
                seeded,
                outline,
                sam_failed,
            } => {
                report.seeded = seeded;
                report.outline = Some(outline);
                let note = match sam_failed {
                    Some(e) => Some(format!(
                        "sky found from the model's labels alone: the outline model failed ({e})"
                    )),
                    None if any_positive && !have_sam => Some(
                        "sky found; a pick that adds sky needs the Object model, which is not \
                         downloaded"
                            .to_string(),
                    ),
                    None => None,
                };
                (Some(matte), note)
            }
            sky::Found::None(why) => {
                tracing::info!("no sky found in {name}: {why}");
                report.no_sky = Some(why);
                (None, Some(no_sky_note(file.as_deref())))
            }
        };
        let st = report.stages;
        tracing::info!(
            "sky of {name}: prior {:.3}s on {} (load {:.2}s), gate {:.3}s, seeds {:.3}s, \
             outline {:.3}s ({} decodes; SAM load {:.2}s, embed {:.3}s{}), edge {:.3}s",
            report.prior,
            provider.name(),
            report.load,
            st.gate,
            st.seeds,
            st.outline,
            report.decodes,
            report.sam_load,
            report.embed,
            report
                .sam_on
                .map_or(String::new(), |p| format!(" on {}", p.name())),
            st.edge,
        );
        self.last_sky = Some(report);
        // A sky with no picks is the one Foliage is cut by: kept for
        // the develop, so a Foliage after it, or a Sky after a Foliage,
        // costs no second sky. Only at a size Foliage asks for: a Sky's
        // wide matte is no Foliage's cut, and kept it would hold its
        // floats for nothing and put out the narrow one.
        if is_foliages_cut(!picks.is_empty(), size) {
            self.sky_cut = Some(((stamp, size), matte.clone(), note.clone()));
        }
        Ok((matte, provider, note))
    }
}

/// Whether a Sky made at `size`, `picked` or not, is the sky Foliage is
/// cut by, kept for the develop: no picks, and the width Foliage's
/// raster takes (`brush::raster_width` gives Foliage the narrow one on
/// every frame).
fn is_foliages_cut(picked: bool, size: (usize, usize)) -> bool {
    !picked && size.0 == RASTER_WIDTH
}

/// A Part's raster as `Ai::part` made it.
struct PartMade {
    data: Vec<u8>,
    provider: Provider,
    /// Why it is empty: its person is not settled here.
    part: Option<Unresolved>,
    /// Why a Whole person kept SAM's edge.
    edgeless: Option<Edgeless>,
}

/// Why a Whole person kept SAM's edge rather than taking Subject's.
#[derive(Debug)]
struct Edgeless {
    why: String,
    /// The Subject model was in the store and did not run: the raster
    /// is not kept on disk, whose key says Subject was there.
    failed: bool,
}

/// Whether `shape` is a Whole person, whose edge is Subject's matte
/// inside the person's region (`sam3::find`), so whose raster changes
/// when a Subject model arrives.
pub fn takes_subject_edge(shape: &Shape) -> bool {
    matches!(
        shape,
        Shape::Part { phrase, route: greycard_edit::mask::Route::Whole, .. }
            if phrase == sam3::PERSON
    )
}

/// A Part as `Ai::part` takes it: the phrase, its route and the person.
type PartAsked<'a> = (
    &'a str,
    &'a greycard_edit::mask::Route,
    Option<&'a greycard_edit::mask::Person>,
);

/// The base's size turned back to the camera's own orientation.
fn unturned_size(image: &WorkingImage, turn: u8) -> (usize, usize) {
    if turn % 2 == 1 {
        (image.height, image.width)
    } else {
        (image.width, image.height)
    }
}

/// A rectangle of the unturned picture, `size` its pixels, rendered as
/// the People model's view is: under no edit, at its own pixels, turned
/// back to the camera's orientation.
fn parts_region(
    image: &WorkingImage,
    kind: greycard_render::finish::Source,
    turn: u8,
    size: (usize, usize),
    rect: sam3::Rect,
) -> Rgb8 {
    let shown = region(image, &neutral(), kind, turn_rect(turn, size, rect));
    rotate_rgb(&shown, (4 - turn % 4) % 4)
}

/// Who a Part's `person` is among `found`, the people on a picture's
/// unturned view (`candidates`, the same people on the picture as it
/// stands, turned `turn`), `own` when it is the picture they were
/// picked on (by its hash), `aspect` the picture's height over its
/// width as it stands and `vaspect` the view's. On their own picture,
/// the face at `at`, whatever the picture's colors or turn have become
/// since and however many faces are found there now, as long as it is
/// still near their signature (look-neutral, so steady): a turn the
/// shape did not follow, or two files that hash alike, fall through to
/// the rule for other pictures, `sam3::choose`. A face at `at` that is
/// no longer near their signature (its base changed: white balance,
/// profile, lens, noise or demosaic) is not taken alone, but where the
/// rule would not decide either, it asks with that face the guess
/// rather than finding nobody. A person picked on a group and the one
/// person of a portrait are settled alike.
fn settle(
    p: &greycard_edit::mask::Person,
    found: &[sam3::Person],
    candidates: &[Candidate],
    own: bool,
    turn: u8,
    (aspect, vaspect): (f32, f32),
) -> Choice {
    let wanted = sam3::Signature::new(
        sam3::SignatureKind::from_name(&p.signature.kind),
        p.signature.values.clone(),
    );
    let near = |i: usize| {
        wanted.as_ref().is_some_and(|w| {
            let s = &found[i].signature;
            sam3::same_render(w, s) || sam3::distance(w, s).is_some_and(|d| d <= sam3::BOUND)
        })
    };
    let own_face = own.then(|| at_face(candidates, p.at)).flatten();
    if let Some(i) = own_face.filter(|&i| near(i)) {
        return Choice::Person(i, 0.0);
    }
    let there: Vec<(sam3::Signature, [f32; 2])> =
        found.iter().map(|q| (q.signature.clone(), q.at)).collect();
    // `at` in the unturned view's units, as the people's are.
    let f = turn_frac((4 - turn % 4) % 4, [p.at[0], p.at[1] / aspect]);
    let at = [f[0], f[1] * vaspect];
    let choice = match &wanted {
        Some(w) => sam3::choose(w, at, &there),
        // A signature of a known kind that does not read (a value too
        // many or too few) is anyone's.
        None if found.is_empty() => Choice::Nobody,
        None => Choice::Ask { guess: None },
    };
    match (choice, own_face) {
        // On their own picture with a face where theirs was, but read
        // apart from them now (a white balance, profile, lens or
        // demosaic changed since moves the render the signatures are
        // read from): asked, that face the guess, never nobody.
        (Choice::Ask { .. } | Choice::Nobody, Some(i)) => Choice::Ask { guess: Some(i) },
        (choice, _) => choice,
    }
}

/// What `Ai::part` makes of a `choice` among `people`: the person
/// at an index, or why the raster is empty. A person not found
/// (`Choice::Nobody`) on a picture with people on it asks as an unsure
/// choice does, with no guess and words of its own, so one of them can
/// be picked to have the part; on a picture with no one, nobody.
fn decided(choice: Choice, people: Vec<Candidate>) -> Result<usize, Unresolved> {
    let (guess, why) = match choice {
        Choice::Person(i, _) => return Ok(i),
        Choice::Nobody if people.is_empty() => return Err(Unresolved::Nobody),
        Choice::Ask { guess } => (guess, Asking::Unsure),
        Choice::Nobody => (None, Asking::SomeoneElse),
    };
    Err(Unresolved::Ask(Ask { people, guess, why }))
}

/// Which face on another frame of the set a person picked on one frame
/// is, for "use this person on all": the face the match decides alone
/// (`choice`, `sam3::choose` of their signature: the one person of a
/// portrait within BOUND), else on a group the match's guess (within
/// BOUND and clearly the nearest) when it is also the face where theirs
/// was on the frame picked on (`placed`, `at_face` of the pick's
/// place). On a group colors alone never decide, and neither does the
/// place alone, which is whoever stands there; the two together are
/// the people of one sequence. A frame where they part, reframed or
/// its people moved, finds no one and is left asking.
///
/// A person picked out of a group (`group`) is one of several who were
/// near in colors there, so on a frame where the match decides alone,
/// one face found, the place must agree too: a groupmate standing on
/// her own is not taken for them. A person of a portrait is decided by
/// their colors alone, as a pasted mask's is.
fn taken(choice: Choice, placed: Option<usize>, group: bool) -> Option<usize> {
    match choice {
        Choice::Person(i, _) if !group || placed == Some(i) => Some(i),
        Choice::Ask { guess: Some(g) } if placed == Some(g) => Some(g),
        Choice::Person(..) | Choice::Ask { .. } | Choice::Nobody => None,
    }
}

/// A person picked for a People shape, as "use this person on all"
/// looks for them on the other frames.
#[derive(Debug, Clone, PartialEq)]
pub struct Picked {
    /// As the pick stored them.
    pub person: greycard_edit::mask::Person,
    /// Picked out of an ask of two or more people.
    pub group: bool,
    /// The frame picked on, its quarter turns and its height over its
    /// width as it stood: the place is compared only on a frame that
    /// stands the same way.
    pub turn: u8,
    pub aspect: f32,
}

/// [`taken`] of `wanted` among `found` (and `candidates`, the same as
/// the panel has them), on a picture turned `turn`, `aspect` its
/// height over its width and `vaspect` the People view's. A frame
/// turned another way than the one picked on, or of another shape,
/// has no place to compare: there the place is unknown, which on a
/// group takes no one.
fn same_person(
    wanted: &Picked,
    found: &[sam3::Person],
    candidates: &[Candidate],
    turn: u8,
    (aspect, vaspect): (f32, f32),
) -> Option<usize> {
    let p = &wanted.person;
    let w = sam3::Signature::new(
        sam3::SignatureKind::from_name(&p.signature.kind),
        p.signature.values.clone(),
    )?;
    let there: Vec<(sam3::Signature, [f32; 2])> =
        found.iter().map(|q| (q.signature.clone(), q.at)).collect();
    let f = turn_frac((4 - turn % 4) % 4, [p.at[0], p.at[1] / aspect]);
    let choice = sam3::choose(&w, [f[0], f[1] * vaspect], &there);
    let alike = wanted.turn % 4 == turn % 4 && (wanted.aspect - aspect).abs() <= 0.01;
    let placed = alike.then(|| at_face(candidates, p.at)).flatten();
    taken(choice, placed, wanted.group)
}

/// The people on the preview `picture` shows, found once for its key.
fn people_of<'p>(
    sam: &mut Sam3,
    presets: &Presets,
    picture: &mut Picture,
    kept: &'p mut Option<(u64, Vec<sam3::Person>)>,
) -> Result<&'p [sam3::Person], String> {
    if kept.as_ref().is_none_or(|k| k.0 != picture.key) {
        let found = sam3::people(sam, presets, picture).map_err(|e| e.to_string())?;
        *kept = Some((picture.key, found));
    }
    Ok(&kept.as_ref().expect("just found").1)
}

/// The status line for a Part whose person is not settled.
fn part_note(part: &Unresolved, file: Option<&Path>) -> String {
    match part {
        Unresolved::Ask(ask) => ask.words().to_string(),
        Unresolved::Nobody => {
            let name = file
                .and_then(|f| f.file_name())
                .map_or("this picture".to_string(), |n| {
                    n.to_string_lossy().into_owned()
                });
            format!("no one is in {name}")
        }
    }
}

/// The status line for a Sky shape on a frame with no sky.
fn no_sky_note(file: Option<&Path>) -> String {
    none_found_note("sky", file)
}

/// The status line for a shape the model finds none of on a frame:
/// "no water found in FILE".
pub fn none_found_note(what: &str, file: Option<&Path>) -> String {
    let name = file
        .and_then(|f| f.file_name())
        .map_or("this picture".to_string(), |n| {
            n.to_string_lossy().into_owned()
        });
    format!("no {what} found in {name}")
}

/// What a model sees: the picture under the global look alone, no
/// geometry, no vignette, grain, or output sharpen, in sRGB, no more
/// than `PREVIEW` on its long side.
pub fn preview(image: &WorkingImage, edit: &Edit, kind: greycard_render::finish::Source) -> Rgb8 {
    display(image, edit, kind, Some(PREVIEW))
}

/// `preview`'s rendering of one rectangle of `image` (its pixels,
/// clamped to it), at those pixels: what a crop the People model looks
/// closer at is cut from, so an eye in a full-length frame is the
/// sensor's pixels rather than a few dozen of the preview's.
pub fn region(
    image: &WorkingImage,
    edit: &Edit,
    kind: greycard_render::finish::Source,
    rect: sam3::Rect,
) -> Rgb8 {
    let x0 = rect.x0.min(image.width.saturating_sub(1));
    let y0 = rect.y0.min(image.height.saturating_sub(1));
    let x1 = rect.x1.clamp(x0 + 1, image.width.max(x0 + 1));
    let y1 = rect.y1.clamp(y0 + 1, image.height.max(y0 + 1));
    let (w, h) = (x1 - x0, y1 - y0);
    let c = WorkingImage::CHANNELS;
    let mut crop = WorkingImage::new(w, h);
    for y in 0..h {
        let from = ((y0 + y) * image.width + x0) * c;
        crop.data[y * w * c..(y + 1) * w * c].copy_from_slice(&image.data[from..from + w * c]);
    }
    display(&crop, edit, kind, None)
}

/// The global look alone over `image`, in sRGB, at most `long_edge`
/// on its long side.
fn display(
    image: &WorkingImage,
    edit: &Edit,
    kind: greycard_render::finish::Source,
    long_edge: Option<u32>,
) -> Rgb8 {
    let mut edit = edit.clone();
    edit.adjustments.clear();
    edit.geometry = Default::default();
    edit.vignette = Default::default();
    edit.grain = Default::default();
    edit.output_sharpen = Default::default();
    let settings = greycard_render::export::Settings {
        format: greycard_render::export::Format::Jpeg,
        long_edge,
        space: greycard_render::export::Space::Srgb,
        // The models see the picture as it is, not sharpened for a
        // screen.
        sharpen: greycard_render::export::Sharpen::Off,
        ..Default::default()
    };
    let source = (image.width as u32, image.height as u32);
    // No guide plane: the models see a picture the tone equalizer's
    // shifts read the pixel's own luminance for, as they did before
    // the tone equalizer. What a mask is drawn around takes no stop.
    let rendered = greycard_render::export::render(
        image,
        &edit,
        &greycard_edit_io::Stores::user(),
        source,
        &settings,
        &HashMap::new(),
        f32::INFINITY,
        None,
        kind,
        // No masks, so no white of theirs, and no bands.
        None,
        None,
    );
    let greycard_render::export::Pixels::Eight(data) = rendered.pixels else {
        unreachable!("a JPEG render is eight bit");
    };
    Rgb8::new(rendered.width as usize, rendered.height as usize, data)
}

/// With `GREYCARD_AI_DUMP` set to a directory, what the model saw and
/// what it said, as PNGs, for looking at.
fn dump(rgb: &Rgb8, mask: &greycard_ai::Mask, refined: &greycard_ai::Mask) {
    let Some(dir) = std::env::var_os("GREYCARD_AI_DUMP") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let save = |name: &str, w: usize, h: usize, data: Vec<u8>, color: image::ExtendedColorType| {
        let path = dir.join(name);
        let file = match std::fs::File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("dump {}: {e}", path.display());
                return;
            }
        };
        use image::ImageEncoder;
        if let Err(e) = image::codecs::png::PngEncoder::new(std::io::BufWriter::new(file))
            .write_image(&data, w as u32, h as u32, color)
        {
            tracing::warn!("dump {}: {e}", path.display());
        }
    };
    save(
        "preview.png",
        rgb.width,
        rgb.height,
        rgb.data.clone(),
        image::ExtendedColorType::Rgb8,
    );
    save(
        "mask.png",
        mask.width,
        mask.height,
        mask.to_u8(),
        image::ExtendedColorType::L8,
    );
    save(
        "refined.png",
        refined.width,
        refined.height,
        refined.to_u8(),
        image::ExtendedColorType::L8,
    );
}

/// A cached raster, if the file is there and is `width` by `height`
/// (another height means another aspect: made for a different develop
/// of the file).
fn read_raster(path: &Path, width: usize, height: usize) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?.into_luma8();
    if img.width() as usize != width || img.height() as usize != height {
        return None;
    }
    Some(img.into_raw())
}

/// [`read_raster`], a row more or less than `height` taken too.
fn read_raster_near(path: &Path, width: usize, height: usize) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?.into_luma8();
    if img.width() as usize != width || (img.height() as usize).abs_diff(height) > 1 {
        return None;
    }
    Some(img.into_raw())
}

/// A raster `height` rows tall, as wide as `data` makes it.
fn write_raster(path: &Path, height: usize, data: &[u8]) {
    use image::ImageEncoder;
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let part = path.with_extension("part");
        let file = std::fs::File::create(&part)?;
        image::codecs::png::PngEncoder::new(std::io::BufWriter::new(file)).write_image(
            data,
            (data.len() / height.max(1)) as u32,
            height as u32,
            image::ExtendedColorType::L8,
        )?;
        std::fs::rename(&part, path)?;
        Ok(())
    };
    match write() {
        Ok(()) => evict_other_bases(path),
        Err(e) => tracing::warn!("mask cache {}: {e}", path.display()),
    }
}

/// How many bases' rasters a slot keeps on disk: the last made, and
/// two before it, so virtual copies of one frame whose bases differ
/// (a white balance each) are read in turn rather than made again.
const BASES_A_SLOT: usize = 3;

/// The rasters of `path`'s slot (`<slot>-<print>.png`,
/// [`Ai::cached_path_at`]) but the newest [`BASES_A_SLOT`] removed, and
/// the slot's raster from before rasters were kept by their base
/// (`<slot>.png`), which no print names: the folder keeps a few a slot
/// however often a white balance or a lens moves the base.
fn evict_other_bases(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return;
    };
    let Some((slot, _)) = name.split_once('-') else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let unprinted = format!("{slot}.png");
    let mut others = Vec::new();
    for entry in entries.flatten() {
        let other = entry.file_name();
        let Some(other) = other.to_str() else {
            continue;
        };
        if other == unprinted {
            let _ = std::fs::remove_file(entry.path());
        } else if other != name
            && other.ends_with(".png")
            && other.split_once('-').is_some_and(|(s, _)| s == slot)
        {
            let made = entry.metadata().and_then(|m| m.modified()).ok();
            others.push((made, entry.path()));
        }
    }
    // Newest first; the written one is newest of all.
    others.sort_by_key(|(made, _)| std::cmp::Reverse(*made));
    for (_, other) in others.into_iter().skip(BASES_A_SLOT - 1) {
        // Read meanwhile by another run, or held open on Windows: left
        // for the next write to take.
        let _ = std::fs::remove_file(other);
    }
}

/// The fills kept on disk's format: bumped when a change to either
/// engine, or to how a fill is put together, makes another fill of the
/// same patch, so the old ones are made again. The frame's grain is
/// kept in the fill ([`regrain`]), so a change to how it is measured or
/// drawn counts: 4, the surround's line drawn to its bins' relative
/// error, without the bins of texture; 5, the window's print kept in
/// the file and compared, no longer hashed into its name.
const FILL_VERSION: u8 = 5;

/// The version before the print was kept in the file: its slot is named
/// with the print hashed in, and its file holds none. Read once, where
/// a fill at [`FILL_VERSION`] is not, and kept again at it
/// ([`Ai::fill_from_hashed_slot`]).
const FILL_VERSION_HASHED: u8 = 4;

/// The first bytes of a fill kept on disk.
const FILL_MAGIC: &[u8; 6] = b"GCFILL";

/// A fill kept on disk's header: the magic, the version and engine
/// bytes, and [`fill_numbers`] as u32s.
const FILL_HEADER: usize = FILL_MAGIC.len() + 2 + 8 * 4;

/// The side of the blocks [`window_print`] takes means over.
const PRINT_BLOCK: usize = 16;

/// A print of the picture under a fill's window ([`window_print`]):
/// each block's mean per channel as f16 bits, [`EMPTY_BLOCK`] for a
/// block with none of the window's known pixels.
type WindowPrint = Vec<[u16; 3]>;

/// A print block with none of the window's known pixels in it.
const EMPTY_BLOCK: u16 = u16::MAX;

/// How far a block's mean may move, relative to the larger of the two
/// and [`PRINT_FLOOR`], and the print still be of the picture the fill
/// was made from. Measured on two raws, a Canon CR3 and a Nikon NEF,
/// five windows each (`the_fill_print_measured`): the CPU's base and
/// lavapipe's (CA and demosaic on the device) differ by at most 0.0008,
/// one step of half precision; one mired of white balance moves some
/// block by 0.0056 and 0.016, a step of the lens's distortion by 0.83
/// and 0.95, and exposure, the finish's, by nothing. The tolerance sits
/// between, nearer the rounding: a few steps of half precision.
const PRINT_TOLERANCE: f32 = 0.003;

/// The level under which a block's mean is compared as if it were
/// this: a 256th of the working range, whose white is 1.0, so a black
/// block, whose relative moves are large and mean nothing, compares
/// sanely.
const PRINT_FLOOR: f32 = 1.0 / 256.0;

/// How far the median block's mean of the reduced develop's picture
/// may move from the full develop's, relative as [`PRINT_TOLERANCE`]
/// ([`Compare::Median`]): the bin stands in for the demosaic, the
/// profiled denoiser is left out, and a block's edge falls between the
/// reduced picture's pixels, which moves single blocks by up to 0.34
/// and the median by 0.002 and 0.004 on the raws measured. One mired
/// moves the median by 0.003, three by 0.009, a lens step by 0.06 and
/// 0.08: a reduced picture tells a fill's picture from one moved by a
/// few mired or more, not by one.
const REDUCED_PRINT_TOLERANCE: f32 = 0.01;

/// How two prints are compared ([`prints_agree`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compare {
    /// Every block's mean within the tolerance, an empty block matching
    /// only an empty one: the full develop's, against the picture the
    /// fill was made from.
    Every,
    /// The median block's move within the tolerance, over the values
    /// known in both, at least one: the reduced develop's, whose bin
    /// moves a block at an edge as far as an edit does, while the run of
    /// blocks stays where the full develop's are.
    Median,
}

/// Whether the picture whose print is `now` is the one `stored` was
/// taken of, as `compare` says, each value's move relative to the
/// larger of the two and [`PRINT_FLOOR`].
fn prints_agree(stored: &[[u16; 3]], now: &[[u16; 3]], tolerance: f32, compare: Compare) -> bool {
    if stored.len() != now.len() {
        return false;
    }
    let mut moves = Vec::new();
    for (s, n) in stored.iter().zip(now) {
        for (&s, &n) in s.iter().zip(n) {
            if s == EMPTY_BLOCK || n == EMPTY_BLOCK {
                if compare == Compare::Every && s != n {
                    return false;
                }
                continue;
            }
            let moved = value_move(s, n);
            if compare == Compare::Every {
                if moved > tolerance {
                    return false;
                }
            } else {
                moves.push(moved);
            }
        }
    }
    match compare {
        Compare::Every => true,
        Compare::Median if moves.is_empty() => false,
        Compare::Median => {
            let mid = moves.len() / 2;
            let (_, median, _) = moves.select_nth_unstable_by(mid, f32::total_cmp);
            *median <= tolerance
        }
    }
}

/// How far a print's value `b` is from `a`, both f16 bits, relative to
/// the larger of the two and [`PRINT_FLOOR`]; NaN for a value that is
/// not a number, which no tolerance takes.
fn value_move(a: u16, b: u16) -> f32 {
    let (a, b) = (
        half::f16::from_bits(a).to_f32(),
        half::f16::from_bits(b).to_f32(),
    );
    let moved = (a - b).abs() / a.abs().max(b.abs()).max(PRINT_FLOOR);
    if moved.is_nan() { f32::INFINITY } else { moved }
}

/// The largest relative move of any block between two prints, as
/// [`prints_agree`] measures it, over the blocks known in both: what
/// the tolerances were measured with.
#[cfg(test)]
pub(crate) fn print_distance(a: &[[u16; 3]], b: &[[u16; 3]]) -> f32 {
    a.iter()
        .zip(b)
        .flat_map(|(a, b)| a.iter().zip(b))
        .filter(|(a, b)| **a != EMPTY_BLOCK && **b != EMPTY_BLOCK)
        .map(|(&a, &b)| value_move(a, b))
        .fold(0.0, f32::max)
}

/// The print of a fill over `region` of a picture `width` by `height`
/// under its window, taken of `image`, that picture reduced by
/// `factor`: what a fill would be kept and served under.
#[cfg(test)]
pub(crate) fn fill_print(
    region: &Region,
    image: &WorkingImage,
    width: usize,
    height: usize,
    factor: usize,
) -> WindowPrint {
    let (window, _) = window_for(region, width, height);
    reduced_print(image, region, window, factor)
}

/// A print hashed, as the slots at [`FILL_VERSION_HASHED`] were named.
fn print_hash(print: &[[u16; 3]]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    print.to_vec().hash(&mut h);
    h.finish()
}

/// A coarse print of the picture a fill is made from: the means of the
/// window's pixels outside the hole over 16-pixel blocks, at half
/// precision, a block with none of them marked empty. Whatever changes
/// the base develop under the window (white balance, profile, lens,
/// noise, a turn) changes it, and a fill made from another picture is
/// not served for this one.
fn window_print(image: &WorkingImage, region: &Region, window: Window) -> WindowPrint {
    reduced_print(image, region, window, 1)
}

/// [`window_print`] of the full develop's `window` and `region`, taken
/// of `image`, that develop's picture reduced by `factor`: a reduced
/// pixel is in the block its center falls in, and left out where any
/// of the full pixels it stands for is in the hole. A factor of one is
/// [`window_print`] itself. A frame turned whose side is not a whole
/// number of bins leaves the reduced picture's pixels off the full
/// ones by that side modulo the factor, at most one full pixel at a
/// factor of two; the median compare takes it in.
fn reduced_print(
    image: &WorkingImage,
    region: &Region,
    window: Window,
    factor: usize,
) -> WindowPrint {
    let covered = |px: usize, py: usize| {
        px >= region.x
            && px < region.x + region.width
            && py >= region.y
            && py < region.y + region.height
            && region.cover[(py - region.y) * region.width + (px - region.x)] > 0.0
    };
    let (bw, bh) = (
        window.width.div_ceil(PRINT_BLOCK),
        window.height.div_ceil(PRINT_BLOCK),
    );
    let mut sums = vec![([0.0f64; 3], 0usize); bw * bh];
    let (x0, x1) = (
        window.x / factor,
        (window.x + window.width).div_ceil(factor).min(image.width),
    );
    let (y0, y1) = (
        window.y / factor,
        (window.y + window.height)
            .div_ceil(factor)
            .min(image.height),
    );
    let inside = |c: usize, from: usize, len: usize| c >= from && c < from + len;
    for ry in y0..y1 {
        let cy = ry * factor + factor / 2;
        if !inside(cy, window.y, window.height) {
            continue;
        }
        for rx in x0..x1 {
            let cx = rx * factor + factor / 2;
            if !inside(cx, window.x, window.width) {
                continue;
            }
            let hole = (0..factor)
                .any(|dy| (0..factor).any(|dx| covered(rx * factor + dx, ry * factor + dy)));
            if hole {
                continue;
            }
            let i = (ry * image.width + rx) * 3;
            let (sum, n) =
                &mut sums[((cy - window.y) / PRINT_BLOCK) * bw + (cx - window.x) / PRINT_BLOCK];
            for (s, v) in sum.iter_mut().zip(&image.data[i..i + 3]) {
                *s += f64::from(*v);
            }
            *n += 1;
        }
    }
    sums.iter()
        .map(|(sum, n)| {
            if *n == 0 {
                return [EMPTY_BLOCK; 3];
            }
            sum.map(|v| half::f16::from_f64(v / *n as f64).to_bits())
        })
        .collect()
}

/// A fill kept on disk: the magic, the version, the engine, then the
/// picture's size, the window and the region's size as little-endian
/// u32s; from version 5 the print's block count as a u32 and its
/// blocks as little-endian f16 bits; then the region's values as
/// little-endian f16. Half precision halves a fill that can run to tens
/// of megabytes; its step, under a part in two thousand, is below the
/// grain put back over the fill.
#[allow(clippy::too_many_arguments)]
fn write_fill(
    path: &Path,
    version: u8,
    engine: Engine,
    size: (usize, usize),
    window: Window,
    region: (usize, usize),
    print: &[[u16; 3]],
    data: &[f32],
) -> bool {
    use std::io::Write;
    let part = path.with_extension("fill-part");
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut out = std::io::BufWriter::new(std::fs::File::create(&part)?);
        out.write_all(FILL_MAGIC)?;
        out.write_all(&[version, engine_byte(engine)])?;
        for n in fill_numbers(size, window, region) {
            out.write_all(&u32::try_from(n)?.to_le_bytes())?;
        }
        if version > FILL_VERSION_HASHED {
            out.write_all(&u32::try_from(print.len())?.to_le_bytes())?;
            for v in print.iter().flatten() {
                out.write_all(&v.to_le_bytes())?;
            }
        }
        for v in data {
            out.write_all(&half::f16::from_f32(*v).to_le_bytes())?;
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        std::fs::rename(&part, path)?;
        Ok(())
    };
    match write() {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("fill cache {}: {e}", path.display());
            let _ = std::fs::remove_file(&part);
            false
        }
    }
}

/// The files a fill's slot (`<slot>.fill`, [`fill_slot`]) keeps its
/// bases' fills in, `<slot>-<n>.fill`, [`BASES_A_SLOT`] of them:
/// virtual copies of a frame whose bases differ (a white balance each)
/// read theirs in turn rather than make it again.
fn slot_files(slot: &Path) -> Vec<PathBuf> {
    let stem = slot
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let dir = slot.parent().unwrap_or(Path::new(""));
    (0..BASES_A_SLOT)
        .map(|n| dir.join(format!("{stem}-{n}.fill")))
        .collect()
}

/// The fill in `slot` whose print `takes`, at [`FILL_VERSION`], its
/// files read newest first.
fn read_slot(
    slot: &Path,
    size: (usize, usize),
    window: Window,
    region: (usize, usize),
    takes: impl Fn(&[[u16; 3]]) -> bool,
) -> Option<(Engine, WindowPrint, Vec<f32>)> {
    let mut files: Vec<_> = slot_files(slot)
        .into_iter()
        .filter_map(|f| {
            let made = std::fs::metadata(&f).and_then(|m| m.modified()).ok()?;
            Some((made, f))
        })
        .collect();
    files.sort_by_key(|(made, _)| std::cmp::Reverse(*made));
    files
        .iter()
        .find_map(|(_, f)| read_fill(f, FILL_VERSION, size, window, region, &takes))
}

/// A fill kept in `slot` at [`FILL_VERSION`]: over the file whose print
/// is `print`'s within [`PRINT_TOLERANCE`], else in a file not there,
/// else over the oldest. True when it was written.
fn write_slot(
    slot: &Path,
    engine: Engine,
    size: (usize, usize),
    window: Window,
    region: (usize, usize),
    print: &[[u16; 3]],
    data: &[f32],
) -> bool {
    let files = slot_files(slot);
    let same = |f: &Path| {
        let mut same = false;
        read_fill(f, FILL_VERSION, size, window, region, |stored| {
            same = prints_agree(stored, print, PRINT_TOLERANCE, Compare::Every);
            false
        });
        same
    };
    let made = |f: &Path| std::fs::metadata(f).and_then(|m| m.modified()).ok();
    let Some(target) = files
        .iter()
        .find(|f| same(f))
        .or_else(|| files.iter().find(|f| !f.exists()))
        .or_else(|| files.iter().min_by_key(|f| made(f)))
    else {
        return false;
    };
    write_fill(
        target,
        FILL_VERSION,
        engine,
        size,
        window,
        region,
        print,
        data,
    )
}

/// A fill kept on disk, if the file is there and was written at
/// `version` for a picture of `size`, `window` and a region of
/// `region`'s size, and `takes` its print (none before version 5): the
/// engine that made it, the print, and the values.
fn read_fill(
    path: &Path,
    version: u8,
    size: (usize, usize),
    window: Window,
    region: (usize, usize),
    takes: impl FnOnce(&[[u16; 3]]) -> bool,
) -> Option<(Engine, WindowPrint, Vec<f32>)> {
    use std::io::Read;
    // The header and the print first: a slot of another version or
    // picture is not read through on every develop.
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; FILL_HEADER];
    file.read_exact(&mut header).ok()?;
    let (magic, rest) = header.split_at(FILL_MAGIC.len());
    let (&[v, e], head) = rest.split_first_chunk::<2>()?;
    if magic != FILL_MAGIC || v != version {
        return None;
    }
    let engine = match e {
        0 => Engine::Lama,
        1 => Engine::Remove,
        _ => return None,
    };
    let numbers = fill_numbers(size, window, region);
    let same = head
        .as_chunks::<4>()
        .0
        .iter()
        .zip(numbers)
        .all(|(b, n)| u32::from_le_bytes(*b) as usize == n);
    if !same {
        return None;
    }
    let mut print = Vec::new();
    let mut print_bytes = 0;
    if version > FILL_VERSION_HASHED {
        let mut count = [0u8; 4];
        file.read_exact(&mut count).ok()?;
        let count = u32::from_le_bytes(count) as usize;
        let blocks = window.width.div_ceil(PRINT_BLOCK) * window.height.div_ceil(PRINT_BLOCK);
        if count != blocks {
            return None;
        }
        let mut bytes = vec![0u8; count * 6];
        file.read_exact(&mut bytes).ok()?;
        print = bytes
            .as_chunks::<6>()
            .0
            .iter()
            .map(|b| {
                [
                    u16::from_le_bytes([b[0], b[1]]),
                    u16::from_le_bytes([b[2], b[3]]),
                    u16::from_le_bytes([b[4], b[5]]),
                ]
            })
            .collect();
        print_bytes = 4 + count * 6;
    }
    let want = region.0 * region.1 * 3 * 2;
    if file.metadata().ok()?.len() != (FILL_HEADER + print_bytes + want) as u64 || !takes(&print) {
        return None;
    }
    let mut values = vec![0u8; want];
    file.read_exact(&mut values).ok()?;
    let data = values
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| half::f16::from_le_bytes(*b).to_f32())
        .collect();
    Some((engine, print, data))
}

/// Where a fill lives on disk under `root`: named by a hash of the
/// file, the patch as [`same_fill`] compares it, its seed, the
/// picture's size, the region, the window, and both engines' models;
/// at [`FILL_VERSION_HASHED`], `print` hashed in too.
#[allow(clippy::too_many_arguments)]
fn fill_slot(
    root: &Path,
    file: &Path,
    patch: &Patch,
    region: &Region,
    size: (usize, usize),
    window: Window,
    print: Option<u64>,
) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut h);
    "fill".hash(&mut h);
    patch.id.hash(&mut h);
    patch.seed.hash(&mut h);
    for p in &patch.points {
        p.map(f32::to_bits).hash(&mut h);
    }
    patch.radius.to_bits().hash(&mut h);
    patch.feather.to_bits().hash(&mut h);
    std::mem::discriminant(&patch.method).hash(&mut h);
    patch.remove.hash(&mut h);
    patch.fast.hash(&mut h);
    size.hash(&mut h);
    (region.x, region.y, region.width, region.height).hash(&mut h);
    window.hash(&mut h);
    greycard_ai::FILL.id.hash(&mut h);
    REMOVE.id.hash(&mut h);
    REMOVE_FAST.id.hash(&mut h);
    if let Some(print) = print {
        print.hash(&mut h);
    }
    root.join(format!("{:016x}.fill", h.finish()))
}

/// What reads the fills kept on disk for a develop that makes none:
/// the reduced develop's, for one file.
#[derive(Debug, Clone)]
pub struct FillReader {
    root: PathBuf,
    file: PathBuf,
}

impl FillReader {
    /// The fill kept for `patch` on the full develop's picture, `full`
    /// in size, read and taken down by `factor` with a box filter to
    /// `region` of `image`, that picture reduced by `factor`: served
    /// when the print of `image` under the fill's window is the stored
    /// one's within [`REDUCED_PRINT_TOLERANCE`]. None for a fill not
    /// kept, made for another picture, or a factor that does not divide
    /// the print's blocks. No model runs.
    pub fn fill_read_only(
        &self,
        patch: &Patch,
        region: &Region,
        image: &WorkingImage,
        factor: usize,
        full: (usize, usize),
    ) -> Option<Vec<f32>> {
        if factor == 0 || !PRINT_BLOCK.is_multiple_of(factor) {
            return None;
        }
        let full_region = patch.region(full.0, full.1)?;
        let (window, _) = window_for(&full_region, full.0, full.1);
        let path = fill_slot(
            &self.root,
            &self.file,
            patch,
            &full_region,
            full,
            window,
            None,
        );
        let print = reduced_print(image, &full_region, window, factor);
        let (fw, fh) = (full_region.width, full_region.height);
        let (_, _, data) = read_slot(&path, full, window, (fw, fh), |stored| {
            prints_agree(stored, &print, REDUCED_PRINT_TOLERANCE, Compare::Median)
        })?;
        tracing::info!(
            "fill {}: read from the fill cache for a picture reduced by {factor}",
            patch.name()
        );
        Some(box_down(&data, &full_region, region, image, factor))
    }
}

/// A fill over `full`, the full develop's region, taken down by
/// `factor` to `region` of `image`: each pixel the mean of the full
/// pixels it stands for inside `full`, or the picture's own where none
/// are. On a turned frame whose side is not a whole number of bins the
/// two grids are off by that side modulo the factor (at most one full
/// pixel at two), which a thumbnail does not show.
fn box_down(
    data: &[f32],
    full: &Region,
    region: &Region,
    image: &WorkingImage,
    factor: usize,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(region.width * region.height * 3);
    for y in region.y..region.y + region.height {
        for x in region.x..region.x + region.width {
            let mut sum = [0.0f32; 3];
            let mut n = 0usize;
            for fy in y * factor..(y + 1) * factor {
                if fy < full.y || fy >= full.y + full.height {
                    continue;
                }
                for fx in x * factor..(x + 1) * factor {
                    if fx < full.x || fx >= full.x + full.width {
                        continue;
                    }
                    let i = ((fy - full.y) * full.width + (fx - full.x)) * 3;
                    for (s, v) in sum.iter_mut().zip(&data[i..i + 3]) {
                        *s += v;
                    }
                    n += 1;
                }
            }
            if n == 0 {
                let i = (y * image.width + x) * 3;
                out.extend_from_slice(&image.data[i..i + 3]);
            } else {
                out.extend(sum.map(|s| s / n as f32));
            }
        }
    }
    out
}

/// The numbers a fill kept on disk is checked by.
fn fill_numbers(size: (usize, usize), window: Window, region: (usize, usize)) -> [usize; 8] {
    [
        size.0,
        size.1,
        window.x,
        window.y,
        window.width,
        window.height,
        region.0,
        region.1,
    ]
}

fn engine_byte(engine: Engine) -> u8 {
    match engine {
        Engine::Lama => 0,
        Engine::Remove => 1,
    }
}

fn mul(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

fn invert(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let c = |r: usize, k: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
        m[r1][k1] * m[r2][k2] - m[r1][k2] * m[r2][k1]
    };
    std::array::from_fn(|r| std::array::from_fn(|k| c(k, r) / det))
}

fn encode(v: f32) -> f32 {
    greycard_render::finish::encode(v.clamp(0.0, 1.0))
}

fn decode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// The shape an offer makes, by the name `Shape::of_kind` takes.
pub fn kind_of(offer: &greycard_ai::landscape::Offer) -> &'static str {
    match offer.kind {
        greycard_ai::landscape::Kind::Sky => "Sky",
        greycard_ai::landscape::Kind::Group(g) => {
            greycard_edit::mask::LandscapeGroup::from(g.name().to_string()).label()
        }
    }
}

/// An offer on the menu: its name and the share of the frame it is.
pub fn words_of(offer: &greycard_ai::landscape::Offer) -> String {
    let percent = (offer.area * 100.0).round().max(1.0);
    format!("{} {percent:.0}%", kind_of(offer))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Subject on a WebGPU machine with neither model: the GPU file
    /// is offered; its fetch fails (faked here), and the original is
    /// offered next; declining the original leaves the shape waiting;
    /// the original arriving is asked for. Declining the GPU file also
    /// turns to the original rather than blocking it.
    #[test]
    fn a_subject_turns_to_the_original_when_the_gpu_model_cannot_be_had() {
        use greycard_ai::{Provider, SUBJECT, SUBJECT_WEBGPU};
        let gpu = [Provider::WebGpu, Provider::Cpu];
        let shape = Shape::Subject {};
        let nothing = |_: &greycard_ai::Model| false;
        // Nothing in the store yet: the record cannot say the original
        // failed on WebGPU, since it has never been tried.
        let no_record = nothing;
        let (mut declined, mut failed) = (Vec::new(), Vec::new());

        assert_eq!(
            step(&shape, nothing, &gpu, &declined, &failed, true, no_record),
            Some(Step::Offer(&SUBJECT_WEBGPU, false))
        );
        // The sheet is up or a fetch is on its way: wait.
        assert_eq!(
            step(&shape, nothing, &gpu, &declined, &failed, false, no_record),
            Some(Step::Wait)
        );
        // Its fetch fails.
        failed.push(SUBJECT_WEBGPU.id);
        assert_eq!(
            step(&shape, nothing, &gpu, &declined, &failed, true, no_record),
            Some(Step::Offer(&SUBJECT, false))
        );
        // Offline, the original fails too: no third offer.
        {
            let mut failed = failed.clone();
            failed.push(SUBJECT.id);
            assert_eq!(
                step(&shape, nothing, &gpu, &declined, &failed, true, no_record),
                Some(Step::Wait)
            );
            // Once it is in the store after all, it is asked for.
            let original = |m: &greycard_ai::Model| m.id == SUBJECT.id;
            assert_eq!(
                step(&shape, original, &gpu, &declined, &failed, true, no_record),
                Some(Step::Ask(&SUBJECT))
            );
        }
        declined.push(SUBJECT.id);
        assert_eq!(
            step(&shape, nothing, &gpu, &declined, &failed, true, no_record),
            Some(Step::Wait)
        );
        let original = |m: &greycard_ai::Model| m.id == SUBJECT.id;
        assert_eq!(
            step(&shape, original, &gpu, &declined, &failed, true, no_record),
            Some(Step::Ask(&SUBJECT))
        );

        // Declined, not failed: the same turn to the original.
        let (declined, failed) = (vec![SUBJECT_WEBGPU.id], Vec::new());
        assert_eq!(
            step(&shape, nothing, &gpu, &declined, &failed, true, no_record),
            Some(Step::Offer(&SUBJECT, false))
        );
        // And on a CPU-only machine the original from the start.
        assert_eq!(
            step(&shape, nothing, &[Provider::Cpu], &[], &[], true, no_record),
            Some(Step::Offer(&SUBJECT, false))
        );
    }

    /// The original already in the store, and on record as failing on
    /// WebGPU for this adapter: the rewrite is offered, and the offer
    /// says why. Declining it leaves the original in use, not waiting.
    #[test]
    fn a_subject_already_in_the_store_offers_the_rewrite_when_it_falls_back() {
        use greycard_ai::{Provider, SUBJECT, SUBJECT_WEBGPU};
        let gpu = [Provider::WebGpu, Provider::Cpu];
        let shape = Shape::Subject {};
        let original = |m: &greycard_ai::Model| m.id == SUBJECT.id;
        let failed_on_webgpu = |_: &greycard_ai::Model| true;

        assert_eq!(
            step(&shape, original, &gpu, &[], &[], true, failed_on_webgpu),
            Some(Step::Offer(&SUBJECT_WEBGPU, true)),
            "the store's original falls back to the CPU here: the rewrite is offered"
        );
        // Declined: the original stands, asked for rather than waited on.
        assert_eq!(
            step(
                &shape,
                original,
                &gpu,
                &[SUBJECT_WEBGPU.id],
                &[],
                true,
                failed_on_webgpu
            ),
            Some(Step::Ask(&SUBJECT))
        );
        // Nothing on record: the original stands from the start.
        let no_record = |_: &greycard_ai::Model| false;
        assert_eq!(
            step(&shape, original, &gpu, &[], &[], true, no_record),
            Some(Step::Ask(&SUBJECT))
        );
        // Another sheet up, or a fetch already running: nothing can be
        // offered this round, but the original already runs, so it is
        // asked for rather than left waiting on an offer that is not
        // coming.
        assert_eq!(
            step(&shape, original, &gpu, &[], &[], false, failed_on_webgpu),
            Some(Step::Ask(&SUBJECT))
        );
    }

    /// A Background wants exactly what a Subject want does: the same
    /// model, the same fallback to the store's own original when
    /// nothing can be offered this round. Before this was fixed,
    /// `step` only recognized `Shape::Subject {}` for that fallback,
    /// so a Background waited on an offer that was not coming while a
    /// Subject in the same mask already ran.
    #[test]
    fn a_background_falls_back_to_the_original_exactly_as_a_subject_does() {
        use greycard_ai::{Provider, SUBJECT, SUBJECT_WEBGPU};
        let gpu = [Provider::WebGpu, Provider::Cpu];
        let shape = Shape::Background {};
        let original = |m: &greycard_ai::Model| m.id == SUBJECT.id;
        let failed_on_webgpu = |_: &greycard_ai::Model| true;

        assert_eq!(
            step(&shape, original, &gpu, &[], &[], true, failed_on_webgpu),
            Some(Step::Offer(&SUBJECT_WEBGPU, true))
        );
        assert_eq!(
            step(
                &shape,
                original,
                &gpu,
                &[SUBJECT_WEBGPU.id],
                &[],
                true,
                failed_on_webgpu
            ),
            Some(Step::Ask(&SUBJECT))
        );
        let no_record = |_: &greycard_ai::Model| false;
        assert_eq!(
            step(&shape, original, &gpu, &[], &[], true, no_record),
            Some(Step::Ask(&SUBJECT))
        );
        // The sheet busy, the WebGPU rewrite on record as failing: the
        // store's own original already runs, so a Background is asked
        // for it rather than left waiting, exactly as a Subject is.
        assert_eq!(
            step(&shape, original, &gpu, &[], &[], false, failed_on_webgpu),
            Some(Step::Ask(&SUBJECT))
        );
    }

    /// A Sky wants the prior, then SAM: each offered in turn, and asked
    /// for once both are in. SAM declined or failing leaves the prior
    /// alone asked for, its own labels the outline.
    #[test]
    fn a_sky_is_offered_the_prior_then_sam_and_runs_without_sam_if_it_must() {
        use greycard_ai::{Provider, SAM, SKY, SUBJECT};
        let gpu = [Provider::WebGpu, Provider::Cpu];
        let shape = Shape::Sky { picks: Vec::new() };
        let no_record = |_: &greycard_ai::Model| false;
        let nothing = |_: &greycard_ai::Model| false;
        assert_eq!(
            step(&shape, nothing, &gpu, &[], &[], true, no_record),
            Some(Step::Offer(&SKY, false))
        );
        let prior = |m: &greycard_ai::Model| m.id == SKY.id;
        assert_eq!(
            step(&shape, prior, &gpu, &[], &[], true, no_record),
            Some(Step::Offer(&SAM, false))
        );
        let both = |m: &greycard_ai::Model| m.id == SKY.id || m.id == SAM.id;
        assert_eq!(
            step(&shape, both, &gpu, &[], &[], true, no_record),
            Some(Step::Ask(&SKY))
        );
        assert_eq!(
            step(&shape, prior, &gpu, &[SAM.id], &[], true, no_record),
            Some(Step::Ask(&SKY))
        );
        assert_eq!(
            step(&shape, prior, &gpu, &[], &[SAM.id], true, no_record),
            Some(Step::Ask(&SKY))
        );
        // The prior declined: the shape waits, whatever else the store
        // has; the Subject original is never asked for in its place.
        let subject_too = |m: &greycard_ai::Model| m.id == SUBJECT.id || m.id == SAM.id;
        assert_eq!(
            step(&shape, subject_too, &gpu, &[SKY.id], &[], true, no_record),
            Some(Step::Wait)
        );
        assert_eq!(
            step(&shape, subject_too, &gpu, &[], &[], false, no_record),
            Some(Step::Wait)
        );
    }

    /// A Whole person with the People model in the store is asked for
    /// at once, and the Subject model its edge runs is offered beside
    /// it, as a Subject would be offered; not with either Subject file
    /// in, the sheet busy, or that file declined; never for another
    /// part.
    #[test]
    fn a_whole_person_runs_at_once_and_offers_subject_beside_it() {
        use greycard_ai::{Provider, SAM3, SUBJECT, SUBJECT_WEBGPU};
        use greycard_edit::mask::Route;
        let gpu = [Provider::WebGpu, Provider::Cpu];
        let no_record = |_: &greycard_ai::Model| false;
        let whole = Shape::Part {
            phrase: "person".into(),
            route: Route::Whole,
            person: None,
        };
        let hair = Shape::Part {
            phrase: "hair".into(),
            route: Route::Whole,
            person: None,
        };
        let people: fn(&greycard_ai::Model) -> bool = |m| m.id == SAM3.id;
        assert_eq!(
            step(&whole, people, &gpu, &[], &[], true, no_record),
            Some(Step::Ask(&SAM3))
        );
        let offer = |shape, have, declined: &[&'static str], free| {
            edge_offer(shape, have, &gpu, declined, &[], free, no_record).map(|m| m.id)
        };
        assert_eq!(offer(&whole, people, &[], true), Some(SUBJECT_WEBGPU.id));
        assert_eq!(offer(&hair, people, &[], true), None);
        assert_eq!(offer(&whole, people, &[], false), None);
        // The GPU file declined: the original, as for a Subject.
        assert_eq!(
            offer(&whole, people, &[SUBJECT_WEBGPU.id], true),
            Some(SUBJECT.id)
        );
        assert_eq!(
            offer(&whole, people, &[SUBJECT_WEBGPU.id, SUBJECT.id], true),
            None
        );
        let with_subject: fn(&greycard_ai::Model) -> bool =
            |m| m.id == SAM3.id || m.id == SUBJECT.id;
        assert_eq!(offer(&whole, with_subject, &[], true), None);
    }

    /// An Object waiting on SAM with the sheet busy waits: the Subject
    /// original in the store is not asked for in SAM's place.
    #[test]
    fn an_object_waiting_on_sam_is_not_given_the_subject_model() {
        use greycard_ai::{Provider, SUBJECT};
        let shape = Shape::Object {
            picks: vec![greycard_edit::mask::Pick {
                pos: [0.5, 0.5],
                positive: true,
            }],
            boxes: Vec::new(),
        };
        let subject = |m: &greycard_ai::Model| m.id == SUBJECT.id;
        assert_eq!(
            step(
                &shape,
                subject,
                &[Provider::Cpu],
                &[],
                &[],
                false,
                |_: &greycard_ai::Model| false
            ),
            Some(Step::Wait)
        );
    }

    /// With nobody to offer a fetch to, the window's chooser with no
    /// declines: the store's original, on record as failing on WebGPU
    /// and with no rewrite beside it, is run (the window runs it once
    /// the rewrite's offer is declined), where `model_with` alone named
    /// the rewrite the store has not got; an empty store names what
    /// `model_with` names, which the export then reports missing.
    #[test]
    fn an_unattended_subject_runs_the_original_the_store_has() {
        use greycard_ai::{Provider, SUBJECT, SUBJECT_WEBGPU};
        let gpu = [Provider::WebGpu, Provider::Cpu];
        let shape = Shape::Subject {};
        let original = |m: &Model| m.id == SUBJECT.id;
        let failed_on_webgpu = |_: &Model| true;
        assert_eq!(
            unattended_with(&shape, original, &gpu, &[], failed_on_webgpu).map(|m| m.id),
            Some(SUBJECT.id)
        );
        assert_eq!(
            model_with(&shape, original, &gpu, &[], failed_on_webgpu).map(|m| m.id),
            Some(SUBJECT_WEBGPU.id),
            "the guess without the chooser"
        );
        let nothing = |_: &Model| false;
        let no_record = |_: &Model| false;
        for providers in [&gpu[..], &[Provider::Cpu]] {
            let chosen = unattended_with(&shape, nothing, providers, &[], no_record);
            assert_eq!(
                chosen.map(|m| m.id),
                model_with(&shape, nothing, providers, &[], no_record).map(|m| m.id)
            );
            assert!(chosen.is_some_and(|m| !nothing(m)), "reported missing");
        }
        assert_eq!(
            unattended_with(&shape, nothing, &[Provider::Cpu], &[], no_record).map(|m| m.id),
            Some(SUBJECT.id)
        );
    }

    /// A raster 64 wide whose left half is the shape, and patches
    /// placed on it, in units of its width.
    fn half_shape() -> Raster {
        let (w, h) = (64, 32);
        let data = (0..w * h)
            .map(|i| if i % w < w / 2 { 255 } else { 0 })
            .collect();
        Raster::from_data(0.5, w, data)
    }

    fn patch_at(id: u64, x: f32, seed: u32) -> Patch {
        Patch {
            id,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[x, 0.25]],
            radius: 0.05,
            seed,
            ..Default::default()
        }
    }

    /// A patch meets a raster where its hole lies on the shape, not
    /// where it lies off it, nor on an empty raster; a picture whose
    /// fills are not all made, or a retouch switched off, counts none.
    #[test]
    fn a_patch_meets_a_raster_where_its_hole_is_on_the_shape() {
        let shape = half_shape();
        let on = patch_at(1, 0.2, 0);
        let off = patch_at(2, 0.8, 0);
        let retouch = Retouch {
            enabled: true,
            patches: vec![on.clone(), off.clone()],
        };
        let under = overlapping(&retouch, &shape);
        assert_eq!(under.iter().map(|p| p.id).collect::<Vec<_>>(), vec![1]);
        let empty = Raster::from_data(0.5, 64, vec![0; 64 * 32]);
        assert!(overlapping(&retouch, &empty).is_empty());
        // Faint matte haze is not the shape.
        let haze = Raster::from_data(0.5, 64, vec![MEETS - 1; 64 * 32]);
        assert!(overlapping(&retouch, &haze).is_empty());

        let image = WorkingImage::new(4, 2);
        let whole = Retouched {
            retouch: &retouch,
            image: &image,
            unmade: &[],
        };
        assert_eq!(whole.print_over(&shape).map(|p| p.1), Some(1));
        assert!(whole.print_over(&empty).is_none());
        let waiting = Retouched {
            unmade: &[1],
            ..whole
        };
        assert!(
            waiting.print_over(&shape).is_none(),
            "the fill over it unmade"
        );
        let elsewhere = Retouched {
            unmade: &[2],
            ..whole
        };
        assert_eq!(
            elsewhere.print_over(&shape),
            whole.print_over(&shape),
            "a fill unmade elsewhere"
        );
        // A heal or a clone keeps what is under it.
        for method in [
            greycard_edit::retouch::Method::Heal,
            greycard_edit::retouch::Method::Clone,
        ] {
            let kept = Retouch {
                enabled: true,
                patches: vec![Patch {
                    method,
                    source: Some([0.1, 0.0]),
                    ..on.clone()
                }],
            };
            assert!(overlapping(&kept, &shape).is_empty(), "{method:?}");
        }
        let off_switch = Retouch {
            enabled: false,
            ..retouch.clone()
        };
        let switched = Retouched {
            retouch: &off_switch,
            ..whole
        };
        assert!(switched.print_over(&shape).is_none());
    }

    /// The print changes with a patch over the shape (moved, another
    /// seed, another engine, taken away) and not with one off it.
    #[test]
    fn the_print_follows_the_patches_over_the_shape_alone() {
        let shape = half_shape();
        let image = WorkingImage::new(4, 2);
        let print = |patches: Vec<Patch>| {
            let retouch = Retouch {
                enabled: true,
                patches,
            };
            Retouched {
                retouch: &retouch,
                image: &image,
                unmade: &[],
            }
            .print_over(&shape)
            .map(|p| p.0)
        };
        let first = print(vec![patch_at(1, 0.2, 0), patch_at(2, 0.8, 0)]);
        assert!(first.is_some());
        assert_eq!(
            print(vec![patch_at(1, 0.2, 0), patch_at(2, 0.7, 3)]),
            first,
            "a patch off the shape moved"
        );
        assert_eq!(print(vec![patch_at(1, 0.2, 0)]), first, "one off it gone");
        assert_ne!(print(vec![patch_at(1, 0.25, 0)]), first, "moved");
        assert_ne!(print(vec![patch_at(1, 0.2, 1)]), first, "another seed");
        let mut remove = patch_at(1, 0.2, 0);
        remove.set_fill_choice(FillChoice::Fast);
        assert_ne!(print(vec![remove]), first, "another engine");
        assert_ne!(
            print(vec![patch_at(1, 0.2, 0), patch_at(3, 0.3, 0)]),
            first,
            "a second over it"
        );
        assert_eq!(print(vec![patch_at(2, 0.8, 0)]), None, "none over it");
    }

    /// A Subject a patch meets is made again on the retouched picture,
    /// into a slot of its own (read here from disk, as no model is in
    /// the store); one no patch meets keeps the base's raster. What the
    /// panel was given is asked for again only when a develop changes
    /// the patches over it.
    #[test]
    fn a_subject_under_a_patch_is_made_again_on_the_retouched_picture() {
        let dir = crate::testing::scratch_dir("retouched-mask");
        let mut ai = Ai::for_test_on_disk(Store::at(dir.join("models")));
        ai.providers = vec![Provider::Cpu];
        ai.forget(Some(PathBuf::from("open.CR3")));
        let shape = Shape::Subject {};
        let key = (1, 0);
        let model = Some(&greycard_ai::SUBJECT);
        let image = WorkingImage::new(200, 100);
        let base = Arc::new(half_shape());
        ai.cache
            .insert((key, 0), (shape.clone(), Print::test(1), base.clone()));
        let edit = Edit::default();
        let kind = greycard_render::finish::Source::default();
        let off = Retouch {
            enabled: true,
            patches: vec![patch_at(2, 0.8, 0)],
        };
        let on = Retouch {
            enabled: true,
            patches: vec![patch_at(1, 0.2, 0), patch_at(2, 0.8, 0)],
        };
        let retouched = |r| Retouched {
            retouch: r,
            image: &image,
            unmade: &[],
        };
        let (made, print) = ai
            .raster_retouched(
                Base::stamped(1),
                &image,
                Some(retouched(&off)),
                &edit,
                kind,
                0,
                key,
                &shape,
                model,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&made.raster, &base));
        assert_eq!(print, None);
        ai.given(key, &shape, model, print, Print::test(1));
        assert!(
            ai.to_ask_again(Some(retouched(&off)), Print::test(1), 0, |_, _| true)
                .is_empty()
        );
        let again = ai.to_ask_again(Some(retouched(&on)), Print::test(1), 0, |_, _| true);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].0, key);
        assert!(
            ai.to_ask_again(Some(retouched(&on)), Print::test(1), 0, |_, _| false)
                .is_empty(),
            "a shape no longer drawn"
        );

        // The retouched picture's raster in its own slot.
        let (print_on, _) = retouched(&on).print_over(&base).unwrap();
        let width = raster_width(&shape, (image.width, image.height));
        let height = raster_height(width, 0.5);
        ai.salt = Some(print_on);
        let slot = ai.cached_path_at(&shape, model, 0, width).unwrap();
        ai.salt = None;
        assert_ne!(
            Some(&slot),
            ai.cached_path_at(&shape, model, 0, width).as_ref()
        );
        std::fs::create_dir_all(slot.parent().unwrap()).unwrap();
        write_raster(&slot, height, &vec![7u8; width * height]);
        let (made, print) = ai
            .raster_retouched(
                Base::stamped(1),
                &image,
                Some(retouched(&on)),
                &edit,
                kind,
                0,
                key,
                &shape,
                model,
            )
            .unwrap();
        assert_eq!(print, Some(print_on));
        assert!(made.raster.data().iter().all(|&v| v == 7));
        // The base's raster is where it was, for the next question.
        assert!(Arc::ptr_eq(&ai.cache[&(key, 0)].2, &base));
        ai.given(key, &shape, model, print, Print::test(1));
        assert!(
            ai.to_ask_again(Some(retouched(&on)), Print::test(1), 0, |_, _| true)
                .is_empty()
        );
        // Taken away, the base's again.
        assert_eq!(
            ai.to_ask_again(Some(retouched(&off)), Print::test(1), 0, |_, _| true)
                .len(),
            1
        );
        assert_eq!(
            ai.to_ask_again(None, Print::test(1), 0, |_, _| true).len(),
            1
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// An open starts the worker afresh: nothing given is known until
    /// the panel asks again, and once it has, a fill drawn over the
    /// shape asks for it again.
    #[test]
    fn after_a_reopen_the_ask_again_finds_a_fill_drawn_after() {
        let mut ai = Ai::for_test(Store::at("no-store"), vec![Provider::Cpu]);
        let file = PathBuf::from("open.CR3");
        ai.forget(Some(file.clone()));
        let shape = Shape::Subject {};
        let key = (1, 0);
        let image = WorkingImage::new(200, 100);
        let edit = Edit::default();
        let kind = greycard_render::finish::Source::default();
        let base = Arc::new(half_shape());
        let fill = Retouch {
            enabled: true,
            patches: vec![patch_at(1, 0.2, 0)],
        };
        let retouched = Retouched {
            retouch: &fill,
            image: &image,
            unmade: &[],
        };
        ai.cache
            .insert((key, 0), (shape.clone(), Print::test(1), base.clone()));
        ai.given(key, &shape, None, None, Print::test(1));
        // Reopened: what was given is forgotten with the rest.
        ai.forget(Some(file));
        ai.cache
            .insert((key, 0), (shape.clone(), Print::test(1), base.clone()));
        assert!(
            ai.to_ask_again(Some(retouched), Print::test(1), 0, |_, _| true)
                .is_empty()
        );
        // The panel's ask after the open, on no retouch yet.
        let (_, print) = ai
            .raster_retouched(
                Base::stamped(1),
                &image,
                None,
                &edit,
                kind,
                0,
                key,
                &shape,
                None,
            )
            .unwrap();
        ai.given(key, &shape, None, print, Print::test(1));
        assert_eq!(
            ai.to_ask_again(Some(retouched), Print::test(1), 0, |_, _| true)
                .len(),
            1
        );
    }

    /// A shape that cannot be made again (here no model, no slot) keeps
    /// the base's raster under the print of the fills over it, so the
    /// next develop does not ask again, and is tried again once a model
    /// arrives; the retouched picture's preview is kept beside the
    /// base's, not in its place.
    #[test]
    fn a_shape_not_made_again_is_not_asked_again_each_develop() {
        let dir = crate::testing::scratch_dir("retouched-mask-fails");
        let mut ai = Ai::for_test_on_disk(Store::at(dir.join("models")));
        ai.providers = vec![Provider::Cpu];
        ai.forget(Some(PathBuf::from("open.CR3")));
        let shape = Shape::Subject {};
        let key = (1, 0);
        let image = WorkingImage::new(200, 100);
        let edit = Edit::default();
        let kind = greycard_render::finish::Source::default();
        let base = Arc::new(half_shape());
        ai.cache
            .insert((key, 0), (shape.clone(), Print::test(1), base.clone()));
        ai.preview_of(1, &image, &edit, kind);
        let on = Retouch {
            enabled: true,
            patches: vec![patch_at(1, 0.2, 0)],
        };
        let retouched = Retouched {
            retouch: &on,
            image: &image,
            unmade: &[],
        };
        let want = retouched.print_over(&base).map(|p| p.0);
        let (made, print) = ai
            .raster_retouched(
                Base::stamped(1),
                &image,
                Some(retouched),
                &edit,
                kind,
                0,
                key,
                &shape,
                None,
            )
            .unwrap();
        assert!(Arc::ptr_eq(&made.raster, &base), "the base's kept");
        assert_eq!(print, want);
        ai.given(key, &shape, None, print, Print::test(1));
        assert!(
            ai.to_ask_again(Some(retouched), Print::test(1), 0, |_, _| true)
                .is_empty()
        );
        assert_eq!(
            ai.preview.as_ref().map(|p| p.0),
            Some(1),
            "the base's preview"
        );
        assert!(
            ai.other_side.preview.as_ref().is_some_and(|p| p.0 != 1),
            "the retouched picture's beside it"
        );
        // A model arrives: it is tried again, once.
        assert_eq!(
            ai.retry_fallbacks(),
            vec![(key, shape.clone(), None)],
            "not stuck on the base's"
        );
        assert!(ai.retry_fallbacks().is_empty());
        // Now it can be had (its slot filled, as a model would): made.
        let model = Some(&greycard_ai::SUBJECT);
        let width = raster_width(&shape, (image.width, image.height));
        let height = raster_height(width, 0.5);
        ai.salt = want;
        let slot = ai.cached_path_at(&shape, model, 0, width).unwrap();
        ai.salt = None;
        std::fs::create_dir_all(slot.parent().unwrap()).unwrap();
        write_raster(&slot, height, &vec![7u8; width * height]);
        let (made, print) = ai
            .raster_retouched(
                Base::stamped(1),
                &image,
                Some(retouched),
                &edit,
                kind,
                0,
                key,
                &shape,
                model,
            )
            .unwrap();
        assert_eq!(print, want);
        assert!(made.raster.data().iter().all(|&v| v == 7));
        assert!(ai.retry_fallbacks().is_empty());
        crate::testing::remove_dir_retry(&dir);
    }

    /// Each choice runs where what it needs is here, and says why not
    /// otherwise; a patch drawn asks for the sticky choice and falls to
    /// the other remove, then to LaMa; the default is Fast only on
    /// unified memory with its files here, and Patch where neither
    /// remove can ever run: no WebGPU, or a card under the remove's
    /// smallest need.
    #[test]
    fn the_fill_choices_run_where_their_models_are() {
        use FillChoice::{Best, Fast, Patch as Lama};
        let all = FillHaves {
            fill: true,
            remove: true,
            fast: true,
            webgpu: true,
            card: Some(16 << 30),
        };
        for c in FillChoice::ALL {
            assert_eq!(all.why_not(c), None);
            assert_eq!(all.missing(c), None);
            assert_eq!(all.drawn(c), c);
        }
        let no_fast = FillHaves { fast: false, ..all };
        assert_eq!(no_fast.why_not(Fast), None, "a download is not a reason");
        assert_eq!(no_fast.missing(Fast), Some(&REMOVE_FAST));
        assert_eq!(no_fast.missing(Best), None);
        assert_eq!(no_fast.drawn(Fast), Best);
        // Fast's files need the remove's beside them: that first.
        let no_remove = FillHaves {
            remove: false,
            fast: false,
            ..all
        };
        assert_eq!(no_remove.missing(Fast), Some(&REMOVE));
        assert_eq!(no_remove.missing(Best), Some(&REMOVE));
        assert_eq!(no_remove.drawn(Best), Lama);
        let cpu = FillHaves {
            webgpu: false,
            ..all
        };
        assert_eq!(cpu.why_not(Best), Some(FillNotHere::NoGpu));
        assert_eq!(cpu.why_not(Fast), Some(FillNotHere::NoGpu));
        assert_eq!(cpu.default_choice(false), Lama);
        assert_eq!(cpu.why_not(Lama), None);
        assert_eq!(cpu.drawn(Fast), Lama);
        let no_lama = FillHaves { fill: false, ..all };
        assert_eq!(no_lama.why_not(Lama), None);
        assert_eq!(no_lama.missing(Lama), Some(&greycard_ai::FILL));
        assert_eq!(all.default_choice(true), Fast);
        assert_eq!(all.default_choice(false), Best);
        assert_eq!(no_fast.default_choice(true), Best);
        // A 4 GB card: both removes never here, said with the figures,
        // and a Fill drawn is Patch's; its memory not known, as on
        // unified memory, nothing is greyed.
        let small = FillHaves {
            card: Some(41 * (1 << 30) / 10),
            ..all
        };
        let need = remove_needs(512, false);
        for c in [Best, Fast] {
            assert_eq!(
                small.why_not(c),
                Some(FillNotHere::Card {
                    need,
                    have: 41 * (1 << 30) / 10
                })
            );
            assert_eq!(small.drawn(c), Lama);
        }
        assert_eq!(small.why_not(Lama), None);
        assert_eq!(small.default_choice(false), Lama);
        assert_eq!(
            small.why_not(Best).unwrap().words(),
            "Needs 6.3 GiB VRAM, 4.1 available. Use Patch."
        );
        let at_need = FillHaves {
            card: Some(need),
            ..all
        };
        assert_eq!(at_need.why_not(Best), None);
        let unknown = FillHaves { card: None, ..all };
        assert_eq!(unknown.why_not(Best), None);
        assert_eq!(unknown.default_choice(false), Best);
    }

    /// What runs here: the remove's two models want WebGPU, 16-bit
    /// floats, and a card that holds them, in the fill's own words; one
    /// Subject file of the two; and nothing at all with ONNX Runtime
    /// off.
    #[test]
    fn here_says_what_never_runs_and_why() {
        use greycard_ai::{DENOISE_FAST, SUBJECT, SUBJECT_WEBGPU};
        let cpu = Here::default();
        let gpu = Here {
            webgpu: true,
            f16: Some((true, "a card".into())),
            card: Some((16 << 30, "a card".into())),
            ..Here::default()
        };
        for m in [&REMOVE, &REMOVE_FAST, &SUBJECT_WEBGPU] {
            let why = cpu.why_not(m);
            if m.id == SUBJECT_WEBGPU.id {
                assert_eq!(why, Some(NeverHere::OtherSubject(false)));
            } else {
                assert_eq!(why, Some(NeverHere::NoWebGpu), "{}", m.id);
            }
            assert_eq!(gpu.why_not(m), None, "{}", m.id);
        }
        assert_eq!(
            NeverHere::NoWebGpu.words(),
            FillHaves::default()
                .why_not(FillChoice::Best)
                .unwrap()
                .words()
        );
        assert_eq!(
            NeverHere::OtherSubject(false).short(),
            "the original runs here"
        );
        assert_eq!(cpu.why_not(&SUBJECT), None);
        assert_eq!(gpu.why_not(&SUBJECT), Some(NeverHere::OtherSubject(true)));
        for m in [&SKY, &SAM, &SAM3, &DENOISE_FAST, &greycard_ai::FILL] {
            assert_eq!(cpu.why_not(m), None, "{}", m.id);
            assert_eq!(gpu.why_not(m), None, "{}", m.id);
        }
        let pascal = Here {
            f16: Some((false, "GTX 1080".into())),
            ..gpu.clone()
        };
        let why = pascal.why_not(&REMOVE_FAST).unwrap();
        assert_eq!(
            why.words(),
            remove_f16_refusal(Some(&(false, "GTX 1080".into()))).unwrap()
        );
        assert_eq!(why.short(), "no 16-bit floats in shaders");
        assert_eq!(pascal.why_not(&SKY), None);
        // A card smaller than the smallest removal's need, as `room`
        // refuses it; one unknown (unified memory) is not a reason.
        let small = Here {
            card: Some((6 << 30, "RTX 2060".into())),
            ..gpu.clone()
        };
        let why = small.why_not(&REMOVE).unwrap();
        assert_eq!(
            why,
            NeverHere::ShortCard("RTX 2060".into(), 6 << 30, remove_needs(512, false))
        );
        assert!(
            why.words().starts_with("RTX 2060 has 6.0 GiB"),
            "{}",
            why.words()
        );
        assert!(why.short().starts_with("needs ") && !why.short().contains("RTX"));
        let unified = Here {
            card: None,
            ..gpu.clone()
        };
        assert_eq!(unified.why_not(&REMOVE), None);
        let off = Here {
            off: Some("this processor has no AVX2".into()),
            ..gpu
        };
        for m in greycard_ai::MODELS {
            assert_eq!(
                off.why_not(m),
                Some(NeverHere::Off("this processor has no AVX2".into()))
            );
        }
        // Anywhere but a Mac, nothing is warmed.
        if !cfg!(target_os = "macos") {
            warm_core_ml(&Store::at("/nonexistent"), &SKY);
        }
    }

    /// The remove makes a Fill drawn for it when its window fits and
    /// it can run; a bigger window is LaMa's; where it cannot run, the
    /// patch is left as it was; a patch drawn for LaMa stays LaMa's.
    #[test]
    fn the_engine_is_chosen_by_the_store_the_providers_and_the_window() {
        let window = |width, height| Window {
            x: 10,
            y: 20,
            width,
            height,
        };
        let gpu = [Provider::WebGpu, Provider::Cpu];
        let cpu = [Provider::Cpu];
        let engine = |marked, have, providers: &[Provider], w| {
            engine_for(marked, have, providers, w).map_err(|e| match e {
                NoFill::Missing(m) => m.id,
                NoFill::NoGpu => "no GPU",
                NoFill::Failed(why) => panic!("{why}"),
                NoFill::Card { .. } => panic!("engine_for asks no card"),
                NoFill::Later => panic!("engine_for never defers"),
            })
        };
        assert_eq!(
            engine(true, true, &gpu, window(1024, 600)),
            Ok(Engine::Remove)
        );
        assert_eq!(engine(true, true, &gpu, window(1, 1)), Ok(Engine::Remove));
        // Past the remove's size it is taken down, up to four times.
        assert_eq!(
            engine(true, true, &gpu, window(1025, 600)),
            Ok(Engine::Remove)
        );
        assert_eq!(
            engine(true, true, &gpu, window(2000, 2000)),
            Ok(Engine::Remove)
        );
        assert_eq!(
            engine(true, true, &gpu, window(4096, 300)),
            Ok(Engine::Remove)
        );
        assert_eq!(
            engine(true, true, &gpu, window(5000, 5000)),
            Ok(Engine::Lama)
        );
        assert_eq!(
            engine(true, true, &gpu, window(300, 4097)),
            Ok(Engine::Lama)
        );
        assert_eq!(engine(true, true, &cpu, window(2000, 2000)), Err("no GPU"));
        assert_eq!(engine(true, true, &cpu, window(500, 500)), Err("no GPU"));
        assert_eq!(engine(true, false, &cpu, window(500, 500)), Err("no GPU"));
        assert_eq!(engine(true, false, &gpu, window(500, 500)), Err(REMOVE.id));
        assert_eq!(
            engine(true, false, &cpu, window(5000, 500)),
            Ok(Engine::Lama)
        );
        for providers in [&gpu[..], &cpu[..]] {
            assert_eq!(
                engine(false, true, providers, window(500, 500)),
                Ok(Engine::Lama)
            );
        }
        // A Fast patch runs four steps, a Best one 25.
        let mut fill = Patch {
            method: greycard_edit::retouch::Method::Fill,
            ..Default::default()
        };
        fill.set_fill_choice(FillChoice::Fast);
        assert_eq!(steps_for(&fill), greycard_ai::remove::Steps::Lcm4);
        fill.set_fill_choice(FillChoice::Best);
        assert_eq!(steps_for(&fill), greycard_ai::remove::Steps::Euler25);
        // The seed's noise is the patch's own.
        let p = |id, seed| Patch {
            id,
            seed,
            ..Default::default()
        };
        assert_ne!(remove_seed(&p(1, 0)), remove_seed(&p(2, 0)));
        assert_ne!(remove_seed(&p(1, 0)), remove_seed(&p(1, 1)));
    }

    /// A fill whose model ran and failed is not run again on every
    /// develop: `fill` gives back why, and the kept picture is not made
    /// again for it, until its seed moves on or another file opens.
    #[test]
    fn a_failed_fill_is_not_run_again() {
        // LaMa's file at its size, so the store has it and the engine
        // is LaMa's; its contents would not load.
        let dir = crate::testing::scratch_dir("failed-fill");
        let store = Store::at(&dir);
        for f in greycard_ai::FILL.files {
            let path = store.path(&greycard_ai::FILL, f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::File::create(&path)
                .unwrap()
                .set_len(f.bytes)
                .unwrap();
        }
        let mut ai = Ai::for_test(store, vec![Provider::Cpu]);
        let image = WorkingImage::new(200, 100);
        let mut patch = Patch {
            id: 7,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[0.5, 0.25]],
            radius: 0.05,
            ..Default::default()
        };
        let retouch = |patch: &Patch| Retouch {
            enabled: true,
            patches: vec![patch.clone()],
        };
        let region = patch.region(image.width, image.height).unwrap();
        assert!(ai.could_fill_more(&retouch(&patch), &image));
        let (window, _) = ai.window_of(&patch, &region, &image);
        ai.remember_failure(&patch, window, "the card answered garbage".into());
        assert!(!ai.could_fill_more(&retouch(&patch), &image));
        match ai.fill(&patch, &region, &image) {
            Err(NoFill::Failed(why)) => assert_eq!(why, "the card answered garbage"),
            other => panic!("{:?}", other.map(|(_, e)| e)),
        }
        // Another variation is tried.
        patch.seed = 1;
        assert!(ai.could_fill_more(&retouch(&patch), &image));
        patch.seed = 0;
        // So is the patch changed.
        let mut bigger = patch.clone();
        bigger.radius = 0.06;
        assert!(ai.could_fill_more(&retouch(&bigger), &image));
        // And so is it on another file.
        ai.forget(None);
        assert!(ai.could_fill_more(&retouch(&patch), &image));
        // What is ahead is counted a patch at a time: a failed one is
        // not, nor one that is not a Fill.
        let mut heal = bigger.clone();
        heal.id = 8;
        heal.method = greycard_edit::retouch::Method::Heal;
        let both = Retouch {
            enabled: true,
            patches: vec![patch.clone(), bigger.clone(), heal],
        };
        assert_eq!(ai.fills_unmade(&both, &image), 2);
        ai.remember_failure(&patch, window, "refused".into());
        assert_eq!(ai.fills_unmade(&both, &image), 1);
        // On a picture the fill was not made from, the fill is ahead
        // of a develop (the count is asked before the retouch) but a
        // kept point past the retouch is not refused for it: a fill
        // in memory for the patch and window is kept there.
        let bigger_region = bigger.region(image.width, image.height).unwrap();
        let (bigger_window, _) = ai.window_of(&bigger, &bigger_region, &image);
        ai.keep_fill(
            &bigger,
            &bigger_region,
            &image,
            bigger_window,
            Engine::Lama,
            vec![0.5; bigger_region.width * bigger_region.height * 3],
        );
        let mut later = image.clone();
        later.data.iter_mut().for_each(|v| *v += 0.1);
        assert!(!ai.fill_kept(&bigger, &bigger_region, &later));
        assert_eq!(ai.fills_unmade(&both, &later), 1);
        assert!(!ai.could_fill_more(&both, &later));
        assert_eq!(ai.fills_unmade(&both, &image), 0);
        crate::testing::remove_dir_retry(&dir);
    }

    /// A fill made is read back from disk once the file is opened
    /// again, with no model in the store, as it was first shown; a
    /// changed patch, another picture size, or a fill of another
    /// version is made again.
    #[test]
    fn a_fill_comes_back_from_disk() {
        let dir = crate::testing::scratch_dir("fill-on-disk");
        let mut ai = Ai::for_test_on_disk(Store::at(dir.join("models")));
        // Never probed: the engine asked is the CPU's, so no remove.
        ai.providers = vec![Provider::Cpu];
        let file = PathBuf::from("open.CR3");
        ai.forget(Some(file.clone()));
        let image = WorkingImage::new(200, 100);
        let patch = Patch {
            id: 7,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[0.5, 0.25]],
            radius: 0.05,
            remove: true,
            seed: 2,
            ..Default::default()
        };
        let region = patch.region(image.width, image.height).unwrap();
        let (window, _) = ai.window_of(&patch, &region, &image);
        let made: Vec<f32> = (0..region.width * region.height * 3)
            .map(|i| i as f32 * 0.001 + 0.1)
            .collect();
        let kept = ai.keep_fill(
            &patch,
            &region,
            &image,
            window,
            Engine::Remove,
            made.clone(),
        );
        for (k, m) in kept.iter().zip(&made) {
            assert!((k - m).abs() <= m * 1e-3, "{k} against {m}");
        }
        let size = (image.width, image.height);
        let print = window_print(&image, &region, window);
        let slot = ai.fill_path(&patch, &region, size, window).expect("a slot");
        let path = slot_files(&slot)[0].clone();
        assert!(path.is_file() && !slot.exists());

        // Opened again: memory is empty, the store has no model.
        ai.forget(Some(file.clone()));
        assert!(ai.fill_kept(&patch, &region, &image));
        ai.forget(Some(file.clone()));
        match ai.fill(&patch, &region, &image) {
            Ok((data, engine)) => {
                assert_eq!(engine, Engine::Remove);
                assert_eq!(data, kept, "the fill first shown");
            }
            Err(e) => panic!("{e:?}"),
        }

        // A patch drawn bigger, or another seed, is another fill.
        ai.forget(Some(file.clone()));
        let mut bigger = patch.clone();
        bigger.radius = 0.06;
        let bigger_region = bigger.region(image.width, image.height).unwrap();
        assert!(!ai.fill_kept(&bigger, &bigger_region, &image));
        assert!(ai.fill(&bigger, &bigger_region, &image).is_err());
        let mut varied = patch.clone();
        varied.seed = 3;
        assert!(!ai.fill_kept(&varied, &region, &image));
        // The same patch made in four steps is another fill, in
        // another slot and in memory; made in full again, the first.
        let mut fast = patch.clone();
        fast.set_fill_choice(FillChoice::Fast);
        assert_ne!(
            ai.fill_path(&fast, &region, size, window),
            ai.fill_path(&patch, &region, size, window)
        );
        assert!(!ai.fill_kept(&fast, &region, &image));
        let mut lama = patch.clone();
        lama.set_fill_choice(FillChoice::Patch);
        assert!(!ai.fill_kept(&lama, &region, &image));
        ai.forget(Some(file.clone()));
        assert!(ai.fill_kept(&patch, &region, &image));
        assert!(ai.fill_in_memory(&fast, window, &print).is_none());
        ai.forget(Some(file.clone()));
        // So is the patch on the picture developed otherwise: another
        // white balance, or a virtual copy's, under the window.
        let mut warmer = image.clone();
        for px in warmer.data.as_chunks_mut::<3>().0 {
            px[0] = 0.3;
        }
        assert!(!ai.fill_kept(&patch, &region, &warmer));
        assert!(ai.fill_kept(&patch, &region, &image));
        // The fill now in memory is no more the warmer picture's than
        // the slot was: the base changed under the window, the fill
        // is made again.
        assert!(ai.fill_in_memory(&patch, window, &print).is_some());
        assert!(!ai.fill_kept(&patch, &region, &warmer));
        assert!(ai.fill_kept(&patch, &region, &image));
        // So is the patch on a picture of another size.
        let other = WorkingImage::new(400, 200);
        let other_region = patch.region(other.width, other.height).unwrap();
        assert!(!ai.fill_kept(&patch, &other_region, &other));
        // And on another file.
        ai.forget(Some(PathBuf::from("other.CR3")));
        assert!(!ai.fill_kept(&patch, &region, &image));

        // The slot read with another version, or a header that does not
        // match, is a miss.
        let dims = (region.width, region.height);
        let any = |_: &[[u16; 3]]| true;
        assert!(read_fill(&path, FILL_VERSION, size, window, dims, any).is_some());
        assert!(read_fill(&path, FILL_VERSION + 1, size, window, dims, any).is_none());
        assert!(read_fill(&path, FILL_VERSION, (201, 100), window, dims, any).is_none());
        assert!(read_fill(&path, FILL_VERSION, size, window, dims, |_| false).is_none());
        // Written at an old version (the grain measured before its line
        // left texture out, say), the slot misses where it stands, and
        // the fill is made again in the same slot.
        write_fill(
            &path,
            FILL_VERSION - 2,
            Engine::Remove,
            size,
            window,
            dims,
            &[],
            &kept,
        );
        ai.forget(Some(file.clone()));
        assert!(!ai.fill_kept(&patch, &region, &image));

        // A failure is not kept on disk.
        ai.remember_failure(&varied, window, "refused".into());
        ai.forget(Some(file));
        assert!(ai.failed_before(&varied, &region, &image).is_none());

        // Off the disk, nothing is written.
        let mut memory = Ai::for_test(Store::at(dir.join("models")), vec![Provider::Cpu]);
        memory.forget(Some(PathBuf::from("open.CR3")));
        assert!(memory.fill_path(&patch, &region, size, window).is_none());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A fill patch on a 200 by 100 picture with a texture under its
    /// window, the picture, its region, and the window.
    fn print_case() -> (WorkingImage, Patch, Region) {
        let mut image = WorkingImage::new(200, 100);
        for (i, px) in image.data.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let (x, y) = ((i % 200) as f32, (i / 200) as f32);
            *px = [
                0.2 + 0.3 * x / 200.0,
                0.3 + 0.1 * (y / 7.0).sin(),
                0.1 + 0.05 * (x / 5.0).cos(),
            ];
        }
        let patch = Patch {
            id: 7,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[0.5, 0.25]],
            radius: 0.05,
            remove: true,
            seed: 2,
            ..Default::default()
        };
        let region = patch.region(image.width, image.height).unwrap();
        (image, patch, region)
    }

    /// The picture with every value scaled by `by`.
    fn scaled(image: &WorkingImage, by: f32) -> WorkingImage {
        let mut out = image.clone();
        out.data.iter_mut().for_each(|v| *v *= by);
        out
    }

    /// The fill's slot is named by the patch, the picture's size, and
    /// the window, never its pixels; the print is in the file, and a
    /// picture moved by less than the tolerance is served the fill, on
    /// disk and in memory, while one moved by more is not.
    #[test]
    fn a_fill_is_served_on_a_print_within_the_tolerance() {
        let dir = crate::testing::scratch_dir("fill-print");
        let mut ai = Ai::for_test_on_disk(Store::at(dir.join("models")));
        ai.providers = vec![Provider::Cpu];
        let file = PathBuf::from("open.CR3");
        ai.forget(Some(file.clone()));
        let (image, patch, region) = print_case();
        let (window, _) = ai.window_of(&patch, &region, &image);
        let size = (image.width, image.height);
        let path = ai.fill_path(&patch, &region, size, window).unwrap();
        let data = vec![0.4; region.width * region.height * 3];
        ai.keep_fill(&patch, &region, &image, window, Engine::Remove, data);
        // The pixels do not name the slot.
        let other = scaled(&image, 1.5);
        assert_eq!(ai.fill_path(&patch, &region, size, window), Some(path));

        let near = scaled(&image, 1.0 + PRINT_TOLERANCE / 4.0);
        let far = scaled(&image, 1.0 + PRINT_TOLERANCE * 2.0);
        // In memory.
        assert!(ai.fill_kept(&patch, &region, &near));
        assert!(!ai.fill_kept(&patch, &region, &far));
        assert!(!ai.fill_kept(&patch, &region, &other));
        // On disk.
        for (picture, served) in [(&near, true), (&far, false), (&image, true)] {
            ai.forget(Some(file.clone()));
            assert_eq!(ai.fill_kept(&patch, &region, picture), served);
        }
        crate::testing::remove_dir_retry(&dir);
    }

    /// Prints compare block by block, relative to the larger of the two
    /// and a floor: a block empty on one side and not the other never
    /// matches, a black block moves by the floor's share, and the
    /// reduced develop's compare passes over empty blocks.
    #[test]
    fn prints_compare_with_a_floor_and_exact_empties() {
        let f = |v: f32| half::f16::from_f32(v).to_bits();
        let a = vec![[f(0.5), f(0.25), f(0.0)], [EMPTY_BLOCK; 3]];
        let mut b = a.clone();
        assert!(prints_agree(&a, &b, PRINT_TOLERANCE, Compare::Every));
        b[0][0] = f(0.5 * (1.0 + PRINT_TOLERANCE * 0.5));
        assert!(prints_agree(&a, &b, PRINT_TOLERANCE, Compare::Every));
        b[0][0] = f(0.5 * (1.0 + PRINT_TOLERANCE * 2.0));
        assert!(!prints_agree(&a, &b, PRINT_TOLERANCE, Compare::Every));
        // Black against a hair above it: within the floor's share.
        let mut c = a.clone();
        c[0][2] = f(PRINT_FLOOR * PRINT_TOLERANCE * 0.5);
        assert!(prints_agree(&a, &c, PRINT_TOLERANCE, Compare::Every));
        // An empty block that is not empty in the other.
        let mut d = a.clone();
        d[1] = [f(0.5); 3];
        assert!(!prints_agree(&a, &d, PRINT_TOLERANCE, Compare::Every));
        assert!(prints_agree(&a, &d, PRINT_TOLERANCE, Compare::Median));
        assert!(!prints_agree(
            &[[EMPTY_BLOCK; 3]],
            &[[f(0.5); 3]],
            1.0,
            Compare::Median
        ));
        // Another count of blocks.
        assert!(!prints_agree(&a, &a[..1], PRINT_TOLERANCE, Compare::Every));
        // One block moved far, the rest where they were: not the
        // picture block by block, the same picture by the median.
        let mut many = vec![[f(0.5), f(0.25), f(0.125)]; 5];
        let stored = many.clone();
        many[2] = [f(0.9); 3];
        assert!(!prints_agree(
            &stored,
            &many,
            PRINT_TOLERANCE,
            Compare::Every
        ));
        assert!(prints_agree(
            &stored,
            &many,
            PRINT_TOLERANCE,
            Compare::Median
        ));
        let all: Vec<_> = stored.iter().map(|_| [f(0.9); 3]).collect();
        assert!(!prints_agree(
            &stored,
            &all,
            PRINT_TOLERANCE,
            Compare::Median
        ));
    }

    /// A fill kept at the version before, under its print hashed into
    /// the name, is read once, kept again at this version under the new
    /// name, and its old slot let go.
    #[test]
    fn a_fill_of_the_version_before_is_kept_again_once() {
        let dir = crate::testing::scratch_dir("fill-hashed");
        let mut ai = Ai::for_test_on_disk(Store::at(dir.join("models")));
        ai.providers = vec![Provider::Cpu];
        let file = PathBuf::from("open.CR3");
        ai.forget(Some(file.clone()));
        let (image, patch, region) = print_case();
        let (window, _) = ai.window_of(&patch, &region, &image);
        let size = (image.width, image.height);
        let dims = (region.width, region.height);
        let print = window_print(&image, &region, window);
        let old = ai
            .fill_path_hashed(&patch, &region, size, window, &print)
            .unwrap();
        let new = ai.fill_path(&patch, &region, size, window).unwrap();
        assert_ne!(old, new);
        let data: Vec<f32> = (0..dims.0 * dims.1 * 3)
            .map(|i| half::f16::from_f32(i as f32 * 1e-4).to_f32())
            .collect();
        write_fill(
            &old,
            FILL_VERSION_HASHED,
            Engine::Remove,
            size,
            window,
            dims,
            &[],
            &data,
        );
        // Another picture does not find it under its own print.
        assert!(!ai.fill_kept(&patch, &region, &scaled(&image, 1.5)));
        assert!(old.is_file());
        // This one does, and keeps it again.
        assert!(ai.fill_kept(&patch, &region, &image));
        let first = slot_files(&new)[0].clone();
        assert!(!old.exists() && first.is_file());
        let (engine, stored, read) =
            read_fill(&first, FILL_VERSION, size, window, dims, |_| true).unwrap();
        assert_eq!((engine, &stored, &read), (Engine::Remove, &print, &data));
        ai.forget(Some(file));
        match ai.fill(&patch, &region, &image) {
            Ok((served, engine)) => assert_eq!((served, engine), (data, Engine::Remove)),
            Err(e) => panic!("{e:?}"),
        }
        crate::testing::remove_dir_retry(&dir);
    }

    /// Virtual copies of one frame, a white balance each past the
    /// tolerance, keep their fills of one patch in one slot's files and
    /// read them back in turn; a fourth base takes the oldest's file,
    /// and the same base again writes over its own.
    #[test]
    fn a_slot_keeps_its_bases_fills_in_turn() {
        let dir = crate::testing::scratch_dir("fill-bases");
        let mut ai = Ai::for_test_on_disk(Store::at(dir.join("models")));
        ai.providers = vec![Provider::Cpu];
        let file = PathBuf::from("open.CR3");
        ai.forget(Some(file.clone()));
        let (image, patch, region) = print_case();
        let (window, _) = ai.window_of(&patch, &region, &image);
        let cells = region.width * region.height * 3;
        let bases: Vec<_> = (0..4)
            .map(|i| scaled(&image, 1.0 + 0.1 * i as f32))
            .collect();
        let keep = |ai: &mut Ai, i: usize| {
            ai.keep_fill(
                &patch,
                &region,
                &bases[i],
                window,
                Engine::Remove,
                vec![0.1 * (i + 1) as f32; cells],
            );
            // Apart in time, so the oldest is told.
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let served = |ai: &mut Ai, i: usize| {
            ai.forget(Some(file.clone()));
            ai.fill(&patch, &region, &bases[i]).ok().map(|(d, _)| d[0])
        };
        let near =
            |v: Option<f32>, i: usize| v.is_some_and(|v| (v - 0.1 * (i + 1) as f32).abs() < 1e-3);
        for i in 0..3 {
            keep(&mut ai, i);
        }
        for i in [0, 1, 2, 0, 2] {
            assert!(near(served(&mut ai, i), i), "base {i}");
        }
        // The same base again: its own file, nothing evicted.
        keep(&mut ai, 1);
        let slot = ai
            .fill_path(&patch, &region, (image.width, image.height), window)
            .unwrap();
        assert!(slot_files(&slot).iter().all(|f| f.is_file()));
        for i in 0..3 {
            assert!(near(served(&mut ai, i), i), "base {i} again");
        }
        // A fourth evicts the oldest written, the first.
        keep(&mut ai, 3);
        assert!(near(served(&mut ai, 3), 3));
        assert!(served(&mut ai, 0).is_none());
        assert!(near(served(&mut ai, 1), 1) && near(served(&mut ai, 2), 2));
        crate::testing::remove_dir_retry(&dir);
    }

    /// The fill of the version before is left where it is when it
    /// cannot be kept again: a slot that cannot be written to.
    #[test]
    fn the_version_before_stays_when_it_cannot_be_kept_again() {
        let dir = crate::testing::scratch_dir("fill-hashed-unwritable");
        let mut ai = Ai::for_test_on_disk(Store::at(dir.join("models")));
        ai.providers = vec![Provider::Cpu];
        ai.forget(Some(PathBuf::from("open.CR3")));
        let (image, patch, region) = print_case();
        let (window, _) = ai.window_of(&patch, &region, &image);
        let size = (image.width, image.height);
        let dims = (region.width, region.height);
        let print = window_print(&image, &region, window);
        let old = ai
            .fill_path_hashed(&patch, &region, size, window, &print)
            .unwrap();
        let data = vec![0.25; dims.0 * dims.1 * 3];
        assert!(write_fill(
            &old,
            FILL_VERSION_HASHED,
            Engine::Remove,
            size,
            window,
            dims,
            &[],
            &data
        ));
        // Every file of the new slot a directory: no write lands.
        let slot = ai.fill_path(&patch, &region, size, window).unwrap();
        for f in slot_files(&slot) {
            std::fs::create_dir_all(&f).unwrap();
        }
        assert!(!write_slot(
            &slot,
            Engine::Remove,
            size,
            window,
            dims,
            &print,
            &data
        ));
        // Served all the same, from the old file, which stays.
        assert!(ai.fill_kept(&patch, &region, &image));
        assert!(old.is_file());
        assert!(
            std::fs::read_dir(old.parent().unwrap())
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().ends_with(".fill-part")),
            "no part file left behind"
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// The box filter by hand: a full region at (3, 2), 4 by 2, taken
    /// down by 2 to a reduced region at (0, 1), 4 by 1. Each reduced
    /// pixel is the mean of the full region's pixels under it, and the
    /// picture's own where none are.
    #[test]
    fn the_box_filter_pins_the_regions_offsets() {
        let region = |x, y, width, height| Region {
            x,
            y,
            width,
            height,
            cover: vec![1.0; width * height],
            ring: vec![0.0; width * height],
        };
        let full = region(3, 2, 4, 2);
        // Each full pixel's value is its column plus ten its row, in
        // the region.
        let data: Vec<f32> = (0..8)
            .flat_map(|i| [(i % 4) as f32 + 10.0 * (i / 4) as f32; 3])
            .collect();
        let image = WorkingImage::from_data(8, 4, vec![9.0; 8 * 4 * 3]).unwrap();
        let out = box_down(&data, &full, &region(0, 1, 4, 1), &image, 2);
        let firsts: Vec<f32> = out.chunks(3).map(|p| p[0]).collect();
        // (0, 1) stands for full columns 0 and 1: none in the region.
        // (1, 1) for column 3 at rows 2 and 3: 0 and 10.
        // (2, 1) for columns 4 and 5: 1, 2, 11, and 12.
        // (3, 1) for columns 6 and 7, the region ending at 6: 3 and 13.
        assert_eq!(firsts, [9.0, 5.0, 6.5, 8.0]);
    }

    /// The picture taken down by `factor`, each pixel its block's mean.
    fn reduced_by(image: &WorkingImage, factor: usize) -> WorkingImage {
        let (w, h) = (image.width / factor, image.height / factor);
        let mut out = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    let mut sum = 0.0;
                    for dy in 0..factor {
                        for dx in 0..factor {
                            sum += image.data
                                [((y * factor + dy) * image.width + x * factor + dx) * 3 + c];
                        }
                    }
                    out.data[(y * w + x) * 3 + c] = sum / (factor * factor) as f32;
                }
            }
        }
        out
    }

    /// A fill kept at full size is read by a develop reduced by 2 or 4,
    /// taken down to the reduced region, within a step of the fill
    /// box-filtered; a fill not on disk, a picture moved, or a factor
    /// that does not divide the print's blocks refuses.
    #[test]
    fn a_reduced_develop_reads_a_fill_kept() {
        let dir = crate::testing::scratch_dir("fill-reduced");
        let mut ai = Ai::for_test_on_disk(Store::at(dir.join("models")));
        ai.providers = vec![Provider::Cpu];
        let file = PathBuf::from("open.CR3");
        ai.forget(Some(file.clone()));
        let mut big = WorkingImage::new(400, 240);
        for (i, px) in big.data.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let (x, y) = ((i % 400) as f32, (i / 400) as f32);
            *px = [0.2 + 0.3 * x / 400.0, 0.3 + 0.1 * y / 240.0, 0.15];
        }
        let full = (big.width, big.height);
        let patch = Patch {
            id: 3,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[0.5, 0.3]],
            radius: 0.04,
            remove: true,
            ..Default::default()
        };
        let reader = ai.fill_reader(&file).expect("on disk");
        let other_reader = ai.fill_reader(Path::new("other.CR3")).unwrap();
        let region = patch.region(full.0, full.1).unwrap();
        let (window, _) = ai.window_of(&patch, &region, &big);
        let fill: Vec<f32> = (0..region.width * region.height)
            .flat_map(|i| {
                let (x, y) = ((i % region.width) as f32, (i / region.width) as f32);
                [0.5 + 0.001 * x, 0.4 + 0.001 * y, 0.3]
            })
            .collect();
        let small = reduced_by(&big, 2);
        let small_region = patch.region(small.width, small.height).unwrap();
        // Not on disk yet.
        assert!(
            reader
                .fill_read_only(&patch, &small_region, &small, 2, full)
                .is_none()
        );
        let kept = ai.keep_fill(&patch, &region, &big, window, Engine::Remove, fill);
        for factor in [2, 4] {
            let small = reduced_by(&big, factor);
            let small_region = patch.region(small.width, small.height).unwrap();
            let read = reader
                .fill_read_only(&patch, &small_region, &small, factor, full)
                .unwrap_or_else(|| panic!("read at {factor}"));
            assert_eq!(read.len(), small_region.width * small_region.height * 3);
            let want = box_down(&kept, &region, &small_region, &small, factor);
            for (r, w) in read.iter().zip(&want) {
                assert!((r - w).abs() <= 1e-6, "{r} against {w} at {factor}");
            }
            // Inside the hole, the fill's values, not the picture's.
            let centre =
                (small_region.height / 2 * small_region.width + small_region.width / 2) * 3;
            assert!(read[centre] > 0.45, "{} at {factor}", read[centre]);
            // Another file's slot is not this one's.
            assert!(
                other_reader
                    .fill_read_only(&patch, &small_region, &small, factor, full)
                    .is_none()
            );
            // A picture the fill was not made from.
            let moved = scaled(&small, 1.0 + REDUCED_PRINT_TOLERANCE * 2.0);
            assert!(
                reader
                    .fill_read_only(&patch, &small_region, &moved, factor, full)
                    .is_none()
            );
        }
        // A third does not divide the print's blocks.
        let third = reduced_by(&big, 3);
        let third_region = patch.region(third.width, third.height).unwrap();
        assert!(
            reader
                .fill_read_only(&patch, &third_region, &third, 3, (402, 240))
                .is_none()
        );
        assert!(
            reader
                .fill_read_only(&patch, &third_region, &third, 3, full)
                .is_none()
        );
        // Off the disk, no reader.
        let memory = Ai::for_test(Store::at(dir.join("models")), vec![Provider::Cpu]);
        assert!(memory.fill_reader(&file).is_none());
        crate::testing::remove_dir_retry(&dir);
    }

    /// The remove's answer at the frame's own pixels is smooth under
    /// the hole, as its VAE gives it back; the frame's noise is put
    /// back there all the same, where LaMa's at its own pixels keeps
    /// its own.
    #[test]
    fn a_removes_answer_gets_the_frames_noise_back() {
        assert!(regrains(Engine::Remove, 1.0));
        assert!(!regrains(Engine::Lama, 1.0));
        assert!(regrains(Engine::Lama, 0.5));
        let (w, h, sigma) = (240usize, 240usize, 0.01f32);
        let mut state = 0x7e57_u64;
        let mut gauss = || {
            let mut u = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 11) as f32 + 0.5) / (1u64 << 53) as f32
            };
            let (u1, u2) = (u(), u());
            (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
        };
        let mut image = WorkingImage::new(w, h);
        for px in image.data.as_chunks_mut::<3>().0.iter_mut() {
            let shared = sigma * gauss();
            for v in px.iter_mut() {
                *v = 0.3 + shared + 0.3 * sigma * gauss();
            }
        }
        // A disc of radius 40 in the middle.
        let (rx, rs) = (70usize, 100usize);
        let cover: Vec<f32> = (0..rs * rs)
            .map(|i| {
                let (x, y) = ((i % rs) as f32 - 50.0, (i / rs) as f32 - 50.0);
                ((40.0 - (x * x + y * y).sqrt()) / 4.0).clamp(0.0, 1.0)
            })
            .collect();
        let region = Region {
            x: rx,
            y: rx,
            width: rs,
            height: rs,
            cover,
            ring: vec![0.0; rs * rs],
        };
        let (window, _) = window_for(&region, w, h);
        assert!(window.width <= greycard_ai::remove::MAX);
        let covered = |px: usize, py: usize| {
            (rx..rx + rs).contains(&px)
                && (rx..rx + rs).contains(&py)
                && region.cover[(py - rx) * rs + px - rx] > 0.0
        };
        let (ww, wh) = (window.width, window.height);
        let mut values = Vec::new();
        let mut hole = Vec::new();
        for py in window.y..window.y + wh {
            let row = (py * w + window.x) * 3;
            values.extend_from_slice(&image.data[row..row + ww * 3]);
            hole.extend((window.x..window.x + ww).map(|px| f32::from(u8::from(covered(px, py)))));
        }
        let input = Rgbf::new(ww, wh, values);
        // The decoder's answer: the picture outside, flat inside.
        let mut decoded = input.clone();
        for (i, &m) in hole.iter().enumerate() {
            if m > 0.0 {
                decoded.data[i * 3..i * 3 + 3].copy_from_slice(&[0.3; 3]);
            }
        }
        let made =
            greycard_ai::remove::composite(&input, &hole, &decoded).expect("an honest answer");
        let mut out = Vec::with_capacity(rs * rs * 3);
        for y in 0..rs {
            for x in 0..rs {
                out.extend(made.at(rx + x - window.x, rx + y - window.y));
            }
        }
        let middle: Vec<usize> = (0..rs * rs).filter(|&i| region.cover[i] >= 1.0).collect();
        // Summed in f64: thousands of equal values summed in f32 put
        // the mean off by more than the flatness asked for below.
        let spread = |data: &[f32]| {
            let v: Vec<f64> = middle
                .iter()
                .map(|&i| f64::from(data[i * 3] + data[i * 3 + 1] + data[i * 3 + 2]) / 3.0)
                .collect();
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / v.len() as f64).sqrt() as f32
        };
        // Not exactly flat: the middle's evening carries a trace of the
        // band's own grain into the hole, a hundredth of the grain.
        assert!(spread(&out) < 0.02 * sigma, "flat before: {}", spread(&out));
        let r = regrain(&mut out, &region, window, &image, 3).expect("the frame has noise");
        let want = sigma * (1.0f32 + 0.09 / 3.0).sqrt();
        let after = spread(&out);
        assert!((after / want - 1.0).abs() < 0.2, "{after} against {want}");
        assert!(r.after[0] > 0.8 * r.frame[0], "{:?} {:?}", r.after, r.frame);
    }

    /// A deep hole in a flat field of known noise, filled smooth at a
    /// smaller scale as the model fills it, comes back with its
    /// surround's noise: far below it before the grain, within 15%
    /// after, and the same on a second run. The field's grain is
    /// mostly shared by the three channels, as a developed frame's is,
    /// and the grain put back keeps that split of luma and chroma
    /// rather than coloring the hole.
    #[test]
    fn a_deep_hole_filled_small_gets_the_frames_noise_back() {
        let (w, h, sigma) = (640usize, 640usize, 0.01f32);
        let mut state = 0x5eed_u64;
        let mut gauss = || {
            let mut u = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 11) as f32 + 0.5) / (1u64 << 53) as f32
            };
            let (u1, u2) = (u(), u());
            (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
        };
        let mut image = WorkingImage::new(w, h);
        for px in image.data.as_chunks_mut::<3>().0.iter_mut() {
            let shared = sigma * gauss();
            for v in px.iter_mut() {
                *v = 0.2 + shared + 0.3 * sigma * gauss();
            }
        }
        // A disc of radius 150 with a soft edge.
        let (rx, ry, rs) = (150usize, 150usize, 340usize);
        let cover: Vec<f32> = (0..rs * rs)
            .map(|i| {
                let (x, y) = ((i % rs) as f32 - 170.0, (i / rs) as f32 - 170.0);
                ((150.0 - (x * x + y * y).sqrt()) / 8.0).clamp(0.0, 1.0)
            })
            .collect();
        let region = Region {
            x: rx,
            y: ry,
            width: rs,
            height: rs,
            cover,
            ring: vec![0.0; rs * rs],
        };
        let (window, depth) = window_for(&region, w, h);
        let hole: Vec<f32> = (0..window.width * window.height)
            .map(|i| {
                let (px, py) = (window.x + i % window.width, window.y + i / window.width);
                let inside = (rx..rx + rs).contains(&px) && (ry..ry + rs).contains(&py);
                let c = if inside {
                    region.cover[(py - ry) * rs + px - rx]
                } else {
                    0.0
                };
                f32::from(u8::from(c > 0.0))
            })
            .collect();
        let mut values = Vec::new();
        for py in window.y..window.y + window.height {
            let row = (py * w + window.x) * 3;
            values.extend_from_slice(&image.data[row..row + window.width * 3]);
        }
        let input = Rgbf::new(window.width, window.height, values);
        // The model's answer at its scale: smooth, the known pixels'
        // mean in the hole.
        let made = greycard_ai::fill::filled(&input, &hole, depth, 1024, |img, part| {
            let known: Vec<usize> = (0..part.len()).filter(|&i| part[i] == 0.0).collect();
            let mut mean = [0.0f32; 3];
            for &i in &known {
                for (c, m) in mean.iter_mut().enumerate() {
                    *m += img.data[i * 3 + c] / known.len() as f32;
                }
            }
            let mut out = img.clone();
            for (i, &m) in part.iter().enumerate() {
                if m > 0.0 {
                    out.data[i * 3..i * 3 + 3].copy_from_slice(&mean);
                }
            }
            Ok(out)
        })
        .unwrap();
        assert!(
            made.scale < 0.5,
            "a hole this deep is filled small: {}",
            made.scale
        );
        let mut out = Vec::with_capacity(rs * rs * 3);
        for y in 0..rs {
            for x in 0..rs {
                let (px, py) = (rx + x, ry + y);
                out.extend(made.image.at(px - window.x, py - window.y));
            }
        }
        // The luma's and the chroma's standard deviations per channel
        // over the pixels `pick` names.
        let spread = |data: &[f32], pick: &dyn Fn(usize) -> bool| {
            let v: Vec<[f32; 2]> = (0..data.len() / 3)
                .filter(|&i| pick(i))
                .map(|i| {
                    let [r, g, b] = [data[i * 3], data[i * 3 + 1], data[i * 3 + 2]];
                    [(r + g + b) / 3.0, (r - b) / 2.0]
                })
                .collect();
            let n = v.len() as f32;
            [0, 1].map(|k| {
                let mean = v.iter().map(|p| p[k]).sum::<f32>() / n;
                (v.iter().map(|p| (p[k] - mean).powi(2)).sum::<f32>() / n).sqrt()
            })
        };
        let middle = |i: usize| region.cover[i] >= 1.0;
        let surround = spread(&image.data, &|i| {
            let (x, y) = ((i % w) as f32 - 320.0, (i / w) as f32 - 320.0);
            (x * x + y * y).sqrt() > 200.0
        });
        let before = spread(&out, &middle);
        let mut again = out.clone();
        regrain(&mut out, &region, window, &image, 7).expect("the frame has noise");
        regrain(&mut again, &region, window, &image, 7).unwrap();
        assert_eq!(out, again);
        let mut patched = image.clone();
        greycard_core::develop::retouch::apply_replacement(&mut patched, &region, &out, 1.0);
        let after = spread(&patched.data, &|i| {
            let (x, y) = (i % w, i / w);
            (rx..rx + rs).contains(&x)
                && (ry..ry + rs).contains(&y)
                && middle((y - ry) * rs + x - rx)
        });
        // Luma: the shared part and a third of the rest; chroma: the
        // rest's alone.
        let want = [
            sigma * (1.0f32 + 0.09 / 3.0).sqrt(),
            0.3 * sigma / 2f32.sqrt(),
        ];
        for k in 0..2 {
            assert!(
                (surround[k] / want[k] - 1.0).abs() < 0.05,
                "surround {surround:?}"
            );
            assert!(
                before[k] < 0.3 * surround[k],
                "before {before:?} against {surround:?}"
            );
            assert!(
                (after[k] / surround[k] - 1.0).abs() < 0.15,
                "after {after:?} against {surround:?} (before {before:?})"
            );
        }
    }

    #[test]
    fn the_inverse_undoes_the_matrix_and_the_transfer_round_trips() {
        let m = greycard_render::export::Space::Srgb.matrix();
        let inv = invert(&m);
        let v = [0.3f32, 0.5, 0.1];
        let back = mul(&inv, mul(&m, v));
        for k in 0..3 {
            assert!((back[k] - v[k]).abs() < 1e-5);
        }
        for &x in &[0.0f32, 0.002, 0.18, 0.5, 1.0] {
            assert!((decode(encode(x)) - x).abs() < 1e-5);
        }
    }

    /// A Background reads the Subject matte the model made, one minus
    /// it, byte for byte — the CPU reference the composed Background
    /// is checked against — and the model runs once for the pair:
    /// whichever shape asks first fills `subject_matte`, and the one
    /// that reads it after touches neither the store nor a model, and
    /// still says where the shared run happened rather than going
    /// quiet the way a plain cache hit does.
    #[test]
    fn a_background_is_one_minus_the_shared_subject_matte_and_costs_it_nothing() {
        let mut ai = Ai::new();
        // Not the real user's store: a shared-matte hit must need
        // none, and the last check below wants a load that fails.
        ai.store = None;
        ai.file = Some(PathBuf::from("frame.CR3"));
        ai.print = Print::test(1);
        let base: Vec<u8> = (0..RASTER_WIDTH * RASTER_WIDTH)
            .map(|i| (i % 256) as u8)
            .collect();
        ai.subject_matte = Some((5, base.clone(), Provider::Cpu));
        let image = WorkingImage::new(4, 4);
        let edit = Edit::default();

        let background = ai
            .raster(
                Base::stamped(5),
                &image,
                &edit,
                greycard_render::finish::Source::SCENE,
                0,
                (1, 0),
                &Shape::Background {},
                None,
            )
            .expect("the shared matte, no store or model needed");
        assert_eq!(
            background.provider,
            Some(Provider::Cpu),
            "a shared hit still says where the run it reads happened"
        );
        let expected: Vec<u8> = base.iter().map(|&v| 255 - v).collect();
        assert_eq!(background.raster.data(), expected.as_slice());

        // The Subject reads the very same run, not a second one.
        let subject = ai
            .raster(
                Base::stamped(5),
                &image,
                &edit,
                greycard_render::finish::Source::SCENE,
                0,
                (1, 1),
                &Shape::Subject {},
                None,
            )
            .expect("the same shared matte");
        assert_eq!(subject.provider, Some(Provider::Cpu));
        assert_eq!(subject.raster.data(), base.as_slice());

        // A different stamp (a new base develop) is not the same run.
        let stale = ai.raster(
            Base::stamped(6),
            &image,
            &edit,
            greycard_render::finish::Source::SCENE,
            0,
            (1, 2),
            &Shape::Background {},
            None,
        );
        assert!(stale.is_err(), "no store to make a fresh matte from");
    }

    /// An install with only the Subject original: `step` offers the
    /// rewrite (the store's original is on record as falling back to
    /// the CPU), the offer is declined, and `panel::mask::step` then
    /// asks for the original by name (`mask.rs`'s own test covers
    /// that choice). What used to go wrong from here is the worker's
    /// side of it: `Subject::load`'s internal `model_for(store,
    /// providers, &[])` had no way to know the rewrite was declined
    /// this session, so with the same failure on record it picked the
    /// rewrite anyway — not in the store, so the load failed with
    /// "is not in the model store" even though the original the panel
    /// asked for was sitting right there. `raster` and `cached_path`
    /// now take the model `step` chose directly, rather than
    /// re-deriving it, so they cannot land on a different one. No
    /// real weights are wanted for this: a raster already on disk,
    /// under the original's cache slot, comes back without a model
    /// ever being loaded.
    #[test]
    fn a_subject_raster_is_cached_under_the_model_step_chose_not_a_fresh_guess() {
        let dir = std::env::temp_dir().join(format!(
            "greycard-ai-subject-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        crate::testing::remove_dir_retry(&dir);
        let mut ai = Ai::new();
        ai.store = Some(Store::at(dir.join("models")));
        ai.file = Some(PathBuf::from("open.CR3"));
        ai.print = Print::test(1);
        ai.providers = vec![Provider::WebGpu, Provider::Cpu];
        let shape = Shape::Subject {};

        // The cache slot depends on which model is named: an install
        // that declines the rewrite must not land on its slot by
        // accident.
        let original_path = ai
            .cached_path(&shape, Some(&greycard_ai::SUBJECT), 0)
            .expect("a path with the original named");
        let rewrite_path = ai
            .cached_path(&shape, Some(&greycard_ai::SUBJECT_WEBGPU), 0)
            .expect("a path with the rewrite named");
        assert_ne!(original_path, rewrite_path);

        // A raster already made under the original's slot: what a
        // Subject want gets back when `step` named the original,
        // whether or not the file itself is in the store.
        let data = vec![128u8; RASTER_WIDTH * RASTER_WIDTH];
        write_raster(&original_path, RASTER_WIDTH, &data);
        let made = ai
            .raster(
                Base::stamped(1),
                &WorkingImage::new(4, 4),
                &Edit::default(),
                greycard_render::finish::Source::SCENE,
                0,
                (7, 0),
                &shape,
                Some(&greycard_ai::SUBJECT),
            )
            .expect("the cached raster comes back, no model load needed");
        assert!(
            made.provider.is_none(),
            "a cache hit names no provider: nothing was loaded to make it"
        );

        crate::testing::remove_dir_retry(&dir);
    }

    /// A Sky raster with nothing in it is the gate's answer, said on
    /// the status line whenever it comes back, from the disk cache
    /// too; a sky with something in it says nothing of the kind. The
    /// cache's slot follows whether SAM is in the store, so a sky made
    /// from the prior alone is made again once SAM arrives.
    #[test]
    fn an_empty_sky_says_no_sky_was_found_and_the_cache_follows_sam() {
        // Under the workspace's target, not the system's temp: the
        // raster is written at the raster's full size.
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-scratch/greycard-ai-sky-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        crate::testing::remove_dir_retry(&dir);
        let mut ai = Ai::new();
        let store = Store::at(dir.join("models"));
        ai.store = Some(store.clone());
        ai.file = Some(PathBuf::from("DSCF0153.RAF"));
        ai.print = Print::test(1);
        let shape = Shape::Sky { picks: Vec::new() };
        let without_sam = ai.cached_path(&shape, None, 0).expect("a path");
        write_raster(
            &without_sam,
            RASTER_WIDTH,
            &vec![0u8; RASTER_WIDTH * RASTER_WIDTH],
        );
        let made = ai
            .raster(
                Base::stamped(1),
                &WorkingImage::new(4, 4),
                &Edit::default(),
                greycard_render::finish::Source::SCENE,
                0,
                (9, 0),
                &shape,
                None,
            )
            .expect("the cached raster");
        assert_eq!(made.note.as_deref(), Some("no sky found in DSCF0153.RAF"));
        // Asked again, from the memory cache: said again.
        let again = ai
            .raster(
                Base::stamped(1),
                &WorkingImage::new(4, 4),
                &Edit::default(),
                greycard_render::finish::Source::SCENE,
                0,
                (9, 0),
                &shape,
                None,
            )
            .expect("the kept raster");
        assert_eq!(again.note, made.note);
        let some = Raster::from_data(1.0, RASTER_WIDTH, vec![7u8; RASTER_WIDTH * RASTER_WIDTH]);
        assert_eq!(ai.empty_note(&shape, &some), None);
        assert_eq!(
            ai.empty_note(&Shape::Subject {}, &Raster::from_data(1.0, 4, vec![0; 16])),
            None
        );

        // SAM arrives: another slot.
        for f in SAM.files {
            let path = store.path(&SAM, f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::File::create(&path)
                .unwrap()
                .set_len(f.bytes)
                .unwrap();
        }
        assert!(store.have(&SAM));
        assert_ne!(ai.cached_path(&shape, None, 0), Some(without_sam));
        crate::testing::remove_dir_retry(&dir);
    }

    /// One raster for one base, whoever asks: the viewport's raster is
    /// the window's export's (the same in memory, though its picture is
    /// another and its stamp the export's), and a run with no window
    /// reads it from disk. A base of another print (the white balance
    /// moved, the reduced develop's) reads none of it, and a panel given
    /// a raster of the old base asks for it again.
    #[test]
    fn one_base_one_raster_and_another_base_none_of_it() {
        use crate::mask_base::{Size, print};
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-scratch/greycard-ai-base-print-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        crate::testing::remove_dir_retry(&dir);
        let store = Store::at(dir.join("models"));
        let file = dir.join("frame.RAF");
        let sky = Shape::Sky { picks: Vec::new() };
        let mut edit = Edit::default();
        edit.light.exposure = 0.5;
        let frame = (6000usize, 4000usize);
        let rows = raster_height(RASTER_WIDTH, 4000.0 / 6000.0);
        let full = print(Some(&file), &edit, 0, Size::Full);
        let fresh = || {
            let mut ai = Ai::new();
            ai.store = Some(store.clone());
            ai.forget(Some(file.clone()));
            ai
        };
        let mut ai = fresh();
        assert_eq!(ai.base_print(&edit, 0, Size::Full), full);
        ai.print = full;
        let slot = ai
            .cached_path_at(&sky, None, 0, RASTER_WIDTH)
            .expect("a slot");
        write_raster(&slot, rows, &vec![9u8; RASTER_WIDTH * rows]);
        let kind = greycard_render::finish::Source::SCENE;
        let ask = |ai: &mut Ai, base: Base, picture: (usize, usize), edit: &Edit| {
            ai.raster_for(
                frame,
                base,
                &WorkingImage::new(picture.0, picture.1),
                edit,
                kind,
                0,
                (1, 0),
                &sky,
                None,
            )
        };
        let viewport = ask(
            &mut ai,
            Base {
                stamp: 5,
                print: full,
            },
            (6, 4),
            &edit,
        )
        .expect("the viewport's raster");
        assert!(viewport.raster.data().iter().all(|&v| v == 9));
        ai.given((1, 0), &sky, None, None, full);
        // The window's export: its own picture, the export's stamp, and
        // the edit's print, worked out from the edit alike.
        let export_print = ai.base_print(&edit, 0, Size::Full);
        let export = ask(
            &mut ai,
            Base {
                stamp: 1,
                print: export_print,
            },
            (12, 8),
            &edit,
        )
        .expect("the export's raster");
        assert!(Arc::ptr_eq(&export.raster, &viewport.raster));
        // No window: another process, the same disk.
        let mut alone = fresh();
        let headless = ask(
            &mut alone,
            Base {
                stamp: 1,
                print: full,
            },
            (12, 8),
            &edit,
        )
        .expect("the headless raster");
        assert!(headless.provider.is_none(), "read, not made");
        assert_eq!(headless.raster.data(), viewport.raster.data());

        // The white balance moved: another base, and none of the old.
        let mut warmer = edit.clone();
        warmer.white_balance = greycard_edit::WhiteBalance::Custom {
            temperature: 6500.0,
            tint: 0.0,
        };
        let moved = ai.base_print(&warmer, 0, Size::Full);
        assert_ne!(moved, full);
        let stale = ask(
            &mut ai,
            Base {
                stamp: 6,
                print: moved,
            },
            (6, 4),
            &warmer,
        );
        assert!(
            stale.is_err(),
            "made again with no model to make it, not read: {:?}",
            stale.map(|m| m.raster.data()[0])
        );
        assert!(
            ai.to_ask_again(None, full, 0, |_, _| true).is_empty(),
            "the same base: nothing to ask"
        );
        assert!(
            ai.to_ask_again(None, moved, 0, |_, _| true).is_empty(),
            "no fill moved"
        );
        assert!(ai.of_another_base(full, |_, _| true).is_empty());
        let again = ai.of_another_base(moved, |_, _| true);
        assert_eq!(
            again
                .iter()
                .map(|(k, s, _)| (*k, s.clone()))
                .collect::<Vec<_>>(),
            vec![((1, 0), sky.clone())],
            "a new base: asked for again"
        );
        // The reduced develop's base is a proxy's: it reads none of the
        // full develop's, before the decode or after.
        let reduced = print(Some(&file), &edit, 0, Size::Reduced);
        assert_ne!(reduced, full);
        assert!(!matches!(
            alone.raster_ready(&file, &sky, 0, Some(frame), reduced),
            Ready::Cached
        ));
        assert!(
            alone
                .cached_raster(&file, &sky, 0, (6000, 4000), reduced)
                .is_none()
        );
        assert!(matches!(
            alone.raster_ready(&file, &sky, 0, Some(frame), full),
            Ready::Cached
        ));
        // The reduced develop's raster in a slot beside the full one's:
        // each read back, neither removed by the other.
        alone.print = reduced;
        let proxy_slot = alone
            .cached_path_at(&sky, None, 0, RASTER_WIDTH)
            .expect("a slot");
        let stem = |p: &Path| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            name.split_once('-').unwrap().0.to_string()
        };
        assert_ne!(stem(&proxy_slot), stem(&slot));
        write_raster(&proxy_slot, rows, &vec![3u8; RASTER_WIDTH * rows]);
        assert!(slot.is_file());
        let read = |ai: &mut Ai, print| {
            ai.cached_raster(&file, &sky, 0, (6000, 4000), print)
                .expect("read back")
                .data()[0]
        };
        assert_eq!(read(&mut alone, reduced), 3);
        assert_eq!(read(&mut alone, full), 9);
        assert_eq!(read(&mut alone, reduced), 3);

        // A slot keeps the newest three bases' rasters, and lets go of
        // its raster from before rasters were kept by their base.
        let unprinted = slot.with_file_name(format!("{}.png", stem(&slot)));
        std::fs::write(&unprinted, b"made before prints").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&slot)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000))
            .unwrap();
        let newer: Vec<PathBuf> = [moved, Print::test(7), Print::test(8)]
            .into_iter()
            .map(|print| {
                ai.print = print;
                let p = ai
                    .cached_path_at(&sky, None, 0, RASTER_WIDTH)
                    .expect("a slot");
                assert_eq!(stem(&p), stem(&slot));
                write_raster(&p, rows, &vec![4u8; RASTER_WIDTH * rows]);
                assert!(!unprinted.exists(), "the raster from before prints goes");
                p
            })
            .collect();
        assert!(!slot.exists(), "the oldest of four goes");
        assert!(newer.iter().all(|p| p.is_file()));
        assert!(proxy_slot.is_file(), "another slot's is not touched");
        crate::testing::remove_dir_retry(&dir);
    }

    /// A Sky on a frame past the raster rule (`brush::raster_width`) is
    /// kept in a slot of its own, the wide raster's; a Sky on a frame
    /// under it, and every other shape on any frame, keeps the slot it
    /// had, so its raster is read as it was. A frame the rule moved
    /// finds its old narrow raster ready before the decode (its size
    /// is not known yet) and not readable after it, so its masks are
    /// made again; the reduced develop reads the wide one back, and so
    /// does the open frame's.
    #[test]
    fn a_wide_frames_sky_is_kept_in_a_slot_of_its_own() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-scratch/greycard-ai-wide-sky-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        crate::testing::remove_dir_retry(&dir);
        let mut ai = Ai::new();
        ai.store = Some(Store::at(dir.join("models")));
        let file = PathBuf::from("frame.RAF");
        ai.file = Some(file.clone());
        ai.print = Print::test(1);
        let sky = Shape::Sky { picks: Vec::new() };
        let slot = |ai: &Ai, shape: &Shape, frame: (usize, usize)| {
            ai.cached_path_at(shape, None, 0, raster_width(shape, frame))
                .expect("a slot")
        };
        let narrow = ai.cached_path(&sky, None, 0).expect("a slot");
        // Under the rule: 24 MP either way, 45 MP upright.
        for frame in [(6000, 4000), (4000, 6000), (5464, 8192)] {
            assert_eq!(slot(&ai, &sky, frame), narrow, "{frame:?}");
        }
        // Past it: 45 MP and 100 MP.
        let wide = slot(&ai, &sky, (8736, 11648));
        assert_ne!(wide, narrow);
        assert_eq!(slot(&ai, &sky, (8192, 5464)), wide);
        // Every other shape keeps its slot on the largest frame.
        for shape in [
            Shape::Landscape {
                group: greycard_edit::mask::LandscapeGroup::Foliage,
            },
            Shape::Landscape {
                group: greycard_edit::mask::LandscapeGroup::Water,
            },
            Shape::Subject {},
        ] {
            assert_eq!(
                slot(&ai, &shape, (11648, 8736)),
                ai.cached_path(&shape, None, 0).expect("a slot"),
                "{shape:?}"
            );
        }

        // A 100 MP frame upright, 3 by 4: its old narrow raster.
        let full = (8736u32, 11648u32);
        assert_eq!(raster_height(RASTER_WIDTH, 4.0 / 3.0), 2731);
        write_raster(&narrow, 2731, &vec![9u8; RASTER_WIDTH * 2731]);
        let sized = Some((full.0 as usize, full.1 as usize));
        // Its size known before the decode: not ready, so its masks are
        // made before a develop is spent on it. Not known: either slot.
        assert!(!matches!(
            ai.raster_ready(&file, &sky, 0, sized, Print::test(1)),
            Ready::Cached
        ));
        assert!(matches!(
            ai.raster_ready(&file, &sky, 0, None, Print::test(1)),
            Ready::Cached
        ));
        assert!(matches!(
            ai.raster_ready(&file, &sky, 0, Some((3000, 4000)), Print::test(1)),
            Ready::Cached
        ));
        assert!(
            ai.cached_raster(&file, &sky, 0, full, Print::test(1))
                .is_none()
        );
        // A frame of that aspect under the rule still reads it.
        let small = ai
            .cached_raster(&file, &sky, 0, (3000, 4000), Print::test(1))
            .expect("the narrow raster");
        assert_eq!((small.width, small.height), (RASTER_WIDTH, 2731));

        // The wide one made: ready, read back at its width.
        std::fs::remove_file(&narrow).unwrap();
        assert!(!matches!(
            ai.raster_ready(&file, &sky, 0, None, Print::test(1)),
            Ready::Cached
        ));
        write_raster(&wide, 5461, &vec![7u8; WIDE_RASTER_WIDTH * 5461]);
        assert!(matches!(
            ai.raster_ready(&file, &sky, 0, sized, Print::test(1)),
            Ready::Cached
        ));
        assert!(matches!(
            ai.raster_ready(&file, &sky, 0, None, Print::test(1)),
            Ready::Cached
        ));
        let read = ai
            .cached_raster(&file, &sky, 0, full, Print::test(1))
            .expect("the wide raster");
        assert_eq!((read.width, read.height), (WIDE_RASTER_WIDTH, 5461));
        assert!(read.data().iter().all(|&v| v == 7));
        // The open frame's raster, from a picture of the frame's shape.
        let made = ai
            .raster_for(
                (full.0 as usize, full.1 as usize),
                Base::stamped(1),
                &WorkingImage::new(3, 4),
                &Edit::default(),
                greycard_render::finish::Source::SCENE,
                0,
                (3, 0),
                &sky,
                None,
            )
            .expect("the cached wide raster");
        assert_eq!(made.raster.width, WIDE_RASTER_WIDTH);
        assert!(made.provider.is_none());
        crate::testing::remove_dir_retry(&dir);
    }

    /// The index's masks are made on the reduced develop's binned
    /// picture, whose height the binning rounds; the raster takes its
    /// rows from the frame, so the one the index writes is the one the
    /// reduced develop and the open frame read back, exactly. An X-Trans
    /// frame of 6240 by 4160 binned by three is 2080 by 1386: its
    /// picture's aspect gives 2729 rows at 4096, the frame's 2731; one of
    /// 7728 by 5152 is 2576 by 1717, 2730 rows against 2731.
    #[test]
    fn a_binned_pictures_raster_has_the_frames_rows() {
        for (frame, picture, rows) in [
            ((6240usize, 4160usize), (2080usize, 1386usize), 2731usize),
            ((7728, 5152), (2576, 1717), 2731),
        ] {
            let off = raster_height(WIDE_RASTER_WIDTH, picture.1 as f32 / picture.0 as f32);
            assert_ne!(
                off, rows,
                "{frame:?}: the picture's rows are not the frame's"
            );
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
                "../../target/test-scratch/greycard-ai-binned-sky-{}-{:?}-{}",
                std::process::id(),
                std::thread::current().id(),
                frame.0
            ));
            crate::testing::remove_dir_retry(&dir);
            let mut ai = Ai::new();
            ai.store = Some(Store::at(dir.join("models")));
            let file = PathBuf::from("frame.RAF");
            ai.file = Some(file.clone());
            ai.print = Print::test(1);
            let sky = Shape::Sky { picks: Vec::new() };
            assert_eq!(raster_width(&sky, frame), WIDE_RASTER_WIDTH);
            let slot = ai
                .cached_path_at(&sky, None, 0, WIDE_RASTER_WIDTH)
                .expect("a slot");
            write_raster(&slot, rows, &vec![5u8; WIDE_RASTER_WIDTH * rows]);
            // The index's ask, on the binned picture: the frame's rows,
            // read from the slot exactly, nothing made.
            let made = ai
                .raster_for(
                    frame,
                    Base::stamped(1),
                    &WorkingImage::new(picture.0, picture.1),
                    &Edit::default(),
                    greycard_render::finish::Source::SCENE,
                    0,
                    (4, 0),
                    &sky,
                    None,
                )
                .expect("the cached raster, read on the binned picture");
            assert!(made.provider.is_none(), "{frame:?}: read, not made");
            assert_eq!(
                (made.raster.width, made.raster.height),
                (WIDE_RASTER_WIDTH, rows)
            );
            assert_eq!(made.raster.aspect, frame.1 as f32 / frame.0 as f32);
            // The reduced develop's read, by the frame's size.
            let read = ai
                .cached_raster(
                    &file,
                    &sky,
                    0,
                    (frame.0 as u32, frame.1 as u32),
                    Print::test(1),
                )
                .expect("the reduced develop reads it");
            assert_eq!(read.height, rows);
            crate::testing::remove_dir_retry(&dir);
        }
    }

    /// A Sky's wide matte is not kept as Foliage's cut: only a sky
    /// with no picks at a size Foliage asks for is, so the wide one
    /// holds no floats for nothing and does not put out a narrow cut
    /// already kept.
    #[test]
    fn a_wide_sky_is_not_kept_as_foliages_cut() {
        let frame = (8736, 11648);
        let foliage = Shape::Landscape {
            group: greycard_edit::mask::LandscapeGroup::Foliage,
        };
        let sky = Shape::Sky { picks: Vec::new() };
        let size = |shape: &Shape| {
            let w = raster_width(shape, frame);
            (w, raster_height(w, frame.1 as f32 / frame.0 as f32))
        };
        assert!(is_foliages_cut(false, size(&foliage)));
        assert!(!is_foliages_cut(false, size(&sky)));
        assert!(!is_foliages_cut(true, size(&foliage)));
    }

    /// A rectangle's rendering is the preview's rendering of the same
    /// pixels: on a picture under the preview's size, where the
    /// preview is the picture at its own pixels, the two agree byte
    /// for byte; a rectangle past the edge is held to the picture.
    #[test]
    fn a_region_is_the_previews_rendering_of_its_pixels() {
        let (w, h) = (64usize, 48usize);
        let mut image = WorkingImage::new(w, h);
        for (i, v) in image.data.iter_mut().enumerate() {
            *v = ((i * 37) % 101) as f32 / 120.0;
        }
        let mut edit = Edit::default();
        edit.light.exposure = 0.4;
        let kind = greycard_render::finish::Source::SCENE;
        let whole = preview(&image, &edit, kind);
        assert_eq!((whole.width, whole.height), (w, h));
        let rect = sam3::Rect {
            x0: 10,
            y0: 5,
            x1: 30,
            y1: 21,
        };
        let part = region(&image, &edit, kind, rect);
        assert_eq!((part.width, part.height), (20, 16));
        for y in 0..16 {
            for x in 0..20 {
                let a = &part.data[(y * 20 + x) * 3..][..3];
                let b = &whole.data[((y + 5) * w + x + 10) * 3..][..3];
                assert_eq!(a, b, "({x}, {y})");
            }
        }
        let past = region(
            &image,
            &edit,
            kind,
            sam3::Rect {
                x0: 50,
                y0: 40,
                x1: 90,
                y1: 90,
            },
        );
        assert_eq!((past.width, past.height), (14, 8));
    }

    /// The People model's view is the camera's own orientation: a crop
    /// of the turned picture at `turn_rect`'s rectangle, turned back, is
    /// that crop of the unturned picture; a point maps as its pixel
    /// does; four quarter turns are none.
    #[test]
    fn the_unturned_view_maps_back_and_forth() {
        let (w, h) = (7usize, 4usize);
        let pic: Vec<u8> = (0..w * h * 3).map(|i| (i * 7 % 251) as u8).collect();
        let crop = |data: &[u8], w: usize, r: sam3::Rect| -> Vec<u8> {
            (r.y0..r.y1)
                .flat_map(|y| (r.x0..r.x1).flat_map(move |x| (0..3).map(move |c| (y, x, c))))
                .map(|(y, x, c)| data[(y * w + x) * 3 + c])
                .collect()
        };
        let r = sam3::Rect {
            x0: 1,
            y0: 1,
            x1: 4,
            y1: 3,
        };
        for turn in 0..4u8 {
            let (turned, tw, th) = rotate(&pic, w, h, 3, turn);
            let t = turn_rect(turn, (w, h), r);
            let (back, bw, bh) = rotate(
                &crop(&turned, tw, t),
                t.x1 - t.x0,
                t.y1 - t.y0,
                3,
                (4 - turn) % 4,
            );
            assert_eq!((bw, bh), (3, 2), "turn {turn}");
            assert_eq!(back, crop(&pic, w, r), "turn {turn}");
            // A pixel's center lands where its rotation puts it.
            let (x, y) = (5usize, 1usize);
            let f = turn_frac(
                turn,
                [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32],
            );
            let (px, py) = ((f[0] * tw as f32) as usize, (f[1] * th as f32) as usize);
            assert_eq!(
                turned[(py * tw + px) * 3..][..3],
                pic[(y * w + x) * 3..][..3],
                "turn {turn}"
            );
            let (round, _, _) = rotate(&turned, tw, th, 3, (4 - turn) % 4);
            assert_eq!(round, pic);
        }
    }

    /// On the picture a person was picked on, the face at `at`: the
    /// nearest within a face's width, and nobody farther.
    #[test]
    fn the_face_at_a_place_is_the_nearest_within_its_width() {
        let people = [
            candidate([0.2, 0.1, 0.3, 0.2], None),
            candidate([0.32, 0.1, 0.42, 0.2], None),
        ];
        assert_eq!(at_face(&people, [0.26, 0.16]), Some(0));
        assert_eq!(at_face(&people, [0.36, 0.14]), Some(1));
        assert_eq!(at_face(&people, [0.6, 0.6]), None);
    }

    /// A person found on a picture's unturned view, `aspect` its height
    /// over its width: the face's box, and a colors signature from each
    /// band's lightness, head, upper and lower (none: not shown, as for
    /// a face no body went with).
    fn found_person(face: [f32; 4], bands: [Option<f32>; 3], aspect: f32) -> sam3::Person {
        let band = |l: Option<f32>| {
            l.map_or(sam3::Band::default(), |l| sam3::Band {
                mean: [l, 5.0, -5.0],
                std: [8.0, 3.0, 3.0],
                fill: 0.5,
            })
        };
        sam3::Person {
            face,
            score: 0.9,
            body: None,
            signature: sam3::Signature::colors(bands.map(band), 0.12).unwrap(),
            at: [
                (face[0] + face[2]) / 2.0,
                (face[1] + face[3]) / 2.0 * aspect,
            ],
        }
    }

    /// The one person of a portrait, kept by the part made there as the
    /// panel keeps her (`Candidate::person`), is settled as a person
    /// picked on a group is: on her own picture she is the face where
    /// hers was, however many faces are found there now and whichever
    /// way it is turned; on her other frame, found by her colors; on a
    /// stranger's portrait, nobody; on a group with her, asked, with her
    /// as the guess only when she is clearly the nearest. A face no body
    /// went with is settled on her own picture alone and asked about
    /// everywhere else.
    #[test]
    fn a_portraits_one_person_is_settled_as_a_pick_is() {
        let a = 0.75;
        let hash = Some("portrait".to_string());
        let on = |found: &[sam3::Person], turn: u8, aspect: f32| -> Vec<Candidate> {
            found
                .iter()
                .map(|q| Candidate::of(q, turn, aspect, &hash))
                .collect()
        };
        let face = [0.4, 0.2, 0.52, 0.36];
        let her = found_person(face, [Some(60.0), Some(40.0), Some(30.0)], a);
        let kept = Candidate::of(&her, 0, a, &hash).person();
        assert_eq!(kept.picture, hash);
        let settled = |kept: &greycard_edit::mask::Person, found: &[sam3::Person], own: bool| {
            settle(kept, found, &on(found, 0, a), own, 0, (a, a))
        };
        // Someone found beside her later, as alike as a stranger in a
        // group can be: the colors alone would ask.
        let twin = found_person(
            [0.1, 0.2, 0.22, 0.36],
            [Some(60.0), Some(41.0), Some(30.0)],
            a,
        );

        // Her own picture: her, alone or with the face found beside her,
        // and with no hash to know it by, by her same render.
        assert!(matches!(
            settled(&kept, std::slice::from_ref(&her), true),
            Choice::Person(0, _)
        ));
        let two = [twin.clone(), her.clone()];
        assert!(matches!(settled(&kept, &two, true), Choice::Person(1, _)));
        assert!(matches!(settled(&kept, &two, false), Choice::Person(1, _)));
        // Turned a quarter, the shape turned with it: still her.
        let t = greycard_edit::mask::Turned::new(1, 1.0 / a);
        let mut turned = kept.clone();
        turned.at = t.pos(kept.at);
        assert!(matches!(
            settle(&turned, &two, &on(&two, 1, 1.0 / a), true, 1, (1.0 / a, a)),
            Choice::Person(1, _)
        ));

        // Her other frame: found by her colors.
        let later = found_person(
            [0.3, 0.1, 0.4, 0.24],
            [Some(61.0), Some(42.0), Some(31.0)],
            a,
        );
        assert!(matches!(
            settled(&kept, std::slice::from_ref(&later), false),
            Choice::Person(0, _)
        ));
        // A stranger's portrait: nobody, never the stranger.
        let stranger = found_person(face, [Some(70.0), Some(80.0), Some(75.0)], a);
        assert_eq!(
            settled(&kept, std::slice::from_ref(&stranger), false),
            Choice::Nobody
        );
        // Her own picture after its base changed (a white balance or a
        // profile moves the render her signature is read from past
        // BOUND): the face where hers was is asked about, the guess,
        // never nobody.
        assert_eq!(
            settled(&kept, std::slice::from_ref(&stranger), true),
            Choice::Ask { guess: Some(0) }
        );
        assert_eq!(
            settled(&kept, &[twin.clone(), stranger.clone()], true),
            Choice::Ask { guess: Some(1) }
        );
        // The stranger picked there, the part made for her once asked
        // about (`decided`): hers on that picture from then on.
        let picked = Candidate::of(&stranger, 0, a, &hash).person();
        assert!(matches!(
            settled(&picked, std::slice::from_ref(&stranger), true),
            Choice::Person(0, _)
        ));
        // A group with her: asked, her the guess when clearly nearest.
        let other = found_person(
            [0.7, 0.2, 0.8, 0.34],
            [Some(50.0), Some(10.0), Some(90.0)],
            a,
        );
        let group = [stranger.clone(), later.clone(), other];
        assert_eq!(
            settled(&kept, &group, false),
            Choice::Ask { guess: Some(1) }
        );
        let alike = found_person(
            [0.7, 0.2, 0.8, 0.34],
            [Some(60.0), Some(43.0), Some(32.0)],
            a,
        );
        assert_eq!(
            settled(&kept, &[later.clone(), alike], false),
            Choice::Ask { guess: None }
        );

        // A face no body went with: hers on her own picture, by her
        // same render, beside another face too; asked about elsewhere,
        // where her head alone tells no one apart.
        let head = found_person(face, [Some(60.0), None, None], a);
        let kept = Candidate::of(&head, 0, a, &hash).person();
        assert!(matches!(
            settled(&kept, std::slice::from_ref(&head), true),
            Choice::Person(0, _)
        ));
        assert!(matches!(
            settled(&kept, &[twin, head.clone()], true),
            Choice::Person(1, _)
        ));
        assert_eq!(
            settled(&kept, std::slice::from_ref(&later), false),
            Choice::Ask { guess: None }
        );
        assert_eq!(
            settled(&kept, std::slice::from_ref(&stranger), false),
            Choice::Ask { guess: None }
        );
    }

    /// A person found on the unturned view, shown on the picture turned
    /// a quarter clockwise: the face box and the place where the turn
    /// puts them, the body turned with them.
    #[test]
    fn a_person_on_the_view_is_shown_where_the_turn_puts_them() {
        let body = greycard_ai::Mask::new(
            2,
            2,
            vec![1.0, 0.0, 0.0, 0.0], // the top left cell
        );
        let found = sam3::Person {
            face: [0.1, 0.2, 0.3, 0.4],
            score: 0.9,
            body: Some(body),
            signature: sam3::Signature::new(sam3::SignatureKind::Colors1, vec![0.0; 22]).unwrap(),
            at: [0.2, 0.3],
        };
        let aspect = 0.5;
        let c = Candidate::of(&found, 1, aspect, &Some("ab".into()));
        // (x, y) -> (1 - y, x): the box from (0.6, 0.1) to (0.8, 0.3).
        for (got, want) in c.face.iter().zip([0.6, 0.1 * aspect, 0.8, 0.3 * aspect]) {
            assert!((got - want).abs() < 1e-6, "{:?}", c.face);
        }
        assert!((c.at[0] - 0.7).abs() < 1e-6 && (c.at[1] - 0.2 * aspect).abs() < 1e-6);
        assert_eq!(c.body.as_ref().unwrap().data, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(c.person().picture.as_deref(), Some("ab"));
    }

    fn candidate(face: [f32; 4], body: Option<[f32; 4]>) -> Candidate {
        let body = body.map(|b| {
            let n = 32;
            Arc::new(greycard_ai::Mask::new(
                n,
                n,
                (0..n * n)
                    .map(|i| {
                        let (u, v) = (
                            (i % n) as f32 / n as f32 + 0.5 / n as f32,
                            (i / n) as f32 / n as f32 + 0.5 / n as f32,
                        );
                        let inside = u >= b[0] && u < b[2] && v >= b[1] && v < b[3];
                        if inside { 0.95 } else { 0.0 }
                    })
                    .collect(),
            ))
        });
        Candidate {
            face,
            at: [(face[0] + face[2]) / 2.0, (face[1] + face[3]) / 2.0],
            signature: greycard_edit::mask::Signature {
                kind: "colors-1".into(),
                values: vec![0.0; 22],
            },
            body,
            picture: None,
        }
    }

    /// A click picks the face under it, else the body (the smaller of
    /// two that hold it, so a child held up is the child), else no one.
    /// The bodies are in the picture's fractions, the click in the
    /// masks' units.
    #[test]
    fn a_click_picks_the_face_then_the_body_under_it() {
        let aspect = 0.5;
        let people = [
            // An adult, tall, holding the child's body in theirs.
            candidate([0.2, 0.05, 0.3, 0.1], Some([0.1, 0.0, 0.5, 1.0])),
            candidate([0.3, 0.2, 0.36, 0.24], Some([0.25, 0.4, 0.45, 0.7])),
            // A face no body went with.
            candidate([0.8, 0.1, 0.9, 0.2], None),
        ];
        assert_eq!(picked(&people, (0.25, 0.07), aspect), Some(0));
        assert_eq!(picked(&people, (0.33, 0.22), aspect), Some(1));
        assert_eq!(picked(&people, (0.85, 0.15), aspect), Some(2));
        // On the child's body, inside the adult's: the child.
        assert_eq!(picked(&people, (0.35, 0.55 * aspect), aspect), Some(1));
        // On the adult's alone.
        assert_eq!(picked(&people, (0.15, 0.9 * aspect), aspect), Some(0));
        // Under the bodiless face, and off the picture: no one.
        assert_eq!(picked(&people, (0.85, 0.4), aspect), None);
        assert_eq!(picked(&people, (1.5, 0.1), aspect), None);
    }

    /// A raster made on the base unturned is not served for the base
    /// turned, from memory or from disk, though the shape's JSON is the
    /// same and a half turn keeps the raster's height; the unturned
    /// slot is the one it always had.
    #[test]
    fn a_turned_base_is_not_served_the_unturned_raster() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-scratch/greycard-ai-turn-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        crate::testing::remove_dir_retry(&dir);
        let mut ai = Ai::new();
        ai.store = Some(Store::at(dir.join("models")));
        ai.file = Some(PathBuf::from("square.CR3"));
        ai.print = Print::test(1);
        let shape = Shape::Subject {};
        let model = Some(&greycard_ai::SUBJECT);
        let flat = ai.cached_path(&shape, model, 0).expect("a path");
        assert_eq!(ai.cached_path(&shape, model, 4), Some(flat.clone()));
        for turn in 1..4 {
            assert_ne!(ai.cached_path(&shape, model, turn), Some(flat.clone()));
        }
        write_raster(&flat, RASTER_WIDTH, &vec![9u8; RASTER_WIDTH * RASTER_WIDTH]);
        let ask = |ai: &mut Ai, turn: u8| {
            ai.raster(
                Base::stamped(1),
                &WorkingImage::new(4, 4),
                &Edit::default(),
                greycard_render::finish::Source::SCENE,
                turn,
                (3, 0),
                &shape,
                model,
            )
        };
        assert!(ask(&mut ai, 0).is_ok(), "unturned: the disk's");
        // Now in memory too, for the unturned base only: turned, the
        // model is wanted, and there is none in this store.
        assert!(ask(&mut ai, 2).is_err(), "a half turn is not served it");
        assert!(ask(&mut ai, 0).is_ok());
        crate::testing::remove_dir_retry(&dir);
    }

    /// A Part's raster on disk is keyed by its person as well as its
    /// phrase: one person's mask is never served for another's.
    #[test]
    fn a_parts_cache_slot_is_its_persons() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-scratch/greycard-ai-part-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut ai = Ai::new();
        ai.store = Some(Store::at(dir.join("models")));
        ai.file = Some(PathBuf::from("DSCF0835.RAF"));
        ai.print = Print::test(1);
        let person = |x: f32| {
            Some(greycard_edit::mask::Person {
                signature: greycard_edit::mask::Signature {
                    kind: "colors-1".into(),
                    values: vec![1.0; 22],
                },
                at: [x, 0.3],
                picture: None,
            })
        };
        let part = |person| Shape::Part {
            phrase: "lips".into(),
            route: greycard_edit::mask::Route::Mouth,
            person,
        };
        let everyone = ai.cached_path(&part(None), None, 0).expect("a path");
        let her = ai.cached_path(&part(person(0.3)), None, 0).expect("a path");
        let him = ai.cached_path(&part(person(0.7)), None, 0).expect("a path");
        assert_ne!(everyone, her);
        assert_ne!(her, him);
        assert_eq!(
            ai.cached_path(&part(person(0.3)), None, 0),
            Some(her.clone())
        );
        // The whole person: a slot of its own for each person too.
        let whole = |person| Shape::Part {
            phrase: "person".into(),
            route: greycard_edit::mask::Route::Whole,
            person,
        };
        let whole_her = ai
            .cached_path(&whole(person(0.3)), None, 0)
            .expect("a path");
        let whole_him = ai
            .cached_path(&whole(person(0.7)), None, 0)
            .expect("a path");
        assert!(whole_her != whole_him && whole_her != her);
        assert_eq!(
            model_for(&whole(None), None, &[]).map(|m| m.id),
            Some(SAM3.id)
        );
        assert_eq!(
            model_for(&part(None), None, &[]).map(|m| m.id),
            Some(SAM3.id)
        );
        assert!(prompted(&part(None)));
    }

    /// A choice as `Ai::part` takes it: the person not found asks, with
    /// no guess and its own words, on a portrait and on a group alike;
    /// on a picture of no one it is nobody, as before.
    #[test]
    fn a_person_not_found_asks_unless_no_one_is_there() {
        let one = |n: usize| -> Vec<Candidate> {
            (0..n)
                .map(|i| {
                    let x = 0.2 + 0.4 * i as f32;
                    Candidate::of(
                        &found_person(
                            [x, 0.1, x + 0.1, 0.25],
                            [Some(50.0), Some(20.0), Some(40.0)],
                            1.0,
                        ),
                        0,
                        1.0,
                        &None,
                    )
                })
                .collect()
        };
        for n in [1, 3] {
            match decided(Choice::Nobody, one(n)) {
                Err(Unresolved::Ask(ask)) => {
                    assert_eq!(ask.people.len(), n);
                    assert_eq!(ask.guess, None);
                    assert_eq!(ask.why, Asking::SomeoneElse);
                    assert!(
                        ask.words()
                            .starts_with("this mask was made for someone else")
                    );
                    let words = ask.words();
                    assert_eq!(part_note(&Unresolved::Ask(ask), None), words);
                }
                other => panic!("{n} people: {other:?}"),
            }
        }
        assert!(matches!(
            decided(Choice::Nobody, Vec::new()),
            Err(Unresolved::Nobody)
        ));
        match decided(Choice::Ask { guess: Some(1) }, one(3)) {
            Err(Unresolved::Ask(ask)) => {
                assert_eq!((ask.guess, ask.why), (Some(1), Asking::Unsure));
                assert_eq!(ask.words(), "which person? click one, or All people");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(decided(Choice::Person(2, 0.1), one(3)), Ok(2)));
    }

    /// A person picked on one frame of a group, looked for on another
    /// frame of it: found where the match's guess and the place she was
    /// picked at agree; not where they part, nor where her colors leave
    /// no guess; on a portrait of her, found where she stood, and never
    /// by her colors alone, since she was picked out of a group; on a
    /// stranger's, not. A frame turned or shaped otherwise has no place
    /// to compare. Picked on a portrait of her, her colors decide alone
    /// wherever she stands.
    #[test]
    fn a_person_picked_on_one_frame_is_found_on_another_by_colors_and_place() {
        let a = 0.75;
        let on = |found: &[sam3::Person]| -> Vec<Candidate> {
            found
                .iter()
                .map(|q| Candidate::of(q, 0, a, &Some("another".into())))
                .collect()
        };
        let left = [0.1, 0.2, 0.22, 0.36];
        let right = [0.6, 0.2, 0.72, 0.36];
        let hers = [Some(60.0), Some(40.0), Some(30.0)];
        let picked = Picked {
            person: Candidate::of(
                &found_person(left, hers, a),
                0,
                a,
                &Some("picked on".into()),
            )
            .person(),
            group: true,
            turn: 0,
            aspect: a,
        };
        let find = |found: &[sam3::Person]| same_person(&picked, found, &on(found), 0, (a, a));
        let her_later = |face| found_person(face, [Some(61.0), Some(41.0), Some(31.0)], a);
        let stranger = |face| found_person(face, [Some(50.0), Some(10.0), Some(90.0)], a);
        let twin = |face| found_person(face, [Some(59.0), Some(39.0), Some(29.0)], a);
        // Where she was, and the nearest by far: her.
        assert_eq!(find(&[her_later(left), stranger(right)]), Some(0));
        assert_eq!(find(&[stranger(right), her_later(left)]), Some(1));
        // The two moved places: the guess is her, the place someone
        // else's, so no one.
        assert_eq!(find(&[stranger(left), her_later(right)]), None);
        // Someone as alike as a stranger in a group can be beside her:
        // no guess, so no one, whatever stands at her place.
        assert_eq!(find(&[her_later(left), twin(right)]), None);
        // Alone on a frame: her where she stood, not elsewhere, since a
        // groupmate alone is as near in colors as she is.
        assert_eq!(find(&[her_later(left)]), Some(0));
        assert_eq!(find(&[her_later(right)]), None);
        // A stranger's portrait, at her very place: no one.
        assert_eq!(find(&[stranger(left)]), None);
        assert_eq!(find(&[]), None);
        // Turned another way, or of another shape: no place to compare.
        let turned = |found: &[sam3::Person]| same_person(&picked, found, &on(found), 2, (a, a));
        assert_eq!(turned(&[her_later(left), stranger(right)]), None);
        let wider = |found: &[sam3::Person]| same_person(&picked, found, &on(found), 0, (0.5, a));
        assert_eq!(wider(&[her_later(left), stranger(right)]), None);
        // Picked on a portrait of her: her colors decide alone, wherever
        // she stands and however the frame is turned.
        let portrait = Picked {
            group: false,
            ..picked.clone()
        };
        let alone = |found: &[sam3::Person], turn: u8| {
            same_person(&portrait, found, &on(found), turn, (a, a))
        };
        assert_eq!(alone(&[her_later(right)], 0), Some(0));
        assert_eq!(alone(&[her_later(right)], 2), Some(0));
        assert_eq!(alone(&[stranger(left)], 0), None);
        // The rule itself: a decision stands, from a group only at the
        // place; a guess only at the place.
        assert_eq!(taken(Choice::Person(2, 0.1), None, false), Some(2));
        assert_eq!(taken(Choice::Person(2, 0.1), None, true), None);
        assert_eq!(taken(Choice::Person(2, 0.1), Some(2), true), Some(2));
        assert_eq!(
            taken(Choice::Ask { guess: Some(1) }, Some(1), true),
            Some(1)
        );
        assert_eq!(taken(Choice::Ask { guess: Some(1) }, Some(0), true), None);
        assert_eq!(taken(Choice::Ask { guess: None }, Some(0), true), None);
        assert_eq!(taken(Choice::Nobody, Some(0), false), None);
    }

    /// An export of a part not settled writes it empty and says why:
    /// one asked about needs a person picked; one whose person is not
    /// found, whoever else is there, has its person not in the picture.
    #[test]
    fn an_export_says_a_part_made_for_someone_else_has_no_one_there() {
        let ask = |why| {
            Unresolved::Ask(Ask {
                why,
                ..Default::default()
            })
        };
        assert_eq!(ask(Asking::Unsure).left_out(), PART_ASKS);
        assert_eq!(ask(Asking::SomeoneElse).left_out(), PART_NOBODY);
        assert_eq!(Unresolved::Nobody.left_out(), PART_NOBODY);
        let items: Vec<String> = [ask(Asking::SomeoneElse), Unresolved::Nobody]
            .iter()
            .map(|u| {
                format!(
                    "Lips 1's Lips shape: {}; its mask written empty",
                    u.left_out()
                )
            })
            .collect();
        assert_eq!(parts_left_out(&items), (0, 2));
    }

    #[test]
    fn an_export_says_which_frames_went_with_a_parts_mask_empty() {
        let items = vec![
            format!("Lips 1's Lips shape: {PART_ASKS}; its mask written empty"),
            "the look Kodak: not in the look directory; written without it".to_string(),
            format!("Top 2's Top shape: {PART_NOBODY}; its mask written empty"),
        ];
        assert_eq!(parts_left_out(&items), (1, 1));
        assert_eq!(parts_words(false, (0, 0)), "");
        assert_eq!(
            parts_words(false, (1, 0)),
            ", a People mask needs a person picked"
        );
        assert_eq!(
            parts_words(true, (1, 2)),
            ", 1 frame: a People mask needs a person picked, \
             2 frames: a People mask's person isn't in this picture"
        );
    }

    #[test]
    fn a_sky_waits_on_the_prior_then_sam() {
        let have = |ids: &'static [&'static str]| move |m: &Model| ids.contains(&m.id);
        let sky = Shape::Sky { picks: Vec::new() };
        let cpu = [Provider::Cpu];
        let no = |_: &Model| false;
        let pick = |h: &dyn Fn(&Model) -> bool, unavailable: &[&str]| {
            model_with(&sky, h, &cpu, unavailable, no).map(|m| m.id)
        };
        assert_eq!(pick(&have(&[]), &[]), Some(SKY.id));
        assert_eq!(pick(&have(&[SAM.id]), &[]), Some(SKY.id));
        assert_eq!(pick(&have(&[SKY.id]), &[]), Some(SAM.id));
        assert_eq!(pick(&have(&[SKY.id]), &[SAM.id]), Some(SKY.id));
        assert_eq!(pick(&have(&[SKY.id, SAM.id]), &[]), Some(SKY.id));
        assert!(prompted(&sky));
    }

    /// A Landscape shape wants the sky's prior alone, whatever SAM's
    /// state; a group this build does not know wants nothing and is
    /// never asked for.
    #[test]
    fn a_landscape_shape_waits_on_the_prior_alone() {
        let cpu = [Provider::Cpu];
        let no = |_: &Model| false;
        for kind in ["Water", "Foliage", "Mountain", "Building"] {
            let shape = Shape::of_kind(kind);
            for have in [&[][..], &[SKY.id][..], &[SKY.id, SAM.id][..]] {
                let model = model_with(&shape, |m: &Model| have.contains(&m.id), &cpu, &[], no);
                assert_eq!(model.map(|m| m.id), Some(SKY.id), "{kind} with {have:?}");
            }
            assert!(prompted(&shape));
        }
        let later = Shape::Landscape {
            group: greycard_edit::mask::LandscapeGroup::Other("grass".into()),
        };
        assert_eq!(model_with(&later, |_: &Model| true, &cpu, &[], no), None);
        assert!(!prompted(&later));
    }

    /// Logits for a picture of a sky over a sea, a query each, as the
    /// model gives them: the sky 99.9 percent sure over the top 40
    /// rows of the 160, the sea 95 percent over the rest.
    fn sky_over_sea() -> sky::Logits {
        let (q, c, side) = (sky::QUERIES, sky::CLASSES, sky::MASK_SIDE);
        let mut class = vec![0.0f32; q * c];
        for i in 0..q {
            class[i * c + c - 1] = 20.0;
        }
        let mut masks = vec![-10.0f32; q * side * side];
        for (i, (label, p, rows)) in [
            (sky::SKY_CLASS as usize, 0.999f32, 0..40),
            (103, 0.95, 40..160),
        ]
        .into_iter()
        .enumerate()
        {
            class[i * c..(i + 1) * c].fill(0.0);
            class[i * c + label] = (p * 133.0 / (1.0 - p)).ln();
            for y in rows {
                masks[i * side * side + y * side..i * side * side + (y + 1) * side].fill(10.0);
            }
        }
        sky::Logits { class, masks }
    }

    /// The Landscape button and the shapes it makes read the scene kept
    /// for the develop: with one there, the button's offer, a Water
    /// shape and a Building the gate refuses all come without a model
    /// (none is in this store to load), and the refused one is empty
    /// and says so, from the memory cache too.
    #[test]
    fn the_button_and_its_shapes_read_the_kept_scene() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-scratch/greycard-ai-landscape-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        crate::testing::remove_dir_retry(&dir);
        let mut ai = Ai::for_test(Store::at(dir.join("models")), vec![Provider::Cpu]);
        ai.file = Some(PathBuf::from("coast.CR3"));
        ai.print = Print::test(1);
        let (w, h) = (64usize, 64usize);
        let mut image = WorkingImage::new(w, h);
        for (i, p) in image
            .data
            .as_chunks_mut::<{ WorkingImage::CHANNELS }>()
            .0
            .iter_mut()
            .enumerate()
        {
            let sea = i / w >= h / 4;
            p.copy_from_slice(if sea {
                &[0.02, 0.06, 0.12]
            } else {
                &[0.3, 0.45, 0.8]
            });
        }
        let edit = Edit::default();
        let kind = greycard_render::finish::Source::SCENE;
        ai.preview_of(1, &image, &edit, kind);
        let (pw, ph) = {
            let p = &ai.preview.as_ref().unwrap().1;
            (p.width, p.height)
        };
        let scene = Scene::label(&sky_over_sea(), (640, 640), pw, ph);
        ai.scene = Some((1, scene, Provider::Cpu));
        let (offers, provider, _) = ai.landscape(1, &image, &edit, kind).expect("the offer");
        assert_eq!(provider, Provider::Cpu);
        let kinds: Vec<_> = offers.iter().map(|o| o.kind).collect();
        assert_eq!(
            kinds,
            [
                landscape::Kind::Group(landscape::Group::Water),
                landscape::Kind::Sky
            ]
        );
        let water = Shape::of_kind("Water");
        let made = ai
            .raster(
                Base::stamped(1),
                &image,
                &edit,
                kind,
                0,
                (1, 0),
                &water,
                None,
            )
            .expect("the water");
        assert!(made.note.is_none());
        let data = made.raster.data();
        let height = data.len() / RASTER_WIDTH;
        let at = |y: usize| data[y * RASTER_WIDTH + RASTER_WIDTH / 2];
        assert!(at(height * 7 / 8) > 240 && at(height / 16) < 15);
        let building = Shape::of_kind("Building");
        for _ in 0..2 {
            let made = ai
                .raster(
                    Base::stamped(1),
                    &image,
                    &edit,
                    kind,
                    0,
                    (1, 1),
                    &building,
                    None,
                )
                .expect("an empty building");
            assert!(made.raster.data().iter().all(|&v| v == 0));
            assert_eq!(made.note.as_deref(), Some("no building found in coast.CR3"));
        }
        assert_eq!(ai.scene_runs, 0, "nothing ran the model");
        // A Sky with no picks after a Foliage's cut is that cut, not
        // a sky made again; one with picks is made.
        let height = RASTER_WIDTH; // the picture is square
        let marked = greycard_ai::Mask::new(RASTER_WIDTH, height, vec![0.5; RASTER_WIDTH * height]);
        ai.sky_cut = Some(((1, (RASTER_WIDTH, height)), Some(marked), None));
        let sky = Shape::Sky { picks: Vec::new() };
        let made = ai
            .raster(Base::stamped(1), &image, &edit, kind, 0, (2, 0), &sky, None)
            .expect("the kept sky");
        assert!(
            made.raster.data().iter().all(|&v| v == 128),
            "the cut, read back"
        );
        let picked = Shape::Sky {
            picks: vec![greycard_edit::mask::Pick {
                pos: [0.5, 0.1],
                positive: false,
            }],
        };
        let made = ai
            .raster(
                Base::stamped(1),
                &image,
                &edit,
                kind,
                0,
                (2, 1),
                &picked,
                None,
            )
            .expect("a sky with a pick");
        assert!(
            made.raster.data().iter().any(|&v| v != 128),
            "made, not the cut"
        );
        crate::testing::remove_dir_retry(&dir);
    }

    /// Each group has its own slot in the disk cache, and none is a
    /// sky's.
    #[test]
    fn a_landscape_raster_is_cached_by_its_group() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-scratch/greycard-ai-landscape-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut ai = Ai::new();
        ai.store = Some(Store::at(dir.join("models")));
        ai.file = Some(PathBuf::from("coast.CR3"));
        ai.print = Print::test(1);
        let mut slots: Vec<PathBuf> = ["Water", "Foliage", "Mountain", "Building", "Sky"]
            .into_iter()
            .map(|k| ai.cached_path(&Shape::of_kind(k), None, 0).expect("a slot"))
            .collect();
        slots.sort();
        slots.dedup();
        assert_eq!(slots.len(), 5);
    }

    /// A fill whose model is to run is asked first whether to run now:
    /// told not, it is left for later with nothing made or kept, and
    /// asked again it is made and kept; a kept one is never asked.
    #[test]
    fn a_fill_told_not_now_is_left_for_later() {
        let mut ai = Ai::for_test(Store::at("no-store"), vec![Provider::Cpu]);
        ai.fill_by_test = Some(0.25);
        let image = WorkingImage::new(200, 100);
        let patch = Patch {
            id: 3,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[0.5, 0.5]],
            radius: 0.05,
            ..Default::default()
        };
        let region = patch.region(image.width, image.height).unwrap();
        let mut asked = 0;
        let later = ai.fill_or_run(
            &patch,
            &region,
            &image,
            |_| {
                asked += 1;
                false
            },
            &mut |_, _| {},
        );
        assert!(matches!(later, Err(NoFill::Later)));
        assert!(!ai.fill_kept(&patch, &region, &image));
        let retouch = Retouch {
            enabled: true,
            patches: vec![patch.clone()],
        };
        assert_eq!(ai.fills_unmade_ids(&retouch, &image), [3]);
        let (made, engine) = ai
            .fill_or_run(
                &patch,
                &region,
                &image,
                |_| {
                    asked += 1;
                    true
                },
                &mut |_, _| {},
            )
            .expect("made");
        assert_eq!(engine, Engine::Lama);
        assert!(made.iter().all(|v| *v == 0.25));
        assert!(ai.fill_kept(&patch, &region, &image));
        assert!(ai.fills_unmade_ids(&retouch, &image).is_empty());
        ai.fill_or_run(&patch, &region, &image, |_| panic!("kept"), &mut |_, _| {})
            .expect("kept");
        assert_eq!(asked, 2);
        // The queue's hold is a flag until the remove is loaded, and
        // let go with nothing to let go of.
        ai.fills_queued(true);
        assert!(ai.fills_queued);
        ai.release_fills();
        assert!(!ai.fills_queued);
    }

    /// A removal is begun only with the card's peak and the margin
    /// free, less the UNet's weights when it is held already.
    #[test]
    fn a_removal_needs_its_peak_free() {
        const GIB: u64 = 1 << 30;
        assert!(short_of_memory("the card", 6 * GIB, 512, false).is_some());
        assert!(short_of_memory("the card", 7 * GIB, 512, false).is_none());
        assert!(short_of_memory("the card", 7 * GIB, 1024, false).is_some());
        assert!(short_of_memory("the card", 8 * GIB, 1024, false).is_none());
        assert!(short_of_memory("the card", 2 * GIB, 512, true).is_some());
        assert!(short_of_memory("the card", 3 * GIB, 512, true).is_none());
        assert!(short_of_memory("the card", 5 * GIB, 1024, true).is_some());
        assert!(short_of_memory("the card", 6 * GIB, 1024, true).is_none());
        let why = short_of_memory("the card", 3 * GIB, 1024, false).unwrap();
        assert_eq!(
            why,
            "the card has 3.0 GiB free, short of the 7.1 GiB a removal at 1024 needs; another program may be using it"
        );
    }

    /// The learned denoiser's need follows its tile, measured at 256,
    /// 384, 512, and 1024 and past 1024 by the tile's area; a learned
    /// mask's is one figure.
    #[test]
    fn the_denoiser_and_the_masks_need_their_peaks_free() {
        const GIB: u64 = 1 << 30;
        assert!(denoise_needs(256) < denoise_needs(384));
        assert!(denoise_needs(384) < denoise_needs(512));
        assert_eq!(denoise_needs(500), denoise_needs(512));
        // The 4 GB card's 1.8 GiB: short at 512, room at 384.
        assert!(short_of("c", 18 * GIB / 10, denoise_needs(512), "d").is_some());
        assert!(short_of("c", 18 * GIB / 10, denoise_needs(384), "d").is_none());
        assert!(short_of("c", 2 * GIB, denoise_needs(512), "d").is_none());
        assert!(short_of("c", 4 * GIB, denoise_needs(1024), "d").is_some());
        assert!(short_of("c", 5 * GIB, denoise_needs(1024), "d").is_none());
        assert!(denoise_needs(2048) > 4 * denoise_needs(1024) - 4 * CARD_MARGIN);
        assert!(short_of("c", 4 * GIB, mask_needs(MaskModel::People), "m").is_some());
        assert!(short_of("c", 2 * GIB, mask_needs(MaskModel::Sky), "m").is_none());
        assert!(short_of("c", 5 * GIB, mask_needs(MaskModel::People), "m").is_none());
        assert!(short_of("c", 6 * GIB, mask_needs(MaskModel::Subject), "m").is_some());
        let why = short_of("the card", GIB, denoise_needs(1024), "the learned denoiser").unwrap();
        assert!(
            why.starts_with("the card has 1.0 GiB free, short of the 4.3 GiB"),
            "{why}"
        );
    }

    /// A test's Ai on a card with `free` free, its masks' release
    /// letting go of `freed` and counting its calls, and its CPU runs
    /// counted.
    fn mask_on_card(
        free: u64,
        freed: Vec<(&'static str, u64)>,
    ) -> (
        Ai,
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut ai = Ai::for_test(Store::at("no-store"), vec![Provider::WebGpu, Provider::Cpu]);
        ai.free_memory_by_test = Some(free);
        ai.set_cpu_fallback(true);
        let (released, told) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let r = released.clone();
        let mut left = Some(freed);
        ai.set_card_release(Some(Box::new(move || {
            r.fetch_add(1, Ordering::Relaxed);
            left.take().unwrap_or_default()
        })));
        let t = told.clone();
        ai.set_on_cpu(Some(Box::new(move || {
            t.fetch_add(1, Ordering::Relaxed);
        })));
        (ai, released, told)
    }

    /// A learned mask on a card short of it after the give-back, with
    /// nothing of the engine's to let go, runs on the CPU: never
    /// refused, the worker told before the run and after it, and the
    /// models loaded on the CPU alone. Asked again in the next job with
    /// the memory back, or not known, it is the card's; on a machine
    /// without WebGPU the card is not asked.
    #[test]
    fn a_mask_the_card_is_short_of_runs_on_the_cpu() {
        use std::sync::atomic::Ordering;
        let (mut ai, released, told) = mask_on_card(1 << 30, Vec::new());
        ai.mask_room(MaskModel::People).expect("not refused");
        assert!(ai.masks_on_cpu());
        assert_eq!(ai.mask_providers(), [Provider::Cpu]);
        assert_eq!(released.load(Ordering::Relaxed), 1, "the engine asked");
        assert_eq!(told.load(Ordering::Relaxed), 1, "said before the run");
        assert!(ai.take_short().is_none(), "not a refusal");
        assert!(ai.take_mask_short(), "the worker gives the card back after");
        // The same job asks no more.
        ai.mask_room(MaskModel::People).expect("not refused");
        assert_eq!(told.load(Ordering::Relaxed), 1);
        assert_eq!(released.load(Ordering::Relaxed), 1);
        ai.clear_shorts();
        ai.free_memory_by_test = Some(8 << 30);
        ai.mask_room(MaskModel::People).expect("not refused");
        assert!(!ai.masks_on_cpu());
        assert_eq!(ai.mask_providers(), [Provider::WebGpu, Provider::Cpu]);
        ai.free_memory_by_test = None;
        ai.fill_by_test = Some(0.5);
        ai.mask_room(MaskModel::People).expect("not refused");
        assert!(!ai.masks_on_cpu());
        let mut cpu = Ai::for_test(Store::at("no-store"), vec![Provider::Cpu]);
        cpu.free_memory_by_test = Some(1 << 30);
        cpu.mask_room(MaskModel::People).expect("not refused");
        assert!(!cpu.masks_on_cpu() && !cpu.take_mask_short());
    }

    /// A card short of a learned mask until the engine lets go of its
    /// working sets runs the mask on it, and the log names what went
    /// and the free memory before and after; one short still after it
    /// runs the mask on the CPU and says so. A card with the room from
    /// the start lets nothing go.
    #[test]
    fn the_engine_lets_go_for_a_mask_before_the_cpu_is_asked() {
        use std::sync::atomic::Ordering;
        const GIB: u64 = 1 << 30;
        let need = mask_needs(MaskModel::People);
        let sets = vec![
            ("the sharpen's working set", GIB / 2),
            ("the local contrast's working set", GIB),
        ];
        // Short by a half, a gigabyte and a half let go: the card's.
        let (mut ai, released, told) = mask_on_card(need - GIB / 2, sets.clone());
        ai.mask_room(MaskModel::People).expect("not refused");
        assert!(!ai.masks_on_cpu());
        assert_eq!(released.load(Ordering::Relaxed), 1);
        assert_eq!(told.load(Ordering::Relaxed), 0);
        assert_eq!(ai.free_memory_by_test, Some(need + GIB));
        assert!(!ai.take_mask_short() && ai.take_short().is_none());
        // Short by two, a gigabyte and a half let go: the CPU's.
        let (mut ai, released, told) = mask_on_card(need - 2 * GIB, sets.clone());
        ai.mask_room(MaskModel::People).expect("not refused");
        assert!(ai.masks_on_cpu());
        assert_eq!(released.load(Ordering::Relaxed), 1);
        assert_eq!(told.load(Ordering::Relaxed), 1);
        assert!(ai.take_mask_short());
        // The room from the start: nothing let go.
        let (mut ai, released, told) = mask_on_card(need, sets.clone());
        ai.mask_room(MaskModel::People).expect("not refused");
        assert!(!ai.masks_on_cpu());
        assert_eq!(released.load(Ordering::Relaxed), 0);
        assert_eq!(told.load(Ordering::Relaxed), 0);
        // What the log says of each.
        let why = short_of("the card", need - GIB / 2, need, "a learned mask").unwrap();
        assert_eq!(
            second_ask_said(&why, &sets, Some(GIB), Some(5 * GIB / 2), true),
            "a learned mask: begun on the card once the engine let go of the sharpen's \
             working set (0.5 GiB) and the local contrast's working set (1.0 GiB), \
             1.0 GiB free before and 2.5 after"
        );
        let said = second_ask_said(&why, &sets, Some(GIB / 2), Some(2 * GIB), false);
        assert!(said.starts_with(&why), "{said}");
        assert!(
            said.ends_with(
                "the engine let go of the sharpen's working set (0.5 GiB) and the local \
                 contrast's working set (1.0 GiB), 0.5 GiB free before and 2.0 after, \
                 short still: the mask runs on the CPU"
            ),
            "{said}"
        );
        assert!(
            second_ask_said(&why, &[], Some(GIB), Some(GIB), false)
                .ends_with("nothing of the engine's to let go: the mask runs on the CPU")
        );
        assert_eq!(
            freed_words(&[("a", 0), ("b", GIB), ("c", 3 * GIB)]),
            "a (0.0 GiB), b (1.0 GiB), and c (3.0 GiB)"
        );
    }

    /// What records which sessions were loaded on the CPU for the
    /// card's memory goes wherever those sessions go, so a session
    /// loaded on the card after is never taken for one of them.
    #[test]
    fn a_dropped_cpu_session_clears_the_cpu_choice() {
        let all = CpuChosen {
            sam: true,
            sam3: true,
            subject: true,
            sky: true,
        };
        let mut ai = Ai::for_test(Store::at("no-store"), vec![Provider::WebGpu, Provider::Cpu]);
        ai.cpu_chosen = all;
        ai.drop_sessions();
        assert_eq!(ai.cpu_chosen, CpuChosen::default());
        ai.cpu_chosen = all;
        ai.free_memory_by_test = Some(8 << 30);
        assert!(!give_back_card(&mut [&mut ai], true), "nothing held");
        ai.drop_sessions();
        assert_eq!(ai.cpu_chosen, CpuChosen::default(), "given back");
        ai.cpu_chosen = all;
        ai.release_parts();
        assert!(!ai.cpu_chosen.sam3 && ai.cpu_chosen.sam);
        ai.forget_subject();
        assert!(!ai.cpu_chosen.subject);
        // With the room again the choice is spent, nothing held to drop.
        ai.mask_room(MaskModel::People).expect("the room");
        assert_eq!(ai.cpu_chosen, CpuChosen::default());
    }

    /// A learned mask that may not fall to the CPU (a frame's masks made
    /// in the background, a person looked for on another frame, a save)
    /// is refused on a short card as before, and nothing is let go.
    #[test]
    fn a_mask_that_may_not_fall_to_the_cpu_is_refused() {
        use std::sync::atomic::Ordering;
        let (mut ai, released, told) = mask_on_card(1 << 30, vec![("a", 1 << 30)]);
        ai.set_cpu_fallback(false);
        let why = ai.mask_room(MaskModel::People).expect_err("refused");
        assert!(why.contains("a learned mask"), "{why}");
        assert!(!ai.masks_on_cpu());
        assert_eq!(released.load(Ordering::Relaxed), 0);
        assert_eq!(told.load(Ordering::Relaxed), 0);
        assert!(ai.take_mask_short() && ai.take_short().is_some());
        // Turned off, a CPU choice made under it goes.
        ai.set_cpu_fallback(true);
        ai.mask_room(MaskModel::People).expect("on the CPU");
        assert!(ai.masks_on_cpu());
        ai.set_cpu_fallback(false);
        assert!(!ai.masks_on_cpu());
        assert_eq!(ai.mask_providers(), [Provider::WebGpu, Provider::Cpu]);
    }

    /// An export's frame short of a mask: refused on the first pass
    /// after its tries, the short met; tried again once the card is
    /// given back, it runs on the CPU, the short still met so the set's
    /// later frames ask once, and those go to the CPU after one try. A
    /// canceled export runs no mask on the CPU.
    #[test]
    fn an_export_falls_to_the_cpu_only_when_tried_again() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let asked = Arc::new(AtomicUsize::new(0));
        let patience = |tries: u32, canceled: bool| {
            let asked = asked.clone();
            Patience {
                tries,
                wait: std::time::Duration::ZERO,
                canceled: Arc::new(move || {
                    asked.fetch_add(1, Ordering::Relaxed);
                    canceled
                }),
            }
        };
        let (mut ai, _, told) = mask_on_card(1 << 30, Vec::new());
        // The first pass: three asks, refused.
        ai.set_patience(Some(patience(PATIENT_TRIES, false)));
        ai.set_cpu_fallback(false);
        assert!(ai.mask_room(MaskModel::People).is_err());
        assert!(ai.hit_short(), "the short is met");
        assert_eq!(asked.swap(0, Ordering::Relaxed), 2, "waited its tries");
        // Given back and tried again: the CPU.
        ai.retry_patiently();
        ai.set_cpu_fallback(true);
        ai.mask_room(MaskModel::People).expect("on the CPU");
        assert!(ai.masks_on_cpu());
        assert!(ai.hit_short(), "a patient walk keeps it met");
        assert!(ai.take_short().is_none(), "the frame does not fail on it");
        assert!(ai.take_mask_short(), "the worker gives back after");
        assert_eq!(told.load(Ordering::Relaxed), 1);
        // The set's next frame, at one try: the CPU without waiting.
        asked.store(0, Ordering::Relaxed);
        ai.set_patience(Some(patience(1, false)));
        ai.mask_room(MaskModel::People).expect("on the CPU");
        assert!(ai.masks_on_cpu());
        assert_eq!(
            asked.load(Ordering::Relaxed),
            1,
            "no wait, the cancel looked at"
        );
        // A canceled export runs nothing on the CPU.
        let (mut ai, released, told) = mask_on_card(1 << 30, vec![("a", 1 << 30)]);
        ai.set_patience(Some(patience(PATIENT_TRIES, true)));
        assert!(ai.mask_room(MaskModel::People).is_err());
        assert!(!ai.masks_on_cpu());
        assert_eq!(released.load(Ordering::Relaxed), 0);
        assert_eq!(told.load(Ordering::Relaxed), 0);
    }

    /// A remove's fill the card has not the memory for is refused
    /// plainly at its run, before anything loads. It is not remembered
    /// as failed: with the memory back it is counted and made. Where
    /// the free memory is not known it is made as before.
    #[test]
    fn a_fill_the_card_cannot_hold_is_refused_and_asked_again() {
        let mut ai = Ai::for_test(Store::at("no-store"), vec![Provider::WebGpu, Provider::Cpu]);
        ai.fill_by_test = Some(0.25);
        let image = WorkingImage::new(200, 100);
        let patch = Patch {
            id: 4,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[0.5, 0.5]],
            radius: 0.05,
            remove: true,
            ..Default::default()
        };
        let region = patch.region(image.width, image.height).unwrap();
        let retouch = Retouch {
            enabled: true,
            patches: vec![patch.clone()],
        };
        ai.free_memory_by_test = Some(1 << 30);
        // Counted ahead whatever the card has: its run asks.
        assert_eq!(ai.fills_unmade_ids(&retouch, &image), [4]);
        assert!(ai.could_fill_more(&retouch, &image));
        match ai.fill_or_run(&patch, &region, &image, |_| true, &mut |_, _| {}) {
            Err(NoFill::Failed(why)) => {
                assert!(why.starts_with("the card has 1.0 GiB free"), "{why}");
                assert!(why.contains("a removal at 512"), "{why}");
            }
            other => panic!("{:?}", other.map(|(_, e)| e)),
        }
        assert!(ai.failed_before(&patch, &region, &image).is_none());
        assert!(!ai.fill_kept(&patch, &region, &image));
        // Another process lets the memory go.
        ai.free_memory_by_test = Some(16 << 30);
        assert_eq!(ai.fills_unmade_ids(&retouch, &image), [4]);
        let (made, _) = ai.fill(&patch, &region, &image).expect("made");
        assert!(made.iter().all(|v| *v == 0.25));
        // Not known: made as before.
        let mut other = patch.clone();
        other.id = 5;
        ai.free_memory_by_test = None;
        let region = other.region(image.width, image.height).unwrap();
        ai.fill(&other, &region, &image).expect("made");
        // With the UNet held already, only the rest of the peak is
        // asked: 3 GiB is short of a cold removal at 512, not of a held one.
        let mut held = patch.clone();
        held.id = 6;
        let region = held.region(image.width, image.height).unwrap();
        ai.free_memory_by_test = Some(3 << 30);
        ai.unet_held_by_test = Some(false);
        assert!(matches!(
            ai.fill(&held, &region, &image),
            Err(NoFill::Failed(_))
        ));
        assert!(ai.take_short().is_some());
        // Refused, the patch is not asked again until something changes.
        assert!(ai.fills_unmade_ids(&retouch_of(&held), &image).is_empty());
        held.id = 8;
        ai.unet_held_by_test = Some(true);
        ai.fill(&held, &region, &image).expect("made");
        assert!(ai.take_short().is_none());
        // A patch not marked for the remove, LaMa's, is not held to it.
        let mut lama = patch.clone();
        lama.id = 7;
        lama.remove = false;
        ai.free_memory_by_test = Some(1 << 30);
        let region = lama.region(image.width, image.height).unwrap();
        ai.fill(&lama, &region, &image).expect("made");
        // Nor is a remove's patch where WebGPU is not offered.
        let mut cpu = Ai::for_test(Store::at("no-store"), vec![Provider::Cpu]);
        cpu.fill_by_test = Some(0.25);
        cpu.free_memory_by_test = Some(1 << 30);
        let region = patch.region(image.width, image.height).unwrap();
        cpu.fill(&patch, &region, &image).expect("made");
    }

    const GIB: u64 = 1 << 30;

    /// A test's Ai whose sessions hold `held` of a card with `free`
    /// free, its fills made with no model.
    fn on_card(free: u64, held: u64) -> Ai {
        let mut ai = Ai::for_test(Store::at("no-store"), vec![Provider::WebGpu, Provider::Cpu]);
        ai.fill_by_test = Some(0.25);
        ai.free_memory_by_test = Some(free);
        ai.freed_by_test = held;
        ai
    }

    fn retouch_of(patch: &Patch) -> Retouch {
        Retouch {
            enabled: true,
            patches: vec![patch.clone()],
        }
    }

    fn remove_patch(id: u64) -> Patch {
        Patch {
            id,
            method: greycard_edit::retouch::Method::Fill,
            points: vec![[0.5, 0.5]],
            radius: 0.05,
            remove: true,
            ..Default::default()
        }
    }

    /// The card is given back once a fill's model has run in any of the
    /// worker's models while the card is short of the floor, and not
    /// again until another runs; a fill kept runs nothing and gives
    /// nothing back; nor is it where the card's free memory is not
    /// known, as on unified memory.
    #[test]
    fn the_card_is_given_back_after_a_fill_ran() {
        let mut ai = on_card(3 * GIB, 6 * GIB);
        let mut other = Ai::for_test(Store::at("no-store"), vec![Provider::Cpu]);
        assert!(ai.holds_sessions());
        assert!(!give_back_card(&mut [&mut ai, &mut other], false));
        let image = WorkingImage::new(200, 100);
        // LaMa's: a removal would give the card back before it began.
        let patch = Patch {
            remove: false,
            ..remove_patch(9)
        };
        let region = patch.region(image.width, image.height).unwrap();
        ai.fill(&patch, &region, &image).expect("made");
        assert!(give_back_due(&[&ai, &other]));
        assert!(give_back_card(&mut [&mut ai, &mut other], false));
        assert!(!ai.holds_sessions() && !other.holds_sessions());
        assert_eq!(ai.free_memory().map(|(f, _)| f), Some(9 * GIB));
        assert!(!give_back_card(&mut [&mut ai, &mut other], false));
        // Kept, it is served without a run.
        ai.fill(&patch, &region, &image).expect("kept");
        assert!(!give_back_due(&[&ai]));
        assert!(!give_back_card(&mut [&mut ai], false));
        // Forced with nothing held, nothing goes.
        assert!(!give_back_card(&mut [&mut ai], true));
        // Unknown free memory: the models stay.
        let mut unknown = Ai::for_test(Store::at("no-store"), vec![Provider::Cpu]);
        unknown.fill_by_test = Some(0.25);
        unknown.freed_by_test = GIB;
        let other = remove_patch(10);
        let region = other.region(image.width, image.height).unwrap();
        unknown.fill(&other, &region, &image).expect("made");
        assert!(!give_back_due(&[&unknown]));
        assert!(!give_back_card(&mut [&mut unknown], true));
        assert!(unknown.holds_sessions());
    }

    /// A removal short of the card after its own give-back has the
    /// engine let go of what a develop makes again and asks once more:
    /// with the room then, it begins and nothing is refused; short
    /// still, it is refused with the card as it is then, what went the
    /// log's. With no release, or nothing in it, the refusal is as
    /// before.
    #[test]
    fn the_engine_lets_go_for_a_removal_before_it_is_refused() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let image = WorkingImage::new(200, 100);
        let patch = remove_patch(21);
        let region = patch.region(image.width, image.height).unwrap();
        let free = 59 * GIB / 10;
        let with_release = |bytes: u64| {
            let mut ai = on_card(free, 0);
            let calls = Arc::new(AtomicUsize::new(0));
            let c = calls.clone();
            ai.set_card_release(Some(Box::new(move || {
                c.fetch_add(1, Ordering::Relaxed);
                vec![("the sharpen's working set", bytes)]
            })));
            (ai, calls)
        };
        // 5.9 GiB free and 0.6 let go: a cold 512 (6.3) begins.
        let (mut ai, calls) = with_release(6 * GIB / 10);
        ai.fill(&patch, &region, &image)
            .expect("made once the engine let go");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(ai.free_memory_by_test, Some(free + 6 * GIB / 10));
        assert!(!ai.take_fill_short() && ai.take_short().is_none());
        assert!(ai.fill_kept(&patch, &region, &image));
        // 0.2 let go: short still, refused, and said what went.
        let (mut ai, calls) = with_release(2 * GIB / 10);
        let why = match ai.fill(&patch, &region, &image) {
            Err(NoFill::Failed(why)) => why,
            other => panic!("{:?}", other.map(|(_, e)| e)),
        };
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            why,
            "the card has 6.1 GiB free, short of the 6.3 GiB a removal at 512 needs; another program may be using it"
        );
        assert!(ai.take_fill_short());
        assert_eq!(ai.take_short().as_deref(), Some(why.as_str()));
        // Refused, not asked again until something changes.
        assert!(ai.fills_unmade_ids(&retouch_of(&patch), &image).is_empty());
        // No release: refused as before.
        let mut ai = on_card(free, 0);
        let plain = match ai.fill(&patch, &region, &image) {
            Err(NoFill::Failed(why)) => why,
            other => panic!("{:?}", other.map(|(_, e)| e)),
        };
        assert_eq!(
            plain,
            "the card has 5.9 GiB free, short of the 6.3 GiB a removal at 512 needs; another program may be using it"
        );
        assert!(ai.take_fill_short());
        // A release with nothing in it: the same.
        let (mut ai, calls) = with_release(0);
        match ai.fill(&patch, &region, &image) {
            Err(NoFill::Failed(why)) => assert_eq!(why, plain),
            other => panic!("{:?}", other.map(|(_, e)| e)),
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        // A canceled export lets nothing go.
        let (mut ai, calls) = with_release(6 * GIB / 10);
        ai.set_patience(Some(Patience {
            tries: 1,
            wait: std::time::Duration::ZERO,
            canceled: Arc::new(|| true),
        }));
        assert!(matches!(
            ai.fill(&patch, &region, &image),
            Err(NoFill::Failed(_))
        ));
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    /// Short of a removal because of its own sessions, the Ai gives the
    /// card back and asks once more, and the fill is made; short after
    /// that, it is refused and the worker told.
    #[test]
    fn a_short_gives_back_the_cards_own_first() {
        let image = WorkingImage::new(200, 100);
        let patch = remove_patch(11);
        let region = patch.region(image.width, image.height).unwrap();
        // 6 GiB free, 4 held by its own sessions: a cold 512 wants 6.3.
        let mut ai = on_card(6 * GIB, 4 * GIB);
        ai.fill(&patch, &region, &image)
            .expect("made once given back");
        assert!(!ai.holds_sessions());
        assert!(!ai.take_fill_short());
        // Short with nothing of its own to give.
        let mut short = on_card(2 * GIB, 0);
        assert!(matches!(
            short.fill(&patch, &region, &image),
            Err(NoFill::Failed(_))
        ));
        assert!(short.take_fill_short());
        assert!(short.take_short().is_some());
    }

    /// A patient walk asks the card its tries, the export's cancel
    /// looked at between, and once it has given up does not wait again;
    /// a window's job does not wait at all.
    #[test]
    fn a_patient_walk_waits_then_stops_waiting() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let image = WorkingImage::new(200, 100);
        let looked = Arc::new(AtomicUsize::new(0));
        let mut ai = on_card(GIB, 0);
        let counted = looked.clone();
        ai.set_patience(Some(Patience {
            tries: 3,
            wait: std::time::Duration::ZERO,
            canceled: Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                false
            }),
        }));
        let first = remove_patch(12);
        let region = first.region(image.width, image.height).unwrap();
        assert!(ai.fill(&first, &region, &image).is_err());
        assert_eq!(looked.load(Ordering::SeqCst), 2);
        assert!(ai.hit_short());
        let second = remove_patch(13);
        assert!(ai.fill(&second, &region, &image).is_err());
        assert_eq!(looked.load(Ordering::SeqCst), 2, "no wait after giving up");
        // Counting the fills ahead never waits, patient or not.
        ai.set_patience(Some(Patience {
            tries: 3,
            wait: std::time::Duration::ZERO,
            canceled: Arc::new(|| panic!("counting waited")),
        }));
        let retouch = Retouch {
            enabled: true,
            patches: vec![first.clone()],
        };
        assert_eq!(ai.fills_unmade_ids(&retouch, &image), [12]);
        // Canceled, it stops asking.
        ai.set_patience(Some(Patience {
            tries: 3,
            wait: std::time::Duration::ZERO,
            canceled: Arc::new(|| true),
        }));
        assert!(ai.fill(&first, &region, &image).is_err());
        // No patience: one ask.
        ai.set_patience(None);
        assert!(ai.fill(&first, &region, &image).is_err());
    }

    /// The learned denoiser's last word before its run asks the card
    /// once; where the card will not say, it runs; and the walk tells
    /// the refusal from any other failure.
    #[test]
    fn the_denoiser_is_refused_on_a_short_card() {
        set_free_by_test(Some(vec![Some(GIB)]));
        let short = denoise_room(1024).expect_err("short");
        assert!(
            short.0.contains("the learned denoiser at a tile of 1024"),
            "{short}"
        );
        set_free_by_test(Some(vec![Some(3 * GIB)]));
        assert!(denoise_room(512).is_ok());
        set_free_by_test(Some(vec![None]));
        assert!(denoise_room(1024).is_ok());
        set_free_by_test(None);
        let e = anyhow::Error::new(CardShort("short".into()));
        assert!(matches!(
            crate::worker::learned_failure(&e),
            crate::walk::NoLearned::Short(_)
        ));
        let e = anyhow::anyhow!("garbage");
        assert!(matches!(
            crate::worker::learned_failure(&e),
            crate::walk::NoLearned::Failed(_)
        ));
    }

    /// A test's Ai on a card with `free` free whose release lets go of
    /// `bytes`, counting its calls.
    fn denoiser_on_card(free: u64, bytes: u64) -> (Ai, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut ai = Ai::for_test(Store::at("no-store"), vec![Provider::WebGpu, Provider::Cpu]);
        ai.free_memory_by_test = Some(free);
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut left = Some(vec![("the sharpen's working set", bytes)]);
        ai.set_card_release(Some(Box::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
            left.take().unwrap_or_default()
        })));
        (ai, calls)
    }

    /// The learned denoiser short of the card at its tile of 512, as on
    /// issue 130's 4 GB card (1.8 GiB free, 1.95 needed), takes its
    /// rungs in order: the engine lets go of what a develop makes again
    /// and it runs at its tile; with nothing let go, or not enough, the
    /// largest smaller tile the card has the room for; short at every
    /// tile, it is refused in the words it always had, and the walk's
    /// engine denoise stands in. A handled short leaves no refusal.
    #[test]
    fn the_denoisers_ladder_ends_in_the_engines_denoise() {
        use std::sync::atomic::Ordering;
        let free = 18 * GIB / 10;
        // 0.5 let go: 2.3 free, room at 512.
        let (mut ai, calls) = denoiser_on_card(free, GIB / 2);
        assert_eq!(ai.denoise_room_at(512).unwrap(), DenoiseOn::Card);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(ai.take_short().is_none());
        assert!(!ai.short_failed());
        // Nothing let go: 384 needs 1.05.
        let (mut ai, _) = denoiser_on_card(free, 0);
        assert_eq!(ai.denoise_room_at(512).unwrap(), DenoiseOn::Tile(384));
        assert!(ai.take_short().is_none());
        assert!(!ai.short_failed());
        // Not enough let go for 512: the smaller tile still.
        let (mut ai, _) = denoiser_on_card(GIB, GIB / 10);
        assert_eq!(ai.denoise_room_at(512).unwrap(), DenoiseOn::Tile(384));
        // Room at 256 alone.
        let (mut ai, _) = denoiser_on_card(GIB, 0);
        assert_eq!(ai.denoise_room_at(512).unwrap(), DenoiseOn::Tile(256));
        // From 1024, 512 is the first smaller tile asked.
        let (mut ai, _) = denoiser_on_card(2 * GIB, 0);
        assert_eq!(ai.denoise_room_at(1024).unwrap(), DenoiseOn::Tile(512));
        // Short at every tile: refused in the words the window has
        // always shown, a failure for the set.
        let (mut ai, _) = denoiser_on_card(GIB / 2, GIB / 10);
        let short = ai.denoise_room_at(512).expect_err("refused");
        assert_eq!(
            short.0,
            "the card has 0.6 GiB free, short of the 1.9 GiB the learned denoiser at a tile of 512 needs; another program may be using it"
        );
        assert_eq!(ai.take_short().as_deref(), Some(short.0.as_str()));
        assert!(ai.short_failed());
        // Refused, the walk takes the engine's own denoise in its place.
        assert!(matches!(
            crate::worker::learned_failure(&anyhow::Error::new(short)),
            crate::walk::NoLearned::Short(_)
        ));
        // The card with the room, or its memory not known: no rung.
        let (mut ai, calls) = denoiser_on_card(3 * GIB, GIB);
        assert_eq!(ai.denoise_room_at(512).unwrap(), DenoiseOn::Card);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        let mut unknown = Ai::for_test(Store::at("no-store"), vec![Provider::WebGpu]);
        assert_eq!(unknown.denoise_room_at(512).unwrap(), DenoiseOn::Card);
        // Its own sessions given back first: room then, nothing let go.
        let (mut ai, calls) = denoiser_on_card(free, GIB);
        ai.freed_by_test = GIB;
        assert_eq!(ai.denoise_room_at(512).unwrap(), DenoiseOn::Card);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        // Its own tile past the whole card: the engine lets go before the
        // smaller tiles are judged (0.8 and 0.4 let go: room at 384).
        let (mut ai, calls) = denoiser_on_card(8 * GIB / 10, 4 * GIB / 10);
        ai.card_by_test = Some(Some(4 * GIB));
        assert_eq!(ai.denoise_room_at(1024).unwrap(), DenoiseOn::Tile(384));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        // Its own sessions given back there too (0.4 of them).
        let (mut ai, calls) = denoiser_on_card(8 * GIB / 10, 0);
        ai.card_by_test = Some(Some(4 * GIB));
        ai.freed_by_test = 4 * GIB / 10;
        assert_eq!(ai.denoise_room_at(1024).unwrap(), DenoiseOn::Tile(384));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        // A canceled export lets nothing go.
        let (mut ai, calls) = denoiser_on_card(GIB / 2, GIB);
        ai.set_patience(Some(Patience {
            tries: 1,
            wait: std::time::Duration::ZERO,
            canceled: Arc::new(|| true),
        }));
        assert!(ai.denoise_room_at(512).is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    /// A patient walk (an export) short of the denoiser's tile runs at a
    /// smaller one at once, waiting for nothing; short at every tile, it
    /// waits its tries once, at the smallest, and is refused.
    #[test]
    fn an_export_short_of_the_denoisers_tile_runs_at_a_smaller_one() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut ai = Ai::for_test(Store::at("no-store"), vec![Provider::WebGpu]);
        ai.free_memory_by_test = Some(18 * GIB / 10);
        let looked = Arc::new(AtomicUsize::new(0));
        let counted = looked.clone();
        ai.set_patience(Some(Patience {
            tries: 3,
            wait: std::time::Duration::ZERO,
            canceled: Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                false
            }),
        }));
        let waits = waits_by_test();
        assert_eq!(ai.denoise_room_at(512).unwrap(), DenoiseOn::Tile(384));
        // A smaller tile fits: no wait, nothing met.
        assert_eq!(waits_by_test(), waits, "no wait");
        assert_eq!(looked.load(Ordering::SeqCst), 0);
        assert!(ai.take_short().is_none());
        assert!(!ai.hit_short() && !ai.short_failed());
        // Short at every tile: it waits its tries at the smallest, then
        // is refused at its own, a failure met for the set.
        ai.free_memory_by_test = Some(GIB / 2);
        let short = ai.denoise_room_at(512).expect_err("refused");
        assert_eq!(waits_by_test(), waits + 2);
        assert!(short.0.contains("at a tile of 512"), "{short}");
        assert!(ai.hit_short() && ai.short_failed());
        // The next ask of the walk waits for nothing.
        assert!(ai.denoise_room_at(512).is_err());
        assert_eq!(waits_by_test(), waits + 2, "once a set");
    }

    /// A short's figures are said to a tenth in the log, or to a
    /// hundredth where a tenth would round away what is compared.
    #[test]
    fn a_shorts_figures_are_what_is_compared() {
        assert_eq!(gib_said(denoise_needs(512)), "1.95");
        assert_eq!(gib_said(remove_needs(1024, false)), "7.1");
        assert_eq!(gib_said(GIB), "1.0");
        assert_eq!(gib_said(18 * GIB / 10), "1.8");
    }

    /// A removal refused for the card leaves its need and the card's
    /// free memory for the window's word on it, taken once.
    #[test]
    fn a_refused_removal_leaves_its_figures() {
        let image = WorkingImage::new(200, 100);
        let patch = remove_patch(33);
        let region = patch.region(image.width, image.height).unwrap();
        let mut ai = on_card(2 * GIB, 0);
        assert!(matches!(
            ai.fill(&patch, &region, &image),
            Err(NoFill::Failed(_))
        ));
        assert!(ai.short_failed());
        assert_eq!(
            ai.take_short_figures(),
            Some((remove_needs(512, false), 2 * GIB))
        );
        assert_eq!(ai.take_short_figures(), None);
    }

    /// A set whose frames are short of the card by what the engine's
    /// own working sets hold, as issue 130's 4 GB card, never waits:
    /// each frame lets go of the sets its walk made again before any
    /// wait, the learned denoiser runs at its own tile, and a removal
    /// is made.
    #[test]
    fn a_set_short_by_the_engines_own_sets_never_waits() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let patience = || {
            Some(Patience {
                tries: PATIENT_TRIES,
                wait: std::time::Duration::ZERO,
                canceled: Arc::new(|| false),
            })
        };
        let waits = waits_by_test();
        let (mut ai, calls) = denoiser_on_card(18 * GIB / 10, 0);
        let c = calls.clone();
        ai.set_card_release(Some(Box::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
            vec![("the sharpen's working set", 12 * GIB / 10)]
        })));
        for _ in 0..3 {
            ai.free_memory_by_test = Some(18 * GIB / 10);
            ai.set_patience(patience());
            assert_eq!(ai.denoise_room_at(512).unwrap(), DenoiseOn::Card);
            assert!(!ai.hit_short() && !ai.short_failed());
        }
        // A card a smaller tile fits: no wait either.
        ai.set_card_release(None);
        for _ in 0..3 {
            ai.free_memory_by_test = Some(18 * GIB / 10);
            ai.set_patience(patience());
            assert_eq!(ai.denoise_room_at(512).unwrap(), DenoiseOn::Tile(384));
        }
        assert_eq!(calls.load(Ordering::Relaxed), 3, "let go each frame");
        let image = WorkingImage::new(200, 100);
        let mut ai = on_card(2 * GIB, 0);
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        ai.set_card_release(Some(Box::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
            vec![("the sharpen's working set", 5 * GIB)]
        })));
        for i in 0..3 {
            ai.free_memory_by_test = Some(2 * GIB);
            ai.set_patience(patience());
            let patch = remove_patch(50 + i);
            let region = patch.region(image.width, image.height).unwrap();
            ai.fill(&patch, &region, &image).expect("made");
            assert!(!ai.hit_short() && !ai.short_failed());
        }
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        assert_eq!(waits_by_test(), waits, "no waits");
    }

    /// A removal refused by this round's own ask is said with its
    /// refusal's figure though the card freed a little since; one asked
    /// in no round says it only while the refusal holds.
    #[test]
    fn a_refusal_just_met_is_said_whatever_the_card_frees_after() {
        let image = WorkingImage::new(200, 100);
        let patch = remove_patch(63);
        let region = patch.region(image.width, image.height).unwrap();
        let mut ai = on_card(2 * GIB, 0);
        assert!(matches!(
            ai.fill(&patch, &region, &image),
            Err(NoFill::Failed(_))
        ));
        ai.free_memory_by_test = Some(2 * GIB + (10 << 20));
        let retouch = retouch_of(&patch);
        let ids = [patch.id];
        let said = vec![(patch.name(), remove_needs(512, false), 2 * GIB)];
        assert_eq!(
            ai.card_shorts(&retouch, Some(&ids), &ids, &[patch.name()]),
            said
        );
        assert!(ai.card_shorts(&retouch, Some(&ids), &[], &[]).is_empty());
        ai.free_memory_by_test = Some(2 * GIB);
        assert_eq!(ai.card_shorts(&retouch, Some(&ids), &[], &[]), said);
        assert!(
            ai.card_shorts(&retouch, Some(&ids), &ids, &[]).is_empty(),
            "asked again"
        );
    }

    /// An export's learned mask short of the card goes to the CPU as it
    /// always did: the engine's sets are not let go for it.
    #[test]
    fn an_exports_mask_lets_nothing_go() {
        use std::sync::atomic::Ordering;
        let (mut ai, released, _) = mask_on_card(GIB, vec![("the sharpen's working set", 8 * GIB)]);
        ai.set_patience(Some(Patience {
            tries: 1,
            wait: std::time::Duration::ZERO,
            canceled: Arc::new(|| false),
        }));
        ai.set_cpu_fallback(true);
        assert!(ai.mask_room(MaskModel::People).is_ok());
        assert!(ai.masks_on_cpu());
        assert_eq!(released.load(Ordering::Relaxed), 0);
    }

    /// A removal on a card under the remove's need is refused for the
    /// card before its model is asked for: not downloaded, it is still
    /// the card that is said, and nothing is offered.
    #[test]
    fn a_card_under_the_need_is_said_before_a_missing_model() {
        let image = WorkingImage::new(200, 100);
        let patch = remove_patch(62);
        let region = patch.region(image.width, image.height).unwrap();
        let mut ai = Ai::for_test(Store::at("no-store"), vec![Provider::WebGpu, Provider::Cpu]);
        ai.card_by_test = Some(Some(4 * GIB));
        assert!(matches!(
            ai.fill(&patch, &region, &image),
            Err(NoFill::Card { need, have }) if need == remove_needs(512, false) && have == 4 * GIB
        ));
    }

    /// A removal on a card whose whole memory is under the remove's need
    /// is refused at once with the need and the card, before any
    /// give-back or release, and the worker is not told to give the
    /// card back.
    #[test]
    fn a_removal_past_the_whole_card_is_refused_at_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let image = WorkingImage::new(200, 100);
        let patch = remove_patch(61);
        let region = patch.region(image.width, image.height).unwrap();
        let mut ai = on_card(2 * GIB, GIB);
        ai.card_by_test = Some(Some(4 * GIB));
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        ai.set_card_release(Some(Box::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
            Vec::new()
        })));
        assert!(matches!(
            ai.fill(&patch, &region, &image),
            Err(NoFill::Card { need, have }) if need == remove_needs(512, false) && have == 4 * GIB
        ));
        assert_eq!(calls.load(Ordering::Relaxed), 0, "nothing let go");
        assert!(!ai.take_fill_short(), "nothing given back");
        assert!(!ai.short_failed());
        // A card the remove fits: asked as ever.
        ai.card_by_test = Some(Some(8 * GIB));
        assert!(matches!(
            ai.fill(&patch, &region, &image),
            Err(NoFill::Failed(_))
        ));
        assert!(ai.take_fill_short());
    }

    /// No session is dropped to give the card back while a merge runs.
    #[test]
    fn a_merge_holds_the_give_back() {
        let image = WorkingImage::new(200, 100);
        let patch = remove_patch(14);
        let region = patch.region(image.width, image.height).unwrap();
        let mut ai = on_card(8 * GIB, GIB);
        ai.fill(&patch, &region, &image).expect("made");
        let merge = MergeRunning::new();
        assert!(!give_back_card(&mut [&mut ai], true));
        assert!(ai.holds_sessions());
        drop(merge);
        assert!(give_back_card(&mut [&mut ai], true));
    }

    /// After a fill's model ran, the sessions are kept while the card
    /// has the floor and the margin free with them held, given back
    /// under it, and given back whatever it has when forced.
    #[test]
    fn the_sessions_are_kept_while_the_card_has_room() {
        let image = WorkingImage::new(200, 100);
        let patch = Patch {
            remove: false,
            ..remove_patch(15)
        };
        let region = patch.region(image.width, image.height).unwrap();
        let room = KEEP_FLOOR + CARD_MARGIN;
        let mut ai = on_card(room + GIB, GIB);
        ai.fill(&patch, &region, &image).expect("made");
        assert!(give_back_due(&[&ai]));
        assert!(!give_back_card(&mut [&mut ai], false));
        assert!(ai.holds_sessions());
        // Asked once a fill: not again until another runs.
        assert!(!give_back_due(&[&ai]));
        // Under the floor, the next run's give-back lets them go.
        let mut short = on_card(room - TENTH_GIB, GIB);
        short.fill(&patch, &region, &image).expect("made");
        assert!(give_back_card(&mut [&mut short], false));
        assert!(!short.holds_sessions());
        // Forced, with room, they go all the same.
        assert!(give_back_card(&mut [&mut ai], true));
        assert!(!ai.holds_sessions());
    }

    /// A count of the fills ahead does not ask the card: a fill it is
    /// short of is counted, nothing is given back, nothing waits, and
    /// the worker is not told; the run asks, and gives back.
    #[test]
    fn a_count_never_gives_the_card_back() {
        let image = WorkingImage::new(200, 100);
        let patch = remove_patch(15);
        let retouch = Retouch {
            enabled: true,
            patches: vec![patch.clone()],
        };
        let mut ai = on_card(6 * GIB, 4 * GIB);
        ai.set_patience(Some(Patience {
            tries: 3,
            wait: std::time::Duration::ZERO,
            canceled: Arc::new(|| panic!("a count waited")),
        }));
        assert_eq!(ai.fills_unmade_ids(&retouch, &image), [15]);
        assert!(ai.holds_sessions(), "nothing given back by a count");
        assert!(!ai.take_fill_short());
        let region = patch.region(image.width, image.height).unwrap();
        ai.set_patience(None);
        ai.fill(&patch, &region, &image)
            .expect("made once given back");
        assert!(!ai.holds_sessions());
    }

    /// A short's flags say which kind it was, and go when the next
    /// develop round, or a new patience, clears them.
    #[test]
    fn the_shorts_are_taken_where_their_job_ends() {
        let mut ai = on_card(GIB, 0);
        assert!(ai.mask_room(MaskModel::People).is_err());
        assert!(!ai.take_fill_short(), "a mask's short is not a fill's");
        assert!(ai.take_mask_short());
        assert!(ai.mask_room(MaskModel::People).is_err());
        ai.clear_shorts();
        assert!(!ai.take_mask_short());
        assert!(ai.take_short().is_none());
        let image = WorkingImage::new(200, 100);
        let patch = remove_patch(16);
        let region = patch.region(image.width, image.height).unwrap();
        assert!(ai.fill(&patch, &region, &image).is_err());
        ai.set_patience(None);
        assert!(!ai.take_fill_short());
    }

    /// The lock a merge and a give-back share, as the editor runs it: no
    /// give-back while a merge holds it, from another thread too, and
    /// both taken after a give-back's panic poisoned it.
    #[test]
    fn a_give_back_waits_for_no_merge_on_the_lock() {
        static LOCK: std::sync::RwLock<()> = std::sync::RwLock::new(());
        let merge = merge_hold(&LOCK);
        assert!(give_back_hold(&LOCK).is_none());
        let elsewhere = std::thread::spawn(|| give_back_hold(&LOCK).is_none());
        assert!(elsewhere.join().unwrap());
        drop(merge);
        let held = give_back_hold(&LOCK).expect("no merge");
        assert!(give_back_hold(&LOCK).is_none(), "one give-back at a time");
        drop(held);
        let _ = std::thread::spawn(|| {
            let _held = give_back_hold(&LOCK);
            panic!("a give-back panicked");
        })
        .join();
        assert!(LOCK.is_poisoned());
        let merge = merge_hold(&LOCK);
        assert!(give_back_hold(&LOCK).is_none());
        drop(merge);
        assert!(give_back_hold(&LOCK).is_some());
    }
}
