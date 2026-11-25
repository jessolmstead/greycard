//! What a merge of ours says of itself, in the file's `DNGPrivateData`:
//! our name, then a line a fact.
//!
//! The first facts say what the develop reads: the kind, the
//! reference, whether the frames were denoised, the headroom, and
//! whether the lens was corrected. Then each frame's name and its
//! head's hash, and the recipe: the options the merge ran with and the
//! geometry it found, enough to make the picture again from its frames
//! with no fit. A number is written in the fewest digits that read
//! back to the same `f64` (or `f32`), so a merge made again places its
//! frames to the bit. A file written before the recipe was kept reads
//! as before, with no recipe.

use super::Noise;
use super::pano::{Axis, Canvas, Kernel, Model, Projection, Rect, of_quaternion, quaternion};
use crate::develop::lens::{ChromaticAberration, Distortion, LensCorrection, Scale, Vignetting};
use crate::register::{Homography, Transform};

/// What a merge of ours says of itself.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tag {
    /// The kind of merge: `hdr`, `pano`, or `stack`.
    pub kind: String,
    /// The frames' file names, in the order given.
    pub frames: Vec<String>,
    /// Which of them the merge is on the scale and in the geometry of.
    pub reference: usize,
    /// Whether the frames were denoised before the merge: the develop's
    /// denoisers stand down on such a file.
    pub denoised: bool,
    /// The stops the samples were stored under the reference's scale
    /// by: the develop brightens by as much, so the file opens at the
    /// reference's exposure.
    pub headroom_stops: f32,
    /// Whether the frames' lens was corrected in the merge (a
    /// panorama's is): the develop's lens profile stands down on such
    /// a file, which is in no one frame's geometry.
    pub lens_corrected: bool,
    /// Each frame's content hash, the library's (its first 64 KB and
    /// its size), in the frames' order: a frame renamed, or another
    /// file under its name, is told from it. None for a frame whose
    /// hash was not taken.
    pub heads: Vec<Option<String>>,
    /// The whole file's BLAKE3, the index's `whole_hash`, for a frame
    /// whose head hash another frame of the merge shares (uncompressed
    /// raws of one size from one burst can): the head alone does not
    /// tell them apart. None for every other frame.
    pub wholes: Vec<Option<String>>,
    /// What made the picture; none for a file written before it was
    /// kept, or one whose recipe does not read.
    pub recipe: Option<Recipe>,
    /// For a recipe that does not read whole, the first fact that did
    /// not read or that it lacks: a file written before the recipe was
    /// kept has none.
    pub unread: Option<String>,
}

impl Tag {
    /// Whether the frames' lateral aberration was measured and
    /// corrected on their mosaics in the merge: the develop's lens
    /// profile leaves it alone on such a file. A recipe that says
    /// `off` did not; a file written before the recipe was kept ran
    /// the fit once, the default. A merge whose lens was corrected
    /// stands the profile down whole already.
    pub fn ca_measured(&self) -> bool {
        !self.lens_corrected && self.recipe.as_ref().is_none_or(|r| r.ca != "off")
    }
}

/// What made a merge's picture: the options it ran with and the
/// geometry it found.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    /// How the frames' CA was fitted, in the engine's words: `once`,
    /// `per-frame`, or `off`.
    pub ca: String,
    /// The learned denoiser's tier each frame was walked with; none
    /// for none.
    pub denoise: Option<String>,
    /// The sensor's noise a bracket's weights were given in place of
    /// what its pairs measure; none for measured.
    pub noise: Option<Noise>,
    /// Whether a bracket's frames were deghosted, in the engine's
    /// words: `on`, `off`, or `auto`. None for a panorama, and for a
    /// bracket written before it was kept, which the engine reads as
    /// `off`: what such a merge ran.
    pub deghost: Option<String>,
    /// How a bracket's moved frames were resampled. None for a
    /// panorama, whose [`Laid`] keeps its own; a bracket written before
    /// it was kept reads as bilinear, what such a merge ran.
    pub kernel: Option<Kernel>,
    pub geometry: Geometry,
}

/// Where a merge's frames went.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    /// A bracket's: each frame's registration onto the reference, in
    /// the frames' order.
    Bracket(Vec<Registration>),
    /// A panorama's.
    Pano(Box<Laid>),
    /// A focus stack's.
    Stack(Stacked),
}

/// A focus stack as merged: the core's options it ran with, and where
/// each frame went.
#[derive(Debug, Clone, PartialEq)]
pub struct Stacked {
    /// How far each band was taken from the frame with the most to say
    /// in it (the core's `stack::Options::selectivity`).
    pub selectivity: f32,
    /// The residual a link was believed under.
    pub max_residual: f32,
    /// Each frame's transform onto the reference, in the frames' order;
    /// none for a frame the merge left out.
    pub placed: Vec<Option<Transform>>,
    /// Whether the samples were stored uncompressed rather than as
    /// lossless JPEG.
    pub uncompressed: bool,
}

/// One frame of a bracket onto its reference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Registration {
    /// Its exposure relative to the reference's, as the merge used it.
    pub exposure: f64,
    /// The move it was resampled by onto the reference; none for a
    /// frame left where it was.
    pub shift: Option<Transform>,
}

/// A panorama as laid.
#[derive(Debug, Clone, PartialEq)]
pub struct Laid {
    /// The frame walked first, whose white and CA fit every frame took:
    /// the one asked or the middle of the list, which the reference
    /// need not be.
    pub anchor: usize,
    /// How each frame was resampled onto the canvas.
    pub kernel: Kernel,
    /// Every frame's turn onto the reference's view as placed: leveled
    /// and centered, so the leveling is in the rotations.
    pub placed: Placed,
    /// The lens's corrections as they ran: the vignetting in the walk,
    /// the distortion and the aberration in the warp.
    pub lens: Vec<LensCorrection>,
    /// Each frame's gains onto the reference's scale.
    pub gains: Vec<[f64; 3]>,
    pub canvas: Canvas,
    pub crop: Rect,
}

/// A panorama's frames as its tag keeps them: a turned model's
/// rotations as unit quaternions, `[w, x, y, z]`, `w` not negative.
/// Four numbers with one constraint rather than nine with six, and
/// unlike three angles, no axis order to agree on and no lock where
/// two axes line up. The quaternion is the truth: a merge places its
/// frames by the rotations made from it ([`Placed::model`]), so a
/// merge made again from the tag places them to the bit.
#[derive(Debug, Clone, PartialEq)]
pub enum Placed {
    Turned {
        focal: f64,
        turns: Vec<[f64; 4]>,
        projection: Projection,
    },
    Planar {
        homographies: Vec<Homography>,
    },
}

impl Placed {
    /// `model` as kept.
    pub fn of(model: &Model) -> Placed {
        match model {
            Model::Turned {
                focal,
                rotations,
                projection,
            } => Placed::Turned {
                focal: *focal,
                turns: rotations.iter().map(quaternion).collect(),
                projection: *projection,
            },
            Model::Planar { homographies } => Placed::Planar {
                homographies: homographies.clone(),
            },
        }
    }

    /// The model the frames are placed by.
    pub fn model(&self) -> Model {
        match self {
            Placed::Turned {
                focal,
                turns,
                projection,
            } => Model::Turned {
                focal: *focal,
                rotations: turns.iter().map(|&q| of_quaternion(q)).collect(),
                projection: *projection,
            },
            Placed::Planar { homographies } => Model::Planar {
                homographies: homographies.clone(),
            },
        }
    }

    /// How many frames it places.
    pub fn len(&self) -> usize {
        match self {
            Placed::Turned { turns, .. } => turns.len(),
            Placed::Planar { homographies } => homographies.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The maker's name `DNGPrivateData` starts with, as the specification
/// asks, ending in its NUL.
const MAKER: &[u8] = b"greycard\0";

/// Numbers in a fact, a space apart.
fn join<T: std::fmt::Display>(values: &[T]) -> String {
    values
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `N` finite numbers split by `sep`; none for any other count.
fn numbers<const N: usize, T>(text: &str, sep: char) -> Option<[T; N]>
where
    T: std::str::FromStr + Copy + Default + Finite,
{
    let mut out = [T::default(); N];
    let mut parts = text.split(sep).filter(|p| !p.is_empty());
    for o in out.iter_mut() {
        *o = parts.next()?.parse().ok().filter(Finite::finite)?;
    }
    parts.next().is_none().then_some(out)
}

/// A number that is not infinite or not a number.
trait Finite {
    fn finite(&self) -> bool;
}

impl Finite for f64 {
    fn finite(&self) -> bool {
        self.is_finite()
    }
}

impl Finite for f32 {
    fn finite(&self) -> bool {
        self.is_finite()
    }
}

impl Finite for usize {
    fn finite(&self) -> bool {
        true
    }
}

/// A frame's number and the numbers after it: `<k> <values>`.
fn of_frame<const N: usize>(value: &str) -> Option<(usize, [f64; N])> {
    let (k, rest) = value.split_once(' ')?;
    Some((k.parse().ok()?, numbers(rest, ' ')?))
}

/// A lens correction in one fact: its scales, then each model it has,
/// `name:values`.
fn correction_text(c: &LensCorrection) -> String {
    let mut parts = vec![
        format!("radius:{}", c.radius_scale),
        format!("vignetting-radius:{}", c.vignetting_scale),
        match c.scale {
            Scale::Auto => "scale:auto".to_string(),
            Scale::Fixed(s) => format!("scale:{s}"),
        },
    ];
    let list = |v: &[f32]| v.iter().map(f32::to_string).collect::<Vec<_>>().join(",");
    match c.distortion {
        Some(Distortion::Poly3 { k1 }) => parts.push(format!("poly3:{k1}")),
        Some(Distortion::Poly5 { k1, k2 }) => parts.push(format!("poly5:{}", list(&[k1, k2]))),
        Some(Distortion::Ptlens { a, b, c }) => parts.push(format!("ptlens:{}", list(&[a, b, c]))),
        Some(Distortion::Acm { k, focal_scale }) => {
            let mut v = k.to_vec();
            v.push(focal_scale);
            parts.push(format!("acm:{}", list(&v)));
        }
        None => {}
    }
    if let Some(ca) = c.chromatic_aberration {
        let mut v = ca.red.to_vec();
        v.extend(ca.blue);
        parts.push(format!("ca:{}", list(&v)));
    }
    if let Some(v) = c.vignetting {
        parts.push(format!("vignetting:{}", list(&v.k)));
    }
    parts.join(" ")
}

fn parse_correction(text: &str) -> Option<LensCorrection> {
    let mut c = LensCorrection::default();
    for part in text.split(' ').filter(|p| !p.is_empty()) {
        let (name, v) = part.split_once(':')?;
        match name {
            "radius" => c.radius_scale = numbers::<1, f32>(v, ',')?[0],
            "vignetting-radius" => c.vignetting_scale = numbers::<1, f32>(v, ',')?[0],
            "scale" if v == "auto" => c.scale = Scale::Auto,
            "scale" => c.scale = Scale::Fixed(numbers::<1, f32>(v, ',')?[0]),
            "poly3" => {
                let [k1] = numbers(v, ',')?;
                c.distortion = Some(Distortion::Poly3 { k1 });
            }
            "poly5" => {
                let [k1, k2] = numbers(v, ',')?;
                c.distortion = Some(Distortion::Poly5 { k1, k2 });
            }
            "ptlens" => {
                let [a, b, cc] = numbers(v, ',')?;
                c.distortion = Some(Distortion::Ptlens { a, b, c: cc });
            }
            "acm" => {
                let [k0, k1, k2, k3, k4, focal_scale] = numbers(v, ',')?;
                c.distortion = Some(Distortion::Acm {
                    k: [k0, k1, k2, k3, k4],
                    focal_scale,
                });
            }
            "ca" => {
                let [r0, r1, r2, b0, b1, b2] = numbers(v, ',')?;
                c.chromatic_aberration = Some(ChromaticAberration {
                    red: [r0, r1, r2],
                    blue: [b0, b1, b2],
                });
            }
            "vignetting" => {
                c.vignetting = Some(Vignetting {
                    k: numbers(v, ',')?,
                })
            }
            _ => return None,
        }
    }
    Some(c)
}

impl Recipe {
    /// The recipe's facts, appended to `text`.
    fn write(&self, text: &mut String) {
        let mut fact = |key: &str, value: String| {
            text.push_str(key);
            text.push('=');
            text.push_str(&value);
            text.push('\n');
        };
        fact("ca", self.ca.clone());
        fact(
            "denoise",
            self.denoise.clone().unwrap_or_else(|| "none".into()),
        );
        if let Some(n) = self.noise {
            fact("noise", join(&[n.shot, n.read]));
        }
        if let Some(d) = &self.deghost {
            fact("deghost", d.clone());
        }
        match &self.geometry {
            Geometry::Bracket(frames) => {
                if let Some(k) = self.kernel {
                    fact("kernel", k.name().into());
                }
                for (k, r) in frames.iter().enumerate() {
                    fact("exposure", format!("{k} {}", r.exposure));
                    if let Some(t) = r.shift {
                        fact("shift", format!("{k} {}", join(&t.m)));
                    }
                }
            }
            Geometry::Pano(laid) => {
                fact("anchor", laid.anchor.to_string());
                fact("kernel", laid.kernel.name().into());
                match &laid.placed {
                    Placed::Turned {
                        focal,
                        turns,
                        projection,
                    } => {
                        fact(
                            "projection",
                            match projection {
                                Projection::Planar => "planar",
                                Projection::Cylinder(Axis::Down) => "cylinder-down",
                                Projection::Cylinder(Axis::Across) => "cylinder-across",
                            }
                            .into(),
                        );
                        fact("focal", focal.to_string());
                        for (k, q) in turns.iter().enumerate() {
                            fact("turn", format!("{k} {}", join(q)));
                        }
                    }
                    Placed::Planar { homographies } => {
                        fact("projection", "homographies".into());
                        for (k, h) in homographies.iter().enumerate() {
                            fact("homography", format!("{k} {}", join(&h.m)));
                        }
                    }
                }
                for c in &laid.lens {
                    fact("correction", correction_text(c));
                }
                for (k, g) in laid.gains.iter().enumerate() {
                    fact("gain", format!("{k} {}", join(g)));
                }
                let c = &laid.canvas;
                fact(
                    "canvas",
                    format!("{} {} {} {}", c.x0, c.y0, c.width, c.height),
                );
                let r = &laid.crop;
                fact("crop", format!("{} {} {} {}", r.x, r.y, r.width, r.height));
            }
            Geometry::Stack(stacked) => {
                fact("selectivity", stacked.selectivity.to_string());
                fact("residual", stacked.max_residual.to_string());
                if stacked.uncompressed {
                    fact("samples", "uncompressed".into());
                }
                for (k, t) in stacked.placed.iter().enumerate() {
                    match t {
                        Some(t) => fact("shift", format!("{k} {}", join(&t.m))),
                        None => fact("dropped", k.to_string()),
                    }
                }
            }
        }
    }
}

/// The recipe's facts as they are read, put together once every line
/// is in.
#[derive(Default)]
struct Read {
    /// Whether any fact was the recipe's.
    seen: bool,
    /// The first recipe fact that did not read: the recipe is dropped,
    /// the rest of the tag kept.
    broken: Option<String>,
    ca: Option<String>,
    denoise: Option<Option<String>>,
    noise: Option<Noise>,
    deghost: Option<String>,
    exposures: Vec<(usize, f64)>,
    shifts: Vec<(usize, Transform)>,
    anchor: Option<usize>,
    kernel: Option<Kernel>,
    projection: Option<String>,
    focal: Option<f64>,
    turns: Vec<(usize, [f64; 4])>,
    homographies: Vec<(usize, [f64; 9])>,
    lens: Vec<LensCorrection>,
    gains: Vec<(usize, [f64; 3])>,
    canvas: Option<Canvas>,
    crop: Option<Rect>,
    selectivity: Option<f32>,
    residual: Option<f32>,
    dropped: Vec<usize>,
    uncompressed: bool,
}

/// One value a frame, in the frames' order: none unless each of the
/// `n` frames has exactly one.
fn each<T: Clone>(mut items: Vec<(usize, T)>, n: usize) -> Option<Vec<T>> {
    items.sort_by_key(|(k, _)| *k);
    (items.len() == n && items.iter().enumerate().all(|(i, (k, _))| *k == i))
        .then(|| items.into_iter().map(|(_, v)| v).collect())
}

impl Read {
    /// Whether `key` is the recipe's, and if it is, its value taken.
    fn take(&mut self, key: &str, value: &str) -> bool {
        let ok = match key {
            "ca" => {
                self.ca = Some(value.to_string());
                true
            }
            "denoise" => {
                self.denoise = Some((value != "none").then(|| value.to_string()));
                true
            }
            "noise" => numbers::<2, f32>(value, ' ')
                .map(|[shot, read]| self.noise = Some(Noise { shot, read }))
                .is_some(),
            "deghost" => {
                self.deghost = Some(value.to_string());
                true
            }
            "exposure" => of_frame::<1>(value)
                .map(|(k, [e])| self.exposures.push((k, e)))
                .is_some(),
            "shift" => of_frame::<6>(value)
                .map(|(k, m)| self.shifts.push((k, Transform { m })))
                .is_some(),
            "anchor" => value.parse().map(|a| self.anchor = Some(a)).is_ok(),
            "kernel" => Kernel::named(value)
                .map(|k| self.kernel = Some(k))
                .is_some(),
            "projection" => {
                self.projection = Some(value.to_string());
                true
            }
            "focal" => numbers::<1, f64>(value, ' ')
                .filter(|[f]| *f > 0.0)
                .map(|[f]| self.focal = Some(f))
                .is_some(),
            "turn" => of_frame::<4>(value)
                .map(|(k, q)| self.turns.push((k, q)))
                .is_some(),
            "homography" => of_frame::<9>(value)
                .map(|(k, m)| self.homographies.push((k, m)))
                .is_some(),
            "correction" => parse_correction(value).map(|c| self.lens.push(c)).is_some(),
            "gain" => of_frame::<3>(value)
                .map(|(k, g)| self.gains.push((k, g)))
                .is_some(),
            "canvas" => {
                let parts: Vec<&str> = value.split(' ').collect();
                match parts.as_slice() {
                    [x0, y0, w, h] => (|| {
                        self.canvas = Some(Canvas {
                            x0: x0.parse().ok().filter(|v: &f64| v.is_finite())?,
                            y0: y0.parse().ok().filter(|v: &f64| v.is_finite())?,
                            width: w.parse().ok()?,
                            height: h.parse().ok()?,
                        });
                        Some(())
                    })()
                    .is_some(),
                    _ => false,
                }
            }
            "selectivity" => numbers::<1, f32>(value, ' ')
                .map(|[s]| self.selectivity = Some(s))
                .is_some(),
            "residual" => numbers::<1, f32>(value, ' ')
                .map(|[r]| self.residual = Some(r))
                .is_some(),
            "dropped" => value.parse().map(|k| self.dropped.push(k)).is_ok(),
            "samples" => {
                self.uncompressed = value == "uncompressed";
                value == "uncompressed"
            }
            "crop" => numbers::<4, usize>(value, ' ')
                .map(|[x, y, width, height]| {
                    self.crop = Some(Rect {
                        x,
                        y,
                        width,
                        height,
                    })
                })
                .is_some(),
            _ => return false,
        };
        self.seen = true;
        if !ok && self.broken.is_none() {
            self.broken = Some(format!("{key}={value}"));
        }
        true
    }

    /// The recipe of a merge of `kind` from `n` frames; the first
    /// fact that did not read, or that it lacks, when it is not whole.
    fn finish(self, kind: &str, n: usize) -> Result<Recipe, String> {
        if let Some(fact) = self.broken {
            return Err(fact);
        }
        let one = |key: &str| format!("no {key}=");
        let every = |key: &str| format!("not one {key}= a frame");
        let mut kernel = None;
        let geometry = match kind {
            "hdr" => {
                let exposures = each(self.exposures, n).ok_or_else(|| every("exposure"))?;
                let mut frames: Vec<Registration> = exposures
                    .into_iter()
                    .map(|exposure| Registration {
                        exposure,
                        shift: None,
                    })
                    .collect();
                for (k, t) in self.shifts {
                    frames
                        .get_mut(k)
                        .ok_or_else(|| format!("shift={k} past the frames"))?
                        .shift = Some(t);
                }
                kernel = Some(self.kernel.unwrap_or(Kernel::Bilinear));
                Geometry::Bracket(frames)
            }
            "pano" => {
                let placed = match self
                    .projection
                    .as_deref()
                    .ok_or_else(|| one("projection"))?
                {
                    "homographies" => Placed::Planar {
                        homographies: each(self.homographies, n)
                            .ok_or_else(|| every("homography"))?
                            .into_iter()
                            .map(|m| Homography { m })
                            .collect(),
                    },
                    p => Placed::Turned {
                        focal: self.focal.ok_or_else(|| one("focal"))?,
                        turns: each(self.turns, n).ok_or_else(|| every("turn"))?,
                        projection: match p {
                            "planar" => Projection::Planar,
                            "cylinder-down" => Projection::Cylinder(Axis::Down),
                            "cylinder-across" => Projection::Cylinder(Axis::Across),
                            _ => return Err(format!("projection={p}")),
                        },
                    },
                };
                let anchor = self
                    .anchor
                    .filter(|&a| a < n)
                    .ok_or_else(|| one("anchor"))?;
                Geometry::Pano(Box::new(Laid {
                    anchor,
                    kernel: self.kernel.ok_or_else(|| one("kernel"))?,
                    placed,
                    lens: self.lens,
                    gains: each(self.gains, n).ok_or_else(|| every("gain"))?,
                    canvas: self.canvas.ok_or_else(|| one("canvas"))?,
                    crop: self.crop.ok_or_else(|| one("crop"))?,
                }))
            }
            "stack" => {
                // Each frame placed or left out, once.
                let mut placed: Vec<Option<Option<Transform>>> = vec![None; n];
                let mut put = |k: usize, t: Option<Transform>, key: &str| match placed.get_mut(k) {
                    Some(slot @ None) => {
                        *slot = Some(t);
                        Ok(())
                    }
                    Some(Some(_)) => Err(format!("{key}={k} a second time")),
                    None => Err(format!("{key}={k} past the frames")),
                };
                for (k, t) in self.shifts {
                    put(k, Some(t), "shift")?;
                }
                for k in self.dropped {
                    put(k, None, "dropped")?;
                }
                Geometry::Stack(Stacked {
                    selectivity: self.selectivity.ok_or_else(|| one("selectivity"))?,
                    max_residual: self.residual.ok_or_else(|| one("residual"))?,
                    placed: placed
                        .into_iter()
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| every("shift= or dropped"))?,
                    uncompressed: self.uncompressed,
                })
            }
            k => return Err(format!("merge={k}")),
        };
        Ok(Recipe {
            ca: self.ca.ok_or_else(|| one("ca"))?,
            denoise: self.denoise.ok_or_else(|| one("denoise"))?,
            noise: self.noise,
            deghost: self.deghost,
            kernel,
            geometry,
        })
    }
}

impl Tag {
    /// The tag's bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut text = format!(
            "merge={}\nreference={}\ndenoised={}\nheadroom={:.4}\n",
            self.kind,
            self.reference,
            u8::from(self.denoised),
            self.headroom_stops
        );
        if self.lens_corrected {
            text.push_str("lens=corrected\n");
        }
        for (k, f) in self.frames.iter().enumerate() {
            // A name is one line: a line break in it would end the fact.
            text.push_str(&format!("frame={}\n", f.replace(['\n', '\r'], " ")));
            if let Some(Some(h)) = self.heads.get(k) {
                text.push_str(&format!("head={h}\n"));
            }
            if let Some(Some(h)) = self.wholes.get(k) {
                text.push_str(&format!("whole={h}\n"));
            }
        }
        if let Some(recipe) = &self.recipe {
            recipe.write(&mut text);
        }
        let mut out = MAKER.to_vec();
        out.extend_from_slice(text.as_bytes());
        out
    }

    /// The tag from `DNGPrivateData`'s bytes; none for another maker's
    /// data, or ours without a kind. A recipe that does not read whole
    /// is left out, and the rest kept.
    pub fn parse(bytes: &[u8]) -> Option<Tag> {
        let text = std::str::from_utf8(bytes.strip_prefix(MAKER)?).ok()?;
        let mut tag = Tag::default();
        let mut read = Read::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "merge" => tag.kind = value.to_string(),
                "reference" => tag.reference = value.parse().ok()?,
                "denoised" => tag.denoised = value == "1",
                "headroom" => {
                    tag.headroom_stops = value
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite() && (0.0..=32.0).contains(v))?;
                }
                "lens" => tag.lens_corrected = value == "corrected",
                "frame" => {
                    tag.frames.push(value.to_string());
                    tag.heads.push(None);
                    tag.wholes.push(None);
                }
                "head" => {
                    if let Some(h) = tag.heads.last_mut() {
                        *h = Some(value.to_string());
                    }
                }
                "whole" => {
                    if let Some(h) = tag.wholes.last_mut() {
                        *h = Some(value.to_string());
                    }
                }
                _ => {
                    read.take(key, value);
                }
            }
        }
        if tag.kind.is_empty() {
            return None;
        }
        if read.seen {
            match read.finish(&tag.kind, tag.frames.len()) {
                Ok(recipe) => tag.recipe = Some(recipe),
                Err(fact) => tag.unread = Some(fact),
            }
        }
        Some(tag)
    }
}

#[cfg(test)]
mod tests {
    use super::super::pano::{IDENTITY, rotation};
    use super::*;

    #[test]
    fn a_merge_measured_its_ca_unless_its_recipe_says_off() {
        let mut tag = hdr();
        assert!(tag.ca_measured());
        tag.recipe.as_mut().unwrap().ca = "off".into();
        assert!(!tag.ca_measured());
        tag.recipe = None;
        assert!(tag.ca_measured());
        tag.lens_corrected = true;
        assert!(!tag.ca_measured());
    }

    fn hdr() -> Tag {
        Tag {
            kind: "hdr".into(),
            frames: vec!["a.CR3".into(), "b.CR3".into(), "c.CR3".into()],
            reference: 1,
            denoised: true,
            headroom_stops: 2.5,
            lens_corrected: false,
            heads: vec![Some("0f".repeat(32)), Some("0f".repeat(32)), None],
            wholes: vec![Some("1e".repeat(32)), Some("2d".repeat(32)), None],
            unread: None,
            recipe: Some(Recipe {
                ca: "once".into(),
                denoise: Some("balanced".into()),
                noise: Some(Noise {
                    shot: 1.234_567_8e-5,
                    read: 3.0e-8,
                }),
                deghost: Some("auto".into()),
                kernel: Some(Kernel::Lanczos3),
                geometry: Geometry::Bracket(vec![
                    Registration {
                        exposure: 0.263_157_894_736_842_1,
                        shift: Some(Transform {
                            m: [1.0, 0.0, 1.802_345_678_901_234_5, 0.0, 1.0, -0.1 / 3.0],
                        }),
                    },
                    Registration {
                        exposure: 1.0,
                        shift: None,
                    },
                    Registration {
                        exposure: 4.078_7,
                        shift: None,
                    },
                ]),
            }),
        }
    }

    fn pano(placed: Placed) -> Tag {
        Tag {
            kind: "pano".into(),
            frames: vec!["x.NEF".into(), "y.NEF".into()],
            reference: 0,
            denoised: false,
            headroom_stops: 1.37,
            lens_corrected: true,
            heads: vec![Some("ab".repeat(32)), Some("cd".repeat(32))],
            wholes: vec![None, None],
            unread: None,
            recipe: Some(Recipe {
                ca: "per-frame".into(),
                denoise: None,
                noise: None,
                deghost: None,
                kernel: None,
                geometry: Geometry::Pano(Box::new(Laid {
                    anchor: 1,
                    kernel: Kernel::Lanczos3,
                    placed,
                    lens: vec![
                        LensCorrection {
                            distortion: Some(Distortion::Ptlens {
                                a: 0.012_345_67,
                                b: -0.03,
                                c: 1e-7,
                            }),
                            chromatic_aberration: Some(ChromaticAberration {
                                red: [1.000_2, 0.0, -1e-5],
                                blue: [0.999_8, 1e-6, 0.0],
                            }),
                            vignetting: Some(Vignetting {
                                k: [-0.5, 0.1, -0.01],
                            }),
                            radius_scale: 0.987_654_3,
                            vignetting_scale: 1.1,
                            scale: Scale::Fixed(1.02),
                        },
                        LensCorrection {
                            distortion: Some(Distortion::Acm {
                                k: [0.1, -0.2, 0.3, 1e-4, -1e-4],
                                focal_scale: 0.6,
                            }),
                            ..LensCorrection::default()
                        },
                        LensCorrection {
                            distortion: Some(Distortion::Poly5 {
                                k1: 0.01,
                                k2: -2e-3,
                            }),
                            ..LensCorrection::default()
                        },
                        LensCorrection {
                            distortion: Some(Distortion::Poly3 { k1: 1e-4 }),
                            ..LensCorrection::default()
                        },
                    ],
                    gains: vec![[1.0; 3], [1.176_47, 1.0 / 3.0, 0.987_654_321_012_345_6]],
                    canvas: Canvas {
                        x0: -3041.0,
                        y0: -1501.0,
                        width: 6243,
                        height: 3017,
                    },
                    crop: Rect {
                        x: 72,
                        y: 88,
                        width: 6080,
                        height: 2840,
                    },
                })),
            }),
        }
    }

    /// Every fact reads back as written: names, hashes, the options, and
    /// the geometry to the bit (the rotations as kept, through their
    /// quaternions).
    #[test]
    fn the_tag_round_trips() {
        let tag = hdr();
        assert_eq!(Tag::parse(&tag.to_bytes()), Some(tag.clone()));
        let named = Tag {
            frames: vec!["a.CR3".into(), "b\nc.CR3".into()],
            heads: vec![None, None],
            wholes: vec![None, None],
            recipe: None,
            ..tag
        };
        let back = Tag::parse(&named.to_bytes()).unwrap();
        assert_eq!(back.frames, ["a.CR3", "b c.CR3"]);
        assert_eq!((back.reference, back.denoised), (1, true));
        assert_eq!(back.headroom_stops, 2.5);
        assert_eq!(back.recipe, None);

        let turned = Placed::of(&Model::Turned {
            focal: 2_563.123_456_789,
            rotations: vec![
                rotation([0.01, -0.7, 0.003]),
                IDENTITY,
                rotation([-3.0, 0.2, 0.1]),
            ],
            projection: Projection::Cylinder(Axis::Down),
        });
        let mut tag = pano(turned);
        tag.frames.push("z.NEF".into());
        tag.heads.push(Some("ef".repeat(32)));
        tag.wholes.push(None);
        if let Some(Recipe {
            geometry: Geometry::Pano(laid),
            ..
        }) = tag.recipe.as_mut()
        {
            laid.gains.push([0.5, 0.25, 0.125]);
        }
        assert_eq!(Tag::parse(&tag.to_bytes()), Some(tag.clone()));
        for projection in [Projection::Planar, Projection::Cylinder(Axis::Across)] {
            let mut t = tag.clone();
            if let Some(Recipe {
                geometry: Geometry::Pano(laid),
                ..
            }) = t.recipe.as_mut()
                && let Placed::Turned { projection: p, .. } = &mut laid.placed
            {
                *p = projection;
            }
            assert_eq!(Tag::parse(&t.to_bytes()), Some(t));
        }
        let planar = pano(Placed::Planar {
            homographies: vec![
                Homography::IDENTITY,
                Homography {
                    m: [
                        1.01,
                        0.002,
                        -812.345_678_9,
                        -0.001,
                        0.99,
                        3.25,
                        1e-6,
                        -2e-7,
                        1.0,
                    ],
                },
            ],
        });
        assert_eq!(Tag::parse(&planar.to_bytes()), Some(planar));

        assert_eq!(Tag::parse(b"Adobe\0MakN"), None);
        assert_eq!(Tag::parse(b"greycard\0reference=1\n"), None);
    }

    /// A file written before the recipe was kept reads as it did, with
    /// no recipe and no hashes; so does one whose recipe is short of a
    /// fact or has one that does not read, its other facts kept.
    #[test]
    fn an_old_tag_or_a_broken_recipe_reads_without_one() {
        let old = b"greycard\0merge=pano\nreference=1\ndenoised=0\nheadroom=1.3700\n\
                    lens=corrected\nframe=a.NEF\nframe=b.NEF\n";
        let tag = Tag::parse(old).unwrap();
        assert_eq!(tag.kind, "pano");
        assert!(tag.lens_corrected);
        assert_eq!(tag.frames, ["a.NEF", "b.NEF"]);
        assert_eq!(tag.heads, [None, None]);
        assert_eq!((tag.recipe, tag.unread), (None, None));

        let full = String::from_utf8(hdr().to_bytes()).unwrap();
        let short = full.replace("exposure=2 4.0787\n", "");
        let back = Tag::parse(short.as_bytes()).unwrap();
        assert_eq!((back.kind.as_str(), back.recipe), ("hdr", None));
        assert_eq!(back.unread.as_deref(), Some("not one exposure= a frame"));
        let broken = full.replace("exposure=2 4.0787", "exposure=2 lots");
        let back = Tag::parse(broken.as_bytes()).unwrap();
        assert_eq!((back.reference, back.recipe), (1, None));
        assert_eq!(back.unread.as_deref(), Some("exposure=2 lots"));
        assert_eq!(back.heads[0].as_deref(), Some("0f".repeat(32).as_str()));
        // A bracket written before its deghosting was kept reads with
        // none, which the engine makes again as off.
        let before = full.replace("deghost=auto\n", "");
        assert_ne!(before, full);
        let back = Tag::parse(before.as_bytes()).unwrap();
        assert_eq!(back.recipe.map(|r| r.deghost), Some(None));
        // And one written before its kernel was kept was resampled
        // bilinearly.
        let before = full.replace("kernel=lanczos3\n", "");
        assert_ne!(before, full);
        let back = Tag::parse(before.as_bytes()).unwrap();
        assert_eq!(back.recipe.and_then(|r| r.kernel), Some(Kernel::Bilinear));
        let no_ca = full.replace("ca=once\n", "");
        let back = Tag::parse(no_ca.as_bytes()).unwrap();
        assert_eq!(back.unread.as_deref(), Some("no ca="));
    }

    /// A focus stack's recipe reads back to the bit: its options, each
    /// frame's transform, and the frame it left out. One with a frame
    /// neither placed nor left out, or with no selectivity, reads
    /// without it.
    #[test]
    fn a_stacks_recipe_round_trips() {
        let tag = Tag {
            kind: "stack".into(),
            frames: vec!["a.CR3".into(), "b.CR3".into(), "c.CR3".into()],
            reference: 1,
            denoised: false,
            headroom_stops: 0.75,
            lens_corrected: false,
            heads: vec![Some("0f".repeat(32)), Some("1e".repeat(32)), None],
            wholes: vec![None, None, None],
            unread: None,
            recipe: Some(Recipe {
                ca: "once".into(),
                denoise: Some("fast".into()),
                noise: None,
                deghost: None,
                kernel: None,
                geometry: Geometry::Stack(Stacked {
                    selectivity: 2.0,
                    max_residual: 0.6,
                    placed: vec![
                        Some(Transform {
                            m: [1.003_1, -0.000_2, -9.25, 0.000_2, 1.003_1, 1.0 / 3.0],
                        }),
                        Some(Transform::IDENTITY),
                        None,
                    ],
                    uncompressed: true,
                }),
            }),
        };
        assert_eq!(Tag::parse(&tag.to_bytes()), Some(tag.clone()));
        let full = String::from_utf8(tag.to_bytes()).unwrap();
        let short = full.replace("dropped=2\n", "");
        let back = Tag::parse(short.as_bytes()).unwrap();
        assert_eq!(back.recipe, None);
        assert_eq!(
            back.unread.as_deref(),
            Some("not one shift= or dropped= a frame")
        );
        let twice = full.replace("dropped=2\n", "dropped=2\ndropped=2\n");
        let back = Tag::parse(twice.as_bytes()).unwrap();
        assert_eq!(back.unread.as_deref(), Some("dropped=2 a second time"));
        let bare = full.replace("selectivity=2\n", "");
        let back = Tag::parse(bare.as_bytes()).unwrap();
        assert_eq!(back.unread.as_deref(), Some("no selectivity="));
    }

    /// A rotation kept as its quaternion comes back within rounding,
    /// the quaternion unit and its `w` not negative.
    #[test]
    fn a_rotation_round_trips_through_its_quaternion() {
        for w in [
            [0.0, 0.0, 0.0],
            [0.1, -0.7, 0.03],
            [3.1, 0.0, 0.0],
            [0.0, -3.1, 0.0],
            [0.0, 0.0, 3.0],
            [1.5, 1.5, -1.5],
        ] {
            let r = rotation(w);
            let back = of_quaternion(quaternion(&r));
            for (a, b) in r.iter().zip(&back) {
                assert!((a - b).abs() < 1e-14, "{w:?}: {r:?} {back:?}");
            }
            let q = quaternion(&r);
            assert!(q[0] >= 0.0);
            assert!((q.iter().map(|v| v * v).sum::<f64>() - 1.0).abs() < 1e-15);
        }
    }
}
