//! The learned masks, on the worker: the models, the preview they see,
//! and the rasters they made. A mask is made once for a shape and kept
//! until the shape changes or another file is opened; it sees the base
//! develop as it was then, since a subject does not move when the
//! white balance does. Made rasters are also kept on disk, keyed by
//! the file, the model and the shape, so opening a file again does not
//! run the model again; so are made fills, beside them.

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
#[cfg(test)]
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
    #[cfg(test)]
    if let Some(on) = TEST_WEBGPU.with(|t| t.get()) {
        return on;
    }
    providers().contains(&Provider::WebGpu)
}

#[cfg(test)]
thread_local! {
    static TEST_WEBGPU: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// [`on_webgpu`]'s answer on this thread, for a test; `None` asks the
/// providers again.
#[cfg(test)]
pub(crate) fn set_test_webgpu(on: Option<bool>) {
    TEST_WEBGPU.with(|t| t.set(on));
}

/// Whether the GPU the models run on shares the machine's memory
/// (`runtime::webgpu_unified`), false when it is not known.
pub fn unified() -> bool {
    #[cfg(test)]
    if let Some(on) = TEST_UNIFIED.with(|t| t.get()) {
        return on;
    }
    greycard_ai::runtime::webgpu_unified().unwrap_or(false)
}

#[cfg(test)]
thread_local! {
    static TEST_UNIFIED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// [`unified`]'s answer on this thread, for a test; `None` asks the
/// adapter.
#[cfg(test)]
pub(crate) fn set_test_unified(on: Option<bool>) {
    TEST_UNIFIED.with(|t| t.set(on));
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

/// Whether the shape has anything for a model to go on.
pub fn prompted(shape: &Shape) -> bool {
    match shape {
        Shape::Subject {} | Shape::Background {} | Shape::Sky { .. } | Shape::Part { .. } => true,
        Shape::Landscape { .. } => !shape.is_unknown(),
        Shape::Object { picks, boxes } => !picks.is_empty() || !boxes.is_empty(),
        _ => false,
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
    pub(crate) last_sky: Option<SkyReport>,
    fill: Option<Fill>,
    /// The generative remove, loaded at the first Fill patch it makes.
    remove: Option<Remove>,
    /// The learned denoiser loaded, by its model's id: one at a time,
    /// since each holds the GPU's memory.
    denoiser: Option<(&'static str, Denoiser)>,
    /// Fills made, by patch id and seed, with the patch, the window
    /// and the engine each was made for: a seed's fill is kept when
    /// "Another" moves the patch on to the next, so undo is free. Each
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
    /// shape it was made for.
    cache: HashMap<(Key, u8), (Shape, Arc<Raster>)>,
    /// Whether made rasters and fills are kept on disk (`cached_path`,
    /// `fill_path`); never for a test that runs the models.
    disk: bool,
    /// The Subject matte at the raster's size, of base develop
    /// `stamp`, and the provider that made it: run once and read by a
    /// Background sharing it, so a mask with both costs the model one
    /// call, not two, whichever shape asks for it first. The provider
    /// is carried along so a hit here still says where its run
    /// happened and still turns Show mask on, as an actual run does.
    subject_matte: Option<(u64, Vec<u8>, Provider)>,
    /// A Whole person keeps SAM's edge whatever the store has: for a
    /// test to set the two edges side by side.
    #[cfg(test)]
    pub(crate) sams_edge_only: bool,
    /// How many times the sky model has run for a scene: for a test
    /// that the Landscape button, a Sky and the Landscape shapes share
    /// one run.
    #[cfg(test)]
    pub(crate) scene_runs: usize,
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
pub(crate) fn turn_frac(turn: u8, [x, y]: [f32; 2]) -> [f32; 2] {
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

/// Which of a frame's left-out items (`worker::write_export`'s) are
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
pub(crate) struct SkyReport {
    /// The prior's provider, and SAM's (encoder) when it ran.
    pub(crate) prior_on: Option<Provider>,
    pub(crate) sam_on: Option<Provider>,
    /// Loading the prior, when this raster loaded it.
    pub(crate) load: f64,
    pub(crate) prior: f64,
    /// SAM's loading and embedding, when this raster paid for them.
    pub(crate) sam_load: f64,
    pub(crate) embed: f64,
    pub(crate) decodes: usize,
    pub(crate) stages: sky::Times,
    /// Bringing the matte to the raster's size.
    pub(crate) raster: f64,
    pub(crate) seeded: bool,
    pub(crate) no_sky: Option<sky::NoSky>,
    /// The outline the matte was made from, at the preview's size.
    pub(crate) outline: Option<greycard_ai::Mask>,
}

/// Whether a learned mask's raster can be had ([`Ai::raster_ready`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ready {
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
}

/// A fill made: the patch, window and engine it was made for, and the
/// region's values.
type FillMade = (Patch, Window, Engine, Vec<f32>);

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
/// remove, the four-step remove's own files, and WebGPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FillHaves {
    pub fill: bool,
    pub remove: bool,
    pub fast: bool,
    pub webgpu: bool,
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
        }
    }

    /// Why `choice` can never make a Fill on this machine, or None:
    /// both removes want WebGPU. A model not downloaded is not a
    /// reason ([`FillHaves::missing`]): it can be fetched.
    pub fn why_not(self, choice: FillChoice) -> Option<&'static str> {
        match choice {
            FillChoice::Patch => None,
            _ if !self.webgpu => Some("Needs a GPU with WebGPU"),
            _ => None,
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
    /// minute a fill there; Best otherwise.
    pub fn default_choice(self, unified: bool) -> FillChoice {
        if unified && self.runs(FillChoice::Fast) {
            FillChoice::Fast
        } else {
            FillChoice::Best
        }
    }
}

/// The sampler a remove's Fill runs: four steps for a Fast one.
pub fn steps_for(patch: &Patch) -> greycard_ai::remove::Steps {
    match patch.fill_choice() {
        FillChoice::Fast => greycard_ai::remove::Steps::Lcm4,
        _ => greycard_ai::remove::Steps::Euler25,
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
fn window_for(region: &Region, width: usize, height: usize) -> (Window, f32) {
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
            denoiser: None,
            fills: HashMap::new(),
            failed: HashMap::new(),
            windows: HashMap::new(),
            preview: None,
            embedding: None,
            sam3: None,
            presets: None,
            encodings: Encodings::new(PART_CROPS),
            people: None,
            parts_view: None,
            picture: None,
            cache: HashMap::new(),
            disk: true,
            subject_matte: None,
            #[cfg(test)]
            sams_edge_only: false,
            #[cfg(test)]
            scene_runs: 0,
        }
    }

    /// The sky prior of the last develop a Sky raster was made on.
    #[cfg(test)]
    pub(crate) fn sky_prior(&self) -> Option<&Prior> {
        self.scene.as_ref().map(|s| &s.1.prior)
    }

    /// Models from `store` on `providers` alone, and no file, so no
    /// disk cache: for a test that runs the models.
    #[cfg(test)]
    pub(crate) fn for_test(store: Store, providers: Vec<Provider>) -> Self {
        Self {
            store: Some(store),
            providers,
            disk: false,
            ..Self::new()
        }
    }

    /// The learned denoiser `model`, loaded the first time it is asked
    /// for; another tier replaces the one loaded.
    pub fn denoiser(&mut self, model: &'static Model) -> Result<&mut Denoiser, NoDenoiser> {
        // No runtime here: fetching the model would not help.
        greycard_ai::runtime::status().map_err(|why| NoDenoiser::Failed(why.to_string()))?;
        let store = self.store.clone().ok_or(NoDenoiser::Missing)?;
        if !store.have(model) {
            return Err(NoDenoiser::Missing);
        }
        if self.providers.is_empty() {
            self.providers = Provider::available();
        }
        if self.denoiser.as_ref().is_none_or(|(id, _)| *id != model.id) {
            self.denoiser = None;
            let denoiser = Denoiser::from_store(&store, model, &self.providers)
                .map_err(|e| NoDenoiser::Failed(e.to_string()))?;
            self.denoiser = Some((model.id, denoiser));
        }
        Ok(&mut self
            .denoiser
            .as_mut()
            .expect("a denoiser was just loaded")
            .1)
    }

    /// Another file: nothing made so far applies.
    pub fn forget(&mut self, file: Option<PathBuf>) {
        self.file = file;
        self.cache.clear();
        self.fills.clear();
        self.failed.clear();
        self.windows.clear();
        self.preview = None;
        self.embedding = None;
        self.scene = None;
        self.sky_cut = None;
        self.subject_matte = None;
        self.encodings.clear();
        self.people = None;
        self.parts_view = None;
    }

    /// No Part is live on the open picture: the People model, its
    /// encodings and its people go, about 2 GB with a picture's crops;
    /// the next Part loads them again.
    pub fn release_parts(&mut self) {
        if self.sam3.is_some() {
            tracing::debug!("the People model let go: no People shape on the picture");
        }
        self.sam3 = None;
        self.presets = None;
        self.encodings.clear();
        self.people = None;
        self.parts_view = None;
    }

    /// The People model lent to `other`, or back from it: the export of
    /// a set's other frames runs its own `Ai` on the same thread, and
    /// has the editor's model rather than a second copy of it.
    pub fn lend_parts(&mut self, other: &mut Ai) {
        if self.sam3.is_some() {
            other.sam3 = self.sam3.take();
            other.presets = self.presets.take();
        }
        // And the Subject model a Whole person's edge runs.
        if self.subject.is_some() {
            other.subject = self.subject.take();
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
        self.subject_matte = None;
        // A Whole person made without Subject's edge is made again.
        self.cache.retain(|_, (s, _)| !takes_subject_edge(s));
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
    pub fn could_fill_more(&mut self, retouch: &Retouch, image: &WorkingImage) -> bool {
        retouch
            .patches
            .iter()
            .filter(|p| p.method == greycard_edit::retouch::Method::Fill)
            .any(|p| {
                p.region(image.width, image.height).is_some_and(|region| {
                    !self.fill_kept(p, &region, image)
                        && self.failed_before(p, &region, image).is_none()
                        && self.engine(p, &region, image).is_ok()
                })
            })
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
        self.fill_in_memory(patch, window).is_some()
            || self.fill_from_disk(patch, region, image, window)
    }

    /// The patch's fill, as it is now, kept in memory.
    fn fill_in_memory(&self, patch: &Patch, window: Window) -> Option<(Vec<f32>, Engine)> {
        self.fills
            .get(&(patch.id, patch.seed))
            .filter(|(p, w, _, _)| same_fill(p, patch) && *w == window)
            .map(|(_, _, engine, data)| (data.clone(), *engine))
    }

    /// The patch's fill read from its slot on disk into memory, if it
    /// is there as the patch and the picture round it are now.
    fn fill_from_disk(
        &mut self,
        patch: &Patch,
        region: &Region,
        image: &WorkingImage,
        window: Window,
    ) -> bool {
        let size = (image.width, image.height);
        let Some(path) = self.fill_path(patch, region, image, window) else {
            return false;
        };
        let Some((engine, data)) = read_fill(
            &path,
            FILL_VERSION,
            size,
            window,
            (region.width, region.height),
        ) else {
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
            (patch.clone(), window, engine, data),
        );
        true
    }

    /// Where the patch's fill on the open file lives on disk: beside
    /// the made rasters, named by a hash of the file, the patch as
    /// [`same_fill`] compares it, its seed, the picture's size, the
    /// region, the window, both engines' models, and a print of the
    /// window's known pixels ([`window_print`]): a fill made under
    /// another white balance, lens, turn or virtual copy is another.
    fn fill_path(
        &self,
        patch: &Patch,
        region: &Region,
        image: &WorkingImage,
        window: Window,
    ) -> Option<PathBuf> {
        let size = (image.width, image.height);
        if !self.disk {
            return None;
        }
        let file = self.file.as_ref()?;
        let root = self.store.as_ref()?.root().parent()?.join("masks");
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
        window_print(image, region, window).hash(&mut h);
        Some(root.join(format!("{:016x}.fill", h.finish())))
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
        if let Some(path) = self.fill_path(patch, region, image, window) {
            write_fill(
                &path,
                FILL_VERSION,
                engine,
                size,
                window,
                (region.width, region.height),
                &data,
            );
        }
        self.fills.insert(
            (patch.id, patch.seed),
            (patch.clone(), window, engine, data.clone()),
        );
        data
    }

    /// The engine the patch's fill is made by here, or why it is left
    /// as it was ([`engine_for`]).
    pub fn engine(
        &mut self,
        patch: &Patch,
        region: &Region,
        image: &WorkingImage,
    ) -> Result<Engine, NoFill> {
        let (window, _) = self.window_of(patch, region, image);
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
        Ok(engine)
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
    /// [`engine_for`]); or why not. Kept by patch, seed and window, so
    /// a slider elsewhere does not run the model again.
    pub fn fill(
        &mut self,
        patch: &Patch,
        region: &Region,
        image: &WorkingImage,
    ) -> Result<(Vec<f32>, Engine), NoFill> {
        let (window, depth) = self.window_of(patch, region, image);
        if let Some(kept) = self.fill_in_memory(patch, window) {
            return Ok(kept);
        }
        if self.fill_from_disk(patch, region, image, window)
            && let Some(kept) = self.fill_in_memory(patch, window)
        {
            return Ok(kept);
        }
        if let Some(why) = self.failed_before(patch, region, image) {
            return Err(NoFill::Failed(why));
        }
        greycard_ai::runtime::status().map_err(|why| NoFill::Failed(why.to_string()))?;
        if self.store.is_none() {
            return Err(NoFill::Missing(&greycard_ai::FILL));
        }
        let engine = self.engine(patch, region, image)?;
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
                // The card's memory is the machine's on unified memory,
                // and the remove keeps more open there.
                let machine = greycard_ai::remove::Machine {
                    memory: greycard_ai::ram::physical(),
                    unified: greycard_ai::runtime::webgpu_unified(),
                };
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
        let to_srgb = crate::export::Space::Srgb.matrix();
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
                (made.image, made.scale)
            }
            Engine::Remove => {
                let remove = self.remove.as_mut().expect("the remove, loaded above");
                if let Err(e) = remove.set_steps(steps_for(patch)) {
                    return Err(self.remember_failure(patch, window, e.to_string()));
                }
                let made = match remove.remove_window(&input, &hole, remove_seed(patch)) {
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
    /// choice `panel::mask::step` already made, with the session's
    /// declines and the providers record folded in, which nothing on
    /// the worker's side can re-derive on its own; else, with
    /// neither, the plain store-and-providers guess a caller outside
    /// the panel's `ask_for` (the export path) is left to make.
    fn subject_model(&self, model: Option<&'static Model>) -> Option<&'static Model> {
        self.subject
            .as_ref()
            .map(|s| s.model())
            .or(model)
            .or_else(|| model_for(&Shape::Subject {}, self.store.as_ref(), &[]))
    }

    /// Where a made raster for `shape` on the open file lives on disk:
    /// under the models' cache, named by a hash of the file, the
    /// model's id and the shape. `model` is `raster`'s own, so the
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
        let file = self.file.as_ref()?;
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
        Some(root.join(format!("{:016x}.png", h.finish())))
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
    /// develop ([`Ai::cached_raster`]).
    pub(crate) fn raster_ready(
        &mut self,
        file: &Path,
        shape: &Shape,
        turn: u8,
        frame: Option<(usize, usize)>,
    ) -> Ready {
        if self.file.as_deref() != Some(file) {
            self.forget(Some(file.to_path_buf()));
        }
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
    #[cfg(test)]
    pub(crate) fn cached_path_of(
        &mut self,
        file: &Path,
        shape: &Shape,
        turn: u8,
    ) -> Option<PathBuf> {
        if self.file.as_deref() != Some(file) {
            self.forget(Some(file.to_path_buf()));
        }
        self.cached_path(shape, None, turn)
    }

    /// Models from `store`, with the disk cache of the rasters beside
    /// it: for a test of the reduced develop's masks.
    #[cfg(test)]
    pub(crate) fn for_test_on_disk(store: Store) -> Self {
        Self {
            store: Some(store),
            ..Self::new()
        }
    }

    /// The raster for `shape` of the frame at `file` from the disk cache
    /// alone, for a develop of the frame whose full develop is `full`
    /// (width, height) at `turn`: nothing is loaded and no model runs.
    /// The reduced develop's, which keeps no picture of a frame whose
    /// masks are not made yet. The raster's width is the frame's
    /// (`brush::raster_width`).
    pub(crate) fn cached_raster(
        &mut self,
        file: &Path,
        shape: &Shape,
        turn: u8,
        full: (u32, u32),
    ) -> Option<Arc<Raster>> {
        if self.file.as_deref() != Some(file) {
            self.forget(Some(file.to_path_buf()));
        }
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
    /// shape, else from the model on the preview of the base develop
    /// `stamp` of `image` under `edit`, finished as `kind` is. `model`
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
    #[allow(clippy::too_many_arguments)]
    pub fn raster(
        &mut self,
        stamp: u64,
        image: &WorkingImage,
        edit: &Edit,
        kind: crate::finish::Source,
        turn: u8,
        key: Key,
        shape: &Shape,
        model: Option<&'static Model>,
    ) -> Result<Made, String> {
        let frame = (image.width, image.height);
        self.raster_for(frame, stamp, image, edit, kind, turn, key, shape, model)
    }

    /// [`Ai::raster`] on a picture of a frame whose full develop is
    /// `frame` (width, height): a reduced develop's, whose picture is
    /// smaller than the frame. The raster's width (`brush::raster_width`),
    /// its height and its aspect all follow the frame, not the picture,
    /// whose binning rounds its height: so the reduced develop, the
    /// index's masks and the open frame make the same raster, row for
    /// row, and keep it in the same slot.
    #[allow(clippy::too_many_arguments)]
    pub fn raster_for(
        &mut self,
        frame: (usize, usize),
        stamp: u64,
        image: &WorkingImage,
        edit: &Edit,
        kind: crate::finish::Source,
        turn: u8,
        key: Key,
        shape: &Shape,
        model: Option<&'static Model>,
    ) -> Result<Made, String> {
        self.raster_of(frame, stamp, image, edit, kind, turn, key, shape, model)
            .map_err(|e| match greycard_ai::runtime::status() {
                Err(why) => why.to_string(),
                Ok(_) => e,
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn raster_of(
        &mut self,
        frame: (usize, usize),
        stamp: u64,
        image: &WorkingImage,
        edit: &Edit,
        kind: crate::finish::Source,
        turn: u8,
        key: Key,
        shape: &Shape,
        model: Option<&'static Model>,
    ) -> Result<Made, String> {
        if let Some((s, r)) = self.cache.get(&(key, turn))
            && s == shape
        {
            return Ok(Made {
                note: self.empty_note(shape, r),
                raster: r.clone(),
                provider: None,
                seconds: 0.0,
                part: None,
            });
        }
        if !prompted(shape) {
            return Err("nothing picked yet".into());
        }
        let width = raster_width(shape, frame);
        let aspect = frame.1 as f32 / frame.0.max(1) as f32;
        let height = raster_height(width, aspect);
        let cached = self.cached_path_at(shape, model, turn, width);
        if let Some(data) = cached
            .as_deref()
            .and_then(|p| read_raster(p, width, height))
        {
            let raster = Arc::new(Raster::from_data(aspect, width, data));
            self.cache
                .insert((key, turn), (shape.clone(), raster.clone()));
            return Ok(Made {
                note: self.empty_note(shape, &raster),
                raster,
                provider: None,
                seconds: 0.0,
                part: None,
            });
        }
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
                .insert((key, turn), (shape.clone(), raster.clone()));
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
                    .insert((key, turn), (shape.clone(), raster.clone()));
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
                .insert((key, turn), (shape.clone(), raster.clone()));
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
                .insert((key, turn), (shape.clone(), raster.clone()));
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
                .insert((key, turn), (shape.clone(), raster.clone()));
            return Ok(Made {
                raster,
                provider: Some(provider),
                seconds: start.elapsed().as_secs_f64(),
                part: None,
                note: None,
            });
        }
        let (_, rgb, luma) = self.preview.as_ref().expect("a preview was just made");
        let (mask, provider) = match shape {
            Shape::Object { picks, boxes } => {
                let sam = match &mut self.sam {
                    Some(s) => s,
                    None => self
                        .sam
                        .insert(Sam::load(&store, &self.providers).map_err(|e| e.to_string())?),
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
            .insert((key, turn), (shape.clone(), raster.clone()));
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
        kind: crate::finish::Source,
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
            self.sam3 = Some(Sam3::load(store, &self.providers).map_err(|e| e.to_string())?);
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
        kind: crate::finish::Source,
        turn: u8,
    ) -> Result<(Vec<Candidate>, Provider, f64), String> {
        greycard_ai::runtime::status().map_err(|why| why.to_string())?;
        let store = self
            .store
            .clone()
            .ok_or("no cache directory for the models")?;
        let start = Instant::now();
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
        kind: crate::finish::Source,
        turn: u8,
        carried: Option<&greycard_edit::mask::Person>,
        wanted: &Picked,
    ) -> Result<PersonOn, String> {
        greycard_ai::runtime::status().map_err(|why| why.to_string())?;
        let store = self
            .store
            .clone()
            .ok_or("no cache directory for the models")?;
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
        kind: crate::finish::Source,
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
        kind: crate::finish::Source,
        (phrase, route, person): PartAsked,
        raster: (usize, usize),
        store: &Store,
    ) -> Result<PartMade, String> {
        let route = route_of(route)?;
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
        kind: crate::finish::Source,
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
        let Ai {
            subject,
            preview,
            providers,
            ..
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
        #[cfg(test)]
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
        let Ai {
            sky: loaded,
            scene,
            preview,
            providers,
            ..
        } = self;
        let (_, rgb, _) = preview.as_ref().expect("a preview is made before a mask");
        let mut load = 0.0;
        if loaded.is_none() {
            let t = Instant::now();
            *loaded = Some(Sky::load(store, providers).map_err(|e| e.to_string())?);
            load = t.elapsed().as_secs_f64();
        }
        let model = loaded.as_mut().expect("the Sky model was just loaded");
        let t = Instant::now();
        let made = model.scene(rgb).map_err(|e| e.to_string())?;
        let run = t.elapsed().as_secs_f64();
        *scene = Some((stamp, made, model.provider()));
        #[cfg(test)]
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
        kind: crate::finish::Source,
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
        let Ai {
            scene,
            sam,
            embedding,
            preview,
            providers,
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
                    if sam.is_none() {
                        let t = Instant::now();
                        *sam = Some(Sam::load(store, providers)?);
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
    kind: crate::finish::Source,
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
pub(crate) fn none_found_note(what: &str, file: Option<&Path>) -> String {
    let name = file
        .and_then(|f| f.file_name())
        .map_or("this picture".to_string(), |n| {
            n.to_string_lossy().into_owned()
        });
    format!("no {what} found in {name}")
}

/// What a model sees: the picture under the global look alone, no
/// geometry, no vignette or grain, in sRGB, no more than `PREVIEW`
/// on its long side.
pub(crate) fn preview(image: &WorkingImage, edit: &Edit, kind: crate::finish::Source) -> Rgb8 {
    display(image, edit, kind, Some(PREVIEW))
}

/// `preview`'s rendering of one rectangle of `image` (its pixels,
/// clamped to it), at those pixels: what a crop the People model looks
/// closer at is cut from, so an eye in a full-length frame is the
/// sensor's pixels rather than a few dozen of the preview's.
pub(crate) fn region(
    image: &WorkingImage,
    edit: &Edit,
    kind: crate::finish::Source,
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
    kind: crate::finish::Source,
    long_edge: Option<u32>,
) -> Rgb8 {
    let mut edit = edit.clone();
    edit.adjustments.clear();
    edit.geometry = Default::default();
    edit.vignette = Default::default();
    edit.grain = Default::default();
    let settings = crate::export::Settings {
        format: crate::export::Format::Jpeg,
        long_edge,
        space: crate::export::Space::Srgb,
        // The models see the picture as it is, not sharpened for a
        // screen.
        sharpen: crate::export::Sharpen::Off,
        ..Default::default()
    };
    let source = (image.width as u32, image.height as u32);
    // No guide plane: the models see a picture the tone equalizer's
    // shifts read the pixel's own luminance for, as they did before
    // the tone equalizer. What a mask is drawn around takes no stop.
    let rendered = crate::export::render(
        image,
        &edit,
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
    let crate::export::Pixels::Eight(data) = rendered.pixels else {
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
    if let Err(e) = write() {
        tracing::warn!("mask cache {}: {e}", path.display());
    }
}

/// The fills kept on disk's format: bumped when a change to either
/// engine, or to how a fill is put together, makes another fill of the
/// same patch, so the old ones are made again.
const FILL_VERSION: u8 = 2;

/// The first bytes of a fill kept on disk.
const FILL_MAGIC: &[u8; 6] = b"GCFILL";

/// A fill kept on disk's header: the magic, the version and engine
/// bytes, and [`fill_numbers`] as u32s.
const FILL_HEADER: usize = FILL_MAGIC.len() + 2 + 8 * 4;

/// The side of the blocks [`window_print`] takes means over.
const PRINT_BLOCK: usize = 16;

/// A coarse print of the picture a fill is made from: the means of the
/// window's pixels outside the hole over 16-pixel blocks, at half
/// precision, a block with none of them marked empty. Whatever changes
/// the base develop under the window (white balance, profile, lens,
/// noise, a turn) changes it, and a fill made from another picture is
/// not served for this one.
fn window_print(image: &WorkingImage, region: &Region, window: Window) -> Vec<[u16; 3]> {
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
    for y in 0..window.height {
        let py = window.y + y;
        for x in 0..window.width {
            let px = window.x + x;
            if covered(px, py) {
                continue;
            }
            let i = (py * image.width + px) * 3;
            let (sum, n) = &mut sums[(y / PRINT_BLOCK) * bw + x / PRINT_BLOCK];
            for (s, v) in sum.iter_mut().zip(&image.data[i..i + 3]) {
                *s += f64::from(*v);
            }
            *n += 1;
        }
    }
    sums.iter()
        .map(|(sum, n)| {
            if *n == 0 {
                return [u16::MAX; 3];
            }
            sum.map(|v| half::f16::from_f64(v / *n as f64).to_bits())
        })
        .collect()
}

/// A fill kept on disk: the magic, the version, the engine, then the
/// picture's size, the window and the region's size as little-endian
/// u32s, then the region's values as little-endian f16. Half precision
/// halves a fill that can run to tens of megabytes; its step, under a
/// part in two thousand, is below the grain put back over the fill.
fn write_fill(
    path: &Path,
    version: u8,
    engine: Engine,
    size: (usize, usize),
    window: Window,
    region: (usize, usize),
    data: &[f32],
) {
    use std::io::Write;
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let part = path.with_extension("fill-part");
        let mut out = std::io::BufWriter::new(std::fs::File::create(&part)?);
        out.write_all(FILL_MAGIC)?;
        out.write_all(&[version, engine_byte(engine)])?;
        for n in fill_numbers(size, window, region) {
            out.write_all(&u32::try_from(n)?.to_le_bytes())?;
        }
        for v in data {
            out.write_all(&half::f16::from_f32(*v).to_le_bytes())?;
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        std::fs::rename(&part, path)?;
        Ok(())
    };
    if let Err(e) = write() {
        tracing::warn!("fill cache {}: {e}", path.display());
    }
}

/// A fill kept on disk, if the file is there and was written at
/// `version` for a picture of `size`, `window` and a region of
/// `region`'s size: the engine that made it and the values.
fn read_fill(
    path: &Path,
    version: u8,
    size: (usize, usize),
    window: Window,
    region: (usize, usize),
) -> Option<(Engine, Vec<f32>)> {
    use std::io::Read;
    // The header alone first: a slot of another version or picture is
    // not read through on every develop.
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
    let want = region.0 * region.1 * 3 * 2;
    if !same || file.metadata().ok()?.len() != (FILL_HEADER + want) as u64 {
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
    Some((engine, data))
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
    crate::finish::encode(v.clamp(0.0, 1.0))
}

fn decode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each choice runs where what it needs is here, and says why not
    /// otherwise; a patch drawn asks for the sticky choice and falls to
    /// the other remove, then to LaMa; the default is Fast only on
    /// unified memory with its files here.
    #[test]
    fn the_fill_choices_run_where_their_models_are() {
        use FillChoice::{Best, Fast, Patch as Lama};
        let all = FillHaves {
            fill: true,
            remove: true,
            fast: true,
            webgpu: true,
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
        assert_eq!(cpu.why_not(Best), Some("Needs a GPU with WebGPU"));
        assert_eq!(cpu.why_not(Fast), Some("Needs a GPU with WebGPU"));
        assert_eq!(cpu.why_not(Lama), None);
        assert_eq!(cpu.drawn(Fast), Lama);
        let no_lama = FillHaves { fill: false, ..all };
        assert_eq!(no_lama.why_not(Lama), None);
        assert_eq!(no_lama.missing(Lama), Some(&greycard_ai::FILL));
        assert_eq!(all.default_choice(true), Fast);
        assert_eq!(all.default_choice(false), Best);
        assert_eq!(no_fast.default_choice(true), Best);
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
        let path = ai
            .fill_path(&patch, &region, &image, window)
            .expect("a slot");
        assert!(path.is_file());

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
            ai.fill_path(&fast, &region, &image, window),
            ai.fill_path(&patch, &region, &image, window)
        );
        assert!(!ai.fill_kept(&fast, &region, &image));
        let mut lama = patch.clone();
        lama.set_fill_choice(FillChoice::Patch);
        assert!(!ai.fill_kept(&lama, &region, &image));
        ai.forget(Some(file.clone()));
        assert!(ai.fill_kept(&patch, &region, &image));
        assert!(ai.fill_in_memory(&fast, window).is_none());
        ai.forget(Some(file.clone()));
        // So is the patch on the picture developed otherwise: another
        // white balance, or a virtual copy's, under the window.
        let mut warmer = image.clone();
        for px in warmer.data.as_chunks_mut::<3>().0 {
            px[0] = 0.3;
        }
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
        let size = (image.width, image.height);
        assert!(read_fill(&path, FILL_VERSION, size, window, dims).is_some());
        assert!(read_fill(&path, FILL_VERSION + 1, size, window, dims).is_none());
        assert!(read_fill(&path, FILL_VERSION, (201, 100), window, dims).is_none());
        // Written at an old version, the slot misses where it stands.
        write_fill(
            &path,
            FILL_VERSION - 1,
            Engine::Remove,
            size,
            window,
            dims,
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
        assert!(memory.fill_path(&patch, &region, &image, window).is_none());
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
        let spread = |data: &[f32]| {
            let v: Vec<f32> = middle
                .iter()
                .map(|&i| (data[i * 3] + data[i * 3 + 1] + data[i * 3 + 2]) / 3.0)
                .collect();
            let mean = v.iter().sum::<f32>() / v.len() as f32;
            (v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
        };
        assert!(spread(&out) < 1e-6, "flat before");
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
        let m = crate::export::Space::Srgb.matrix();
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
        let base: Vec<u8> = (0..RASTER_WIDTH * RASTER_WIDTH)
            .map(|i| (i % 256) as u8)
            .collect();
        ai.subject_matte = Some((5, base.clone(), Provider::Cpu));
        let image = WorkingImage::new(4, 4);
        let edit = Edit::default();

        let background = ai
            .raster(
                5,
                &image,
                &edit,
                crate::finish::Source::SCENE,
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
                5,
                &image,
                &edit,
                crate::finish::Source::SCENE,
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
            6,
            &image,
            &edit,
            crate::finish::Source::SCENE,
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
        let _ = std::fs::remove_dir_all(&dir);
        let mut ai = Ai::new();
        ai.store = Some(Store::at(dir.join("models")));
        ai.file = Some(PathBuf::from("open.CR3"));
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
                1,
                &WorkingImage::new(4, 4),
                &Edit::default(),
                crate::finish::Source::SCENE,
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

        let _ = std::fs::remove_dir_all(&dir);
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
        let _ = std::fs::remove_dir_all(&dir);
        let mut ai = Ai::new();
        let store = Store::at(dir.join("models"));
        ai.store = Some(store.clone());
        ai.file = Some(PathBuf::from("DSCF0153.RAF"));
        let shape = Shape::Sky { picks: Vec::new() };
        let without_sam = ai.cached_path(&shape, None, 0).expect("a path");
        write_raster(
            &without_sam,
            RASTER_WIDTH,
            &vec![0u8; RASTER_WIDTH * RASTER_WIDTH],
        );
        let made = ai
            .raster(
                1,
                &WorkingImage::new(4, 4),
                &Edit::default(),
                crate::finish::Source::SCENE,
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
                1,
                &WorkingImage::new(4, 4),
                &Edit::default(),
                crate::finish::Source::SCENE,
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
        let _ = std::fs::remove_dir_all(&dir);
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
        let _ = std::fs::remove_dir_all(&dir);
        let mut ai = Ai::new();
        ai.store = Some(Store::at(dir.join("models")));
        let file = PathBuf::from("frame.RAF");
        ai.file = Some(file.clone());
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
            ai.raster_ready(&file, &sky, 0, sized),
            Ready::Cached
        ));
        assert!(matches!(
            ai.raster_ready(&file, &sky, 0, None),
            Ready::Cached
        ));
        assert!(matches!(
            ai.raster_ready(&file, &sky, 0, Some((3000, 4000))),
            Ready::Cached
        ));
        assert!(ai.cached_raster(&file, &sky, 0, full).is_none());
        // A frame of that aspect under the rule still reads it.
        let small = ai
            .cached_raster(&file, &sky, 0, (3000, 4000))
            .expect("the narrow raster");
        assert_eq!((small.width, small.height), (RASTER_WIDTH, 2731));

        // The wide one made: ready, read back at its width.
        std::fs::remove_file(&narrow).unwrap();
        assert!(!matches!(
            ai.raster_ready(&file, &sky, 0, None),
            Ready::Cached
        ));
        write_raster(&wide, 5461, &vec![7u8; WIDE_RASTER_WIDTH * 5461]);
        assert!(matches!(
            ai.raster_ready(&file, &sky, 0, sized),
            Ready::Cached
        ));
        assert!(matches!(
            ai.raster_ready(&file, &sky, 0, None),
            Ready::Cached
        ));
        let read = ai
            .cached_raster(&file, &sky, 0, full)
            .expect("the wide raster");
        assert_eq!((read.width, read.height), (WIDE_RASTER_WIDTH, 5461));
        assert!(read.data().iter().all(|&v| v == 7));
        // The open frame's raster, from a picture of the frame's shape.
        let made = ai
            .raster_for(
                (full.0 as usize, full.1 as usize),
                1,
                &WorkingImage::new(3, 4),
                &Edit::default(),
                crate::finish::Source::SCENE,
                0,
                (3, 0),
                &sky,
                None,
            )
            .expect("the cached wide raster");
        assert_eq!(made.raster.width, WIDE_RASTER_WIDTH);
        assert!(made.provider.is_none());
        let _ = std::fs::remove_dir_all(&dir);
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
            let _ = std::fs::remove_dir_all(&dir);
            let mut ai = Ai::new();
            ai.store = Some(Store::at(dir.join("models")));
            let file = PathBuf::from("frame.RAF");
            ai.file = Some(file.clone());
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
                    1,
                    &WorkingImage::new(picture.0, picture.1),
                    &Edit::default(),
                    crate::finish::Source::SCENE,
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
                .cached_raster(&file, &sky, 0, (frame.0 as u32, frame.1 as u32))
                .expect("the reduced develop reads it");
            assert_eq!(read.height, rows);
            let _ = std::fs::remove_dir_all(&dir);
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
        let kind = crate::finish::Source::SCENE;
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
        let _ = std::fs::remove_dir_all(&dir);
        let mut ai = Ai::new();
        ai.store = Some(Store::at(dir.join("models")));
        ai.file = Some(PathBuf::from("square.CR3"));
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
                1,
                &WorkingImage::new(4, 4),
                &Edit::default(),
                crate::finish::Source::SCENE,
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
        let _ = std::fs::remove_dir_all(&dir);
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
        let _ = std::fs::remove_dir_all(&dir);
        let mut ai = Ai::for_test(Store::at(dir.join("models")), vec![Provider::Cpu]);
        ai.file = Some(PathBuf::from("coast.CR3"));
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
        let kind = crate::finish::Source::SCENE;
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
            .raster(1, &image, &edit, kind, 0, (1, 0), &water, None)
            .expect("the water");
        assert!(made.note.is_none());
        let data = made.raster.data();
        let height = data.len() / RASTER_WIDTH;
        let at = |y: usize| data[y * RASTER_WIDTH + RASTER_WIDTH / 2];
        assert!(at(height * 7 / 8) > 240 && at(height / 16) < 15);
        let building = Shape::of_kind("Building");
        for _ in 0..2 {
            let made = ai
                .raster(1, &image, &edit, kind, 0, (1, 1), &building, None)
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
            .raster(1, &image, &edit, kind, 0, (2, 0), &sky, None)
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
            .raster(1, &image, &edit, kind, 0, (2, 1), &picked, None)
            .expect("a sky with a pick");
        assert!(
            made.raster.data().iter().any(|&v| v != 128),
            "made, not the cut"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        let mut slots: Vec<PathBuf> = ["Water", "Foliage", "Mountain", "Building", "Sky"]
            .into_iter()
            .map(|k| ai.cached_path(&Shape::of_kind(k), None, 0).expect("a slot"))
            .collect();
        slots.sort();
        slots.dedup();
        assert_eq!(slots.len(), 5);
    }
}
