//! The learned denoiser: greycard's own network, which replaces the
//! demosaic and the profiled denoiser together. Design and training in
//! the notes, §37; the training code is `tools/denoise`.
//!
//! The contract with the exported model: input `packed` is `[1, 4, h, w]`
//! float32, the mosaic in the variance-stabilized space of the frame's
//! noise model (`greycard_core::develop::denoise::Vst`), packed as the
//! four Bayer positions R, G1, G2, B at half resolution; output `rgb` is
//! `[1, 3, 2h, 2w]` float32, RGB in the same stabilized space, per
//! channel, at the mosaic's resolution. Height and width are free.
//!
//! A frame is run in tiles with a margin, mirrored at the edges in a way
//! that keeps the Bayer phase (a reflection about a sample, not about an
//! edge), and any 2x2 pattern is presented to the network as RGGB by
//! starting the tiles where the pattern reads that way.

use std::path::{Path, PathBuf};

use greycard_core::develop::denoise::Vst;
use greycard_core::develop::noise::NoiseModel;
use greycard_core::raw::CfaPattern;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use crate::registry::Model;
use crate::runtime::{self, Error, Loaded, Provider, Result};
use crate::store::Store;

/// Mosaic samples on a tile's side, before the margin. The network's
/// working memory is the tile's area times its width: at 1536 (864 x
/// 864 packed) the 64-wide model took 6.4 GB of VRAM and the 96-wide
/// one more than an 8 GB card has beside a browser, and the WebGPU
/// provider answers an allocation it cannot make with garbage on one
/// device and a crash on another, never an error. At 1024 the three
/// shipped models take 2 to 5 GB; the price is 11 percent more margin
/// to compute.
pub const DEFAULT_TILE: usize = 1024;

/// The tile a WebGPU adapter with less memory is given: with the
/// margin, 704 samples a side against the default's 1216, about a
/// third of its working memory, at the price of more margin to
/// compute.
pub const SMALL_TILE: usize = 512;

/// The bytes a discrete card needs for [`DEFAULT_TILE`]: 6 GiB, less
/// the little a 6 GB card reports under its sticker. The best tier
/// peaks at 4.2 GB at 1024 and a card has other tenants.
const CARD_FROM: u64 = 11 << 29;

/// The bytes a machine on unified memory needs for [`DEFAULT_TILE`]:
/// 16 GB, which a system can report a GiB under, as the remove's
/// bounds allow for; under that the tile's working memory takes the
/// machine into swap.
const UNIFIED_FROM: u64 = 15 << 30;

/// A tile's answer is checked against its input: in the stabilized
/// space the noise is unit variance, so a denoised sample sits within a
/// unit or so of the noisy one (measured: 0.2 to 0.7 on average over a
/// tile, at every noise level), and an answer this far off on average,
/// plus a twentieth of the tile's own range for what a demosaic
/// legitimately moves between neighbors, is not a picture. What a
/// failed allocation returned measured 5.6.
const PLAUSIBLE: f32 = 3.0;
const PLAUSIBLE_RANGE: f32 = 0.05;
/// Mosaic samples of margin on every side of a tile: more than the
/// network's effective reach, so seams do not show.
pub const DEFAULT_MARGIN: usize = 96;

/// The read-noise variance a silent model is given: a sigma of 1e-4,
/// well under any sensor's.
const SILENT_B: f32 = 1e-8;

/// How to cut a frame for the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tiling {
    pub tile: usize,
    pub margin: usize,
}

impl Default for Tiling {
    fn default() -> Self {
        Self {
            tile: DEFAULT_TILE,
            margin: DEFAULT_MARGIN,
        }
    }
}

/// The packed side of every tile at the default tiling, margins and
/// edge tiles included (the mirror fills those to size): what the
/// model's free `h` and `w` are fixed to on Core ML.
pub const FIXED_SIDE: usize = (DEFAULT_TILE + 2 * DEFAULT_MARGIN) / 2;

impl Tiling {
    /// The tile and margin `run` uses: even, and not below 32 and 2.
    fn even(self) -> (usize, usize) {
        (self.tile.max(32) & !1, self.margin.max(2) & !1)
    }

    /// The packed side of every tile this tiling cuts.
    pub fn packed_side(self) -> usize {
        let (tile, margin) = self.even();
        (tile + 2 * margin) / 2
    }
}

/// The tile the denoiser runs at on `provider`, and why, for the log:
/// what [`Denoiser::load`] and [`Denoiser::from_store`] choose, from
/// what this machine says ([`crate::ram::physical`] and the WebGPU
/// probe); [`Denoiser::load_tiled`] runs the tiling it is handed.
/// Only WebGPU's is chosen: from the card's own memory (`card`) on a
/// discrete adapter, from the machine's (`machine`) on unified memory
/// ([`runtime::webgpu_unified`]), [`DEFAULT_TILE`] where the memory
/// is not known, as before there was a choice. `env` is
/// `GREYCARD_DENOISE_TILE`, a tile in mosaic samples, over the choice
/// on any provider but Core ML, whose session takes the default's
/// shape only.
pub fn choose_tile(
    provider: Provider,
    unified: Option<bool>,
    card: Option<u64>,
    machine: Option<u64>,
    env: Option<&str>,
) -> (usize, String) {
    const GIB: f64 = (1u64 << 30) as f64;
    if provider == Provider::CoreMl {
        return (
            DEFAULT_TILE,
            "Core ML, whose session takes this tile only".to_string(),
        );
    }
    let asked = env.map(|v| (v, v.trim().parse::<usize>().ok().filter(|&t| t > 0)));
    if let Some((v, Some(tile))) = asked {
        return (tile, format!("GREYCARD_DENOISE_TILE={v}"));
    }
    let (tile, mut why) = if provider != Provider::WebGpu {
        (DEFAULT_TILE, format!("on {}", provider.label()))
    } else {
        match (unified, card, machine) {
            (Some(true), _, Some(m)) => {
                let tile = if m >= UNIFIED_FROM {
                    DEFAULT_TILE
                } else {
                    SMALL_TILE
                };
                // To the nearest GiB, as a machine's memory is sold.
                (tile, format!("{} GiB unified", (m + (1 << 29)) >> 30))
            }
            (Some(true), _, None) => (DEFAULT_TILE, "unified, its memory not known".to_string()),
            (_, Some(c), _) => {
                let tile = if c >= CARD_FROM {
                    DEFAULT_TILE
                } else {
                    SMALL_TILE
                };
                (tile, format!("the card's {:.1} GiB", c as f64 / GIB))
            }
            (_, None, _) => (DEFAULT_TILE, "the card's memory not known".to_string()),
        }
    };
    if let Some((v, None)) = asked {
        why.push_str(&format!(", GREYCARD_DENOISE_TILE={v} not a tile"));
    }
    (tile, why)
}

pub struct Denoiser {
    loaded: Loaded,
    version: String,
    /// The tiling chosen for the provider the model settled on.
    tiling: Tiling,
    /// The packed side the session takes, where it takes one only
    /// (Core ML).
    fixed: Option<usize>,
    /// Whether the session runs the tier's fp16 file: Core ML handed
    /// it ([`Denoiser::from_store`]) and kept.
    fp16: bool,
    /// The session on the card, as the provider record names it: a
    /// device lost while it runs puts it off the card
    /// ([`runtime::HELD`]).
    on_card: Option<runtime::OnCard>,
}

/// The variable that keeps Core ML on a tier's own fp32 file when
/// set to anything but `0` or empty: for comparing the two on one
/// Mac, or a frame the fp16 file is suspected on.
pub const FP32_VAR: &str = "GREYCARD_DENOISE_FP32";

fn fp32_wanted(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

/// Whether [`FP32_VAR`] keeps Core ML on the tier's own file this run.
pub fn fp32_refused() -> bool {
    fp32_wanted(std::env::var(FP32_VAR).ok().as_deref())
}

/// The file Core ML opens for a tier and the hash its load is
/// remembered under: the tier's fp16 file, here as `fp16`, where Core
/// ML is `offered` and has not `failed` that file on record, under
/// both files' hashes; otherwise the tier's own file, under its own.
/// So a Mac whose Core ML cannot run the fp16 file opens the fp32 one
/// there again from the next load, as it did before the fp16 file was
/// published, rather than leaving Core ML for good.
fn core_ml_plan(
    own: &str,
    fp16: Option<(&crate::registry::File, PathBuf)>,
    offered: bool,
    failed: impl FnOnce(&str) -> bool,
) -> (Option<PathBuf>, String) {
    if let Some((file, path)) = fp16.filter(|_| offered) {
        let both = format!("{own}+{}", file.sha256);
        if !failed(&both) {
            return (Some(path), both);
        }
        log::info!(
            "Core ML failed {} at an earlier load; it opens the tier's own file",
            file.name
        );
    }
    (None, own.to_string())
}

impl Denoiser {
    /// Load the model at `path` on the first provider that runs it,
    /// its [`Denoiser::tiling`] chosen for that provider
    /// ([`choose_tile`]) and logged. Nothing here is registered, so no
    /// provider's answer for it is remembered; use
    /// [`Denoiser::from_store`] for that.
    pub fn load(path: &Path, providers: &[Provider]) -> Result<Self> {
        Self::load_inner(path, None, providers, None, Tiling::default()).map(Self::choose)
    }

    /// [`Denoiser::load`] for runs at `tiling`, as given: nothing is
    /// chosen, and on Core ML the session takes that tiling's tile
    /// only. For a caller measuring one tiling against another.
    pub fn load_tiled(path: &Path, providers: &[Provider], tiling: Tiling) -> Result<Self> {
        Self::load_inner(path, None, providers, None, tiling)
    }

    fn load_inner(
        path: &Path,
        core_ml: Option<&Path>,
        providers: &[Provider],
        remember: Option<runtime::Remembered<'_>>,
        tiling: Tiling,
    ) -> Result<Self> {
        let packed = tiling.packed_side();
        // With Core ML on offer every provider warms up on a whole
        // tile, so the timing `open` makes between them is a tile's.
        let warm = if providers.contains(&Provider::CoreMl) {
            packed
        } else {
            32
        };
        let side = packed as i64;
        let loaded = runtime::open_fixed(
            path,
            core_ml,
            GraphOptimizationLevel::Level3,
            providers,
            remember,
            &[("h", side), ("w", side)],
            |s| {
                let zeros =
                    Tensor::from_array(([1usize, 4, warm, warm], vec![0.0f32; 4 * warm * warm]))?;
                s.run(ort::inputs!["packed" => zeros])?;
                Ok(())
            },
        )?;
        let fixed = (loaded.provider == Provider::CoreMl).then_some(packed);
        let fp16 = core_ml.is_some() && loaded.provider == Provider::CoreMl;
        let on_card = runtime::OnCard::of(remember, loaded.provider, providers);
        let version = loaded
            .session
            .metadata()
            .ok()
            .and_then(|m| m.custom("greycard.version"))
            .unwrap_or_default();
        Ok(Self {
            loaded,
            version,
            fixed,
            tiling,
            fp16,
            on_card,
        })
    }

    /// A registered tier from the store; [`Error::Missing`] when it
    /// has not been fetched. Its tiling is chosen as
    /// [`Denoiser::load`]'s is. Core ML opens the tier's fp16 file
    /// where the store has it ([`Store::core_ml_path`]), and the tier's
    /// own file where it does not; the other providers always open the
    /// tier's own. What is remembered of a load is kept under both
    /// files' hashes when the fp16 file is in it, so neither load
    /// settles on the other's record.
    ///
    /// Any tier is refused ([`Error::OffCard`]) while any learned
    /// denoiser is on the off-card record for the card this launch runs
    /// on ([`runtime::denoisers_off_here`]): not loaded there, and not on
    /// the CPU after it (§646 cut that rung). Every caller passes here:
    /// the window's develop, an export, a merge, and the CLI.
    pub fn from_store(store: &Store, model: &Model, providers: &[Provider]) -> Result<Self> {
        Self::from_store_gated(store, model, providers, |root| {
            runtime::denoisers_off_here(root, providers)
        })
    }

    /// [`Denoiser::from_store`] with the off-card gate's answer for the
    /// store's root given (`off`), so a test names the card.
    fn from_store_gated(
        store: &Store,
        model: &Model,
        providers: &[Provider],
        off: impl FnOnce(&Path) -> bool,
    ) -> Result<Self> {
        if off(store.root()) {
            log::info!(
                "{}: a learned denoiser is on record as off this card; not loaded ({})",
                model.name,
                runtime::off_card_path(store.root()).display()
            );
            return Err(Error::OffCard);
        }
        if !store.have(model) {
            return Err(Error::Missing(model.name));
        }
        let fp32 = fp32_refused();
        if fp32 {
            log::info!("{FP32_VAR} is set: Core ML opens the tier's fp32 file");
        }
        let fp16 = crate::registry::core_ml_file(model).zip(store.core_ml_path(model));
        let (core_ml, hash) = core_ml_plan(
            model.files[0].sha256,
            fp16,
            providers.contains(&Provider::CoreMl) && !fp32,
            |both| runtime::remembered_failure(store.root(), both, Provider::CoreMl, providers),
        );
        let remember = runtime::Remembered {
            store_root: store.root(),
            model: model.id,
            hash: &hash,
        };
        Self::load_inner(
            &store.path(model, &model.files[0]),
            core_ml.as_deref(),
            providers,
            Some(remember),
            Tiling::default(),
        )
        .map(Self::choose)
    }

    /// The tile chosen for the provider the session settled on, and
    /// logged. The session was opened at the default tiling, the shape
    /// Core ML is fixed to; another provider runs whatever tile it is
    /// handed.
    fn choose(mut self) -> Self {
        let provider = self.provider();
        let webgpu = provider == Provider::WebGpu;
        let unified = webgpu.then(runtime::webgpu_unified).flatten();
        let env = std::env::var("GREYCARD_DENOISE_TILE").ok();
        let (tile, why) = choose_tile(
            provider,
            unified,
            webgpu.then(runtime::webgpu_memory).flatten(),
            (unified == Some(true)).then(crate::ram::physical).flatten(),
            env.as_deref(),
        );
        log::info!("denoiser tile {tile}: {why}");
        self.tiling.tile = tile;
        self
    }

    pub fn provider(&self) -> Provider {
        self.loaded.provider
    }

    /// Where it runs, for a person: the provider's label, and on Core
    /// ML whether the tier's fp16 file is what runs.
    pub fn label(&self) -> &'static str {
        if self.fp16 {
            "CoreML (fp16)"
        } else {
            self.provider().label()
        }
    }

    /// Hold its runs as under way on the card `on` names, whatever it
    /// was loaded on: for a test of the hold, which has no card.
    #[doc(hidden)]
    pub fn hold_on_card(&mut self, on: runtime::OnCard) {
        self.on_card = Some(on);
    }

    /// Whether the session runs the tier's fp16 file.
    pub fn fp16(&self) -> bool {
        self.fp16
    }

    /// The tiling to [`Denoiser::run`] at: chosen for its provider, or
    /// the one [`Denoiser::load_tiled`] was handed.
    pub fn tiling(&self) -> Tiling {
        self.tiling
    }

    /// The version the model file declares, for the edit's record.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Demosaic and denoise a Bayer mosaic. `samples` are one per pixel
    /// with the white balance applied, as the engine's demosaic takes
    /// them; `model` is the frame's noise model transformed for those
    /// gains. Returns interleaved RGB, one triple per pixel, in the
    /// samples' units. Samples below zero are read as zero.
    pub fn run(
        &mut self,
        samples: &[f32],
        width: usize,
        height: usize,
        pattern: &CfaPattern,
        model: &NoiseModel,
        tiling: Tiling,
    ) -> Result<Vec<f32>> {
        if samples.len() != width * height {
            return Err(Error::Shape(format!(
                "{width}x{height} mosaic with {} samples",
                samples.len()
            )));
        }
        let (dx, dy) = rggb_offset(pattern)
            .ok_or_else(|| Error::Shape(format!("not a 2x2 Bayer pattern: {pattern}")))?;
        if width < 4 || height < 4 {
            return Err(Error::Shape(format!("{width}x{height} is too small")));
        }
        let (tile, margin) = tiling.even();
        // A silent channel (a = b = 0) has no transform; give it the
        // faintest read noise so the network sees a finite picture.
        let vst: [Vst; 3] = std::array::from_fn(|c| {
            if model.a[c] <= 0.0 && model.b[c] <= 0.0 {
                Vst::new(0.0, SILENT_B)
            } else {
                Vst::of(model, c)
            }
        });
        let mirror = Mirror { width, height };

        let mut out = vec![0.0f32; width * height * 3];
        // Tile origins in the RGGB frame: even there, so they start at
        // an R; the frame's origin is `(dx, dy)` in sample coordinates,
        // and the first tile starts one block before it to cover the
        // samples on the near side of the origin.
        let side = tile + 2 * margin;
        let half = side / 2;
        if let Some(fixed) = self.fixed.filter(|&f| f != half) {
            return Err(Error::Shape(format!(
                "a {half}x{half} tile on {}, whose session takes {fixed}x{fixed} only",
                self.loaded.provider.name()
            )));
        }
        // The tile it runs at, which the card's free memory may have
        // made smaller than the one chosen at the load.
        log::info!(
            "denoiser runs {width}x{height} at a tile of {tile} on {}",
            self.label()
        );
        // Held as under way on the card while it runs, so a device lost
        // meanwhile, or the run failing on one, puts this model off it.
        let on_card = self.on_card.clone();
        runtime::HELD.run(on_card.as_ref(), || {
            let mut ty = -2 * dy as isize;
            while ty < height as isize {
                let mut tx = -2 * dx as isize;
                while tx < width as isize {
                    // The tile's input window, in sample coordinates.
                    let y_in = ty + dy as isize - margin as isize;
                    let x_in = tx + dx as isize - margin as isize;
                    let mut packed = vec![0.0f32; 4 * half * half];
                    for yy in 0..side {
                        let sy = mirror.y(y_in + yy as isize);
                        for xx in 0..side {
                            let sx = mirror.x(x_in + xx as isize);
                            // The training data is what `normalize_levels`
                            // makes: nothing below black. A consumer that
                            // puts on noise of its own must not hand the
                            // network what it never saw.
                            let v = samples[sy * width + sx].max(0.0);
                            let (c, plane) = match (yy & 1, xx & 1) {
                                (0, 0) => (0, 0),
                                (0, _) => (1, 1),
                                (_, 0) => (1, 2),
                                _ => (2, 3),
                            };
                            packed[plane * half * half + (yy / 2) * half + xx / 2] =
                                vst[c].forward(v);
                        }
                    }
                    // The tile's input stays for the check on its answer.
                    let input = Tensor::from_array(([1usize, 4, half, half], packed.clone()))?;
                    let outputs = self.loaded.session.run(ort::inputs!["packed" => input])?;
                    let (shape, rgb) = outputs["rgb"].try_extract_tensor::<f32>()?;
                    if rgb.len() != 3 * side * side {
                        return Err(Error::Shape(shape.to_string()));
                    }
                    if let Some(why) = implausible(&packed, rgb, side, margin) {
                        return Err(Error::Implausible(why));
                    }
                    // Write the tile's core: samples the margin does not cover.
                    for yy in margin..side - margin {
                        let sy = y_in + yy as isize;
                        if sy < 0 || sy >= height as isize {
                            continue;
                        }
                        for xx in margin..side - margin {
                            let sx = x_in + xx as isize;
                            if sx < 0 || sx >= width as isize {
                                continue;
                            }
                            let o = (sy as usize * width + sx as usize) * 3;
                            for c in 0..3 {
                                out[o + c] =
                                    vst[c].inverse_clean(rgb[c * side * side + yy * side + xx]);
                            }
                        }
                    }
                    tx += tile as isize;
                }
                ty += tile as isize;
            }
            Ok(out)
        })
    }
}

/// Why a tile's answer cannot be right, if it cannot: a value that is
/// not finite, or a core whose samples sit further from the input than
/// any denoiser moves them. `packed` is the tile's input, four planes
/// of `side / 2` square; `rgb` its answer, three planes of `side`.
fn implausible(packed: &[f32], rgb: &[f32], side: usize, margin: usize) -> Option<String> {
    let half = side / 2;
    let mut sum = 0.0f64;
    let mut n = 0usize;
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for yy in margin..side - margin {
        for xx in margin..side - margin {
            let (c, plane) = match (yy & 1, xx & 1) {
                (0, 0) => (0, 0),
                (0, _) => (1, 1),
                (_, 0) => (1, 2),
                _ => (2, 3),
            };
            let input = packed[plane * half * half + (yy / 2) * half + xx / 2];
            let out = rgb[c * side * side + yy * side + xx];
            if !out.is_finite() {
                return Some(format!("a value of {out} at ({xx}, {yy})"));
            }
            sum += (out - input).abs() as f64;
            n += 1;
            lo = lo.min(input);
            hi = hi.max(input);
        }
    }
    let mean = sum / n.max(1) as f64;
    let bound = PLAUSIBLE as f64 + PLAUSIBLE_RANGE as f64 * (hi - lo).max(0.0) as f64;
    (mean > bound).then(|| {
        format!("a tile whose answer sits {mean:.1} stabilized units from its input on average")
    })
}

/// The offset within the 2x2 block at which `pattern` reads RGGB.
pub fn rggb_offset(pattern: &CfaPattern) -> Option<(usize, usize)> {
    let rggb = CfaPattern::rggb();
    (0..2)
        .flat_map(|dy| (0..2).map(move |dx| (dx, dy)))
        .find(|&(dx, dy)| pattern.shifted(dx, dy) == rggb)
}

/// Reflection about the edge samples, which keeps the Bayer phase:
/// `-1` reads `1`, `n` reads `n - 2`.
struct Mirror {
    width: usize,
    height: usize,
}

impl Mirror {
    fn fold(i: isize, n: usize) -> usize {
        let period = 2 * (n as isize - 1);
        let mut i = i.rem_euclid(period);
        if i >= n as isize {
            i = period - i;
        }
        i as usize
    }

    fn x(&self, i: isize) -> usize {
        Self::fold(i, self.width)
    }

    fn y(&self, i: isize) -> usize {
        Self::fold(i, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::raw::CfaColor;

    /// The nine pairs at `from_store`, the load every caller passes: any tier asked
    /// is refused while any tier is on the off-card record for this
    /// card; with nothing there, the load goes on (to its own error
    /// here: the store holds no files).
    #[test]
    fn every_tier_asked_is_refused_while_any_is_off_the_card() {
        use crate::registry::{DENOISE_BALANCED, DENOISE_BEST, DENOISE_FAST};
        let tiers = [&DENOISE_FAST, &DENOISE_BALANCED, &DENOISE_BEST];
        let _flag = runtime::flag_lock();
        runtime::set_test_here(Some(("Test GPU|Vulkan|1.2.3", "b")));
        for recorded in tiers {
            let root = std::env::temp_dir().join(format!(
                "greycard-denoise-off-{}-{}",
                recorded.id,
                std::process::id()
            ));
            greycard_core::testing::remove_dir_retry(&root);
            let store = Store::at(&root);
            // The real load, its own gate: the card named for this
            // thread, as a launch would name its own.
            let load = |asked: &Model| Denoiser::from_store(&store, asked, &[Provider::Cpu]);
            for asked in tiers {
                assert!(
                    matches!(load(asked), Err(Error::Missing(_))),
                    "nothing on record"
                );
            }
            runtime::Held::new().put_off(&[runtime::OnCard::new(
                &root,
                recorded.id,
                recorded.files[0].sha256,
                "Test GPU|Vulkan|1.2.3",
                "b",
            )]);
            for asked in tiers {
                assert!(
                    matches!(load(asked), Err(Error::OffCard)),
                    "{} asked, {} on record",
                    asked.id,
                    recorded.id
                );
            }
            runtime::forget_off_card(&root, &[]);
            greycard_core::testing::remove_dir_retry(&root);
        }
        runtime::set_test_here(None);
    }

    #[test]
    fn the_default_tiling_gives_the_fixed_side() {
        assert_eq!(Tiling::default().packed_side(), FIXED_SIDE);
        assert_eq!(FIXED_SIDE, 608);
        let small = Tiling {
            tile: 512,
            ..Default::default()
        };
        assert_eq!(small.packed_side(), 352);
        // An odd tile is cut even, as `run` cuts it.
        let odd = Tiling {
            tile: 513,
            margin: 7,
        };
        assert_eq!(odd.packed_side(), (512 + 12) / 2);
    }

    const GIB: u64 = 1 << 30;

    fn webgpu(unified: Option<bool>, card: Option<u64>, machine: Option<u64>) -> usize {
        choose_tile(Provider::WebGpu, unified, card, machine, None).0
    }

    #[test]
    fn a_discrete_card_under_six_gib_gets_the_small_tile() {
        let d = Some(false);
        assert_eq!(webgpu(d, Some(4 * GIB), None), SMALL_TILE);
        assert_eq!(webgpu(d, Some(CARD_FROM - 1), None), SMALL_TILE);
        assert_eq!(webgpu(d, Some(CARD_FROM), None), DEFAULT_TILE);
        assert_eq!(webgpu(d, Some(6 * GIB), None), DEFAULT_TILE);
        assert_eq!(webgpu(d, Some(16 * GIB), Some(8 * GIB)), DEFAULT_TILE);
        // A card's memory not known keeps the tile it always had, and
        // so does an adapter of no known type.
        assert_eq!(webgpu(d, None, Some(8 * GIB)), DEFAULT_TILE);
        assert_eq!(webgpu(None, None, None), DEFAULT_TILE);
        assert_eq!(webgpu(None, Some(4 * GIB), None), SMALL_TILE);
        let (_, why) = choose_tile(Provider::WebGpu, d, Some(4 * GIB), None, None);
        assert_eq!(why, "the card's 4.0 GiB");
        let (_, why) = choose_tile(Provider::WebGpu, d, None, None, None);
        assert_eq!(why, "the card's memory not known");
    }

    #[test]
    fn unified_memory_under_sixteen_gb_gets_the_small_tile() {
        let u = Some(true);
        // The machine's memory decides, not the share the driver sets
        // aside for the integrated GPU.
        assert_eq!(webgpu(u, Some(GIB), Some(8 * GIB)), SMALL_TILE);
        assert_eq!(webgpu(u, None, Some(UNIFIED_FROM - 1)), SMALL_TILE);
        assert_eq!(webgpu(u, None, Some(UNIFIED_FROM)), DEFAULT_TILE);
        assert_eq!(webgpu(u, Some(GIB), Some(16 * GIB)), DEFAULT_TILE);
        assert_eq!(webgpu(u, Some(GIB), None), DEFAULT_TILE);
        let (_, why) = choose_tile(Provider::WebGpu, u, None, Some(16 * GIB), None);
        assert_eq!(why, "16 GiB unified");
    }

    #[test]
    fn only_webgpu_has_its_tile_chosen() {
        for p in [Provider::Cpu, Provider::Cuda, Provider::CoreMl] {
            assert_eq!(
                choose_tile(p, Some(false), Some(2 * GIB), Some(4 * GIB), None).0,
                DEFAULT_TILE,
                "{p:?}"
            );
        }
    }

    #[test]
    fn the_environment_overrides_the_choice_but_not_core_ml() {
        let env = |p, v| choose_tile(p, Some(false), Some(4 * GIB), None, Some(v));
        assert_eq!(
            env(Provider::WebGpu, "1024"),
            (1024, "GREYCARD_DENOISE_TILE=1024".to_string())
        );
        assert_eq!(env(Provider::Cpu, "768").0, 768);
        assert_eq!(env(Provider::Cuda, "768").0, 768);
        assert_eq!(env(Provider::WebGpu, " 512 ").0, 512);
        assert_eq!(env(Provider::CoreMl, "512").0, DEFAULT_TILE);
        // What is not a tile is said and passed over.
        for v in ["", "0", "big", "-512"] {
            let (tile, why) = env(Provider::WebGpu, v);
            assert_eq!(tile, SMALL_TILE, "{v:?}");
            assert!(why.ends_with("not a tile"), "{why}");
        }
    }

    #[test]
    fn core_ml_takes_the_fp16_file_unless_refused_or_failed() {
        let file = crate::registry::core_ml_file(&crate::registry::DENOISE_BEST).unwrap();
        let path = PathBuf::from("/store/best/denoise-v20-fp16.onnx");
        let both = format!("own+{}", file.sha256);
        let plan = |fp16: Option<(&crate::registry::File, PathBuf)>, offered, failed: bool| {
            let mut asked = None;
            let out = core_ml_plan("own", fp16, offered, |h| {
                asked = Some(h.to_string());
                failed
            });
            (out, asked)
        };
        // Here, offered, never failed: the fp16 file, under both hashes,
        // the record asked under both.
        assert_eq!(
            plan(Some((file, path.clone())), true, false),
            ((Some(path.clone()), both.clone()), Some(both.clone()))
        );
        // Failed on record: the tier's own file, under its own hash.
        assert_eq!(
            plan(Some((file, path.clone())), true, true),
            ((None, "own".into()), Some(both))
        );
        // Not offered (no Core ML, or the variable set), or not here:
        // the tier's own, and the record not asked.
        assert_eq!(
            plan(Some((file, path)), false, false),
            ((None, "own".into()), None)
        );
        assert_eq!(plan(None, true, false), ((None, "own".into()), None));
    }

    #[test]
    fn the_fp32_variable_is_on_unless_empty_or_zero() {
        assert!(!fp32_wanted(None));
        assert!(!fp32_wanted(Some("")));
        assert!(!fp32_wanted(Some("0")));
        assert!(fp32_wanted(Some("1")));
        assert!(fp32_wanted(Some("yes")));
    }

    #[test]
    fn mirror_keeps_the_phase() {
        let m = Mirror {
            width: 6,
            height: 5,
        };
        assert_eq!(m.x(-1), 1);
        assert_eq!(m.x(-3), 3);
        assert_eq!(m.x(6), 4);
        assert_eq!(m.x(7), 3);
        assert_eq!(m.y(5), 3);
        for i in -8..12 {
            assert_eq!(m.x(i) % 2, i.rem_euclid(2) as usize, "{i}");
        }
    }

    #[test]
    fn a_plausible_tile_passes_and_garbage_does_not() {
        let side = 8;
        let half = side / 2;
        let packed = vec![3.0f32; 4 * half * half];
        // Every output within a unit of the input: a denoiser's answer.
        let rgb = vec![3.5f32; 3 * side * side];
        assert!(implausible(&packed, &rgb, side, 2).is_none());
        // Far away everywhere: an allocation that failed silently.
        let rgb = vec![3.0f32 + 2.0 * PLAUSIBLE; 3 * side * side];
        assert!(implausible(&packed, &rgb, side, 2).is_some());
        // One NaN in the core is enough: (3, 3) is a blue position, so
        // the blue plane is the one read there.
        let mut rgb = vec![3.5f32; 3 * side * side];
        rgb[2 * side * side + 3 * side + 3] = f32::NAN;
        assert!(implausible(&packed, &rgb, side, 2).is_some());
        // A NaN in the margin is not the core's business.
        let mut rgb = vec![3.5f32; 3 * side * side];
        rgb[0] = f32::NAN;
        assert!(implausible(&packed, &rgb, side, 2).is_none());
    }

    #[test]
    fn rggb_offset_finds_each_phase() {
        let rggb = CfaPattern::rggb();
        assert_eq!(rggb_offset(&rggb), Some((0, 0)));
        assert_eq!(rggb_offset(&rggb.shifted(1, 0)), Some((1, 0)));
        assert_eq!(rggb_offset(&rggb.shifted(0, 1)), Some((0, 1)));
        assert_eq!(rggb_offset(&rggb.shifted(1, 1)), Some((1, 1)));
        let cmy = CfaPattern::new(2, 2, vec![CfaColor::Other(1); 4]).unwrap();
        assert_eq!(rggb_offset(&cmy), None);
    }
}
