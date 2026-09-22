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

use std::path::Path;

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

pub struct Denoiser {
    loaded: Loaded,
    version: String,
}

impl Denoiser {
    /// Load the model at `path` on the first provider that runs it.
    /// Nothing here is registered, so no provider's answer for it is
    /// remembered; use [`Denoiser::from_store`] for that.
    pub fn load(path: &Path, providers: &[Provider]) -> Result<Self> {
        Self::load_inner(path, providers, None)
    }

    fn load_inner(
        path: &Path,
        providers: &[Provider],
        remember: Option<runtime::Remembered<'_>>,
    ) -> Result<Self> {
        let loaded = runtime::open(
            path,
            GraphOptimizationLevel::Level3,
            providers,
            remember,
            |s| {
                let zeros = Tensor::from_array(([1usize, 4, 32, 32], vec![0.0f32; 4 * 32 * 32]))?;
                s.run(ort::inputs!["packed" => zeros])?;
                Ok(())
            },
        )?;
        let version = loaded
            .session
            .metadata()
            .ok()
            .and_then(|m| m.custom("greycard.version"))
            .unwrap_or_default();
        Ok(Self { loaded, version })
    }

    /// A registered tier from the store; [`Error::Missing`] when it
    /// has not been fetched.
    pub fn from_store(store: &Store, model: &Model, providers: &[Provider]) -> Result<Self> {
        if !store.have(model) {
            return Err(Error::Missing(model.name));
        }
        let remember = runtime::Remembered {
            store_root: store.root(),
            model: model.id,
            hash: model.files[0].sha256,
        };
        Self::load_inner(
            &store.path(model, &model.files[0]),
            providers,
            Some(remember),
        )
    }

    pub fn provider(&self) -> Provider {
        self.loaded.provider
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
        let tile = tiling.tile.max(32) & !1;
        let margin = tiling.margin.max(2) & !1;
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
                        packed[plane * half * half + (yy / 2) * half + xx / 2] = vst[c].forward(v);
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
