//! The learned masks, on the worker: the models, the preview they see,
//! and the rasters they made. A mask is made once for a shape and kept
//! until the shape changes or another file is opened; it sees the base
//! develop as it was then, since a subject does not move when the
//! white balance does. Made rasters are also kept on disk, keyed by
//! the file, the model and the shape, so opening a file again does not
//! run the model again.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use greycard_ai::sam::Embedding;
use greycard_ai::sam3::{self, Choice, Encodings, Picture, Presets, Sam3};
use greycard_ai::sky::{self, Prior};
use greycard_ai::{
    Denoiser, Fill, Model, Prompt, Provider, Rgb8, Rgbf, SAM, SAM3, SKY, Sam, Sky, Store, Subject,
    refine,
};
use greycard_core::develop::retouch::Region;
use greycard_core::image::WorkingImage;
use greycard_edit::Edit;
use greycard_edit::brush::{RASTER_WIDTH, Raster};
use greycard_edit::mask::Shape;
use greycard_edit::retouch::Patch;

/// A learned component: its adjustment's id and its index in the mask.
pub type Key = (u64, usize);

/// The providers this build offers on this machine, asked once.
pub fn providers() -> &'static [Provider] {
    static PROVIDERS: std::sync::OnceLock<Vec<Provider>> = std::sync::OnceLock::new();
    PROVIDERS.get_or_init(Provider::available)
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

/// What a Sky raster was made from, beside the file and the shape, in
/// its disk cache's key: the stages' version, and whether SAM made the
/// outline, so a sky made without SAM is made again once SAM is in,
/// and one made by an older pipeline once the pipeline changes.
const SKY_PIPELINE: &str = "sky-1";

/// What a Part raster was made by beside the file and the shape, in
/// its disk cache's key: the routes' and the people's version, so a
/// part made by an older rule is made again.
const PART_PIPELINE: &str = "part-2";

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
    /// The sky prior of base develop `stamp`, kept like the embedding,
    /// so a pick on a Sky shape decodes again without running it.
    sky_prior: Option<(u64, Prior)>,
    /// What the last Sky raster took, stage by stage.
    pub(crate) last_sky: Option<SkyReport>,
    fill: Option<Fill>,
    /// The learned denoiser loaded, by its model's id: one at a time,
    /// since each holds the GPU's memory.
    denoiser: Option<(&'static str, Denoiser)>,
    /// Fills made, by patch id, with the patch each was made for.
    fills: HashMap<u64, (Patch, Vec<f32>)>,
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
    /// Whether made rasters are kept on disk (`cached_path`); never for
    /// a test that runs the models.
    disk: bool,
    /// The Subject matte at the raster's size, of base develop
    /// `stamp`, and the provider that made it: run once and read by a
    /// Background sharing it, so a mask with both costs the model one
    /// call, not two, whichever shape asks for it first. The provider
    /// is carried along so a hit here still says where its run
    /// happened and still turns Show mask on, as an actual run does.
    subject_matte: Option<(u64, Vec<u8>, Provider)>,
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

/// Why a fill was not made: the patch is left as it was either way.
#[derive(Debug, Clone)]
pub enum NoFill {
    /// The model is not in the store.
    Missing,
    /// It could not run.
    Failed(String),
}

/// Whether a kept fill was made for this patch as it is now: its
/// opacity is applied afterwards and does not count.
fn same_fill(a: &Patch, b: &Patch) -> bool {
    a.points == b.points && a.radius == b.radius && a.feather == b.feather && a.method == b.method
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
            sky_prior: None,
            last_sky: None,
            fill: None,
            denoiser: None,
            fills: HashMap::new(),
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
        }
    }

    /// The sky prior of the last develop a Sky raster was made on.
    #[cfg(test)]
    pub(crate) fn sky_prior(&self) -> Option<&Prior> {
        self.sky_prior.as_ref().map(|p| &p.1)
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
        self.preview = None;
        self.embedding = None;
        self.sky_prior = None;
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
    }

    /// Whether the fill model is in the store, so a fill not kept
    /// will be made rather than left.
    pub fn fill_available(&self) -> bool {
        self.store
            .as_ref()
            .is_some_and(|s| s.have(&greycard_ai::FILL))
    }

    /// Whether the patch's fill is made already and kept, so asking
    /// for it costs nothing.
    pub fn fill_kept(&self, patch: &Patch) -> bool {
        self.fills
            .get(&patch.id)
            .is_some_and(|(p, _)| same_fill(p, patch))
    }

    /// The region's window made up by the fill model from what is
    /// around it, in the picture's own values; or why not. Kept by
    /// patch, so a slider elsewhere does not run the model again.
    pub fn fill(
        &mut self,
        patch: &Patch,
        region: &Region,
        image: &WorkingImage,
    ) -> Result<Vec<f32>, NoFill> {
        if let Some((p, data)) = self.fills.get(&patch.id)
            && same_fill(p, patch)
        {
            return Ok(data.clone());
        }
        let store = self.store.clone().ok_or(NoFill::Missing)?;
        if !store.have(&greycard_ai::FILL) {
            return Err(NoFill::Missing);
        }
        if self.providers.is_empty() {
            self.providers = Provider::available();
        }
        let fill = match &mut self.fill {
            Some(f) => f,
            None => match Fill::load(&store, &self.providers) {
                Ok(f) => self.fill.insert(f),
                Err(e) => {
                    // The worker warns, with the patch's name.
                    tracing::debug!("fill model: {e}");
                    return Err(NoFill::Failed(e.to_string()));
                }
            },
        };
        // A square window round the region, twice its size for context,
        // within the picture.
        let side = (region.width.max(region.height) * 2)
            .max(64)
            .min(image.width.min(image.height));
        let cx = region.x + region.width / 2;
        let cy = region.y + region.height / 2;
        let wx = cx.saturating_sub(side / 2).min(image.width - side);
        let wy = cy.saturating_sub(side / 2).min(image.height - side);
        let to_srgb = crate::export::Space::Srgb.matrix();
        let from_srgb = invert(&to_srgb);
        // Coverage over the window, and the picture's values there.
        let mut hole = vec![0.0f32; side * side];
        let mut window = Vec::with_capacity(side * side * 3);
        let (mut luma_sum, mut luma_n) = (0.0f32, 0usize);
        for y in 0..side {
            for x in 0..side {
                let (px, py) = (wx + x, wy + y);
                let i = (py * image.width + px) * 3;
                let p = [image.data[i], image.data[i + 1], image.data[i + 2]];
                let s = mul(&to_srgb, p).map(|v| v.max(0.0));
                window.extend(s);
                let covered = px >= region.x
                    && px < region.x + region.width
                    && py >= region.y
                    && py < region.y + region.height
                    && region.cover[(py - region.y) * region.width + (px - region.x)] > 0.0;
                if covered {
                    hole[y * side + x] = 1.0;
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
        let encoded: Vec<f32> = window.iter().map(|v| encode(v * gain)).collect();
        let square = Rgbf::new(side, side, encoded)
            .resampled(greycard_ai::fill::SIZE, greycard_ai::fill::SIZE);
        let hole_square = greycard_ai::Mask::new(side, side, hole)
            .resampled(greycard_ai::fill::SIZE, greycard_ai::fill::SIZE)
            .data;
        let filled = match fill.fill(&square, &hole_square) {
            Ok(f) => f,
            Err(e) => {
                tracing::debug!("fill: {e}");
                return Err(NoFill::Failed(e.to_string()));
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
            png("fill_in.png", &square);
            png("fill_out.png", &filled);
            let hole8: Vec<u8> = hole_square.iter().map(|v| (v * 255.0) as u8).collect();
            if let Err(e) = image::save_buffer(
                dir.join("fill_hole.png"),
                &hole8,
                greycard_ai::fill::SIZE as u32,
                greycard_ai::fill::SIZE as u32,
                image::ExtendedColorType::L8,
            ) {
                tracing::warn!("dump fill_hole.png: {e}");
            }
        }
        let back = filled.resampled(side, side);
        // The region's window, decoded to the picture's values.
        let mut out = Vec::with_capacity(region.width * region.height * 3);
        for y in 0..region.height {
            for x in 0..region.width {
                let (px, py) = (region.x + x, region.y + y);
                let (bx, by) = (
                    px.saturating_sub(wx).min(side - 1),
                    py.saturating_sub(wy).min(side - 1),
                );
                let s = back.at(bx, by).map(|v| decode(v) / gain);
                out.extend(mul(&from_srgb, s));
            }
        }
        self.fills.insert(patch.id, (patch.clone(), out.clone()));
        Ok(out)
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
        if !self.disk {
            return None;
        }
        let file = self.file.as_ref()?;
        let model = match shape {
            Shape::Subject {} | Shape::Background {} => self.subject_model(model)?,
            Shape::Sky { .. } => &SKY,
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
        // A turned base is another picture to a raster: a half turn
        // keeps the height a cached one is checked by. Only hashed when
        // turned, so every raster made unturned keeps its slot.
        if !turn.is_multiple_of(4) {
            (turn % 4).hash(&mut h);
        }
        Some(root.join(format!("{:016x}.png", h.finish())))
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
        if let Some((s, r)) = self.cache.get(&(key, turn))
            && s == shape
        {
            return Ok(Made {
                note: self.sky_note(shape, r),
                raster: r.clone(),
                provider: None,
                seconds: 0.0,
                part: None,
            });
        }
        if !prompted(shape) {
            return Err("nothing picked yet".into());
        }
        let aspect = image.height as f32 / image.width as f32;
        let height = ((RASTER_WIDTH as f32 * aspect).round() as usize).max(1);
        let cached = self.cached_path(shape, model, turn);
        if let Some(data) = cached.as_deref().and_then(|p| read_raster(p, height)) {
            let raster = Arc::new(Raster::from_data(aspect, RASTER_WIDTH, data));
            self.cache
                .insert((key, turn), (shape.clone(), raster.clone()));
            return Ok(Made {
                note: self.sky_note(shape, &raster),
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
            let raster = Arc::new(Raster::from_data(aspect, RASTER_WIDTH, data));
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
            let (data, provider, part) = self.part(
                (stamp, turn),
                image,
                kind,
                (phrase, route, person.as_ref()),
                (RASTER_WIDTH, height),
                &store,
            )?;
            let raster = Arc::new(Raster::from_data(aspect, RASTER_WIDTH, data));
            // Only a settled person's part is kept: an ask is asked
            // again, so the panel can show it and an export can say it.
            if part.is_none() {
                if let Some(path) = &cached {
                    write_raster(path, height, raster.data());
                }
                self.cache
                    .insert((key, turn), (shape.clone(), raster.clone()));
            }
            let note = part.as_ref().map(|p| part_note(p, self.file.as_deref()));
            return Ok(Made {
                raster,
                provider: Some(provider),
                seconds: start.elapsed().as_secs_f64(),
                note,
                part,
            });
        }
        if let Shape::Sky { picks } = shape {
            let start = Instant::now();
            let (matte, provider, note) =
                self.sky_matte(stamp, image, picks, aspect, (RASTER_WIDTH, height), &store)?;
            let t = Instant::now();
            let data = match &matte {
                Some(m) => m.resampled(RASTER_WIDTH, height).to_u8(),
                None => vec![0u8; RASTER_WIDTH * height],
            };
            if let Some(report) = &mut self.last_sky {
                report.raster = t.elapsed().as_secs_f64();
            }
            if let Some(path) = &cached {
                write_raster(path, height, &data);
            }
            let raster = Arc::new(Raster::from_data(aspect, RASTER_WIDTH, data));
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
        let (_, rgb, luma) = self.preview.as_ref().expect("a preview was just made");
        let start = Instant::now();
        if matches!(shape, Shape::Subject {} | Shape::Background {}) {
            let subject = match &mut self.subject {
                Some(s) => s,
                None => {
                    let chosen = self
                        .subject_model(model)
                        .ok_or("no Subject model to load")?;
                    self.subject.insert(
                        Subject::load_model(&store, chosen, &self.providers)
                            .map_err(|e| e.to_string())?,
                    )
                }
            };
            let mask = subject.mask(rgb).map_err(|e| e.to_string())?;
            let provider = subject.provider();
            let radius = (rgb.width / 256).max(2);
            let refined = refine(&mask, luma, rgb.width, rgb.height, radius, 1e-3);
            dump(rgb, &mask, &refined);
            let base = refined.resampled(RASTER_WIDTH, height).to_u8();
            self.subject_matte = Some((stamp, base.clone(), provider));
            let data = background_or_not(shape, &base);
            if let Some(path) = &cached {
                write_raster(path, height, &data);
            }
            let raster = Arc::new(Raster::from_data(aspect, RASTER_WIDTH, data));
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
        let data = refined.resampled(RASTER_WIDTH, height).to_u8();
        if let Some(path) = &cached {
            write_raster(path, height, &data);
        }
        let raster = Arc::new(Raster::from_data(aspect, RASTER_WIDTH, data));
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
        };
        let found = people_of(sam, presets, &mut picture, people)?;
        let aspect = image.height as f32 / image.width as f32;
        let out = found
            .iter()
            .map(|p| Candidate::of(p, turn, aspect, &hash))
            .collect();
        Ok((out, sam.providers().0, start.elapsed().as_secs_f64()))
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
    fn part(
        &mut self,
        (stamp, turn): (u64, u8),
        image: &WorkingImage,
        kind: crate::finish::Source,
        (phrase, route, person): PartAsked,
        raster: (usize, usize),
        store: &Store,
    ) -> Result<(Vec<u8>, Provider, Option<Unresolved>), String> {
        let route = route_of(route)?;
        self.parts_model(store)?;
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
        // Said before any model runs: a phrase not in the table is
        // never asked of a text encoder.
        if presets.phrase(phrase).is_none() {
            return Err(format!("\"{phrase}\" is not in the People model's table"));
        }
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
        };
        let empty = || vec![0u8; raster.0 * raster.1];
        // The picture as it stands: its height over its width, the
        // masks' units' aspect.
        let aspect = image.height as f32 / image.width as f32;
        let who = match person {
            None => None,
            Some(p) => {
                let found = people_of(sam, presets, &mut picture, people)?;
                let candidates: Vec<Candidate> = found
                    .iter()
                    .map(|q| Candidate::of(q, turn, aspect, &hash))
                    .collect();
                let own = hash.is_some() && p.picture == hash;
                let vaspect = view.preview.height as f32 / view.preview.width as f32;
                let choice = settle(p, found, &candidates, own, turn, (aspect, vaspect));
                match decided(choice, candidates) {
                    Ok(i) => Some(found[i].clone()),
                    Err(why) => return Ok((empty(), provider, Some(why))),
                }
            }
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
        Ok((data, provider, None))
    }

    /// What the status line says of a Sky raster that comes back from
    /// a cache, in memory or on disk, with nothing in it: the gate
    /// found no sky. The raster is the only record a cache keeps, and
    /// an empty one means exactly that: once the gate passes, at least
    /// half a percent of the frame is labeled sky, the outline is never
    /// empty (the labels are its fallback), and so neither is the matte
    /// made from it. A raster made just now carries the gate's own word
    /// (`sky_matte`). Nothing for any other shape.
    fn sky_note(&self, shape: &Shape, raster: &Raster) -> Option<String> {
        if !matches!(shape, Shape::Sky { .. }) || raster.data().iter().any(|&v| v > 0) {
            return None;
        }
        Some(no_sky_note(self.file.as_deref()))
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
        let Ai {
            sky: loaded,
            sky_prior,
            sam,
            embedding,
            preview,
            providers,
            file,
            ..
        } = self;
        let (_, rgb, luma) = preview.as_ref().expect("a preview is made before a mask");
        if loaded.is_none() {
            let t = Instant::now();
            *loaded = Some(Sky::load(store, providers).map_err(|e| e.to_string())?);
            report.load = t.elapsed().as_secs_f64();
        }
        let model = loaded.as_mut().expect("the Sky model was just loaded");
        let provider = model.provider();
        report.prior_on = Some(provider);
        if sky_prior.as_ref().is_none_or(|p| p.0 != stamp) {
            let t = Instant::now();
            let prior = model.prior(rgb).map_err(|e| e.to_string())?;
            report.prior = t.elapsed().as_secs_f64();
            *sky_prior = Some((stamp, prior));
        }
        let prior = &sky_prior.as_ref().expect("the prior was just made").1;
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
        Ok((matte, provider, note))
    }
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
    let name = file
        .and_then(|f| f.file_name())
        .map_or("this picture".to_string(), |n| {
            n.to_string_lossy().into_owned()
        });
    format!("no sky found in {name}")
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
        // No masks, so no white of theirs.
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

/// A cached raster, if the file is there and is `RASTER_WIDTH` by
/// `height` (another height means another aspect: made for a
/// different develop of the file).
fn read_raster(path: &Path, height: usize) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?.into_luma8();
    if img.width() as usize != RASTER_WIDTH || img.height() as usize != height {
        return None;
    }
    Some(img.into_raw())
}

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
            RASTER_WIDTH as u32,
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
                crate::finish::Source::Scene,
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
                crate::finish::Source::Scene,
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
            crate::finish::Source::Scene,
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
                crate::finish::Source::Scene,
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
                crate::finish::Source::Scene,
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
                crate::finish::Source::Scene,
                0,
                (9, 0),
                &shape,
                None,
            )
            .expect("the kept raster");
        assert_eq!(again.note, made.note);
        let some = Raster::from_data(1.0, RASTER_WIDTH, vec![7u8; RASTER_WIDTH * RASTER_WIDTH]);
        assert_eq!(ai.sky_note(&shape, &some), None);
        assert_eq!(
            ai.sky_note(&Shape::Subject {}, &Raster::from_data(1.0, 4, vec![0; 16])),
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
        let kind = crate::finish::Source::Scene;
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
                crate::finish::Source::Scene,
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
}
