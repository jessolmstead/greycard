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
use greycard_ai::sky::{self, Prior};
use greycard_ai::{
    Denoiser, Fill, Model, Prompt, Provider, Rgb8, Rgbf, SAM, SKY, Sam, Sky, Store, Subject, refine,
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
        Shape::Subject {} => Some(greycard_ai::subject::pick(
            providers.contains(&Provider::WebGpu),
            have,
            |m| unavailable.contains(&m.id),
            original_failed_on_webgpu,
        )),
        Shape::Sky { .. } => Some(sky_model(have, unavailable)),
        Shape::Object { .. } => Some(&SAM),
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

/// Whether the shape has anything for a model to go on.
pub fn prompted(shape: &Shape) -> bool {
    match shape {
        Shape::Subject {} | Shape::Sky { .. } => true,
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
    /// What has been made, with the shape it was made for.
    cache: HashMap<Key, (Shape, Arc<Raster>)>,
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
    /// on a frame with no sky, whose raster is empty.
    pub note: Option<String>,
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
            cache: HashMap::new(),
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
    }

    /// A Subject model just arrived in the store: drop the one
    /// loaded, if any, so the next Subject mask picks up the new
    /// file rather than reusing the session's first choice forever.
    pub fn forget_subject(&mut self) {
        self.subject = None;
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
    fn cached_path(&self, shape: &Shape, model: Option<&'static Model>) -> Option<PathBuf> {
        let file = self.file.as_ref()?;
        let model = match shape {
            Shape::Subject {} => self.subject_model(model)?,
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
        key: Key,
        shape: &Shape,
        model: Option<&'static Model>,
    ) -> Result<Made, String> {
        if let Some((s, r)) = self.cache.get(&key)
            && s == shape
        {
            return Ok(Made {
                note: self.sky_note(shape, r),
                raster: r.clone(),
                provider: None,
                seconds: 0.0,
            });
        }
        if !prompted(shape) {
            return Err("nothing picked yet".into());
        }
        let aspect = image.height as f32 / image.width as f32;
        let height = ((RASTER_WIDTH as f32 * aspect).round() as usize).max(1);
        let cached = self.cached_path(shape, model);
        if let Some(data) = cached.as_deref().and_then(|p| read_raster(p, height)) {
            let raster = Arc::new(Raster::from_data(aspect, RASTER_WIDTH, data));
            self.cache.insert(key, (shape.clone(), raster.clone()));
            return Ok(Made {
                note: self.sky_note(shape, &raster),
                raster,
                provider: None,
                seconds: 0.0,
            });
        }
        let store = self
            .store
            .clone()
            .ok_or("no cache directory for the models")?;
        if self.providers.is_empty() {
            self.providers = Provider::available();
        }
        if self.preview.as_ref().is_none_or(|p| p.0 != stamp) {
            let rgb = preview(image, edit, kind);
            let luma = rgb.luma();
            self.preview = Some((stamp, rgb, luma));
            self.embedding = None;
        }
        if let Shape::Sky { picks } = shape {
            let start = Instant::now();
            let (matte, provider) =
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
            self.cache.insert(key, (shape.clone(), raster.clone()));
            return Ok(Made {
                note: self.sky_note(shape, &raster),
                raster,
                provider: Some(provider),
                seconds: start.elapsed().as_secs_f64(),
            });
        }
        let (_, rgb, luma) = self.preview.as_ref().expect("a preview was just made");
        let start = Instant::now();
        let (mask, provider) = match shape {
            Shape::Subject {} => {
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
                (
                    subject.mask(rgb).map_err(|e| e.to_string())?,
                    subject.provider(),
                )
            }
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
        self.cache.insert(key, (shape.clone(), raster.clone()));
        Ok(Made {
            raster,
            provider: Some(provider),
            seconds: start.elapsed().as_secs_f64(),
            note: None,
        })
    }

    /// What the status line says of a Sky raster with nothing in it:
    /// the gate found no sky. Nothing for any other shape.
    fn sky_note(&self, shape: &Shape, raster: &Raster) -> Option<String> {
        if !matches!(shape, Shape::Sky { .. }) || raster.data().iter().any(|&v| v > 0) {
            return None;
        }
        let name = self
            .file
            .as_deref()
            .and_then(|f| f.file_name())
            .map_or("this picture".to_string(), |n| {
                n.to_string_lossy().into_owned()
            });
        Some(format!("no sky found in {name}"))
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
    ) -> Result<(Option<greycard_ai::Mask>, Provider), String> {
        let mut report = SkyReport::default();
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
        let matte = match found {
            sky::Found::Sky {
                matte,
                seeded,
                outline,
            } => {
                report.seeded = seeded;
                report.outline = Some(outline);
                Some(matte)
            }
            sky::Found::None(why) => {
                tracing::info!("no sky found in {name}: {why}");
                report.no_sky = Some(why);
                None
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
        Ok((matte, provider))
    }
}

/// What a model sees: the picture under the global look alone, no
/// geometry, no vignette or grain, in sRGB, no more than `PREVIEW`
/// on its long side.
pub(crate) fn preview(image: &WorkingImage, edit: &Edit, kind: crate::finish::Source) -> Rgb8 {
    let mut edit = edit.clone();
    edit.adjustments.clear();
    edit.geometry = Default::default();
    edit.vignette = Default::default();
    edit.grain = Default::default();
    let settings = crate::export::Settings {
        format: crate::export::Format::Jpeg,
        long_edge: Some(PREVIEW),
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
            .cached_path(&shape, Some(&greycard_ai::SUBJECT))
            .expect("a path with the original named");
        let rewrite_path = ai
            .cached_path(&shape, Some(&greycard_ai::SUBJECT_WEBGPU))
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
        let dir = std::env::temp_dir().join(format!(
            "greycard-ai-sky-cache-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ai = Ai::new();
        let store = Store::at(dir.join("models"));
        ai.store = Some(store.clone());
        ai.file = Some(PathBuf::from("DSCF0153.RAF"));
        let shape = Shape::Sky { picks: Vec::new() };
        let without_sam = ai.cached_path(&shape, None).expect("a path");
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
        assert_ne!(ai.cached_path(&shape, None), Some(without_sam));
        let _ = std::fs::remove_dir_all(&dir);
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
