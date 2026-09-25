//! Sky: where the picture's sky is, and nothing at all where it has
//! none. A mask on a frame with no sky is worse than no mask, so the
//! first rule here is the gate, and every other stage only narrows
//! what the gate let through.
//!
//! The stages, each a function of its own with a CPU reference and a
//! test (notes, "AI masks: the model search, and a sky trial on 41
//! frames", and the Sky shape's own section):
//!
//! 1. **The prior** ([`Sky::prior`]): EoMT-S, COCO panoptic, labels
//!    every pixel of the preview among COCO's 133 classes. Each query
//!    is kept at a class probability of 0.5 or more, each pixel goes
//!    to the kept query with the highest class times mask probability
//!    ([`Prior::label`]), and the soft sky map is the sky-weighted sum
//!    over all the queries. Not the transformers panoptic
//!    post-processor, which gives the whole frame to a query that is
//!    alone above its threshold.
//! 2. **The gate** ([`gate`]): the model sure of its sky query (class
//!    probability 0.98 or more), a core it is sure of (probability over
//!    0.9) covering a quarter of a percent of the frame after an
//!    erosion of one percent of the long side, at least half a percent
//!    of the frame labeled sky, and that sky touching the top or a side
//!    of the frame. Fail any and there is no sky. Stricter than the
//!    trial's (a core over 0.8 surviving the erosion): on the editor's
//!    own preview a defocused bluish snow slope passed that, and these
//!    two reject it with a wide margin on the test set.
//! 3. **The outline** ([`seeds`], [`outline`]): SAM 2.1 seeded with
//!    up to eight points over the confident sky and eight negatives
//!    over the confident not-sky, one decode per positive with every
//!    negative, the union clipped to the prior's sky grown by 12 px
//!    at 2048. The gate passed and no seed survived the erosion: the
//!    prior's own labels.
//! 4. **The edge** ([`refine_sky`]): one function between the outline
//!    and the raster, so the edge solver can change without anything
//!    else moving: a color-line matte at the frame's own resolution in
//!    its linear values ([`crate::matte`]), brought to the raster's
//!    size by averaging.
//!
//! The trial's reference is Python (the prior, the gate, the seeding
//! and the clip); what differs here and why is said where it differs.

use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
use rayon::prelude::*;

use greycard_core::image::WorkingImage;

use crate::image::{IMAGENET_MEAN, IMAGENET_STD, Mask, Rgb8};
use crate::registry::SKY;
use crate::runtime::{self, Error, Loaded, Provider, Result};
use crate::sam::Prompt;
use crate::store::Store;

/// The model's square.
pub const SIZE: usize = 640;
/// Its queries: each a class and a mask.
pub const QUERIES: usize = 200;
/// COCO panoptic's 133 classes and "no object", last.
pub const CLASSES: usize = 134;
/// The side of each query's mask logits, a quarter of the square.
pub const MASK_SIDE: usize = 160;
/// `sky-other-merged` among the model's classes (COCO panoptic
/// category 187).
pub const SKY_CLASS: u8 = 119;
/// A pixel no kept query claims.
pub const UNLABELED: u8 = 255;

/// The long side the labels are made at before they are brought to
/// the preview, as in the trial: the mask logits are 160 across the
/// square, so finer buys nothing but time.
const WORK: usize = 1024;

/// The preview's long side the trial's pixel distances were set at:
/// they scale with the preview actually used.
const REFERENCE: f32 = 2048.0;

/// The class probability a query needs to be kept.
const KEEP: f32 = 0.5;
/// Sky the prior is sure of: a seed's ground.
const CONFIDENT: f32 = 0.8;
/// The gate's core: sky the prior is surer of, and how much of the
/// frame it must cover once eroded.
const CORE: f32 = 0.9;
const MIN_CORE: f32 = 0.0025;
/// The gate: the class probability the model must give the query its
/// sky comes from.
const SURE: f32 = 0.98;
/// Not-sky the prior is sure of: a negative seed's ground.
const NOT_SKY: f32 = 0.1;
/// The gate: the smallest share of the frame labeled sky.
const MIN_AREA: f32 = 0.005;
/// The gate: the core's erosion, and the reach of "touching the
/// frame's edge", as shares of the long side.
const CORE_EROSION: f32 = 0.01;
/// The seeds' erosion and the clip's growth, in pixels at 2048.
const SEED_EROSION: f32 = 20.0;
const CLIP_GROWTH: f32 = 12.0;
/// Seeds of each sign, at most.
const MAX_SEEDS: usize = 8;

/// The EoMT-S session.
pub struct Sky {
    loaded: Loaded,
}

/// The model's two answers for one picture, as it gives them.
#[derive(Debug, Clone, PartialEq)]
pub struct Logits {
    /// `QUERIES` × `CLASSES`.
    pub class: Vec<f32>,
    /// `QUERIES` × `MASK_SIDE` × `MASK_SIDE`, over the padded square.
    pub masks: Vec<f32>,
}

impl Sky {
    /// Load from the store, on the first provider that runs it.
    pub fn load(store: &Store, providers: &[Provider]) -> Result<Self> {
        if !store.have(&SKY) {
            return Err(Error::Missing(SKY.name));
        }
        let path = store.path(&SKY, &SKY.files[0]);
        let remember = runtime::Remembered {
            store_root: store.root(),
            model: SKY.id,
            hash: SKY.files[0].sha256,
        };
        let loaded = runtime::open(
            &path,
            GraphOptimizationLevel::Level3,
            providers,
            Some(remember),
            |s| {
                let zeros =
                    Tensor::from_array(([1usize, 3, SIZE, SIZE], vec![0.0f32; 3 * SIZE * SIZE]))?;
                s.run(ort::inputs!["pixel_values" => zeros])?;
                Ok(())
            },
        )?;
        Ok(Self { loaded })
    }

    pub fn provider(&self) -> Provider {
        self.loaded.provider
    }

    /// The model run on a prepared input, `3 × SIZE × SIZE` planar
    /// ([`letterbox`]'s planes).
    pub fn logits(&mut self, planes: Vec<f32>) -> Result<Logits> {
        if planes.len() != 3 * SIZE * SIZE {
            return Err(Error::Shape(format!("{} input values", planes.len())));
        }
        let input = Tensor::from_array(([1usize, 3, SIZE, SIZE], planes))?;
        let outputs = self
            .loaded
            .session
            .run(ort::inputs!["pixel_values" => input])?;
        let (shape, class) = outputs["class_queries_logits"].try_extract_tensor::<f32>()?;
        if class.len() != QUERIES * CLASSES {
            return Err(Error::Shape(format!("class_queries_logits {shape}")));
        }
        let (shape, masks) = outputs["masks_queries_logits"].try_extract_tensor::<f32>()?;
        if masks.len() != QUERIES * MASK_SIDE * MASK_SIDE {
            return Err(Error::Shape(format!("masks_queries_logits {shape}")));
        }
        let logits = Logits {
            class: class.to_vec(),
            masks: masks.to_vec(),
        };
        // A card out of memory answers with nothing rather than an
        // error (the denoiser's lesson): NaNs are not a sky.
        if logits.class.iter().any(|v| !v.is_finite()) {
            return Err(Error::Implausible("the class logits are not finite".into()));
        }
        Ok(logits)
    }

    /// The prior for `image`, at its size.
    pub fn prior(&mut self, image: &Rgb8) -> Result<Prior> {
        let boxed = letterbox(image);
        let logits = self.logits(boxed.planes)?;
        Ok(Prior::label(
            &logits,
            (boxed.width, boxed.height),
            image.width,
            image.height,
        ))
    }
}

/// A picture made ready for the model: resized to `SIZE` on its long
/// side, padded with black on the right or at the bottom, normalized.
pub struct Letterbox {
    /// `3 × SIZE × SIZE`, channels planar.
    pub planes: Vec<f32>,
    /// The picture's part of the square.
    pub width: usize,
    pub height: usize,
}

/// `image` as the model wants it. The resize is bilinear with the
/// filter widened by the scale, as the published processor's
/// (torchvision's, antialiased) is, so a 2048 preview does not alias
/// on its way down to 640.
pub fn letterbox(image: &Rgb8) -> Letterbox {
    let long = image.width.max(image.height).max(1);
    let scale = SIZE as f64 / long as f64;
    let width = ((image.width as f64 * scale).round() as usize).clamp(1, SIZE);
    let height = ((image.height as f64 * scale).round() as usize).clamp(1, SIZE);
    let src: Vec<f32> = image.data.iter().map(|&v| v as f32).collect();
    let resized = resize_triangle(&src, image.width, image.height, width, height);
    let mut planes = vec![0.0f32; 3 * SIZE * SIZE];
    for c in 0..3 {
        let pad = -IMAGENET_MEAN[c] / IMAGENET_STD[c];
        let plane = &mut planes[c * SIZE * SIZE..(c + 1) * SIZE * SIZE];
        plane.fill(pad);
        for y in 0..height {
            for x in 0..width {
                let v = resized[(y * width + x) * 3 + c] / 255.0;
                plane[y * SIZE + x] = (v - IMAGENET_MEAN[c]) / IMAGENET_STD[c];
            }
        }
    }
    Letterbox {
        planes,
        width,
        height,
    }
}

/// Three-channel interleaved `src` (`w` × `h`) resized to `nw` × `nh`
/// by a triangle filter as wide as the scale when shrinking: bilinear
/// with antialiasing, separable, rows then columns.
fn resize_triangle(src: &[f32], w: usize, h: usize, nw: usize, nh: usize) -> Vec<f32> {
    let taps = |from: usize, to: usize| -> Vec<(usize, Vec<f32>)> {
        let scale = from as f32 / to as f32;
        let support = scale.max(1.0);
        (0..to)
            .map(|o| {
                let center = (o as f32 + 0.5) * scale;
                let lo = ((center - support).floor().max(0.0)) as usize;
                let hi = ((center + support).ceil() as usize).min(from);
                let mut weights: Vec<f32> = (lo..hi)
                    .map(|i| (1.0 - ((i as f32 + 0.5 - center) / support).abs()).max(0.0))
                    .collect();
                let sum: f32 = weights.iter().sum();
                if sum > 0.0 {
                    weights.iter_mut().for_each(|v| *v /= sum);
                } else {
                    // Nothing under the filter: the nearest pixel.
                    let near = (center as usize).min(from - 1);
                    return (near, vec![1.0]);
                }
                (lo, weights)
            })
            .collect()
    };
    let xt = taps(w, nw);
    let yt = taps(h, nh);
    // Rows first: h × nw.
    let mut rows = vec![0.0f32; h * nw * 3];
    rows.par_chunks_mut(nw * 3)
        .enumerate()
        .for_each(|(y, out)| {
            let row = &src[y * w * 3..(y + 1) * w * 3];
            for (x, (lo, weights)) in xt.iter().enumerate() {
                let mut acc = [0.0f32; 3];
                for (k, wt) in weights.iter().enumerate() {
                    let i = (lo + k) * 3;
                    acc[0] += row[i] * wt;
                    acc[1] += row[i + 1] * wt;
                    acc[2] += row[i + 2] * wt;
                }
                out[x * 3..x * 3 + 3].copy_from_slice(&acc);
            }
        });
    let mut out = vec![0.0f32; nh * nw * 3];
    out.par_chunks_mut(nw * 3)
        .enumerate()
        .for_each(|(y, line)| {
            let (lo, weights) = &yt[y];
            for (k, wt) in weights.iter().enumerate() {
                let row = &rows[(lo + k) * nw * 3..(lo + k + 1) * nw * 3];
                for (o, v) in line.iter_mut().zip(row) {
                    *o += v * wt;
                }
            }
        });
    out
}

/// What the prior says of each pixel of the preview.
#[derive(Debug, Clone, PartialEq)]
pub struct Prior {
    pub width: usize,
    pub height: usize,
    /// The soft sky map, 0 to 1: Σ p(sky|q)·m_q / Σ p(any|q)·m_q.
    pub sky: Vec<f32>,
    /// Each pixel's class, `UNLABELED` where no kept query claims it.
    pub labels: Vec<u8>,
    /// How sure the model is of its sky: the highest class probability
    /// of sky among the kept queries whose class is sky, nothing when
    /// none is.
    pub sure: f32,
}

impl Prior {
    /// The labeling rule, made at `WORK` on the long side and brought
    /// to `width` × `height` (the soft map bilinearly, the labels to
    /// the nearest), from the model's answer for a picture that took
    /// `content` of the square.
    pub fn label(logits: &Logits, content: (usize, usize), width: usize, height: usize) -> Self {
        let long = width.max(height).max(1);
        let s = (WORK as f64 / long as f64).min(1.0);
        let ww = ((width as f64 * s).round() as usize).max(1);
        let wh = ((height as f64 * s).round() as usize).max(1);
        let sure = queries(&logits.class)
            .iter()
            .filter(|q| q.kept && q.label == SKY_CLASS)
            .map(|q| q.best)
            .fold(0.0f32, f32::max);
        let (sky, labels) = label_at(logits, content, ww, wh);
        if (ww, wh) == (width, height) {
            return Self {
                width,
                height,
                sky,
                labels,
                sure,
            };
        }
        let sky = Mask::new(ww, wh, sky).resampled(width, height).data;
        let labels = nearest(&labels, ww, wh, width, height);
        Self {
            width,
            height,
            sky,
            labels,
            sure,
        }
    }

    /// Whether pixel `i` is labeled sky.
    pub fn is_sky(&self, i: usize) -> bool {
        self.labels[i] == SKY_CLASS
    }

    /// The share of the frame labeled sky.
    pub fn sky_area(&self) -> f32 {
        let n = self.labels.iter().filter(|&&l| l == SKY_CLASS).count();
        n as f32 / self.labels.len().max(1) as f32
    }

    /// The labeled sky as a mask, one or nothing.
    pub fn labeled_sky(&self) -> Mask {
        Mask::new(
            self.width,
            self.height,
            self.labels
                .iter()
                .map(|&l| if l == SKY_CLASS { 1.0 } else { 0.0 })
                .collect(),
        )
    }

    /// The long side, and the factor the trial's distances at 2048
    /// scale by here.
    fn scale(&self) -> (usize, f32) {
        let long = self.width.max(self.height);
        (long, long as f32 / REFERENCE)
    }
}

/// Each query's class probabilities without "no object", best class,
/// and whether it is kept.
struct Query {
    sky: f32,
    any: f32,
    best: f32,
    label: u8,
    kept: bool,
}

fn queries(class: &[f32]) -> Vec<Query> {
    class
        .as_chunks::<CLASSES>()
        .0
        .iter()
        .map(|logits| {
            let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let exp: Vec<f32> = logits.iter().map(|l| (l - max).exp()).collect();
            let total: f32 = exp.iter().sum();
            let p: Vec<f32> = exp[..CLASSES - 1].iter().map(|e| e / total).collect();
            let (label, best) =
                p.iter()
                    .enumerate()
                    .fold((0, f32::NEG_INFINITY), |(bi, bv), (i, &v)| {
                        if v > bv { (i, v) } else { (bi, bv) }
                    });
            Query {
                sky: p[SKY_CLASS as usize],
                any: p.iter().sum(),
                best,
                label: label as u8,
                kept: best >= KEEP,
            }
        })
        .collect()
}

/// The labeling rule at `w` × `h`: each query's mask logits cropped to
/// the picture's part of the square, bilinearly resized (half-pixel
/// centers, as `F.interpolate` with `align_corners=False`), then the
/// sigmoid.
fn label_at(logits: &Logits, content: (usize, usize), w: usize, h: usize) -> (Vec<f32>, Vec<u8>) {
    let queries = queries(&logits.class);
    let side = MASK_SIDE as f32 / SIZE as f32;
    let mw = ((content.0 as f32 * side).round() as usize).clamp(1, MASK_SIDE);
    let mh = ((content.1 as f32 * side).round() as usize).clamp(1, MASK_SIDE);
    let axis = |from: usize, to: usize| -> Vec<(usize, usize, f32)> {
        let s = from as f32 / to as f32;
        (0..to)
            .map(|o| {
                let f = ((o as f32 + 0.5) * s - 0.5).max(0.0);
                let a = (f as usize).min(from - 1);
                let b = (a + 1).min(from - 1);
                (a, b, f - a as f32)
            })
            .collect()
    };
    let xs = axis(mw, w);
    let ys = axis(mh, h);
    let mut sky = vec![0.0f32; w * h];
    let mut labels = vec![UNLABELED; w * h];
    sky.par_chunks_mut(w)
        .zip(labels.par_chunks_mut(w))
        .enumerate()
        .for_each(|(y, (sky_row, label_row))| {
            let (y0, y1, ty) = ys[y];
            let mut num = vec![0.0f32; w];
            let mut den = vec![0.0f32; w];
            let mut best = vec![f32::NEG_INFINITY; w];
            let mut win_m = vec![0.0f32; w];
            let mut win_l = vec![UNLABELED; w];
            for (q, query) in queries.iter().enumerate() {
                let plane =
                    &logits.masks[q * MASK_SIDE * MASK_SIDE..(q + 1) * MASK_SIDE * MASK_SIDE];
                let r0 = &plane[y0 * MASK_SIDE..y0 * MASK_SIDE + MASK_SIDE];
                let r1 = &plane[y1 * MASK_SIDE..y1 * MASK_SIDE + MASK_SIDE];
                for (x, &(x0, x1, tx)) in xs.iter().enumerate() {
                    let top = r0[x0] + (r0[x1] - r0[x0]) * tx;
                    let bottom = r1[x0] + (r1[x1] - r1[x0]) * tx;
                    let l = top + (bottom - top) * ty;
                    let m = 1.0 / (1.0 + (-l).exp());
                    num[x] += query.sky * m;
                    den[x] += query.any * m;
                    if query.kept {
                        let p = query.best * m;
                        if p > best[x] {
                            best[x] = p;
                            win_m[x] = m;
                            win_l[x] = query.label;
                        }
                    }
                }
            }
            for x in 0..w {
                sky_row[x] = (num[x] / den[x].max(1e-6)).clamp(0.0, 1.0);
                label_row[x] = if win_m[x] >= 0.5 { win_l[x] } else { UNLABELED };
            }
        });
    (sky, labels)
}

fn nearest(labels: &[u8], w: usize, h: usize, nw: usize, nh: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        let sy = ((y * h) / nh).min(h - 1);
        for x in 0..nw {
            let sx = ((x * w) / nw).min(w - 1);
            out.push(labels[sy * w + sx]);
        }
    }
    out
}

/// Why the gate found no sky.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoSky {
    /// The model is not sure of its sky query: its class probability.
    Unsure(f32),
    /// Too little the prior is sure is sky survives the erosion: the
    /// share of the frame that did.
    NoCore(f32),
    /// Less than half a percent of the frame is labeled sky: the share
    /// it was.
    TooSmall(f32),
    /// The sky touches neither the top nor a side of the frame.
    AwayFromEdges,
}

impl std::fmt::Display for NoSky {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoSky::Unsure(p) if *p <= 0.0 => write!(f, "the model calls nothing sky"),
            NoSky::Unsure(p) => write!(f, "the model is only {:.1}% sure of its sky", p * 100.0),
            NoSky::NoCore(core) => write!(
                f,
                "too little the model is sure is sky ({:.2}% of the frame)",
                core * 100.0
            ),
            NoSky::TooSmall(area) => write!(f, "only {:.2}% labeled sky", area * 100.0),
            NoSky::AwayFromEdges => write!(f, "the sky touches no edge of the frame"),
        }
    }
}

/// The gate: whether the prior has a sky worth a mask. The rule that
/// must never fail: a mask on a frame with no sky is worse than none.
pub fn gate(prior: &Prior) -> std::result::Result<(), NoSky> {
    let (w, h) = (prior.width, prior.height);
    let (long, _) = prior.scale();
    let r = ((CORE_EROSION * long as f32).round() as usize).max(1);
    if prior.sure < SURE {
        return Err(NoSky::Unsure(prior.sure));
    }
    let confident: Vec<bool> = prior.sky.iter().map(|&p| p > CORE).collect();
    let core =
        erode(&confident, w, h, r).iter().filter(|&&v| v).count() as f32 / (w * h).max(1) as f32;
    if core < MIN_CORE {
        return Err(NoSky::NoCore(core));
    }
    let area = prior.sky_area();
    if area < MIN_AREA {
        return Err(NoSky::TooSmall(area));
    }
    // Within the erosion's reach of the top, the left or the right.
    let touches =
        (0..h).any(|y| (0..w).any(|x| prior.is_sky(y * w + x) && (y < r || x < r || x + r >= w)));
    if !touches {
        return Err(NoSky::AwayFromEdges);
    }
    Ok(())
}

/// SAM's seeds, as fractions of the preview: (0, 0) top left.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seeds {
    pub positive: Vec<[f32; 2]>,
    pub negative: Vec<[f32; 2]>,
}

/// Up to eight points spread over the confident sky and eight over the
/// confident not-sky, each ground eroded by 20 px at 2048 so a seed is
/// never on an edge.
pub fn seeds(prior: &Prior) -> Seeds {
    let (w, h) = (prior.width, prior.height);
    let (_, s) = prior.scale();
    let r = ((SEED_EROSION * s).round() as usize).max(1);
    let sky: Vec<bool> = prior.sky.iter().map(|&p| p > CONFIDENT).collect();
    let not: Vec<bool> = prior.sky.iter().map(|&p| p < NOT_SKY).collect();
    Seeds {
        positive: spread(&erode(&sky, w, h, r), w, h, MAX_SEEDS),
        negative: spread(&erode(&not, w, h, r), w, h, MAX_SEEDS),
    }
}

/// Up to `n` points over `mask`: a 6 × 6 grid, a cell taking part when
/// a fifth of it is in the mask, its point the mask pixel nearest the
/// mask's middle in that cell; then the cells chosen farthest first
/// from those already chosen, starting from the fullest. The trial
/// picked a random pixel in each cell and a random `n` of the cells;
/// this is the same spread without the dice, so the same picture
/// always gets the same mask.
fn spread(mask: &[bool], w: usize, h: usize, n: usize) -> Vec<[f32; 2]> {
    const GRID: usize = 6;
    let mut count = [0usize; GRID * GRID];
    let mut sum = [[0.0f64; 2]; GRID * GRID];
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] {
                let c = (y * GRID / h) * GRID + x * GRID / w;
                count[c] += 1;
                sum[c][0] += x as f64;
                sum[c][1] += y as f64;
            }
        }
    }
    let cell_area = (w as f64 / GRID as f64) * (h as f64 / GRID as f64);
    let mut candidates: Vec<(usize, [f32; 2])> = Vec::new();
    for c in 0..GRID * GRID {
        if count[c] as f64 <= 0.2 * cell_area {
            continue;
        }
        let (mx, my) = (sum[c][0] / count[c] as f64, sum[c][1] / count[c] as f64);
        let (cy, cx) = (c / GRID, c % GRID);
        let (x0, x1) = (cx * w / GRID, ((cx + 1) * w / GRID).min(w));
        let (y0, y1) = (cy * h / GRID, ((cy + 1) * h / GRID).min(h));
        let mut best = (f64::INFINITY, 0usize, 0usize);
        for y in y0..y1 {
            for x in x0..x1 {
                if mask[y * w + x] {
                    let d = (x as f64 - mx).powi(2) + (y as f64 - my).powi(2);
                    if d < best.0 {
                        best = (d, x, y);
                    }
                }
            }
        }
        candidates.push((
            count[c],
            [
                (best.1 as f32 + 0.5) / w as f32,
                (best.2 as f32 + 0.5) / h as f32,
            ],
        ));
    }
    if candidates.is_empty() {
        return Vec::new();
    }
    // Fullest first, then farthest from those chosen.
    candidates.sort_by_key(|c| std::cmp::Reverse(c.0));
    let aspect = h as f32 / w as f32;
    let dist = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).hypot((a[1] - b[1]) * aspect);
    let mut chosen = vec![candidates.remove(0).1];
    while chosen.len() < n && !candidates.is_empty() {
        let (i, _) = candidates
            .iter()
            .enumerate()
            .map(|(i, (_, p))| {
                let d = chosen
                    .iter()
                    .map(|&q| dist(*p, q))
                    .fold(f32::INFINITY, f32::min);
                (i, d)
            })
            .fold(
                (0, f32::NEG_INFINITY),
                |acc, v| if v.1 > acc.1 { v } else { acc },
            );
        chosen.push(candidates.remove(i).1);
    }
    chosen
}

/// The sky's outline, one or nothing at the prior's size: SAM from
/// `seeds` through `decode` (the prompts in, the mask in SAM's square
/// out; the caller embeds the picture once), one decode per positive
/// with every negative, the union clipped to the prior's sky grown by
/// 12 px at 2048. `picks` are a person's own points, positive or not,
/// in the preview's fractions: each positive one decodes too and is not
/// clipped (it is theirs to ask), and each negative joins every decode.
/// No positive seed, or nothing left after the clip, and the prior's
/// own labels are the outline: the gate has passed, so there is a sky,
/// and the model's word for it is better than none. Call it only once
/// the gate has passed.
pub fn outline(
    prior: &Prior,
    seeds: &Seeds,
    picks: &[([f32; 2], bool)],
    mut decode: impl FnMut(&[Prompt]) -> Result<Mask>,
) -> Result<Mask> {
    let (w, h) = (prior.width, prior.height);
    let negatives: Vec<Prompt> = seeds
        .negative
        .iter()
        .copied()
        .chain(picks.iter().filter(|p| !p.1).map(|p| p.0))
        .map(|p| Prompt::Point {
            x: p[0],
            y: p[1],
            positive: false,
        })
        .collect();
    let mut seeded = vec![false; w * h];
    let mut picked = vec![false; w * h];
    let positives = seeds
        .positive
        .iter()
        .map(|&p| (p, false))
        .chain(picks.iter().filter(|p| p.1).map(|p| (p.0, true)));
    for (p, theirs) in positives {
        let mut prompts = vec![Prompt::Point {
            x: p[0],
            y: p[1],
            positive: true,
        }];
        prompts.extend(negatives.iter().copied());
        let mask = decode(&prompts)?.resampled(w, h);
        let into = if theirs { &mut picked } else { &mut seeded };
        for (o, v) in into.iter_mut().zip(&mask.data) {
            *o |= *v > 0.5;
        }
    }
    let (_, s) = prior.scale();
    let r = ((CLIP_GROWTH * s).round() as usize).max(1);
    let likely: Vec<bool> = prior.sky.iter().map(|&p| p > 0.5).collect();
    let clip = dilate(&likely, w, h, r);
    let mut out: Vec<bool> = seeded.iter().zip(&clip).map(|(a, b)| *a && *b).collect();
    if !out.iter().any(|&v| v) {
        out = prior.labels.iter().map(|&l| l == SKY_CLASS).collect();
    }
    for (o, p) in out.iter_mut().zip(&picked) {
        *o |= *p;
    }
    Ok(Mask::new(
        w,
        h,
        out.into_iter().map(|v| if v { 1.0 } else { 0.0 }).collect(),
    ))
}

/// What the edge stage sees of the frame: the preview the models saw
/// and its luma, and the frame's own linear picture in the working
/// space at full size, for a solver that wants the scene's values
/// (where the preview clips a bright sky) or the frame's own
/// resolution.
pub struct Frame<'a> {
    pub display: &'a Rgb8,
    pub luma: &'a [f32],
    pub linear: &'a WorkingImage,
}

/// The sky's edge: the outline (one or nothing, at the preview's size)
/// as the matte the raster is made from, `size` (width, height). The
/// one place the edge is made, so a better solver replaces this body
/// and nothing else. The color-line matte at the frame's own
/// resolution ([`crate::matte::color_line`]), averaged down to `size`;
/// where it has nothing to go on (no known sky left once the outline
/// is shrunk, or no linear frame of the preview's shape), the outline
/// feathered ([`feathered`]).
pub fn refine_sky(mask: &Mask, prior: &Prior, frame: &Frame, size: (usize, usize)) -> Mask {
    crate::matte::color_line(mask, prior, frame.linear, size.0, size.1)
        .unwrap_or_else(|| feathered(mask, frame).resampled(size.0, size.1))
}

/// The outline brought to the preview's edges by the guided filter at
/// the Subject rule (radius a 256th of the preview's width, ε 1e-3,
/// the preview's luma as the guide): it feathers the outline and does
/// not move it, so it neither reaches the sky among branches nor
/// closes the gap SAM leaves round hair. The matte's fallback, and
/// what the Sky shape had before it.
pub fn feathered(mask: &Mask, frame: &Frame) -> Mask {
    let (w, h) = (frame.display.width, frame.display.height);
    let radius = (w / 256).max(2);
    crate::refine(mask, frame.luma, w, h, radius, 1e-3)
}

/// SAM for the outline: the prompts in, its mask in its own square out.
pub type Decode<'a> = dyn FnMut(&[Prompt]) -> Result<Mask> + 'a;

/// How long each stage took, in seconds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Times {
    pub gate: f64,
    pub seeds: f64,
    /// SAM's embedding and decodes, and the clip.
    pub outline: f64,
    pub edge: f64,
}

/// The sky found, or why not.
pub enum Found {
    /// The matte, at the size asked for, and whether SAM made the
    /// outline (not the prior's own labels).
    Sky {
        matte: Mask,
        seeded: bool,
        /// The outline the matte was made from, one or nothing at the
        /// prior's size, for looking at.
        outline: Mask,
    },
    None(NoSky),
}

/// Stages 2 to 4 on a prior already made: the gate, then the outline
/// from `decode` (called only once the gate has passed, so a frame
/// with no sky never pays for SAM's embedding; `None` where SAM is not
/// to be had, which leaves the prior's own labels as the outline),
/// then the edge, as a matte of `size` (width, height).
pub fn find(
    prior: &Prior,
    frame: &Frame,
    picks: &[([f32; 2], bool)],
    decode: Option<&mut Decode>,
    size: (usize, usize),
    times: &mut Times,
) -> Result<Found> {
    let t = std::time::Instant::now();
    let gated = gate(prior);
    times.gate = t.elapsed().as_secs_f64();
    if let Err(why) = gated {
        return Ok(Found::None(why));
    }
    let t = std::time::Instant::now();
    let seeds = if decode.is_some() {
        seeds(prior)
    } else {
        Seeds::default()
    };
    times.seeds = t.elapsed().as_secs_f64();
    let t = std::time::Instant::now();
    let seeded = !seeds.positive.is_empty();
    let mask = match decode {
        Some(decode) => outline(prior, &seeds, picks, decode)?,
        None => prior.labeled_sky(),
    };
    times.outline = t.elapsed().as_secs_f64();
    let t = std::time::Instant::now();
    let matte = refine_sky(&mask, prior, frame, size);
    times.edge = t.elapsed().as_secs_f64();
    Ok(Found::Sky {
        matte,
        seeded,
        outline: mask,
    })
}

/// A binary erosion by a square of radius `r` (side 2r + 1): a pixel
/// stays when every pixel of the square about it that is inside the
/// frame is set, so the frame's edge does not eat into a region
/// (OpenCV's default border for an erosion).
pub fn erode(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let sums = integral(mask, w, h);
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (n, set) = window(&sums, w, h, x, y, r);
            set == n
        })
        .collect()
}

/// A binary dilation by a square of radius `r`.
pub fn dilate(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let sums = integral(mask, w, h);
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            window(&sums, w, h, x, y, r).1 > 0
        })
        .collect()
}

/// Summed-area table, one row and column of zeros before.
fn integral(mask: &[bool], w: usize, h: usize) -> Vec<u32> {
    let mut s = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += mask[y * w + x] as u32;
            s[(y + 1) * (w + 1) + x + 1] = s[y * (w + 1) + x + 1] + row;
        }
    }
    s
}

/// The square of radius `r` about (x, y) within the frame: how many
/// pixels, and how many set.
fn window(s: &[u32], w: usize, h: usize, x: usize, y: usize, r: usize) -> (u32, u32) {
    let (x0, y0) = (x.saturating_sub(r), y.saturating_sub(r));
    let (x1, y1) = ((x + r + 1).min(w), (y + r + 1).min(h));
    let at = |x: usize, y: usize| s[y * (w + 1) + x];
    let set = at(x1, y1) + at(x0, y0) - at(x0, y1) - at(x1, y0);
    (((x1 - x0) * (y1 - y0)) as u32, set)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A prior of `w` × `h` from a soft sky map, labeled sky where it
    /// is over a half and something else (tree) where not.
    fn prior_of(w: usize, h: usize, sky: impl Fn(usize, usize) -> f32) -> Prior {
        let mut map = Vec::with_capacity(w * h);
        let mut labels = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let p = sky(x, y);
                map.push(p);
                labels.push(if p > 0.5 { SKY_CLASS } else { 116 });
            }
        }
        Prior {
            width: w,
            height: h,
            sky: map,
            labels,
            sure: 0.999,
        }
    }

    /// A gradient sky over a dark block: the prior as a model would
    /// see it, sure of the top third, sure of the block, unsure
    /// between.
    fn sky_over_block(w: usize, h: usize) -> Prior {
        prior_of(w, h, |_, y| {
            let t = y as f32 / h as f32;
            if t < 0.33 {
                0.95
            } else if t < 0.4 {
                0.95 - (t - 0.33) / 0.07 * 0.9
            } else {
                0.02
            }
        })
    }

    fn picture(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Rgb8 {
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                data.extend(f(x, y));
            }
        }
        Rgb8::new(w, h, data)
    }

    /// Logits for a picture split at `split` of its height: query 0
    /// says sky above, query 1 building below, the rest say nothing.
    fn logits_split(content: (usize, usize), split: f32, sky_class: f32) -> Logits {
        let mut class = vec![-10.0f32; QUERIES * CLASSES];
        for q in 0..QUERIES {
            class[q * CLASSES + CLASSES - 1] = 10.0;
        }
        class[SKY_CLASS as usize] = sky_class;
        class[CLASSES - 1] = -10.0;
        class[CLASSES + 130] = 10.0; // building-other-merged
        class[2 * CLASSES - 1] = -10.0;
        let mut masks = vec![-20.0f32; QUERIES * MASK_SIDE * MASK_SIDE];
        let mh = content.1 as f32 * MASK_SIDE as f32 / SIZE as f32;
        for y in 0..MASK_SIDE {
            let above = (y as f32 + 0.5) < split * mh;
            for x in 0..MASK_SIDE {
                masks[y * MASK_SIDE + x] = if above { 20.0 } else { -20.0 };
                masks[MASK_SIDE * MASK_SIDE + y * MASK_SIDE + x] = if above { -20.0 } else { 20.0 };
            }
        }
        Logits { class, masks }
    }

    #[test]
    fn the_letterbox_keeps_the_aspect_and_pads_with_black() {
        let img = picture(400, 200, |_, _| [255, 255, 255]);
        let boxed = letterbox(&img);
        assert_eq!((boxed.width, boxed.height), (640, 320));
        let white = (1.0 - IMAGENET_MEAN[0]) / IMAGENET_STD[0];
        let black = -IMAGENET_MEAN[0] / IMAGENET_STD[0];
        assert!((boxed.planes[100 * SIZE + 100] - white).abs() < 1e-5);
        assert!((boxed.planes[500 * SIZE + 100] - black).abs() < 1e-5);
        // A portrait pads on the right.
        let tall = letterbox(&picture(300, 600, |_, _| [0, 0, 0]));
        assert_eq!((tall.width, tall.height), (320, 640));
    }

    #[test]
    fn the_resize_averages_rather_than_aliases() {
        // A one-pixel checkerboard shrunk four times is a flat grey.
        let img = picture(
            64,
            64,
            |x, y| if (x + y) % 2 == 0 { [255; 3] } else { [0; 3] },
        );
        let src: Vec<f32> = img.data.iter().map(|&v| v as f32).collect();
        let out = resize_triangle(&src, 64, 64, 16, 16);
        assert!(
            out.iter().all(|v| (v - 127.5).abs() < 20.0),
            "{:?}",
            &out[..6]
        );
        // A flat picture stays flat at any size.
        let flat = vec![80.0f32; 30 * 20 * 3];
        let out = resize_triangle(&flat, 30, 20, 13, 7);
        assert!(out.iter().all(|v| (v - 80.0).abs() < 1e-3));
    }

    #[test]
    fn the_labeling_rule_gives_each_pixel_its_query() {
        let logits = logits_split((640, 427), 0.4, 10.0);
        let prior = Prior::label(&logits, (640, 427), 300, 200);
        assert_eq!((prior.width, prior.height), (300, 200));
        // Above the split sky, below it the building, and the soft map
        // says the same.
        assert!(prior.is_sky(20 * 300 + 150));
        assert_eq!(prior.labels[150 * 300 + 150], 130);
        assert!(prior.sky[20 * 300 + 150] > 0.99);
        assert!(prior.sky[150 * 300 + 150] < 0.01);
        let area = prior.sky_area();
        assert!((area - 0.4).abs() < 0.03, "{area}");
    }

    /// A query the model is not sure of labels nothing, however sure
    /// its mask: the labels are unassigned, and the soft map falls to
    /// what its class probability says.
    #[test]
    fn an_unsure_query_labels_nothing() {
        let mut logits = logits_split((640, 640), 1.0, 0.0);
        // Query 0's sky and no-object logits level: a half each.
        logits.class[CLASSES - 1] = 0.0;
        let prior = Prior::label(&logits, (640, 640), 64, 64);
        assert!(prior.labels.iter().all(|&l| l == UNLABELED));
        assert!(prior.sky.iter().all(|&p| p > 0.95), "{}", prior.sky[0]);
        // The soft map is sure of a sky no query claims: the gate
        // wants a query sure of it.
        assert_eq!(prior.sure, 0.0);
        assert_eq!(gate(&prior), Err(NoSky::Unsure(0.0)));
        // With the labels a query gives, it is as sure as that query.
        let logits = logits_split((640, 427), 0.4, 10.0);
        let prior = Prior::label(&logits, (640, 427), 300, 200);
        assert!(prior.sure > 0.999, "{}", prior.sure);
    }

    #[test]
    fn the_gate_passes_a_sky_over_a_block() {
        assert_eq!(gate(&sky_over_block(300, 200)), Ok(()));
    }

    #[test]
    fn the_gate_finds_nothing_in_a_frame_with_no_sky() {
        // Nothing the prior calls sky.
        let prior = prior_of(300, 200, |_, _| 0.03);
        assert_eq!(gate(&prior), Err(NoSky::NoCore(0.0)));
        // A bluish slope the prior half believes: over a half here and
        // there, sure nowhere.
        let prior = prior_of(300, 200, |x, y| 0.4 + 0.3 * ((x + y) % 7) as f32 / 7.0);
        assert_eq!(gate(&prior), Err(NoSky::NoCore(0.0)));
    }

    /// The defocused snow slope of the test set: labeled sky over a
    /// tenth of the frame at the top left, the soft map over 0.8 on a
    /// band of it, but the query it comes from only 95% sure, and
    /// little of it over 0.9.
    #[test]
    fn the_gate_refuses_a_sky_the_model_is_not_sure_of() {
        let slope = |x: usize, y: usize| {
            if x < 120 && y < 150 {
                if (30..90).contains(&x) { 0.93 } else { 0.85 }
            } else {
                0.05
            }
        };
        let mut prior = prior_of(400, 300, slope);
        prior.sure = 0.954;
        assert_eq!(gate(&prior), Err(NoSky::Unsure(0.954)));
        // Sure of the query, but the map over 0.9 on too little.
        let mut prior = prior_of(400, 300, |x, y| {
            if x < 120 && y < 150 {
                if (50..62).contains(&x) && y < 30 {
                    0.93
                } else {
                    0.85
                }
            } else {
                0.05
            }
        });
        prior.sure = 0.99;
        assert!(matches!(gate(&prior), Err(NoSky::NoCore(c)) if c < MIN_CORE));
    }

    #[test]
    fn the_gate_wants_a_core_that_survives_the_erosion() {
        // A confident line a pixel high: labeled, but thinner than the
        // erosion.
        let prior = prior_of(400, 300, |_, y| if y == 10 { 0.95 } else { 0.05 });
        assert_eq!(gate(&prior), Err(NoSky::NoCore(0.0)));
    }

    #[test]
    fn the_gate_wants_half_a_percent_of_sky() {
        // A confident patch at the top whose core passes, 0.4% of the
        // frame labeled.
        let prior = prior_of(400, 300, |x, y| {
            if (100..140).contains(&x) && y < 14 {
                0.95
            } else {
                0.05
            }
        });
        assert!(matches!(gate(&prior), Err(NoSky::TooSmall(a)) if a < MIN_AREA));
    }

    #[test]
    fn the_gate_wants_the_sky_at_an_edge() {
        // A lake's reflection with no sky above it: a confident sky in
        // the middle of the frame, touching nothing.
        let prior = prior_of(400, 300, |x, y| {
            if (100..300).contains(&x) && (100..200).contains(&y) {
                0.95
            } else {
                0.05
            }
        });
        assert_eq!(gate(&prior), Err(NoSky::AwayFromEdges));
    }

    #[test]
    fn seeds_sit_inside_the_confident_regions_and_spread() {
        let prior = sky_over_block(600, 400);
        let s = seeds(&prior);
        assert!(!s.positive.is_empty() && s.positive.len() <= MAX_SEEDS);
        assert!(!s.negative.is_empty() && s.negative.len() <= MAX_SEEDS);
        // Positives in the top third less the erosion, negatives in
        // the block less it.
        // The confident sky ends at 0.342 of the height and the
        // confident block starts at 0.396; the erosion is 6 px of 400.
        for p in &s.positive {
            assert!(p[1] < 0.342 - 0.014, "{p:?}");
        }
        for p in &s.negative {
            assert!(p[1] > 0.396 + 0.014, "{p:?}");
        }
        // Spread: not all in one column.
        let xs: Vec<f32> = s.positive.iter().map(|p| p[0]).collect();
        let (lo, hi) = xs
            .iter()
            .fold((1.0f32, 0.0f32), |a, &x| (a.0.min(x), a.1.max(x)));
        assert!(hi - lo > 0.4, "{xs:?}");
        // The same picture, the same seeds.
        assert_eq!(seeds(&prior), s);
    }

    /// A decoder standing in for SAM: whatever `region` says of the
    /// preview, at SAM's square.
    fn fake_sam(region: impl Fn(f32, f32) -> bool) -> impl FnMut(&[Prompt]) -> Result<Mask> {
        move |prompts: &[Prompt]| {
            assert!(matches!(prompts[0], Prompt::Point { positive: true, .. }));
            let n = crate::sam::MASK;
            let data = (0..n * n)
                .map(|i| {
                    let (u, v) = ((i % n) as f32 / n as f32, (i / n) as f32 / n as f32);
                    if region(u, v) { 1.0 } else { 0.0 }
                })
                .collect();
            Ok(Mask::new(n, n, data))
        }
    }

    #[test]
    fn the_outline_is_sams_clipped_to_the_prior() {
        let prior = sky_over_block(300, 200);
        let s = seeds(&prior);
        // SAM spills well into the block; the clip keeps it within
        // the prior's sky grown by a few pixels.
        let mut decodes = 0;
        let mut sam = fake_sam(|_, v| v < 0.7);
        let mask = outline(&prior, &s, &[], |p: &[Prompt]| {
            decodes += 1;
            sam(p)
        })
        .unwrap();
        assert_eq!(decodes, s.positive.len());
        let grown = 12.0 * 300.0 / REFERENCE;
        for y in 0..200 {
            let v = mask.at(150, y);
            let t = y as f32 / 200.0;
            if t < 0.3 {
                assert_eq!(v, 1.0, "row {y}");
            }
            if (y as f32) > 0.4 * 200.0 + grown + 1.0 {
                assert_eq!(v, 0.0, "row {y}");
            }
        }
    }

    #[test]
    fn with_no_seed_the_outline_is_the_priors_own_labels() {
        let prior = sky_over_block(300, 200);
        let mask = outline(&prior, &Seeds::default(), &[], |_: &[Prompt]| {
            panic!("no seed, no decode")
        })
        .unwrap();
        assert_eq!(mask, prior.labeled_sky());
        // SAM finding nothing within the clip: the same.
        let s = seeds(&prior);
        let mask = outline(&prior, &s, &[], fake_sam(|_, v| v > 0.9)).unwrap();
        assert_eq!(mask, prior.labeled_sky());
    }

    #[test]
    fn a_persons_negative_joins_every_decode_and_a_positive_is_theirs() {
        let prior = sky_over_block(300, 200);
        let s = seeds(&prior);
        let mut seen = Vec::new();
        let mask = outline(
            &prior,
            &s,
            &[([0.5, 0.9], true), ([0.1, 0.1], false)],
            |p: &[Prompt]| {
                seen.push(p.to_vec());
                // The person's click on the block: SAM gives the block.
                let theirs = matches!(p[0], Prompt::Point { y, .. } if y > 0.8);
                fake_sam(move |_, v| if theirs { v > 0.8 } else { v < 0.3 })(p)
            },
        )
        .unwrap();
        assert_eq!(seen.len(), s.positive.len() + 1);
        for prompts in &seen {
            let negatives = prompts
                .iter()
                .filter(|p| {
                    matches!(
                        p,
                        Prompt::Point {
                            positive: false,
                            ..
                        }
                    )
                })
                .count();
            assert_eq!(negatives, s.negative.len() + 1);
        }
        // Their positive stands though it is outside the prior's sky.
        assert_eq!(mask.at(150, 190), 1.0);
        assert_eq!(mask.at(150, 10), 1.0);
        assert_eq!(mask.at(150, 130), 0.0);
    }

    /// A canopy of thin dark lines over a sky: the prior calls the
    /// canopy tree and is unsure in it; the gate passes on the open
    /// sky, and the outline never reaches past the prior's sky, grown.
    #[test]
    fn a_canopy_over_a_sky_keeps_the_sky_and_not_the_canopy() {
        let (w, h) = (300usize, 200usize);
        let prior = prior_of(w, h, |x, y| {
            if y < 60 {
                0.97
            } else if y < 140 {
                // The canopy's band: the prior is unsure among lines.
                if x % 9 < 2 { 0.1 } else { 0.45 }
            } else {
                0.02
            }
        });
        assert_eq!(gate(&prior), Ok(()));
        let s = seeds(&prior);
        assert!(s.positive.iter().all(|p| p[1] < 0.3));
        let mask = outline(&prior, &s, &[], fake_sam(|_, v| v < 0.75)).unwrap();
        let grown = (12.0 * 300.0 / REFERENCE).round() as usize;
        for y in 0..h {
            for x in 0..w {
                if y > 60 + grown {
                    assert_eq!(mask.at(x, y), 0.0, "({x}, {y})");
                }
            }
        }
        assert_eq!(mask.at(150, 20), 1.0);
    }

    fn frame_parts(w: usize, h: usize) -> (Rgb8, Vec<f32>, WorkingImage) {
        let display = picture(w, h, |_, y| {
            if y < h / 3 {
                [140, 180, 230]
            } else {
                [30, 30, 30]
            }
        });
        let luma = display.luma();
        // The same picture in linear light, at twice the size.
        let mut linear = WorkingImage::new(2 * w, 2 * h);
        for y in 0..2 * h {
            for x in 0..2 * w {
                let d = &display.data[((y / 2) * w + x / 2) * 3..][..3];
                let o = &mut linear.data[(y * 2 * w + x) * 3..][..3];
                for (o, &v) in o.iter_mut().zip(d) {
                    *o = (v as f32 / 255.0).powf(2.2);
                }
            }
        }
        (display, luma, linear)
    }

    #[test]
    fn find_offers_nothing_on_a_frame_with_no_sky_and_never_calls_sam() {
        let (display, luma, linear) = frame_parts(120, 80);
        let frame = Frame {
            display: &display,
            luma: &luma,
            linear: &linear,
        };
        let prior = prior_of(120, 80, |_, _| 0.04);
        let never: &mut dyn FnMut(&[Prompt]) -> Result<Mask> =
            &mut |_| panic!("SAM on a frame with no sky");
        let mut times = Times::default();
        let found = find(&prior, &frame, &[], Some(never), (120, 80), &mut times).unwrap();
        assert!(matches!(found, Found::None(NoSky::NoCore(_))));
    }

    #[test]
    fn find_makes_a_matte_on_the_sky_and_nothing_on_the_block() {
        let (w, h) = (240usize, 160usize);
        let (display, luma, linear) = frame_parts(w, h);
        let frame = Frame {
            display: &display,
            luma: &luma,
            linear: &linear,
        };
        let prior = prior_of(w, h, |_, y| if y < h / 3 { 0.96 } else { 0.03 });
        let mut sam = fake_sam(|_, v| v < 0.34);
        let sam: &mut dyn FnMut(&[Prompt]) -> Result<Mask> = &mut sam;
        let mut times = Times::default();
        let Found::Sky { matte, seeded, .. } =
            find(&prior, &frame, &[], Some(sam), (w, h), &mut times).unwrap()
        else {
            panic!("no sky found over a plain sky");
        };
        assert!(seeded);
        assert_eq!((matte.width, matte.height), (w, h));
        assert!(matte.at(120, 10) > 0.95);
        assert!(matte.at(120, 150) < 0.05);
        // Without SAM: the prior's own labels, filtered the same way.
        let Found::Sky { matte, seeded, .. } =
            find(&prior, &frame, &[], None, (w, h), &mut times).unwrap()
        else {
            panic!("no sky found without SAM");
        };
        assert!(!seeded);
        assert!(matte.at(120, 10) > 0.95 && matte.at(120, 150) < 0.05);
    }

    #[test]
    fn erosion_keeps_the_frames_edge_and_dilation_grows() {
        let (w, h) = (20usize, 10usize);
        // The left half set.
        let m: Vec<bool> = (0..w * h).map(|i| i % w < 10).collect();
        let e = erode(&m, w, h, 2);
        // The frame's own left edge stays; the inner edge moves in 2.
        assert!(e[5 * w]);
        assert!(e[5 * w + 7] && !e[5 * w + 8]);
        let d = dilate(&m, w, h, 3);
        assert!(d[5 * w + 12] && !d[5 * w + 13]);
        assert!(!erode(&[false; 4], 2, 2, 1).iter().any(|&v| v));
    }
}
