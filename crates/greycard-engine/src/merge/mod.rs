//! The tripod HDR merge: a bracket's frames walked one at a time to
//! camera RGB through the stage list (`Purpose::MergeInput`), merged
//! by the core's `merge` as they come, and written as a linear DNG
//! beside the first source.
//!
//! The reference (the middle exposure, unless asked) is walked first
//! and stays, and its CA fit corrects every frame; every other frame
//! is walked, checked for movement, measured against the reference
//! for its exposure, added to the sums, and let go. The darkest frame
//! comes last, walked with its highlights reconstructed, so that where
//! every frame clipped its value stands. What is held at once is the
//! reference, its CA fit, one frame, and the sums, whatever the count.
//!
//! The command line's `greycard merge hdr` and the window's Merge to
//! HDR both run [`hdr`]; neither has a merge of its own. The panorama
//! is [`pano`]'s, and the focus stack [`focus`]'s.

pub mod focus;
pub mod pano;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use greycard_core::decode::RawMetadata;
use greycard_core::develop::ca::CaFit;
use greycard_core::dng::{LinearDngOptions, Merged, write_merged_dng};
use greycard_core::merge::{
    self, Accumulator, Frame, Geometry, Measured, Noise, Recipe, Registration,
};
use greycard_core::output::OnExists;
use greycard_core::raw::{RawFrame, Samples, Shot};
use greycard_core::register::{self, Edge, Kernel, Model};
use greycard_core::{CameraImage, WhiteBalance};
use greycard_edit::Edit;

use crate::stages::{CaFitRef, MergeCa, MergeInput, Purpose, Stage, stages};
use crate::walk::{Buf, Input, Kept, Models, NoHold, plan, walk};

/// What goes after the first frame's name.
pub const SUFFIX: &str = "-HDR";

/// What a merge is asked.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The frame the others are put on, by its place among the frames;
    /// none for the middle exposure.
    pub reference: Option<usize>,
    /// Where the file goes; none for beside the first frame, named
    /// after it ([`default_output`]).
    pub out: Option<PathBuf>,
    /// What to do when that file is there already.
    pub on_exists: OnExists,
    /// The learned denoiser in each frame's demosaic's place; none for
    /// a bracket unless asked.
    pub denoiser: Option<&'static greycard_ai::Model>,
    /// The sensor's noise in the reference's units, in place of what
    /// the frames' pairs measure: for a measurement whose noise is
    /// known.
    pub noise: Option<Noise>,
    /// How the frames' CA is fitted.
    pub ca: CaFits,
    /// Each frame's registration as a recipe keeps it, in place of
    /// measuring: its exposure and its move, the frames' order. For a
    /// merge made again ([`again`]).
    pub registration: Option<Vec<Registration>>,
    /// Whether what moved in the scene is taken from the reference
    /// alone.
    pub deghost: Deghost,
    /// The kernel a moved frame is resampled with; none for Lanczos-3.
    /// A recipe written before it was kept resampled bilinearly.
    pub kernel: Option<Kernel>,
}

/// Whether a merge takes what moved in the scene between its frames
/// from the reference alone (the core's `merge::ghost`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Deghost {
    On,
    /// Never: a tripod's bracket of a still scene.
    Off,
    /// When any frame's fit was believed moved: a bracket shot by
    /// hand. A tripod's has no ghost a four-sigma test should be let
    /// find in its noise.
    #[default]
    Auto,
}

impl Deghost {
    /// Its name in a merge's recipe and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Deghost::On => "on",
            Deghost::Off => "off",
            Deghost::Auto => "auto",
        }
    }

    /// The setting of that name.
    pub fn named(name: &str) -> Option<Deghost> {
        [Deghost::On, Deghost::Off, Deghost::Auto]
            .into_iter()
            .find(|d| d.name() == name)
    }
}

/// How a merge fits its frames' lateral CA.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CaFits {
    /// Once, on the reference, and applied to every frame.
    #[default]
    Once,
    /// Each frame its own: for a measurement of what the one fit is
    /// worth.
    PerFrame,
    /// Not at all: the same measurement's baseline.
    Off,
}

impl CaFits {
    /// Its name in a merge's recipe.
    pub fn name(self) -> &'static str {
        match self {
            CaFits::Once => "once",
            CaFits::PerFrame => "per-frame",
            CaFits::Off => "off",
        }
    }

    /// The fit of that name.
    pub fn named(name: &str) -> Option<CaFits> {
        [CaFits::Once, CaFits::PerFrame, CaFits::Off]
            .into_iter()
            .find(|c| c.name() == name)
    }
}

/// Where a merge is, for a caller to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The `index`th frame of `of` (from 1) is being read.
    Frame { index: usize, of: usize },
    /// A panorama's frames are being placed on its canvas, or a focus
    /// stack's onto its reference.
    Placing,
    /// A panorama's or a focus stack's `index`th frame of `of` is being
    /// blended in.
    Blending { index: usize, of: usize },
    /// The file is being written.
    Writing,
}

/// What a merge says as it goes.
pub trait Progress {
    fn step(&self, step: Step);
}

impl<F: Fn(Step)> Progress for F {
    fn step(&self, step: Step) {
        self(step)
    }
}

/// What became of one frame.
#[derive(Debug, Clone)]
pub struct FrameReport {
    /// The frame's name.
    pub name: String,
    /// Its exposure relative to the reference's by EXIF; none when
    /// either frame's EXIF has no shutter.
    pub exif: Option<f64>,
    /// As measured from the pixels; none for the reference, or when
    /// too few pixels held well in both.
    pub measured: Option<Measured>,
    /// The exposure the merge used.
    pub exposure: f64,
    /// How far it had moved from the reference, at the farthest
    /// corner, in pixels; none for the reference or a fit that did not
    /// hold.
    pub moved: Option<f64>,
    /// Whether it was resampled onto the reference.
    pub resampled: bool,
    /// The share of its pixels the deghosting took from the reference
    /// instead; none for the reference, or when it did not run.
    pub rejected: Option<f64>,
    /// Its noise as the weights read it.
    pub noise: Noise,
}

/// Why a merge whose frames asked for a learned tier made them without
/// it: said on the finished line and as a CLI note, and logged. The
/// card's wins when both apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LearnedSkipped {
    /// A frame is X-Trans, which the learned denoiser has no version for.
    XTrans,
    /// The learned denoiser is off this graphics card: it lost it.
    OffCard,
}

impl LearnedSkipped {
    /// The CLI's note, and the log's.
    pub fn words(self) -> &'static str {
        match self {
            Self::XTrans => "no learned denoise on X-Trans",
            Self::OffCard => "no learned denoise on this graphics card",
        }
    }

    /// Why a frame's walk skipped the learned denoiser, from its list's
    /// skips: one of these two, or none.
    fn of(skipped: &[(&'static str, crate::stages::Skip)]) -> Option<Self> {
        skipped.iter().find_map(|(name, why)| match why {
            crate::stages::Skip::Unavailable(w) if *name == crate::stages::LEARNED_DENOISER => {
                if *w == crate::stages::OFF_CARD {
                    Some(Self::OffCard)
                } else if *w == crate::stages::NO_XTRANS {
                    Some(Self::XTrans)
                } else {
                    None
                }
            }
            _ => None,
        })
    }

    /// The reason for the whole merge, from its frames': the card's
    /// first, then X-Trans, logged once.
    pub(crate) fn of_frames(frames: impl IntoIterator<Item = Option<Self>>) -> Option<Self> {
        let worst = frames.into_iter().flatten().max();
        if let Some(why) = worst {
            tracing::info!("merge: {}", why.words());
        }
        worst
    }
}

/// What a merge did.
#[derive(Debug, Clone)]
pub struct Report {
    pub out: PathBuf,
    pub reference: usize,
    pub frames: Vec<FrameReport>,
    pub width: usize,
    pub height: usize,
    /// Pixels every frame clipped, taken from the darkest frame.
    pub all_clipped: usize,
    /// The stops above the reference's clip the file holds.
    pub headroom_stops: f32,
    pub noise_profile: [f64; 2],
    /// Whether the frames were denoised.
    pub denoised: bool,
    /// The seconds each part took, in order.
    pub seconds: Vec<(&'static str, f64)>,
    /// What the merge went without or did that a tripod bracket should
    /// not need, each in words: a frame resampled, an exposure not
    /// measured, a denoiser not had.
    pub warnings: Vec<String>,
    /// Why frames that asked for a learned tier went without it.
    pub learned_skipped: Option<LearnedSkipped>,
}

/// `<first frame>-HDR.dng`, beside the first frame.
pub fn default_output(first: &Path) -> PathBuf {
    let stem = first
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "merge".into());
    let name = format!("{stem}{SUFFIX}.dng");
    match first.parent() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// What a frame's head says: its body and its shot.
#[derive(Debug, Clone)]
pub struct Head {
    pub make: String,
    pub model: String,
    pub shot: Shot,
}

/// The frames of a merge: files on disk, or made up for a measurement.
/// Each is read when the merge comes to it, and let go after.
pub trait Sources {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The frame's name, for a report and the file's tag.
    fn name(&self, i: usize) -> String;
    /// Its body and shot, without its samples.
    fn head(&self, i: usize) -> Result<Head>;
    /// The frame and its metadata.
    fn load(&self, i: usize) -> Result<(RawFrame, RawMetadata)>;
    /// Its file, when it is one: a merge is never written over it.
    fn path(&self, _i: usize) -> Option<PathBuf> {
        None
    }
    /// Its content hash, the library's, for the file's tag: none for a
    /// frame that is no file.
    fn hash(&self, i: usize) -> Option<String> {
        let path = self.path(i)?;
        greycard_library::hash_file(&path)
            .map_err(|e| log::info!("merge: {}: no hash: {e}", path.display()))
            .ok()
    }
    /// Its whole file's BLAKE3, for a frame whose head hash another
    /// frame shares: none for a frame that is no file.
    fn whole(&self, i: usize) -> Option<String> {
        let path = self.path(i)?;
        whole_hash(&path)
            .map_err(|e| log::info!("merge: {}: no hash: {e}", path.display()))
            .ok()
    }
}

/// BLAKE3 over every byte of `path`: the index's `whole_hash`.
fn whole_hash(path: &Path) -> std::io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Each frame's head hash for the file's tag, and beside it its whole
/// file's hash where another frame has the same head (raws of one size
/// with no compression, shot in one burst, can): the head alone would
/// take one for the other.
fn hashes(sources: &dyn Sources) -> (Vec<Option<String>>, Vec<Option<String>>) {
    let heads: Vec<Option<String>> = (0..sources.len()).map(|i| sources.hash(i)).collect();
    let wholes = (0..heads.len())
        .map(|i| {
            let shared =
                heads[i].is_some() && (0..heads.len()).any(|j| j != i && heads[j] == heads[i]);
            shared.then(|| sources.whole(i)).flatten()
        })
        .collect();
    (heads, wholes)
}

/// What the frames' heads say: each one's EXIF exposure, the
/// reference (the frame asked, or the middle exposure), and each one's
/// ISO over the reference's (1 where either is not known).
struct Heads {
    exif: Vec<Option<f64>>,
    reference: usize,
    iso: Vec<f64>,
}

/// The frames' [`Heads`]. Frames of two bodies, and a frame asked for
/// that is not there, are refused.
fn heads(sources: &dyn Sources, asked: Option<usize>) -> Result<Heads> {
    let of = sources.len();
    let heads = (0..of)
        .map(|i| sources.head(i))
        .collect::<Result<Vec<_>>>()?;
    let body = |h: &Head| (h.make.clone(), h.model.clone());
    if heads.iter().any(|h| body(h) != body(&heads[0])) {
        bail!("a merge wants frames of one body");
    }
    let exif: Vec<Option<f64>> = heads
        .iter()
        .map(|h| merge::exif_exposure(&h.shot))
        .collect();
    let reference = match asked {
        Some(r) if r < of => r,
        Some(r) => bail!(
            "there is no frame {r}: the frames count from 0 to {}",
            of.saturating_sub(1)
        ),
        None => merge::middle(&exif),
    };
    let iso = heads
        .iter()
        .map(|h| match (h.shot.iso, heads[reference].shot.iso) {
            (Some(a), Some(b)) if a > 0 && b > 0 => f64::from(a) / f64::from(b),
            _ => 1.0,
        })
        .collect();
    Ok(Heads {
        exif,
        reference,
        iso,
    })
}

/// The frame of `files` a merge puts the others on, as [`hdr`] picks
/// it: for a caller that wants the reference's edit before the merge.
pub fn reference_of(files: &[PathBuf], asked: Option<usize>) -> Result<usize> {
    if files.len() < 2 {
        bail!("a merge wants two frames or more; got {}", files.len());
    }
    Ok(heads(&Files(files), asked)?.reference)
}

/// The edit a merge's file starts with: the reference frame's, less
/// its exposure (the file opens at the reference's already, and a
/// reference pulled down to save its highlights would pull the
/// merge's headroom down with it) and less the learned denoiser, which
/// a file with no mosaic cannot run.
pub fn edit_for_merge(reference: &Edit) -> Edit {
    let mut edit = reference.clone();
    edit.light.exposure = 0.0;
    edit.noise.learned = greycard_edit::Learned::Off;
    edit
}

/// Raw files on disk.
pub struct Files<'a>(pub &'a [PathBuf]);

impl Sources for Files<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn name(&self, i: usize) -> String {
        name_of(&self.0[i])
    }

    fn head(&self, i: usize) -> Result<Head> {
        let file = &self.0[i];
        if greycard_core::picture::is_picture_path(file) {
            bail!(
                "{} is not a raw file: a bracket is merged before white balance and the \
                 matrix, and a developed picture is past both",
                file.display()
            );
        }
        let p = greycard_core::decode::probe_path(file)
            .with_context(|| format!("reading {}", file.display()))?;
        Ok(Head {
            make: p.make,
            model: p.model,
            shot: Shot {
                exposure_time: p.exposure_time.map(|t| t as f32),
                iso: p.iso,
                f_number: p.fnumber.map(|n| n as f32),
                ..Default::default()
            },
        })
    }

    fn load(&self, i: usize) -> Result<(RawFrame, RawMetadata)> {
        let file = &self.0[i];
        greycard_core::decode::decode_path_with_metadata(file)
            .with_context(|| format!("decoding {}", file.display()))
    }

    fn path(&self, i: usize) -> Option<PathBuf> {
        Some(self.0[i].clone())
    }
}

/// A frame as the merge reads it: camera RGB in its sensor units, its
/// clip mask, its noise.
struct Walked {
    rgb: Vec<f32>,
    mask: Vec<bool>,
    width: usize,
    height: usize,
    noise: Noise,
    gains: [f32; 3],
    white_balance: WhiteBalance,
    denoised: bool,
    stood_in: Option<String>,
    /// Why the learned denoiser was skipped on it, of the two a merge
    /// says.
    learned_skipped: Option<LearnedSkipped>,
    /// The CA's fit, when the frame was asked to hand it back.
    ca_fit: Option<Arc<CaFit>>,
}

/// `file` walked under `merge`: decoded, prepared with nothing clipped,
/// demosaiced, its gains divided back out. The frame is handed back
/// without its samples, for its tags. A frame not of `size`, the
/// reference's, is refused before the walk, whose CA fit would refuse
/// it in its own words.
fn walk_frame(
    sources: &dyn Sources,
    i: usize,
    edit: &Edit,
    merge: MergeInput,
    size: Option<(usize, usize)>,
    models: &mut dyn Models,
) -> Result<(Walked, RawFrame, RawMetadata)> {
    let (frame, metadata) = sources.load(i)?;
    let name = sources.name(i);
    if let Some((w, h)) = size
        && (frame.width, frame.height) != (w, h)
    {
        bail!(
            "{name} is {}x{} and the reference {w}x{h}: a merge wants frames of one size",
            frame.width,
            frame.height
        );
    }
    walk_loaded(&name, frame, metadata, edit, merge, models)
}

/// [`walk_frame`] of a frame already read.
fn walk_loaded(
    name: &str,
    frame: RawFrame,
    metadata: RawMetadata,
    edit: &Edit,
    merge: MergeInput,
    models: &mut dyn Models,
) -> Result<(Walked, RawFrame, RawMetadata)> {
    let frame = Arc::new(frame);
    let input = Input::Raw(frame.clone());
    let purpose = Purpose::MergeInput(merge);
    let steps = stages(edit, &purpose, &input.facts(0), None);
    let places = plan(&steps, &purpose, false);
    let mut kept = Kept::new();
    let walked = walk(
        &steps, &places, &input, &mut kept, models, &mut None, &NoHold,
    )
    .map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
    drop(input);
    drop(kept);
    // A frame the card had not the memory to denoise, asked a few
    // times, would mix the engine's denoise into a learned merge.
    if let Some(crate::walk::NoLearned::Short(why)) = &walked.meta.stood_in {
        bail!("{name}: {why}, and nothing was made");
    }
    let learned = steps.ops().any(|s| matches!(s, Stage::Learned(_)));
    // The card's refusal at the load (the plan did not skip it) is said
    // as the plan's skip is, the merge's note, not a stood-down warning.
    let refused_off_card = walked
        .meta
        .stood_in
        .as_ref()
        .is_some_and(crate::walk::NoLearned::off_card);
    let stood_in = walked
        .meta
        .stood_in
        .as_ref()
        .filter(|_| !refused_off_card)
        .map(|w| format!("{w:?}"));
    let (prepared, rgb, plain) = match walked.buf {
        Buf::Camera { prepared, rgb } => (prepared, rgb, None),
        Buf::CameraPair {
            prepared,
            network,
            plain,
        } => (prepared, network, Some(plain)),
        _ => bail!("{name}: the walk did not end at camera RGB"),
    };
    let gains = prepared.gains;
    let unbalanced = |mut v: Vec<f32>| {
        use rayon::prelude::*;
        v.par_chunks_mut(3).for_each(|p| {
            for c in 0..3 {
                p[c] /= gains[c];
            }
        });
        v
    };
    let rgb = unbalanced(rgb);
    // What clipped is read off the plain demosaic when a denoiser gave
    // the picture: a network smooths a saturated edge.
    let mask = match plain {
        Some(plain) => merge::clip_mask(&unbalanced(plain), prepared.width, prepared.height),
        None => merge::clip_mask(&rgb, prepared.width, prepared.height),
    };
    let noise = prepared
        .noise
        .map_or(Noise::ASSUMED, |e| Noise::of_model(&e.model));
    let out = Walked {
        rgb,
        mask,
        width: prepared.width,
        height: prepared.height,
        noise,
        gains,
        white_balance: prepared.white_balance,
        denoised: learned && stood_in.is_none() && !refused_off_card,
        stood_in,
        learned_skipped: LearnedSkipped::of(&steps.skipped)
            .max(refused_off_card.then_some(LearnedSkipped::OffCard)),
        ca_fit: walked.ca_fit,
    };
    drop(prepared);
    // The tags are read from the frame; its samples are let go.
    let mut frame = Arc::try_unwrap(frame).unwrap_or_else(|f| (*f).clone());
    frame.samples = Samples::U16(Vec::new());
    Ok((out, frame, metadata))
}

/// Merge the tripod bracket `files` under `edit`'s demosaic and CA, and
/// write it as a linear DNG, beside the first frame unless `options`
/// says where. Each frame's progress is said to `progress`; `models`
/// runs the learned denoiser when `options` asks.
pub fn hdr(
    files: &[PathBuf],
    edit: &Edit,
    options: &Options,
    models: &mut dyn Models,
    progress: &dyn Progress,
) -> Result<Report> {
    let first = files.first().map(|f| default_output(f)).unwrap_or_default();
    hdr_of(&Files(files), &first, edit, options, models, progress)
}

/// [`hdr`] over any [`Sources`], written to `options.out` or else
/// `default_out`.
pub fn hdr_of(
    sources: &dyn Sources,
    default_out: &Path,
    edit: &Edit,
    options: &Options,
    models: &mut dyn Models,
    progress: &dyn Progress,
) -> Result<Report> {
    let started = Instant::now();
    let mut seconds = Vec::new();
    let of = sources.len();
    if of < 2 {
        bail!("a merge wants two frames or more; got {of}");
    }
    let Heads {
        exif,
        reference,
        iso,
    } = heads(sources, options.reference)?;
    let relative: Vec<Option<f64>> = exif
        .iter()
        .map(|e| Some(e.as_ref()? / exif[reference]?))
        .collect();
    // The darkest by EXIF: a frame with none is taken as the
    // reference's exposure until it is measured.
    let darkest = (0..of)
        .min_by(|&a, &b| {
            relative[a]
                .unwrap_or(1.0)
                .total_cmp(&relative[b].unwrap_or(1.0))
                .then(a.cmp(&b))
        })
        .expect("two frames or more");
    if let Some(given) = &options.registration
        && given.len() != of
    {
        bail!(
            "the recipe registers {} frames and the merge has {of}",
            given.len()
        );
    }
    let out = match &options.out {
        Some(p) => p.clone(),
        None => default_out.to_path_buf(),
    };
    let reserved = hold(sources, &out, options.on_exists)?;
    let out = reserved.out().to_path_buf();
    let mut warnings = Vec::new();
    let mut reports: Vec<Option<FrameReport>> = vec![None; of];
    let mut registration = vec![
        Registration {
            exposure: 1.0,
            shift: None,
        };
        of
    ];

    // The reference, walked first and held.
    progress.step(Step::Frame { index: 1, of });
    let t = Instant::now();
    let (r, ref_frame, ref_meta) = walk_frame(
        sources,
        reference,
        edit,
        MergeInput {
            white: None,
            reconstruct: reference == darkest,
            denoiser: options.denoiser,
            ca: match options.ca {
                CaFits::Once => MergeCa::Make,
                CaFits::PerFrame => MergeCa::Own,
                CaFits::Off => MergeCa::Off,
            },
            lens: Vec::new(),
        },
        None,
        models,
    )?;
    seconds.push(("reference", t.elapsed().as_secs_f64()));
    if let Some(why) = &r.stood_in {
        warnings.push(format!("the learned denoiser stood down: {why}"));
    }
    let (w, h) = (r.width, r.height);
    let kernel = options.kernel.unwrap_or(Kernel::Lanczos3);
    let size = Some((ref_frame.width, ref_frame.height));
    let white = r.gains.map(f64::from);
    let mut denoised = r.denoised;
    let mut skipped = vec![r.learned_skipped];
    let ref_rgb = r.rgb;
    let ref_mask = r.mask;
    // Every other frame takes the reference's CA fit; where the
    // reference made none, as the reference was asked.
    let ca = match (r.ca_fit, options.ca) {
        (Some(fit), _) => MergeCa::Take(CaFitRef(fit)),
        (None, CaFits::Off) => MergeCa::Off,
        (None, _) => MergeCa::Own,
    };
    // The sensor's noise, in the reference's units: what the frames'
    // pairs say, the reference's own estimate until they have said it.
    // A frame's noise is this at its ISO.
    let mut pairs = merge::PairNoise::default();
    let model = |pairs: &merge::PairNoise| options.noise.or_else(|| pairs.fit()).unwrap_or(r.noise);
    let mut acc = Accumulator::new(w, h);
    // The deghosting, judged against the reference: from the start when
    // asked, or from the first frame believed moved under auto.
    let mut ghosts = match options.deghost {
        Deghost::Off => None,
        Deghost::On => Some(merge::Ghosts::new(w, h, true)),
        Deghost::Auto => Some(merge::Ghosts::new(w, h, false)),
    };
    // The reference goes in once the pairs have said what they can: just
    // before the darkest frame, which fills what every frame clipped.
    let mut reference_in = false;
    let add_reference = |acc: &mut Accumulator, noise: Noise| {
        acc.add(&Frame {
            rgb: &ref_rgb,
            mask: &ref_mask,
            exposure: 1.0,
            noise,
        });
    };

    // The rest, the darkest last.
    let mut order: Vec<usize> = (0..of).filter(|&k| k != reference).collect();
    order.sort_by_key(|&k| k == darkest);
    let mut exposures = vec![1.0f64; of];
    let mut all_clipped = 0;
    // The darkest frame's brightest sample, its reconstruction's top
    // where it has one: with its exposure, the most any merged sample
    // can be.
    let brightest = |rgb: &[f32]| {
        use rayon::prelude::*;
        rgb.par_iter()
            .copied()
            .filter(|v| v.is_finite())
            .reduce(|| 1.0, f32::max)
    };
    let mut top = 1.0f32;
    for (n, &k) in order.iter().enumerate() {
        progress.step(Step::Frame { index: n + 2, of });
        let t = Instant::now();
        let (mut f, _, _) = walk_frame(
            sources,
            k,
            edit,
            MergeInput {
                white: Some(white),
                reconstruct: k == darkest,
                denoiser: options.denoiser,
                ca: ca.clone(),
                lens: Vec::new(),
            },
            size,
            models,
        )?;
        // Frames of one size prepare to one size, unless their crops
        // differ.
        if (f.width, f.height) != (w, h) {
            bail!(
                "{} prepares to {}x{} and the reference to {w}x{h}: a merge wants frames \
                 of one size",
                sources.name(k),
                f.width,
                f.height
            );
        }
        if let Some(why) = &f.stood_in {
            warnings.push(format!("the learned denoiser stood down: {why}"));
        }
        denoised |= f.denoised;
        skipped.push(f.learned_skipped);
        let walked_s = t.elapsed().as_secs_f64();

        let given = options.registration.as_ref().map(|g| g[k]);
        let (measured, moved, shift, exposure, measure_s, fit_s) = match given {
            // As the recipe says: neither measured nor fitted.
            Some(g) => {
                let t = Instant::now();
                let moved = g.shift.map(|shift| {
                    resample(&mut f, &shift, w, h, kernel);
                    shift.corner_distance(&register::Transform::IDENTITY, w, h)
                });
                (
                    None,
                    moved,
                    g.shift,
                    g.exposure,
                    0.0,
                    t.elapsed().as_secs_f64(),
                )
            }
            None => {
                // The exposure first, on the frame as it stands: a
                // tripod's frames have not moved enough to bias a
                // median.
                let t = Instant::now();
                let measure =
                    |f: &Walked| merge::refine_exposure(&ref_rgb, &ref_mask, &f.rgb, &f.mask);
                let mut measured = measure(&f);
                let rough = measured.map(|m| m.ratio).or(relative[k]).unwrap_or(1.0);
                let measure_s = t.elapsed().as_secs_f64();

                // A tripod's frames should not move; a fit says
                // whether they did.
                let t = Instant::now();
                let (moved, shift) = check_moved(&ref_rgb, &f, rough, w, h, &sources.name(k));
                if let Some(shift) = &shift {
                    resample(&mut f, shift, w, h, kernel);
                    measured = measure(&f);
                }
                let fit_s = t.elapsed().as_secs_f64();
                let exposure = match (measured, relative[k]) {
                    (Some(m), _) => m.ratio,
                    (None, Some(e)) => {
                        warnings.push(format!(
                            "{}: too little held well in both frames to measure its \
                             exposure; its EXIF's taken",
                            sources.name(k)
                        ));
                        e
                    }
                    (None, None) => bail!(
                        "{}: its exposure is in neither its EXIF nor its pixels",
                        sources.name(k)
                    ),
                };
                (measured, moved, shift, exposure, measure_s, fit_s)
            }
        };
        let resampled = shift.is_some();
        if resampled && let Some(g) = ghosts.as_mut() {
            g.run(&mut acc);
        }
        if resampled {
            warnings.push(format!(
                "{} had moved {:.2} px and was resampled onto the reference",
                sources.name(k),
                moved.unwrap_or(0.0)
            ));
        }
        registration[k] = Registration { exposure, shift };
        let t = Instant::now();
        exposures[k] = exposure;
        // A resampled frame's noise is smoothed by the resampling and
        // says nothing true of the sensor's.
        if !resampled {
            pairs.add(&ref_rgb, &ref_mask, &f.rgb, &f.mask, exposure, iso[k]);
        }
        let sensor = model(&pairs);
        if k == darkest {
            add_reference(&mut acc, sensor);
            reference_in = true;
        }
        let noise = sensor.at_iso(iso[k] as f32);
        let frame = Frame {
            rgb: &f.rgb,
            mask: &f.mask,
            exposure,
            noise,
        };
        match ghosts.as_mut() {
            Some(g) => g.add(
                &mut acc,
                &frame,
                &merge::Judge {
                    rgb: &ref_rgb,
                    mask: &ref_mask,
                    noise: sensor,
                },
                k,
            ),
            None => acc.add(&frame),
        }
        if k == darkest {
            all_clipped = acc.fill_clipped(&frame);
            top = brightest(&f.rgb);
        }
        seconds.push(("frame", walked_s));
        seconds.push(("fit", fit_s));
        seconds.push(("merge", t.elapsed().as_secs_f64() + measure_s));
        reports[k] = Some(FrameReport {
            name: sources.name(k),
            exif: relative[k],
            measured,
            exposure,
            moved,
            resampled,
            rejected: None,
            noise,
        });
    }
    let sensor = model(&pairs);
    if !reference_in {
        add_reference(&mut acc, sensor);
    }
    if darkest == reference {
        top = brightest(&ref_rgb);
        all_clipped = acc.fill_clipped(&Frame {
            rgb: &ref_rgb,
            mask: &ref_mask,
            exposure: 1.0,
            noise: sensor,
        });
    }
    reports[reference] = Some(FrameReport {
        name: sources.name(reference),
        exif: Some(1.0),
        measured: None,
        exposure: 1.0,
        moved: None,
        resampled: false,
        rejected: None,
        noise: sensor,
    });
    drop(ref_rgb);
    drop(ref_mask);
    if let Some(g) = ghosts {
        let t = Instant::now();
        let (ledger, pending) = g.peak();
        log::info!(
            "merge: deghosting held at most {:.1} MB of samples where the reference \
             clipped and {:.1} MB set aside before a move",
            ledger as f64 / 1e6,
            pending as f64 / 1e6
        );
        for (k, rejected) in g.finish(&mut acc) {
            if let Some(r) = reports[k].as_mut() {
                r.rejected = rejected;
            }
        }
        seconds.push(("deghost", t.elapsed().as_secs_f64()));
    }
    for r in reports.iter().flatten() {
        if let Some(by) = r.moved {
            log::info!(
                "merge: {}: moved {by:.2} px{}{}",
                r.name,
                if r.resampled { ", resampled" } else { "" },
                r.rejected
                    .map_or(String::new(), |f| format!(", {:.2}% deghosted", f * 100.0))
            );
        }
    }
    let mut image = acc.finish();
    // No merged sample is over the darkest frame's clip on the
    // reference's scale; one that is was made by the arithmetic, and
    // would set the headroom the whole file is divided by.
    let e_min = exposures.iter().copied().fold(1.0f64, f64::min);
    let held = merge::hold_to(&mut image, (f64::from(top) / e_min) as f32);
    if held > 0 {
        log::info!("merge: {held} samples above the darkest frame's clip, held to it");
    }

    progress.step(Step::Writing);
    let t = Instant::now();
    let headroom = merge::headroom(&image, r.gains);
    // The file's noise, as the sensor's model has each frame's.
    let frames: Vec<(f64, Noise)> = (0..of)
        .map(|k| (exposures[k], sensor.at_iso(iso[k] as f32)))
        .collect();
    let profile = merge::noise_profile(&frames, headroom);
    let (heads, wholes) = hashes(sources);
    let tag = merge::Tag {
        kind: "hdr".into(),
        frames: (0..of).map(|i| sources.name(i)).collect(),
        reference,
        denoised,
        headroom_stops: headroom.log2(),
        lens_corrected: false,
        heads,
        wholes,
        unread: None,
        recipe: Some(Recipe {
            ca: options.ca.name().into(),
            // The tier that ran: one asked that stood down did not.
            denoise: options
                .denoiser
                .filter(|_| denoised)
                .and_then(greycard_ai::tier_of)
                .map(Into::into),
            noise: options.noise,
            deghost: Some(options.deghost.name().into()),
            kernel: Some(kernel),
            geometry: Geometry::Bracket(registration),
        }),
    };
    let preview = merge::preview(&image, &r.white_balance, 1024)?;
    let software = concat!("greycard ", env!("CARGO_PKG_VERSION"));
    let dng = LinearDngOptions {
        metadata: Some(&ref_meta),
        preview: Some(&preview),
        software: Some(software),
        headroom: Some(headroom),
        ..Default::default()
    };
    // Written in the reserved folder and renamed at the end: a merge
    // cut short (a failure, the window quit, the process killed) leaves
    // no half file under the name, and never touches a file of that
    // name that was there.
    let part = reserved.part();
    let written = (|| -> Result<()> {
        let file = std::fs::File::create(&part)?;
        let mut writer = std::io::BufWriter::new(file);
        write_merged_dng(
            &mut writer,
            &ref_frame,
            &image,
            r.gains,
            &dng,
            &Merged {
                noise_profile: profile,
                tag: &tag,
            },
        )?;
        use std::io::Write as _;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        Ok(())
    })();
    if let Err(e) = written {
        return Err(e.context(format!("writing {}", out.display())));
    }
    reserved.promote(&part)?;
    drop(reserved);
    seconds.push(("write", t.elapsed().as_secs_f64()));
    seconds.push(("total", started.elapsed().as_secs_f64()));
    Ok(Report {
        out,
        reference,
        frames: reports.into_iter().flatten().collect(),
        width: w,
        height: h,
        all_clipped,
        headroom_stops: headroom.log2(),
        noise_profile: profile,
        denoised,
        seconds,
        warnings,
        learned_skipped: LearnedSkipped::of_frames(skipped),
    })
}

/// What a merge made again changes of its recipe.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    /// The learned denoiser's tier on each frame: `Some(None)` for
    /// none, none for the recipe's.
    pub denoise: Option<Option<String>>,
    /// Where the file goes; none for over the merged file itself, which
    /// keeps its name and its sidecar.
    pub out: Option<PathBuf>,
}

/// What a merge made again did: the bracket's report, or the
/// panorama's.
#[derive(Debug, Clone)]
pub enum Again {
    Hdr(Report),
    Pano(Box<pano::Report>),
}

/// What [`again`] says of a file written before its recipe was kept.
pub const NO_RECIPE: &str =
    "this merge was written before its recipe was kept; merge the frames again";

/// The frames the merged file `file`, whose tag is `tag`, is made
/// again from, and the recipe: each found beside it by its name and
/// checked by its hash. What [`again`] refuses before it walks a frame
/// is refused here, saying which: no recipe, one that does not read, a
/// frame named with a path, a frame missing, and one changed since. A
/// question of the frames' heads (and their whole files where two share
/// a head); nothing is decoded.
pub fn frames_of(file: &Path, tag: &merge::Tag) -> Result<(Recipe, Vec<PathBuf>)> {
    let name = name_of(file);
    let Some(recipe) = tag.recipe.clone() else {
        match &tag.unread {
            Some(fact) => bail!("the recipe in {name} cannot be read: {fact}"),
            None => bail!(NO_RECIPE),
        }
    };
    if let Some(k) = (0..tag.frames.len()).find(|&k| tag.heads.get(k).is_none_or(Option::is_none)) {
        bail!(
            "the recipe in {name} cannot be read: no head= for {}",
            tag.frames[k]
        );
    }
    let dir = file
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut frames = Vec::with_capacity(tag.frames.len());
    for (k, frame) in tag.frames.iter().enumerate() {
        // A frame is a file's name in the merge's folder, and nothing
        // that reaches out of it.
        let plain = Path::new(frame).components().count() == 1
            && matches!(
                Path::new(frame).components().next(),
                Some(std::path::Component::Normal(_))
            )
            && !frame.contains(['/', '\\']);
        if !plain {
            bail!("the recipe in {name} names a frame {frame:?} that is not a file's name");
        }
        let path = dir.join(frame);
        if !path.is_file() {
            bail!(
                "{frame} is not beside {}: a merge is made again from its frames, in its folder",
                file.display()
            );
        }
        let hash = greycard_library::hash_file(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let same = Some(&hash) == tag.heads[k].as_ref()
            && match tag.wholes.get(k).cloned().flatten() {
                Some(whole) => {
                    whole_hash(&path).with_context(|| format!("reading {}", path.display()))?
                        == whole
                }
                None => true,
            };
        if !same {
            bail!(
                "{frame} is not the frame that was merged: it has changed since, or another \
                 file has its name"
            );
        }
        frames.push(path);
    }
    Ok((recipe, frames))
}

/// Whether the learned denoiser takes every one of `frames`: each a
/// 2x2 Bayer mosaic, read from its tags; else which does not.
pub fn take_learned(frames: &[PathBuf]) -> Result<()> {
    for frame in frames {
        let mosaic = greycard_core::decode::mosaic_path(frame)
            .with_context(|| format!("reading {}", frame.display()))?;
        if mosaic != Some(greycard_core::raw::Mosaic::Bayer) {
            bail!(
                "the learned denoiser takes a 2x2 Bayer mosaic, and {} is {}",
                name_of(frame),
                match mosaic {
                    Some(greycard_core::raw::Mosaic::XTrans) => "X-Trans",
                    Some(_) => "another mosaic",
                    None => "no mosaic",
                }
            );
        }
    }
    Ok(())
}

/// Whether the merged file `file`, whose tag is `tag`, can be made
/// again with its frames denoised: [`frames_of`] finds them, and the
/// learned denoiser takes them ([`take_learned`]).
pub fn can_denoise_again(file: &Path, tag: &merge::Tag) -> bool {
    !crate::ai::learned_off_card()
        && frames_of(file, tag).is_ok_and(|(_, frames)| take_learned(&frames).is_ok())
}

/// The merged file `file` made again from the recipe its tag keeps,
/// with `overrides`: its frames found beside it by name, each checked
/// by its head's hash, and merged with the recipe's options and its
/// geometry, which is not fitted again: a bracket's exposures and
/// moves, a panorama's turns, lens, gains, canvas, and crop. So the
/// frames land where they did, to the pixel. The new file is written
/// as a merge's is, under the hidden folder that holds the name, and
/// renamed over the old one at the end; nothing else of it changes.
/// `edit` is what the frames are walked under (their demosaic and
/// CA), as the merge's caller gave it.
pub fn again(
    file: &Path,
    overrides: &Overrides,
    edit: &Edit,
    models: &mut dyn Models,
    progress: &dyn Progress,
) -> Result<Again> {
    let tag = greycard_core::decode::merge_tag_path(file)
        .with_context(|| format!("reading {}", file.display()))?
        .with_context(|| {
            format!(
                "{} is not a merge of greycard's: it keeps no recipe",
                file.display()
            )
        })?;
    let (recipe, frames) = frames_of(file, &tag)?;
    let tier = match &overrides.denoise {
        Some(t) => t.clone(),
        None => recipe.denoise.clone(),
    };
    let denoiser = match tier {
        Some(t) => {
            let model = greycard_ai::denoiser(&t)
                .with_context(|| format!("there is no learned denoiser called {t}"))?;
            // The walk would skip it on such a frame and write the
            // file undenoised.
            if let Err(e) = take_learned(&frames) {
                bail!("{e:#}, and nothing was made again");
            }
            // Refused before any frame is walked: a merge made without
            // the tier asked would take the place of one made with it.
            match models.can_denoise(model) {
                Ok(()) => {}
                Err(crate::walk::NoLearned::Missing(_)) => bail!(
                    "the learned denoiser's {t} tier is not fetched, and nothing was made again"
                ),
                Err(crate::walk::NoLearned::Failed(why) | crate::walk::NoLearned::Short(why)) => {
                    bail!(
                        "the learned denoiser's {t} tier cannot run ({why}), and nothing was made \
                     again"
                    )
                }
            }
            Some(model)
        }
        None => None,
    };
    let (out, on_exists) = match &overrides.out {
        Some(p) => (p.clone(), OnExists::default()),
        None => (file.to_path_buf(), OnExists::Overwrite),
    };
    let ca = CaFits::named(&recipe.ca)
        .with_context(|| format!("the recipe's CA fit, {}, is not one we know", recipe.ca))?;
    // A bracket written before the setting was kept was not deghosted:
    // read as off, so it is made again as it was.
    let deghost = match recipe.deghost.as_deref() {
        None => Deghost::Off,
        Some(d) => Deghost::named(d)
            .with_context(|| format!("the recipe's deghosting, {d}, is not one we know"))?,
    };
    match recipe.geometry {
        Geometry::Bracket(registration) => hdr_of(
            &Files(&frames),
            &out,
            edit,
            &Options {
                reference: Some(tag.reference),
                out: Some(out.clone()),
                on_exists,
                denoiser,
                noise: recipe.noise,
                ca,
                registration: Some(registration),
                deghost,
                // A recipe written before the kernel was kept resampled
                // bilinearly.
                kernel: Some(recipe.kernel.unwrap_or(Kernel::Bilinear)),
            },
            models,
            progress,
        )
        .map(Again::Hdr),
        Geometry::Pano(laid) => pano::pano_of(
            &Files(&frames),
            &out,
            edit,
            &pano::Options {
                reference: Some(tag.reference),
                out: Some(out.clone()),
                on_exists,
                denoiser,
                lenses: None,
                kernel: laid.kernel,
                geometry: Some(laid),
            },
            models,
            progress,
        )
        .map(|r| Again::Pano(Box::new(r))),
    }
}

/// The name a merge of `sources` writes, held ([`Reserved`]): `out`
/// as `on_exists` settles it when something is there, and refused
/// when it would be one of the frames, whatever is asked, or when
/// nothing is to be written. A caller that runs more than one merge
/// for one name holds it once, writes each under it
/// ([`pano::pano_under`]), and promotes the one it keeps.
pub fn hold(sources: &dyn Sources, out: &Path, on_exists: OnExists) -> Result<Reserved> {
    // Whatever is asked of a name that is there, a frame is never
    // written over: it is read after the name is settled.
    if let Some(k) =
        (0..sources.len()).find(|&k| sources.path(k).is_some_and(|p| same_file(&p, out)))
    {
        bail!(
            "{} is one of the frames: a merge is never written over its frames",
            sources.name(k)
        );
    }
    let resolved = on_exists.resolve(out);
    let Some(out) = resolved.path() else {
        bail!(
            "{} is there already, and nothing was written",
            out.display()
        );
    };
    Reserved::new(out)
}

/// The name `out` held for one merge: a hidden folder beside it,
/// `.<name>.merging`, and an exclusive lock on the file `lock` in it,
/// held for the merge's whole life. A second merge to the same name
/// finds the lock held and is refused, rather than writing over the
/// first's. A folder whose lock is free was left by a merge cut short
/// (killed, or the window quit): its files are cleared and the folder
/// taken over. It holds the file until its rename and a panorama's
/// spilled frames, and is removed with all in it however the merge
/// ends, the lock let go first, since Windows removes no open file.
pub struct Reserved {
    dir: PathBuf,
    lock: Option<std::fs::File>,
    out: PathBuf,
}

/// The lock's file in a reserved folder.
const LOCK: &str = "lock";

impl Reserved {
    fn new(out: &Path) -> Result<Self> {
        let dir = out.with_file_name(format!(
            ".{}.merging",
            out.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        ));
        let writing =
            |e: std::io::Error| anyhow::Error::new(e).context(format!("writing {}", out.display()));
        match std::fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(writing(e)),
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK))
            .map_err(writing)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => bail!(
                "{} is being written by another merge; if none is running, remove {}",
                out.display(),
                dir.display()
            ),
            Err(std::fs::TryLockError::Error(e)) => return Err(writing(e)),
        }
        let reserved = Self {
            dir,
            lock: Some(lock),
            out: out.to_path_buf(),
        };
        if reserved.clear() {
            log::info!(
                "merge: cleared {}, left by a merge cut short",
                reserved.dir.display()
            );
        }
        Ok(reserved)
    }

    /// Everything in the folder but the lock removed; whether there
    /// was anything.
    fn clear(&self) -> bool {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return false;
        };
        let mut any = false;
        for entry in entries.flatten() {
            if entry.file_name() == LOCK {
                continue;
            }
            any = true;
            let path = entry.path();
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
        }
        any
    }

    /// The name held.
    pub fn out(&self) -> &Path {
        &self.out
    }

    /// Where the file is written until its rename.
    fn part(&self) -> PathBuf {
        self.dir.join("merged.part")
    }

    /// Where a file of the merge's own goes, `name` in the folder: a
    /// plain file's name, and none of the folder's own (the lock, the
    /// part, a spilled frame's `<k>.frame`).
    pub fn join(&self, name: &str) -> PathBuf {
        debug_assert!(
            name != LOCK && !name.contains(['/', '\\']),
            "{name} is not a merge's own file"
        );
        self.dir.join(name)
    }

    /// `file`, written in the folder, renamed onto the name held: the
    /// one rename every merge ends with, over a file there only when
    /// the name was settled to be written over. A moment's hold on that
    /// file is waited out ([`greycard_core::fs::rename_into_place`]).
    pub fn promote(&self, file: &Path) -> Result<PathBuf> {
        greycard_core::fs::rename_into_place(file, &self.out)
            .with_context(|| format!("writing {}", self.out.display()))?;
        Ok(self.out.clone())
    }
}

impl Drop for Reserved {
    fn drop(&mut self) {
        self.clear();
        // The lock's file goes while it is held where the system allows
        // it (not Windows), so no merge starting now finds it free and
        // takes over a folder about to go; then it is let go, closed,
        // and removed where it could not be before.
        let lock = self.dir.join(LOCK);
        let _ = std::fs::remove_file(&lock);
        drop(self.lock.take());
        let _ = std::fs::remove_file(&lock);
        // Only an empty folder: a merge that took it over meanwhile
        // keeps its own.
        let _ = std::fs::remove_dir(&self.dir);
    }
}

/// Whether `a` and `b` name one file: by their canonical paths, the
/// folder's alone for a file not there yet.
fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| {
        std::fs::canonicalize(p).ok().or_else(|| {
            let dir = p.parent().filter(|d| !d.as_os_str().is_empty())?;
            Some(std::fs::canonicalize(dir).ok()?.join(p.file_name()?))
        })
    };
    match (canon(a), canon(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}

/// Whether frame `f` moved off the reference, and if it did, `f`
/// resampled onto it: a similarity fit (a shift, a turn, and a scale,
/// what a hand adds) on the band both hold, taken
/// only past a tenth of a pixel, and believed only where the picture
/// has structure every way and the move fits clearly better than none.
/// A still frame of a flat horizon otherwise reads motion along it from
/// its noise. How far it moved, none for a fit not believed, and the
/// move it was resampled by.
fn check_moved(
    reference: &[f32],
    f: &Walked,
    e: f64,
    w: usize,
    h: usize,
    name: &str,
) -> (Option<f64>, Option<register::Transform>) {
    let (a, b) = merge::fit_planes(reference, &f.rgb, e);
    // Judged against the identity rather than by the fit's residual
    // alone: a still frame is the alternative here, and a bracket's
    // dark frame is noisy enough to sit over a stack's threshold
    // with the move found to a hundredth of a pixel.
    let fit = match register::fit(
        &a,
        &b,
        w,
        h,
        &register::Options {
            model: Model::Similarity,
            ..Default::default()
        },
    ) {
        Ok(fit) => fit,
        Err(e) => {
            log::info!("merge: {name}: no fit ({e}); taken as unmoved");
            return (None, None);
        }
    };
    let by = fit
        .transform
        .corner_distance(&register::Transform::IDENTITY, w, h);
    if by <= merge::MOVED {
        return (Some(by), None);
    }
    let condition = merge::condition(&a, w, h);
    let back = register::warp_plane(
        &b,
        w,
        h,
        w,
        h,
        &fit.transform,
        Edge::Fill(f32::NAN),
        Kernel::Bilinear,
    );
    let (before, after) = (merge::residual(&a, &b), merge::residual(&a, &back));
    if condition < merge::MIN_CONDITION || after > merge::BETTER * before {
        log::info!(
            "merge: {name}: a move of {by:.2} px not believed (structure {condition:.3}, \
             residual {after:.4} against {before:.4} unmoved, fit {:.3}); taken as unmoved",
            fit.residual
        );
        return (None, None);
    }
    (Some(by), Some(fit.transform))
}

/// Frame `f` resampled by `shift` onto the reference by `kernel`, and
/// masked again ([`resampled`]).
fn resample(f: &mut Walked, shift: &register::Transform, w: usize, h: usize, kernel: Kernel) {
    let (rgb, mask) = resampled(std::mem::take(&mut f.rgb), shift, w, h, kernel);
    f.rgb = rgb;
    f.mask = mask;
}

/// Camera RGB `rgb` (`w` by `h`) resampled by `shift` with `kernel`,
/// held to its own range so a wide kernel's ring at a hard edge puts
/// nothing under its darkest or over its brightest, and its clip mask.
/// A wide kernel's ring reaches its reach from a clipped edge, past
/// the mask's own growth by the demosaic's reach: the mask is grown by
/// the kernel's taps past bilinear's as well, so the ring is not
/// merged.
fn resampled(
    rgb: Vec<f32>,
    shift: &register::Transform,
    w: usize,
    h: usize,
    kernel: Kernel,
) -> (Vec<f32>, Vec<bool>) {
    let range = register::sample_range(&rgb);
    let image = CameraImage {
        width: w,
        height: h,
        data: rgb,
    };
    let mut rgb = register::warp_camera_image(&image, shift, Edge::Fill(f32::NAN), kernel).data;
    drop(image);
    if kernel == Kernel::Bilinear {
        let mask = merge::clip_mask(&rgb, w, h);
        return (rgb, mask);
    }
    register::clamp_samples(&mut rgb, range);
    let mask = merge::grow(
        &merge::clip_mask(&rgb, w, h),
        w,
        h,
        (kernel.reach() - 1) as usize,
    );
    (rgb, mask)
}

/// The models a merge runs from the command line or the window's merge
/// thread: the learned denoiser alone, from the user's model store.
pub struct DenoiseModels {
    /// Dropped before `_running`, by field order, so the merge's own
    /// sessions are gone before a give-back may begin.
    ai: crate::ai::Ai,
    /// No session is dropped to give the card back while this lives,
    /// the merge's own included: a merge's room check never gives back,
    /// only waits.
    _running: crate::ai::MergeRunning,
}

impl DenoiseModels {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let mut ai = crate::ai::Ai::new();
        // A frame the card is short for is asked for again as an
        // export's is, then the merge fails.
        ai.set_patience(Some(crate::ai::Patience::of(crate::ai::PATIENT_TRIES)));
        Self {
            ai,
            _running: crate::ai::MergeRunning::new(),
        }
    }
}

fn no_learned(
    e: crate::ai::NoDenoiser,
    model: &'static greycard_ai::Model,
) -> crate::walk::NoLearned {
    match e {
        crate::ai::NoDenoiser::Missing => crate::walk::NoLearned::Missing(model),
        crate::ai::NoDenoiser::Failed(why) => crate::walk::NoLearned::Failed(why),
    }
}

impl Models for DenoiseModels {
    fn can_denoise(
        &mut self,
        model: &'static greycard_ai::Model,
    ) -> std::result::Result<(), crate::walk::NoLearned> {
        self.ai
            .denoiser_there(model)
            .map_err(|e| no_learned(e, model))
    }

    fn denoise(
        &mut self,
        model: &'static greycard_ai::Model,
        frame: &RawFrame,
        prepared: &greycard_core::develop::Prepared,
    ) -> std::result::Result<Vec<f32>, crate::walk::NoLearned> {
        self.ai.denoiser(model).map_err(|e| no_learned(e, model))?;
        let on = self
            .ai
            .denoise_room_for(model)
            .map_err(|short| crate::walk::NoLearned::Short(short.0))?;
        let denoiser = self.ai.denoiser(model).map_err(|e| no_learned(e, model))?;
        let tiling = crate::worker::run_tiling(on, denoiser.tiling());
        let (rgb, report) = crate::worker::network(frame, prepared, model, denoiser, None, tiling)
            .map_err(|e| crate::worker::learned_failure(&e))?;
        log::info!("merge: the learned denoiser: {report:?}");
        Ok(rgb)
    }

    fn fill(
        &mut self,
        _: &greycard_edit::retouch::Patch,
        _: &greycard_core::develop::retouch::Region,
        _: &greycard_core::image::WorkingImage,
    ) -> Option<Vec<f32>> {
        None
    }

    fn could_fill_more(
        &mut self,
        _: &greycard_edit::retouch::Retouch,
        _: &greycard_core::image::WorkingImage,
    ) -> bool {
        false
    }

    fn rasters(
        &mut self,
        _: crate::stages::Masks,
        _: &Arc<greycard_core::image::WorkingImage>,
        _: Option<crate::walk::Patched<'_>>,
        _: &crate::walk::Meta,
    ) -> crate::walk::Rasters {
        crate::walk::Rasters::new()
    }
}

/// A file's name, for a report.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_core::image::WorkingImage;
    use greycard_core::merge::testing::{
        checker, mosaic_of, scene, two_patches, write_dng, write_dng_at,
    };

    const W: usize = 256;
    const H: usize = 160;

    /// A bracket of three made-up frames in a scratch folder: the true
    /// exposures x0.25, x1, x4 of the scene (a ramp from -12 stops to
    /// +4 over the middle frame's clip), EXIF saying 1/400, 1/100, and
    /// 1/25 while the shutters missed (x0.263 and x3.8 against the
    /// middle). `dx` moves a frame's scene.
    fn bracket(what: &str, dx: [usize; 3]) -> (PathBuf, Vec<PathBuf>) {
        bracket_of(what, 17, &|x, y, c, k| scene(x + dx[k], y, c, W, H))
    }

    /// A scratch folder for test `what`.
    fn scratch(what: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("greycard-merge-{what}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// The bracket of [`bracket`] of the scene `light` makes of frame
    /// `k` at a pixel and channel, its noise drawn from `seed`.
    fn bracket_of(
        what: &str,
        seed: u64,
        light: &dyn Fn(usize, usize, usize, usize) -> f32,
    ) -> (PathBuf, Vec<PathBuf>) {
        let dir = scratch(what);
        let gains = [1.0 / 3.8, 1.0, 3.8];
        let shutters = [(1, 400), (1, 100), (1, 25)];
        let files = (0..3)
            .map(|k| {
                let path = dir.join(format!("IMG_{k}.dng"));
                let samples = mosaic_of(
                    W,
                    H,
                    gains[k],
                    Noise::ASSUMED,
                    seed + k as u64,
                    |x, y, c| light(x, y, c, k),
                );
                write_dng(&path, samples, W, H, shutters[k], "Bracket");
                path
            })
            .collect();
        (dir, files)
    }

    fn merged(files: &[PathBuf], options: &Options) -> Report {
        let steps = std::cell::RefCell::new(Vec::new());
        let report = hdr(
            files,
            &Edit::default(),
            options,
            &mut crate::walk::NoModels,
            &|s: Step| steps.borrow_mut().push(s),
        )
        .unwrap();
        assert_eq!(
            steps.into_inner(),
            [
                Step::Frame { index: 1, of: 3 },
                Step::Frame { index: 2, of: 3 },
                Step::Frame { index: 3, of: 3 },
                Step::Writing
            ]
        );
        report
    }

    /// A name held: refused when it is one of the frames, settled by
    /// what is asked of a name that is there, refused to a second
    /// holder while held, a file written under it promoted onto the
    /// name, and the folder gone when it is let go.
    #[test]
    fn a_name_is_held_written_under_and_promoted() {
        let (dir, files) = bracket("held", [0; 3]);
        let sources = Files(&files);
        let refused = hold(&sources, &files[1], OnExists::Overwrite)
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert_eq!(
            refused,
            "IMG_1.dng is one of the frames: a merge is never written over its frames"
        );
        let out = dir.join("merged.dng");
        std::fs::write(&out, b"there").unwrap();
        assert!(hold(&sources, &out, OnExists::Skip).is_err());
        let held = hold(&sources, &out, OnExists::Increment).unwrap();
        assert_eq!(held.out(), dir.join("merged-1.dng"));
        let held = {
            drop(held);
            hold(&sources, &out, OnExists::Overwrite).unwrap()
        };
        assert_eq!(held.out(), out);
        assert!(hold(&sources, &out, OnExists::Overwrite).is_err());
        let folder = dir.join(".merged.dng.merging");
        let file = held.join("kept.dng");
        assert_eq!(file.parent(), Some(folder.as_path()));
        std::fs::write(&file, b"kept").unwrap();
        std::fs::write(held.join("dropped.dng"), b"dropped").unwrap();
        assert_eq!(held.promote(&file).unwrap(), out);
        assert_eq!(std::fs::read(&out).unwrap(), b"kept");
        drop(held);
        assert!(!folder.exists());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The file lands beside the first frame as `<stem>-HDR.dng`, a
    /// number after it when the name is taken; it is a float linear
    /// DNG of the reference's body and lens, its exposures measured
    /// past the EXIF's marks, and says what it was merged from.
    #[test]
    fn a_bracket_is_merged_beside_its_first_frame() {
        let (dir, files) = bracket("tags", [0; 3]);
        // A merge running to the name holds its folder's lock: refused,
        // naming the folder.
        let held = dir.join(".IMG_0-HDR.dng.merging");
        std::fs::create_dir(&held).unwrap();
        let lock = std::fs::File::create(held.join("lock")).unwrap();
        lock.lock().unwrap();
        let refused = hdr(
            &files,
            &Edit::default(),
            &Options::default(),
            &mut crate::walk::NoModels,
            &|_: Step| {},
        )
        .map(|_| ())
        .unwrap_err()
        .to_string();
        assert_eq!(
            refused,
            format!(
                "{} is being written by another merge; if none is running, remove {}",
                dir.join("IMG_0-HDR.dng").display(),
                held.display()
            )
        );
        assert!(!dir.join("IMG_0-HDR.dng").exists());
        // One cut short let its lock go with its process: what it left
        // is cleared, the merge runs, and nothing hidden is left.
        drop(lock);
        std::fs::write(held.join("merged.part"), b"cut short").unwrap();
        let report = merged(&files, &Options::default());
        let hidden: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(hidden.is_empty(), "{hidden:?}");
        assert_eq!(report.out, dir.join("IMG_0-HDR.dng"));
        assert_eq!(report.reference, 1);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let by_name = |n: &str| report.frames.iter().find(|f| f.name == n).unwrap();
        let dark = by_name("IMG_0.dng");
        assert_eq!(dark.exif, Some(0.25));
        let m = dark.measured.unwrap().ratio;
        assert!((m - 1.0 / 3.8).abs() < 0.002, "{m}");
        let bright = by_name("IMG_2.dng");
        assert!((bright.exposure - 3.8).abs() < 0.03, "{}", bright.exposure);
        assert!(!dark.resampled && !bright.resampled);

        let back = greycard_core::decode::decode_path(&report.out).unwrap();
        assert_eq!(
            (back.make.as_str(), back.model.as_str()),
            ("Synthetic", "Bracket")
        );
        assert_eq!(back.shot.lens_model.as_deref(), Some("Synthetic 35mm"));
        assert_eq!(back.shot.exposure_time, Some(0.01));
        assert!(matches!(back.samples, Samples::F32(_)));
        let tag = back.dng.merged.clone().expect("our tag");
        assert_eq!(tag.kind, "hdr");
        assert_eq!(tag.frames, ["IMG_0.dng", "IMG_1.dng", "IMG_2.dng"]);
        assert_eq!((tag.reference, tag.denoised), (1, false));
        // About two stops over the middle frame's clip, past which the
        // darkest frame clipped too.
        assert!(
            (1.5..3.0).contains(&tag.headroom_stops),
            "{}",
            tag.headroom_stops
        );
        assert_eq!(back.dng.baseline_exposure, Some(tag.headroom_stops));
        assert!(back.dng.noise_profile.is_some_and(|n| n.a[0] > 0.0));
        assert!(report.all_clipped > 0);

        // Again: beside it, numbered.
        let again = merged(&files, &Options::default());
        assert_eq!(again.out, dir.join("IMG_0-HDR-1.dng"));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A frame knocked two pixels between shots is fitted, resampled,
    /// and said; one that stayed is left as it is.
    #[test]
    fn a_frame_that_moved_is_resampled_and_said() {
        let (dir, files) = bracket("moved", [0, 0, 2]);
        let report = merged(&files, &Options::default());
        let bright = &report.frames[2];
        assert!(bright.resampled, "{:?}", bright.moved);
        assert!((bright.moved.unwrap() - 2.0).abs() < 0.3);
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("IMG_2.dng had moved"));
        assert!(!report.frames[0].resampled);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A bracket's frame turned half a degree, scaled by a part in five
    /// hundred, and shifted two pixels, as a hand holds a camera: the
    /// similarity the recipe keeps is the truth to a tenth of a pixel at
    /// every corner, the frame is deghosted (its fit was believed), and
    /// the frame that did not move is left alone.
    #[test]
    fn a_frame_turned_and_scaled_by_a_hand_is_found_at_the_corners() {
        let light = |u: f64, v: f64, c: usize| {
            let wave = |t: f64| (std::f64::consts::PI * t / 13.0).cos();
            let fine = 0.5 + 0.5 * wave(u) * wave(v * 1.3 + 0.4 * u);
            let drift = 0.5 + 0.5 * (u * 0.071 + v * 0.113).sin();
            ((0.03 + 0.12 * fine + 0.04 * drift) * [0.55, 1.0, 0.7][c]) as f32
        };
        let middle = [(W - 1) as f64 / 2.0, (H - 1) as f64 / 2.0];
        let hand = register::Transform::similarity(1.002, 0.5f64.to_radians(), middle, [2.0, -1.0]);
        let (dir, files) = bracket_of("turned", 29, &|x, y, c, k| {
            let (u, v) = if k == 2 {
                hand.apply(x as f64, y as f64)
            } else {
                (x as f64, y as f64)
            };
            light(u, v, c)
        });
        let report = merged(&files, &Options::default());
        let (_, tag) = samples_and_tag(&report.out);
        let Some(Recipe {
            geometry: Geometry::Bracket(registration),
            deghost,
            ..
        }) = tag.recipe
        else {
            panic!("a bracket's recipe");
        };
        assert_eq!(deghost.as_deref(), Some("auto"));
        let found = registration[2].shift.expect("the turned frame moved");
        let truth = hand.inverse().unwrap();
        let off = found.corner_distance(&truth, W, H);
        println!("a hand's similarity found to {off:.3} px at the corners");
        assert!(
            off < 0.1,
            "{off} px at the corners: {found:?} against {truth:?}"
        );
        assert!(registration[0].shift.is_none() && !report.frames[0].resampled);
        assert!(report.frames[2].rejected.is_some());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A step from a clipped region to a dark one, resampled half a
    /// pixel by Lanczos-3: the kernel's last lobe lands past the clip
    /// mask's growth by the demosaic's reach, and the mask grown by the
    /// kernel's reach covers it. Bilinear is masked as it was.
    #[test]
    fn a_clipped_edges_ring_is_masked() {
        let (w, h) = (64usize, 16usize);
        let rgb: Vec<f32> = (0..w * h)
            .flat_map(|i| [if i % w < w / 2 { 1.0f32 } else { 0.05 }; 3])
            .collect();
        let shift = register::Transform::translation(0.5, 0.0);
        let (out, mask) = resampled(rgb.clone(), &shift, w, h, Kernel::Lanczos3);
        let before = merge::clip_mask(&out, w, h);
        let y = h / 2;
        let newly: Vec<usize> = (0..w)
            .filter(|&x| mask[y * w + x] && !before[y * w + x])
            .collect();
        assert!(!newly.is_empty());
        let ring = newly
            .iter()
            .map(|&x| (out[(y * w + x) * 3 + 1] - 0.05).abs())
            .fold(0.0f32, f32::max);
        assert!(ring > 0.01, "{ring}");
        // Past the newly masked pixels the ring is under a part in a
        // thousand of the step, up to the last column, which the move
        // takes off the frame.
        let past = newly.iter().max().unwrap() + 1;
        for x in past..w - 1 {
            assert!((out[(y * w + x) * 3 + 1] - 0.05).abs() < 1e-3 * 0.95, "{x}");
        }
        let (out, mask) = resampled(rgb, &shift, w, h, Kernel::Bilinear);
        assert_eq!(mask, merge::clip_mask(&out, w, h));
    }

    /// A still bracket under auto runs no deghosting and merges to the
    /// bit as with it off; a moved one under off rejects nothing, and
    /// under auto says what it rejected.
    #[test]
    fn deghosting_runs_only_when_asked_or_a_frame_moved() {
        let (dir, files) = bracket("still-ghost", [0, 0, 0]);
        let off = merged(
            &files,
            &Options {
                deghost: Deghost::Off,
                ..Default::default()
            },
        );
        let auto = merged(&files, &Options::default());
        let (a, tag) = samples_and_tag(&off.out);
        let (b, _) = samples_and_tag(&auto.out);
        assert!(a.iter().zip(&b).all(|(a, b)| a.to_bits() == b.to_bits()));
        assert!(auto.frames.iter().all(|f| f.rejected.is_none()));
        assert_eq!(tag.recipe.unwrap().deghost.as_deref(), Some("off"));
        greycard_core::testing::remove_dir_retry(&dir);

        let (dir, files) = bracket("moved-ghost", [0, 0, 2]);
        let off = merged(
            &files,
            &Options {
                deghost: Deghost::Off,
                ..Default::default()
            },
        );
        assert!(off.frames[2].resampled);
        assert!(off.frames.iter().all(|f| f.rejected.is_none()));
        let auto = merged(&files, &Options::default());
        assert!(auto.frames[0].rejected.is_some() && auto.frames[2].rejected.is_some());
        assert_eq!(auto.frames[1].rejected, None);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A merged file's samples and its tag.
    fn samples_and_tag(file: &Path) -> (Vec<f32>, merge::Tag) {
        let back = greycard_core::decode::decode_path(file).unwrap();
        let Samples::F32(samples) = back.samples else {
            panic!("float samples");
        };
        (samples, back.dng.merged.expect("our tag"))
    }

    /// The file keeps its recipe: the options, each frame's hash, and
    /// each frame's exposure and move. Made again from it, the bracket
    /// is the same to the sample, its exposures and its move taken from
    /// the recipe rather than measured, and written over the old file
    /// under its name, nothing hidden left.
    #[test]
    fn a_bracket_made_again_is_the_same_to_the_sample() {
        let (dir, files) = bracket("again", [0, 0, 2]);
        let report = merged(&files, &Options::default());
        let (before, tag) = samples_and_tag(&report.out);
        assert_eq!(
            tag.heads,
            files
                .iter()
                .map(|f| Some(greycard_library::hash_file(f).unwrap()))
                .collect::<Vec<_>>()
        );
        let recipe = tag.recipe.clone().expect("a recipe");
        assert_eq!((recipe.ca.as_str(), recipe.denoise), ("once", None));
        assert_eq!(recipe.deghost.as_deref(), Some("auto"));
        assert_eq!(recipe.kernel, Some(Kernel::Lanczos3));
        let Geometry::Bracket(registration) = &recipe.geometry else {
            panic!("a bracket's geometry");
        };
        assert_eq!(registration[1].exposure, 1.0);
        assert_eq!(registration[2].exposure, report.frames[2].exposure);
        assert!(registration[2].shift.is_some() && registration[0].shift.is_none());

        let steps = std::cell::RefCell::new(Vec::new());
        let Again::Hdr(again) = again(
            &report.out,
            &Overrides::default(),
            &Edit::default(),
            &mut crate::walk::NoModels,
            &|s: Step| steps.borrow_mut().push(s),
        )
        .unwrap() else {
            panic!("a bracket");
        };
        assert_eq!(steps.into_inner().len(), 4);
        assert_eq!(again.out, report.out);
        assert_eq!(again.warnings, report.warnings);
        assert!(again.frames.iter().all(|f| f.measured.is_none()));
        let (after, tag_after) = samples_and_tag(&report.out);
        assert_eq!(tag_after, tag);
        assert!(
            before
                .iter()
                .zip(&after)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        assert_eq!(before.len(), after.len());
        let hidden: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.') || n.ends_with("-1.dng"))
            .collect();
        assert!(hidden.is_empty(), "{hidden:?}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A tier asked of a merge made again that is not fetched is
    /// refused before any frame is walked, and the file left as it was;
    /// a merge asked for a tier that stood down keeps none in its
    /// recipe, since none ran.
    #[test]
    fn a_tier_not_there_is_refused_and_a_tier_not_run_not_kept() {
        let (dir, files) = bracket("again-tier", [0; 3]);
        let steps = std::cell::RefCell::new(Vec::new());
        let report = hdr(
            &files,
            &Edit::default(),
            &Options {
                denoiser: greycard_ai::denoiser("balanced"),
                ..Options::default()
            },
            &mut crate::walk::NoModels,
            &|s: Step| steps.borrow_mut().push(s),
        )
        .unwrap();
        assert!(!report.denoised);
        assert!(!report.warnings.is_empty());
        let (_, tag) = samples_and_tag(&report.out);
        assert_eq!(tag.recipe.unwrap().denoise, None);

        let before = std::fs::read(&report.out).unwrap();
        let steps = std::cell::RefCell::new(Vec::new());
        let refused = again(
            &report.out,
            &Overrides {
                denoise: Some(Some("balanced".into())),
                ..Default::default()
            },
            &Edit::default(),
            &mut crate::walk::NoModels,
            &|s: Step| steps.borrow_mut().push(s),
        )
        .map(|_| ())
        .unwrap_err()
        .to_string();
        assert_eq!(
            refused,
            "the learned denoiser's balanced tier is not fetched, and nothing was made again"
        );
        assert!(steps.into_inner().is_empty());
        assert_eq!(std::fs::read(&report.out).unwrap(), before);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Why a merge's frames went without the learned tier, from each
    /// frame's skips: the card's or X-Trans, the card's first when both
    /// apply; any other skip, or none, says nothing.
    #[test]
    fn a_merge_says_why_its_frames_went_without_the_tier() {
        use crate::stages::{LEARNED_DENOISER, NO_XTRANS, OFF_CARD, Skip};
        let off = [(LEARNED_DENOISER, Skip::Unavailable(OFF_CARD))];
        let xtrans = [(LEARNED_DENOISER, Skip::Unavailable(NO_XTRANS))];
        let other = [(
            LEARNED_DENOISER,
            Skip::Unavailable("the learned denoiser needs a 2x2 Bayer mosaic"),
        )];
        assert_eq!(LearnedSkipped::of(&off), Some(LearnedSkipped::OffCard));
        assert_eq!(LearnedSkipped::of(&xtrans), Some(LearnedSkipped::XTrans));
        assert_eq!(LearnedSkipped::of(&other), None);
        assert_eq!(LearnedSkipped::of(&[]), None);
        let frames = |f: &[Option<LearnedSkipped>]| LearnedSkipped::of_frames(f.to_vec());
        assert_eq!(
            frames(&[
                Some(LearnedSkipped::XTrans),
                Some(LearnedSkipped::OffCard),
                None
            ]),
            Some(LearnedSkipped::OffCard)
        );
        assert_eq!(
            frames(&[None, Some(LearnedSkipped::XTrans)]),
            Some(LearnedSkipped::XTrans)
        );
        assert_eq!(frames(&[None, None]), None);
        assert_eq!(
            LearnedSkipped::OffCard.words(),
            "no learned denoise on this graphics card"
        );
        assert_eq!(
            LearnedSkipped::XTrans.words(),
            "no learned denoise on X-Trans"
        );
    }

    /// A merge asked for no tier, or whose tier ran, says nothing of it.
    #[test]
    fn a_merge_with_no_tier_or_its_tier_run_says_nothing_of_it() {
        let (dir, files) = bracket("tier-ran", [0; 3]);
        let none = hdr(
            &files,
            &Edit::default(),
            &Options::default(),
            &mut crate::walk::NoModels,
            &|_: Step| {},
        )
        .unwrap();
        assert_eq!(none.learned_skipped, None);
        crate::ai::set_test_off_card(Some(false));
        let ran = hdr(
            &files,
            &Edit::default(),
            &Options {
                denoiser: Some(&greycard_ai::DENOISE_FAST),
                ..Options::default()
            },
            &mut crate::testing::PlainDenoiser,
            &|_: Step| {},
        )
        .unwrap();
        crate::ai::set_test_off_card(None);
        assert!(ran.denoised);
        assert_eq!(ran.learned_skipped, None);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The learned denoiser had, but every frame's run refused as its
    /// reason says.
    struct Refused(crate::walk::NoLearned);
    impl Models for Refused {
        fn can_denoise(
            &mut self,
            _: &'static greycard_ai::Model,
        ) -> Result<(), crate::walk::NoLearned> {
            Ok(())
        }
        fn denoise(
            &mut self,
            _: &'static greycard_ai::Model,
            _: &RawFrame,
            _: &greycard_core::develop::Prepared,
        ) -> Result<Vec<f32>, crate::walk::NoLearned> {
            Err(self.0.clone())
        }
        fn fill(
            &mut self,
            _: &greycard_edit::retouch::Patch,
            _: &greycard_core::develop::retouch::Region,
            _: &greycard_core::image::WorkingImage,
        ) -> Option<Vec<f32>> {
            None
        }
        fn could_fill_more(
            &mut self,
            _: &greycard_edit::retouch::Retouch,
            _: &greycard_core::image::WorkingImage,
        ) -> bool {
            false
        }
        fn rasters(
            &mut self,
            _: crate::stages::Masks,
            _: &std::sync::Arc<greycard_core::image::WorkingImage>,
            _: Option<crate::walk::Patched<'_>>,
            _: &crate::walk::Meta,
        ) -> crate::walk::Rasters {
            crate::walk::Rasters::new()
        }
    }

    /// A merge's "stood down" warning says what it always said; the
    /// card's refusal at the load (its plan did not skip the tier) is
    /// said as the plan's skip is, the merge's note, and no warning.
    #[test]
    fn a_refusal_for_the_card_at_the_load_is_the_merges_note() {
        use crate::walk::NoLearned;
        let (dir, files) = bracket("refused-load", [0; 3]);
        crate::ai::set_test_off_card(Some(false));
        let run = |why: NoLearned| {
            hdr(
                &files,
                &Edit::default(),
                &Options {
                    denoiser: Some(&greycard_ai::DENOISE_BEST),
                    on_exists: greycard_core::output::OnExists::Overwrite,
                    ..Options::default()
                },
                &mut Refused(why),
                &|_: Step| {},
            )
            .unwrap()
        };
        let off = run(NoLearned::Failed(crate::stages::OFF_CARD.into()));
        assert_eq!(off.learned_skipped, Some(LearnedSkipped::OffCard));
        assert!(!off.denoised);
        assert!(
            !off.warnings.iter().any(|w| w.contains("stood down")),
            "{:?}",
            off.warnings
        );
        let other = run(NoLearned::Failed("bad input".into()));
        assert_eq!(other.learned_skipped, None);
        assert!(
            other
                .warnings
                .iter()
                .any(|w| w == "the learned denoiser stood down: Failed(\"bad input\")"),
            "{:?}",
            other.warnings
        );
        crate::ai::set_test_off_card(None);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// On a recorded card, a merge that asked for no tier says nothing
    /// of it: nothing was skipped.
    #[test]
    fn a_merge_with_no_tier_on_a_recorded_card_says_nothing_of_it() {
        let (dir, files) = bracket("no-tier-off", [0; 3]);
        crate::ai::set_test_off_card(Some(true));
        let r = hdr(
            &files,
            &Edit::default(),
            &Options::default(),
            &mut crate::walk::NoModels,
            &|_: Step| {},
        )
        .unwrap();
        crate::ai::set_test_off_card(None);
        assert_eq!(r.learned_skipped, None);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// On a card a learned denoiser lost, a merge asked for any tier
    /// walks its frames as it walks X-Trans ones: the network skipped,
    /// the plain demosaic, no tier in the recipe, and no word of it
    /// among the warnings. Made again at any tier, it is declined up
    /// front in the existing words, the merge left as it was, and the
    /// window's row is not offered.
    #[test]
    fn a_merge_on_a_card_the_denoiser_lost_skips_it_and_is_not_made_again() {
        /// A merge's models as `DenoiseModels` asks them (the check is
        /// the engine's), never run.
        struct OffCard(crate::ai::Ai);
        impl Models for OffCard {
            fn can_denoise(
                &mut self,
                model: &'static greycard_ai::Model,
            ) -> Result<(), crate::walk::NoLearned> {
                self.0
                    .denoiser_there(model)
                    .map_err(|e| no_learned(e, model))
            }
            fn denoise(
                &mut self,
                _: &'static greycard_ai::Model,
                _: &RawFrame,
                _: &greycard_core::develop::Prepared,
            ) -> Result<Vec<f32>, crate::walk::NoLearned> {
                panic!("the network is never asked on a card it lost")
            }
            fn fill(
                &mut self,
                _: &greycard_edit::retouch::Patch,
                _: &greycard_core::develop::retouch::Region,
                _: &greycard_core::image::WorkingImage,
            ) -> Option<Vec<f32>> {
                None
            }
            fn could_fill_more(
                &mut self,
                _: &greycard_edit::retouch::Retouch,
                _: &greycard_core::image::WorkingImage,
            ) -> bool {
                false
            }
            fn rasters(
                &mut self,
                _: crate::stages::Masks,
                _: &std::sync::Arc<greycard_core::image::WorkingImage>,
                _: Option<crate::walk::Patched<'_>>,
                _: &crate::walk::Meta,
            ) -> crate::walk::Rasters {
                crate::walk::Rasters::new()
            }
        }
        let models = || {
            OffCard(crate::ai::Ai::for_test(
                greycard_ai::Store::at("no-store"),
                Vec::new(),
            ))
        };
        crate::ai::set_test_off_card(Some(true));
        let (dir, files) = bracket("off-card", [0; 3]);
        let mut made = None;
        for tier in ["fast", "balanced", "best"] {
            let report = hdr(
                &files,
                &Edit::default(),
                &Options {
                    denoiser: greycard_ai::denoiser(tier),
                    ..Options::default()
                },
                &mut models(),
                &|_: Step| {},
            )
            .unwrap();
            assert!(!report.denoised, "{tier}");
            assert_eq!(report.learned_skipped, Some(LearnedSkipped::OffCard));
            assert!(
                !report.warnings.iter().any(|w| w.contains("stood down")),
                "{:?}",
                report.warnings
            );
            let (_, tag) = samples_and_tag(&report.out);
            assert_eq!(tag.recipe.unwrap().denoise, None);
            made = Some(report.out);
        }
        let out = made.expect("a merge");
        let (_, tag) = samples_and_tag(&out);
        assert!(!can_denoise_again(&out, &tag));
        let before = std::fs::read(&out).unwrap();
        for tier in ["fast", "balanced", "best"] {
            let refused = again(
                &out,
                &Overrides {
                    denoise: Some(Some(tier.into())),
                    ..Default::default()
                },
                &Edit::default(),
                &mut models(),
                &|_: Step| panic!("nothing is walked"),
            )
            .map(|_| ())
            .unwrap_err()
            .to_string();
            assert_eq!(
                refused,
                format!(
                    "the learned denoiser's {tier} tier cannot run (the learned denoiser is \
                     not on this graphics card), and nothing was made again"
                )
            );
            assert_eq!(std::fs::read(&out).unwrap(), before);
        }
        crate::ai::set_test_off_card(None);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A frame the card stays short of denoising fails the merge, rather
    /// than mix the engine's denoise into a learned merge.
    #[test]
    fn a_frame_the_card_is_short_for_fails_the_merge() {
        /// The learned denoiser had, but every frame refused for the
        /// card's memory, as the merge's own models refuse after their
        /// tries.
        struct Short;
        impl Models for Short {
            fn can_denoise(
                &mut self,
                _: &'static greycard_ai::Model,
            ) -> Result<(), crate::walk::NoLearned> {
                Ok(())
            }
            fn denoise(
                &mut self,
                _: &'static greycard_ai::Model,
                _: &RawFrame,
                _: &greycard_core::develop::Prepared,
            ) -> Result<Vec<f32>, crate::walk::NoLearned> {
                Err(crate::walk::NoLearned::Short(
                    "the card has 1.0 GiB free".into(),
                ))
            }
            fn fill(
                &mut self,
                _: &greycard_edit::retouch::Patch,
                _: &greycard_core::develop::retouch::Region,
                _: &greycard_core::image::WorkingImage,
            ) -> Option<Vec<f32>> {
                None
            }
            fn could_fill_more(
                &mut self,
                _: &greycard_edit::retouch::Retouch,
                _: &greycard_core::image::WorkingImage,
            ) -> bool {
                false
            }
            fn rasters(
                &mut self,
                _: crate::stages::Masks,
                _: &std::sync::Arc<greycard_core::image::WorkingImage>,
                _: Option<crate::walk::Patched<'_>>,
                _: &crate::walk::Meta,
            ) -> crate::walk::Rasters {
                crate::walk::Rasters::new()
            }
        }
        let (dir, files) = bracket("short-card", [0; 3]);
        let options = Options {
            denoiser: Some(&greycard_ai::DENOISE_BEST),
            ..Options::default()
        };
        let e = hdr(
            &files,
            &Edit::default(),
            &options,
            &mut Short,
            &|_: Step| {},
        )
        .map(|_| ())
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("1.0 GiB free") && e.contains("nothing was made"),
            "{e}"
        );
        assert!(!dir.join("IMG_0-HDR.dng").exists());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Frames the learned denoiser does not take (X-Trans here) are
    /// refused a tier before any is walked, where the walk would skip
    /// the denoiser and write the file undenoised; with no tier they
    /// stand, and the window's check says the row has nothing to offer.
    #[test]
    fn a_tier_on_frames_it_cannot_take_is_refused() {
        let dir = scratch("again-xtrans");
        let names = ["DSCF0.dng", "DSCF1.dng", "DSCF2.dng"];
        let frames: Vec<PathBuf> = names
            .iter()
            .map(|n| {
                let p = dir.join(n);
                greycard_library::fixture::write_mosaic_dng(
                    &p,
                    "GGRGGBGGBGGRBRGRBGGGBGGRGGRGGBRBGBRG",
                );
                p
            })
            .collect();
        // The merged file's own body from a calibrated frame, which the
        // fixture's are not; nothing reads its pixels.
        let body = dir.join("body.dng");
        write_dng(
            &body,
            mosaic_of(W, H, 1.0, Noise::ASSUMED, 5, |x, y, c| scene(x, y, c, W, H)),
            W,
            H,
            (1, 100),
            "Body",
        );
        let raw = greycard_core::decode::decode_path(&body).unwrap();
        let (w, h) = (raw.width, raw.height);
        let image = CameraImage::from_data(w, h, vec![0.25; w * h * 3]).unwrap();
        let tag = merge::Tag {
            kind: "hdr".into(),
            frames: names.iter().map(|n| n.to_string()).collect(),
            reference: 1,
            headroom_stops: 1.0,
            heads: frames
                .iter()
                .map(|f| Some(greycard_library::hash_file(f).unwrap()))
                .collect(),
            wholes: vec![None; 3],
            recipe: Some(Recipe {
                ca: "once".into(),
                denoise: None,
                noise: None,
                deghost: None,
                kernel: None,
                geometry: Geometry::Bracket(vec![
                    Registration {
                        exposure: 1.0,
                        shift: None,
                    };
                    3
                ]),
            }),
            ..Default::default()
        };
        let out = dir.join("DSCF0-HDR.dng");
        write_merged_dng(
            std::io::BufWriter::new(std::fs::File::create(&out).unwrap()),
            &raw,
            &image,
            [2.0, 1.0, 1.5],
            &LinearDngOptions {
                headroom: Some(2.0),
                ..Default::default()
            },
            &Merged {
                noise_profile: [1e-5, 1e-8],
                tag: &tag,
            },
        )
        .unwrap();
        let read = greycard_core::decode::merge_tag_path(&out)
            .unwrap()
            .unwrap();
        assert!(frames_of(&out, &read).is_ok());
        assert!(!can_denoise_again(&out, &read));

        let before = std::fs::read(&out).unwrap();
        let steps = std::cell::RefCell::new(Vec::new());
        let refused = again(
            &out,
            &Overrides {
                denoise: Some(Some("balanced".into())),
                ..Default::default()
            },
            &Edit::default(),
            &mut crate::testing::PlainDenoiser,
            &|s: Step| steps.borrow_mut().push(s),
        )
        .map(|_| ())
        .unwrap_err()
        .to_string();
        assert_eq!(
            refused,
            "the learned denoiser takes a 2x2 Bayer mosaic, and DSCF0.dng is X-Trans, and \
             nothing was made again"
        );
        assert!(steps.into_inner().is_empty());
        assert_eq!(std::fs::read(&out).unwrap(), before);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Frames that share a head hash (and so a size) are kept with
    /// their whole files' hashes as well; frames that do not, without.
    #[test]
    fn frames_that_share_a_head_keep_their_whole_hash() {
        let dir = scratch("wholes");
        let head = vec![7u8; greycard_library::hash::HEAD];
        let write = |name: &str, tail: u8| {
            let path = dir.join(name);
            let mut bytes = head.clone();
            bytes.extend([tail; 100]);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let files = [write("a.raw", 1), write("b.raw", 2), {
            let p = dir.join("c.raw");
            std::fs::write(&p, b"another").unwrap();
            p
        }];
        let (heads, wholes) = hashes(&Files(&files));
        assert_eq!(heads[0], heads[1]);
        assert!(heads.iter().all(Option::is_some));
        assert!(wholes[0].is_some() && wholes[1].is_some() && wholes[2].is_none());
        assert_ne!(wholes[0], wholes[1]);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A merge is made again only from its own frames: a file that is
    /// no merge, one written before its recipe was kept, a frame gone
    /// from beside it, and a frame changed since are each refused, and
    /// the file left as it was.
    #[test]
    fn a_merge_without_its_recipe_or_its_frames_is_not_made_again() {
        let (dir, files) = bracket("again-refused", [0; 3]);
        let report = merged(&files, &Options::default());
        let refused = |file: &Path| {
            again(
                file,
                &Overrides::default(),
                &Edit::default(),
                &mut crate::walk::NoModels,
                &|_: Step| {},
            )
            .map(|_| ())
            .unwrap_err()
            .to_string()
        };
        assert_eq!(
            refused(&files[0]),
            format!(
                "{} is not a merge of greycard's: it keeps no recipe",
                files[0].display()
            )
        );
        let old = dir.join("old-HDR.dng");
        greycard_core::merge::testing::write_merged(&files[0], &old);
        assert_eq!(refused(&old), NO_RECIPE);
        // One fact of the recipe spoiled, its length kept, and a frame
        // named out of the folder: each said for what it is.
        let bytes = std::fs::read(&report.out).unwrap();
        let spoiled = |from: &[u8], to: &[u8], name: &str| {
            let at = bytes
                .windows(from.len())
                .position(|w| w == from)
                .expect("the fact");
            let mut b = bytes.clone();
            b[at..at + to.len()].copy_from_slice(to);
            let path = dir.join(name);
            std::fs::write(&path, b).unwrap();
            path
        };
        let broken = spoiled(b"exposure=2 ", b"exposure=2 x", "broken-HDR.dng");
        let fact = refused(&broken);
        assert!(
            fact.starts_with("the recipe in broken-HDR.dng cannot be read: exposure=2 x"),
            "{fact}"
        );
        let outside = spoiled(b"frame=IMG_1.dng", b"frame=../_1.dng", "outside-HDR.dng");
        assert_eq!(
            refused(&outside),
            "the recipe in outside-HDR.dng names a frame \"../_1.dng\" that is not a file's name"
        );

        let before = std::fs::read(&report.out).unwrap();
        let aside = dir.join("aside.dng");
        std::fs::rename(&files[0], &aside).unwrap();
        assert_eq!(
            refused(&report.out),
            format!(
                "IMG_0.dng is not beside {}: a merge is made again from its frames, in its folder",
                report.out.display()
            )
        );
        std::fs::rename(&aside, &files[0]).unwrap();
        let samples = mosaic_of(W, H, 3.8, Noise::ASSUMED, 99, |x, y, c| {
            scene(x, y, c, W, H)
        });
        write_dng(&files[2], samples, W, H, (1, 25), "Bracket");
        assert_eq!(
            refused(&report.out),
            "IMG_2.dng is not the frame that was merged: it has changed since, or another \
             file has its name"
        );
        assert_eq!(std::fs::read(&report.out).unwrap(), before);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Frames of two bodies, one frame, and a frame number past the
    /// end are refused before anything is read.
    #[test]
    fn what_is_no_bracket_is_refused() {
        let (dir, files) = bracket("refused", [0; 3]);
        let other = dir.join("other.dng");
        write_dng(
            &other,
            mosaic_of(W, H, 1.0, Noise::ASSUMED, 3, |x, y, c| scene(x, y, c, W, H)),
            W,
            H,
            (1, 100),
            "Another",
        );
        let run = |files: &[PathBuf], options: &Options| {
            hdr(
                files,
                &Edit::default(),
                options,
                &mut crate::walk::NoModels,
                &|_: Step| {},
            )
            .map(|_| ())
            .unwrap_err()
            .to_string()
        };
        let mixed = [files[0].clone(), other];
        assert_eq!(
            run(&mixed, &Options::default()),
            "a merge wants frames of one body"
        );
        // A frame of another size says so, not what the CA fit made on
        // the reference would say of it.
        let small = dir.join("small.dng");
        write_dng(
            &small,
            mosaic_of(W / 2, H / 2, 1.0, Noise::ASSUMED, 4, |x, y, c| {
                scene(x, y, c, W / 2, H / 2)
            }),
            W / 2,
            H / 2,
            (1, 25),
            "Bracket",
        );
        let sizes = [files[0].clone(), files[1].clone(), small];
        assert_eq!(
            run(&sizes, &Options::default()),
            format!(
                "small.dng is {}x{} and the reference {W}x{H}: a merge wants frames of one size",
                W / 2,
                H / 2
            )
        );
        assert!(run(&files[..1], &Options::default()).starts_with("a merge wants two frames"));
        let past = Options {
            reference: Some(3),
            ..Default::default()
        };
        assert!(run(&files, &past).starts_with("there is no frame 3"));
        // Never over a frame, whatever is asked of a name that is there,
        // and by whatever path it is named.
        for on_exists in [OnExists::Overwrite, OnExists::Increment, OnExists::Skip] {
            let over = Options {
                out: Some(dir.join(".").join("IMG_1.dng")),
                on_exists,
                ..Default::default()
            };
            assert_eq!(
                run(&files, &over),
                "IMG_1.dng is one of the frames: a merge is never written over its frames"
            );
        }
        assert!(!dir.join("IMG_0-HDR.dng").exists());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A still bracket of two flat patches with one edge between them,
    /// the structure all one way: whatever a fit reads along the edge
    /// is not believed, and nothing is resampled. A checker's structure
    /// runs every way: still, it stays; knocked two pixels, it is put
    /// back.
    #[test]
    fn a_fit_on_structure_one_way_is_not_believed() {
        for (what, light) in [
            (
                "patches",
                &(|x, _y, c, _k| two_patches(x, 0, c, W))
                    as &dyn Fn(usize, usize, usize, usize) -> f32,
            ),
            ("checker", &|x, y, c, _k| checker(x, y, c, 0)),
        ] {
            let (dir, files) = bracket_of(what, 31, light);
            let report = merged(&files, &Options::default());
            assert!(report.warnings.is_empty(), "{what}: {:?}", report.warnings);
            assert!(report.frames.iter().all(|f| !f.resampled), "{what}");
            greycard_core::testing::remove_dir_retry(&dir);
        }
        let (dir, files) = bracket_of("checker-knocked", 31, &|x, y, c, k| {
            checker(x, y, c, if k == 2 { 2 } else { 0 })
        });
        let report = merged(&files, &Options::default());
        assert!(report.frames[2].resampled, "{:?}", report.frames[2].moved);
        assert!(!report.frames[0].resampled);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The profile the file carries predicts its noise: two merges of
    /// one scene, their noise drawn apart, differ by twice the file's
    /// variance, which the profile says within a factor of two at a
    /// shadow and a midtone level, and within four a stop over the
    /// bright frame's clip.
    #[test]
    fn the_noise_profile_predicts_the_files_noise() {
        let read = |seed: u64, what: &str| {
            let (dir, files) = bracket_of(what, seed, &|x, y, c, _| scene(x, y, c, W, H));
            let report = merged(&files, &Options::default());
            let back = greycard_core::decode::decode_path(&report.out).unwrap();
            let h = report.headroom_stops.exp2();
            let green: Vec<f32> = back.samples.to_f32().chunks(3).map(|p| p[1] * h).collect();
            greycard_core::testing::remove_dir_retry(&dir);
            (green, report.noise_profile, h)
        };
        let (a, [s, o], h) = read(101, "profile-a");
        let (b, _, _) = read(501, "profile-b");
        // The scene's light by column: a band of columns is a level.
        // Under the bright frame's clip within 2x; a stop over it, where
        // the merge's variance has stepped up, within 4x (a line split
        // across the step can do no better than half of it either way).
        for (stops, within) in [(-9.0f32, 2.0), (-4.0, 2.0), (-1.0, 4.0)] {
            let mut d = Vec::new();
            let mut level = 0.0f64;
            for (i, (x, y)) in a.iter().zip(&b).enumerate() {
                let m = (x + y) / 2.0;
                if (m.log2() - stops).abs() < 0.5 && (8..H - 8).contains(&(i / W)) {
                    d.push(f64::from(x - y));
                    level += f64::from(m);
                }
            }
            assert!(d.len() > 300, "{stops}: {}", d.len());
            let n = d.len() as f64;
            let mean = d.iter().sum::<f64>() / n;
            let measured = d.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n / 2.0;
            let y = level / n / f64::from(h);
            let said = (s * y + o) * f64::from(h * h);
            let ratio = said / measured;
            assert!(
                (1.0 / within..within).contains(&ratio),
                "at {stops} stops: the profile says {said:.3e}, the file has {measured:.3e}"
            );
        }
    }

    /// A bracket made with the ISO (one shutter, ISO 100, 400, 1600):
    /// each frame's noise is the reference's at its ISO, the shot term
    /// with the gain and the read term with its square, and the
    /// exposures are measured as for a shutter bracket.
    #[test]
    fn a_bracket_by_iso_takes_each_frames_noise_at_its_iso() {
        let dir = scratch("iso");
        let isos = [100u16, 400, 1600];
        let files: Vec<PathBuf> = (0..3)
            .map(|k| {
                let r = f32::from(isos[k]) / 400.0;
                let path = dir.join(format!("ISO_{k}.dng"));
                let samples = mosaic_of(
                    W,
                    H,
                    r * [1.05, 1.0, 0.97][k],
                    Noise::ASSUMED.at_iso(r),
                    61 + k as u64,
                    |x, y, c| scene(x, y, c, W, H),
                );
                write_dng_at(&path, samples, W, H, (1, 100), isos[k], "Bracket");
                path
            })
            .collect();
        let report = merged(&files, &Options::default());
        assert_eq!(report.reference, 1);
        let [dark, reference, bright] = [0, 1, 2].map(|k| report.frames[k].clone());
        assert!(
            (dark.exposure - 0.25 * 1.05).abs() < 0.005,
            "{}",
            dark.exposure
        );
        assert!(
            (bright.exposure - 4.0 * 0.97).abs() < 0.05,
            "{}",
            bright.exposure
        );
        let shot = |f: &FrameReport| f.noise.shot / reference.noise.shot;
        let read = |f: &FrameReport| f.noise.read / reference.noise.read;
        // The darkest frame goes in under the model the reference does;
        // the bright one, first in, under the model its own pair gave.
        assert!((shot(&dark) - 0.25).abs() < 1e-5 && (read(&dark) - 0.0625).abs() < 1e-5);
        assert!((2.0..8.0).contains(&shot(&bright)), "{:?}", bright.noise);
        assert!((8.0..32.0).contains(&read(&bright)), "{:?}", bright.noise);
        // The sensor's model, from the pairs, near the one the frames
        // were made with.
        let got = reference.noise.shot / Noise::ASSUMED.shot;
        assert!((0.5..2.0).contains(&got), "{:?}", reference.noise);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// `file` developed as the viewport's walk develops it under the
    /// default edit, on the CPU, brightened by its baseline: what the
    /// finish starts from.
    fn developed(file: &Path) -> WorkingImage {
        let (mut image, source) = scene_of(file);
        // The scene is laid across the frame as written: no turn.
        assert_eq!((image.width, image.height), (W, H));
        let baseline = source.baseline(&Edit::default().light).exp2();
        image.data.iter_mut().for_each(|v| *v *= baseline);
        image
    }

    /// `file` developed as the viewport's walk develops it under the
    /// default edit, on the CPU, before any exposure: the picture the
    /// window holds and Auto reads, and what the finish is handed.
    fn scene_of(file: &Path) -> (WorkingImage, greycard_render::finish::Source) {
        let frame = greycard_core::decode::decode_path(file).unwrap();
        let source = greycard_render::finish::Source::of_frame(&frame);
        let input = Input::Raw(Arc::new(frame));
        let edit = Edit::default();
        let steps = stages(&edit, &Purpose::Viewport, &input.facts(0), None);
        let places = plan(&steps, &Purpose::Viewport, false);
        let walked = walk(
            &steps,
            &places,
            &input,
            &mut Kept::new(),
            &mut crate::walk::NoModels,
            &mut None,
            &NoHold,
        )
        .unwrap();
        let Buf::Working(image) = walked.buf else {
            panic!("the walk ends in the working space");
        };
        ((*image).clone(), source)
    }

    /// Auto on `file` under `edit`, as the window reads it: the
    /// histogram of the picture before any exposure, at the frame's
    /// baseline, with a merge's headroom and the frame's range.
    fn auto_of(file: &Path, edit: &Edit) -> greycard_edit::auto::LightChanges {
        use greycard_edit::auto::{Histogram, auto_light_kept};
        let (image, source) = scene_of(file);
        let pixels: Vec<[f32; 3]> = image.data.chunks(3).map(|p| [p[0], p[1], p[2]]).collect();
        let guide = greycard_render::finish::guide_plane(&image);
        auto_light_kept(
            &Histogram::of_pixels(&pixels),
            source.baseline(&edit.light),
            source.auto_headroom(&guide),
        )
    }

    /// `file` exported under `edit`, its encoded values from 0 to 1.
    fn exported(file: &Path, edit: &Edit) -> Vec<f32> {
        let mut developed =
            crate::worker::FrameDevelop::develop(file, edit, None).expect("it develops");
        let rendered = developed.finish(edit, &greycard_render::export::Settings::default());
        assert_eq!((rendered.width, rendered.height), (W as u32, H as u32));
        match rendered.pixels {
            greycard_render::export::Pixels::Eight(p) => {
                p.iter().map(|v| f32::from(*v) / 255.0).collect()
            }
            greycard_render::export::Pixels::Sixteen(p) => {
                p.iter().map(|v| f32::from(*v) / 65535.0).collect()
            }
        }
    }

    /// The mean encoded luminance of an export's columns from `from`
    /// to `to` of its width, its top and bottom rows left out: the
    /// scene is a ramp across the frame, so a band of columns is a
    /// band of brightness.
    fn shown(px: &[f32], from: f32, to: f32) -> f64 {
        let (x0, x1) = ((from * W as f32) as usize, (to * W as f32) as usize);
        let (mut sum, mut n) = (0.0, 0.0);
        for y in 8..H - 8 {
            for x in x0..x1 {
                let i = (y * W + x) * 3;
                sum += f64::from(0.2627 * px[i] + 0.678 * px[i + 1] + 0.0593 * px[i + 2]);
                n += 1.0;
            }
        }
        sum / n
    }

    /// The develop's pin: the merged bracket and its reference frame,
    /// each developed as a file opens, agree in the midtones and the
    /// highlights the reference held; where the reference clipped, the
    /// merge keeps the scene, stops brighter than the reference's clip,
    /// rather than coming out as a flat frame pulled down to fit.
    #[test]
    fn the_merge_develops_as_its_reference_did_with_its_highlights_kept() {
        let (dir, files) = bracket("develop", [0; 3]);
        let report = merged(&files, &Options::default());
        let reference = developed(&files[1]);
        let merge = developed(&report.out);
        assert_eq!(
            (reference.width, reference.height),
            (merge.width, merge.height)
        );
        let luma = |p: &[f32]| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2];
        // Bands of columns: the scene is a ramp across the frame, -12
        // stops at the left to +4 over the middle frame's clip at the
        // right, so a column band is a band of brightness.
        let band = |img: &WorkingImage, from: f32, to: f32| -> f64 {
            let (x0, x1) = ((from * W as f32) as usize, (to * W as f32) as usize);
            let mut sum = 0.0;
            let mut n = 0.0;
            for y in 8..H - 8 {
                for x in x0..x1 {
                    let i = (y * img.width + x) * 3;
                    sum += f64::from(luma(&img.data[i..i + 3]));
                    n += 1.0;
                }
            }
            sum / n
        };
        // Midtones (-6 to -2 stops) and the highlights the reference
        // held (-2 to -0.5): within 2%.
        for (from, to) in [(0.375, 0.625), (0.625, 0.72)] {
            let (r, m) = (band(&reference, from, to), band(&merge, from, to));
            assert!((m / r - 1.0).abs() < 0.02, "{from}..{to}: {m} against {r}");
        }
        // Two to four stops over the reference's clip: the reference is
        // flat there, the merge climbs on.
        let (r, m) = (band(&reference, 0.9, 1.0), band(&merge, 0.9, 1.0));
        let r_below = band(&reference, 0.8, 0.85);
        assert!(
            (r / r_below) < 1.5,
            "the reference clipped: {r} and {r_below}"
        );
        assert!(
            m > 3.0 * r,
            "the merge holds the highlights: {m} against {r}"
        );

        // And through the finish, as an export makes the file: the
        // merge's export against the reference's, each under the edit
        // a merge starts from, at the reference's exposure.
        let edit = edit_for_merge(&Edit::default());
        let (re, me) = (exported(&files[1], &edit), exported(&report.out, &edit));
        // The midtones and the highlights the reference held within 2%,
        // the brightest of them (display 200 to 250) as well: the
        // merge's display white is its body's, as the reference's is.
        for (from, to) in [(0.375, 0.625), (0.625, 0.72), (0.69, 0.75)] {
            let (r, m) = (shown(&re, from, to), shown(&me, from, to));
            assert!(
                (m / r - 1.0).abs() < 0.02,
                "exported, {from}..{to}: {m} against {r}"
            );
        }
        // The range over the reference's clip is there to bring down:
        // two stops under, the reference's export is flat there and the
        // merge's climbs.
        let mut down = edit.clone();
        down.light.exposure = -2.0;
        let (re, me) = (exported(&files[1], &down), exported(&report.out, &down));
        let rise = |px: &[f32]| shown(px, 0.9, 0.95) - shown(px, 0.8, 0.85);
        assert!(rise(&re) < 0.01, "the reference: {}", rise(&re));
        assert!(rise(&me) > 0.03, "the merge: {}", rise(&me));
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Auto reads a merge as the develop shows it: where its reference
    /// lands on the same scene, its exposure within a quarter stop, its
    /// Highlights within 0.15, and its Whites within three quarters
    /// (the merge's top reads two stops over the reference's, and
    /// Whites holds it under white where the reference's clip needs
    /// less), and the kept range left to Highlights' pull: at the
    /// merge's Auto it climbs under white.
    /// Read without its headroom, as before, the merge read three stops
    /// dark (Highlights -0.39 and Whites -0.08 where the reference
    /// gets -0.9 and -1.02), and the kept range stayed at white.
    #[test]
    fn auto_lands_a_merge_where_its_reference_lands() {
        let (dir, files) = bracket("auto", [0; 3]);
        let report = merged(&files, &Options::default());
        let edit = edit_for_merge(&Edit::default());
        let (reference, merge) = (auto_of(&files[1], &edit), auto_of(&report.out, &edit));
        eprintln!("auto: the reference {reference:?}, the merge {merge:?}");
        for (what, m, r, within) in [
            ("exposure", merge.exposure, reference.exposure, 0.25),
            ("highlights", merge.highlights, reference.highlights, 0.15),
            ("whites", merge.whites, reference.whites, 0.75),
        ] {
            assert!((m - r).abs() <= within, "{what}: {m} against {r}");
        }
        assert!(merge.whites > -2.0, "{merge:?}");

        let mut at = edit.clone();
        at.light = merge.applied(&edit.light);
        let me = exported(&report.out, &at);
        let bands: Vec<f64> = (0..5)
            .map(|k| {
                let from = 0.75 + 0.05 * k as f32;
                shown(&me, from, from + 0.05)
            })
            .collect();
        eprintln!("the merge at its Auto, 0.75 to 1.0: {bands:?}");
        assert!(
            bands.windows(2).all(|b| b[1] > b[0] + 0.005),
            "the kept range climbs: {bands:?}"
        );
        // Under white, the last band's texture lifting its mean into
        // the shoulder.
        assert!(bands[3] < 0.99 && bands[4] < 0.999, "{bands:?}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Auto on the raws named by `GREYCARD_AUTO_RAWS` (paths split by
    /// `:`), as the window reads them, printed to the bit: frames
    /// without headroom compared across a change.
    #[test]
    #[ignore]
    fn auto_on_named_raws() {
        let Ok(raws) = std::env::var("GREYCARD_AUTO_RAWS") else {
            return;
        };
        for raw in raws.split(':') {
            let a = auto_of(Path::new(raw), &Edit::default());
            let bits = [a.exposure, a.highlights, a.shadows, a.whites, a.blacks].map(f32::to_bits);
            println!("auto {raw}: {a:?} {bits:08x?}");
        }
    }

    /// Highlights at -100 brings back what the merge kept over its
    /// reference's clip, as the exposure does: the bands over the clip
    /// climb under white, the brightest of them where a clip at display
    /// white lands at -100 on a frame with no headroom. The reference,
    /// which kept nothing, is flat there as it was.
    #[test]
    fn highlights_brings_back_what_the_merge_kept() {
        use greycard_render::finish::{Baked, Source, White, finish_pixel_with, guide_plane};
        let (dir, files) = bracket("highlights", [0; 3]);
        let report = merged(&files, &Options::default());
        let mut edit = edit_for_merge(&Edit::default());
        edit.light.tone.highlights = -2.0;
        let (re, me) = (exported(&files[1], &edit), exported(&report.out, &edit));
        let bands: Vec<f64> = (0..5)
            .map(|k| {
                let from = 0.75 + 0.05 * k as f32;
                shown(&me, from, from + 0.05)
            })
            .collect();
        eprintln!("the merge at -100, 0.75 to 1.0: {bands:?}");
        let rise = |px: &[f32]| shown(px, 0.9, 0.95) - shown(px, 0.8, 0.85);
        assert!(rise(&re) < 0.01, "the reference: {}", rise(&re));
        assert!(rise(&me) > 0.03, "the merge: {}", rise(&me));
        assert!(
            bands.windows(2).all(|b| b[1] > b[0] + 0.01),
            "the kept range climbs: {bands:?}"
        );

        // Where a clip at display white lands at -100 on a frame with
        // no headroom: a flat region there, the frame's top, its median
        // where the merge's reads.
        let frame = greycard_core::decode::decode_path(&report.out).unwrap();
        let headroom = greycard_edit::baseline::headroom(&frame);
        let body = greycard_edit::baseline::for_frame(&frame);
        assert!(headroom > 2.0, "{headroom}");
        let mut scene = developed(&report.out);
        let brightened = (body + headroom).exp2();
        scene.data.iter_mut().for_each(|v| *v /= brightened);
        let [median, _] = guide_plane(&scene).range;
        let white = White::of(Source::SCENE, &edit.light).stops;
        let at_white = white - body;
        let px = [0.18 * at_white.exp2(); 3];
        let clip = finish_pixel_with(
            px,
            &Baked::global(&edit, Source::SCENE),
            &[],
            0.0,
            None,
            None,
            Some((at_white, [median + headroom, at_white])),
            None,
            None,
            &greycard_render::export::Space::Srgb.matrix(),
        );
        let clip = f64::from(0.2627 * clip[0] + 0.678 * clip[1] + 0.0593 * clip[2]);
        eprintln!("a clip at white at -100: {clip}");
        assert!(clip < 0.99, "{clip}");
        // The pull brings the frame's top region to display white
        // exactly (the finish's own test holds it to 1e-5), and a flat
        // region there lands on `clip`. The bands here are not flat:
        // the scene's texture (30% fine, patches of 0.7 to 1.3) sits
        // over the region's geometric mean that the plane reads, and
        // in the shoulder that lifts a band's mean by about 0.02 (the
        // last column reads 0.986 where a flat region at its reading
        // gives 0.961). So within 0.03, and under white.
        let top = bands[4];
        assert!((top - clip).abs() < 0.03, "{top} against {clip}");
        assert!(top < 0.99, "{top}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The measurement set: a bracket made from one real raw (named by
    /// `GREYCARD_MERGE_RAW`), its photosites scaled and clipped as a
    /// sensor would with shot and read noise added, EXIF told the
    /// nominal steps while the true ones miss them, merged as the file
    /// is and read back; the error against the source before the noise,
    /// by band of brightness, for the reference alone and for the
    /// merge, and the peak resident memory after the merge. Three
    /// frames, or five with `GREYCARD_MERGE_FRAMES=5`. The file goes to
    /// `GREYCARD_MERGE_OUT`.
    #[test]
    #[ignore]
    fn measure_a_synthetic_bracket() {
        let Ok(raw) = std::env::var("GREYCARD_MERGE_RAW") else {
            return;
        };
        let out_dir = PathBuf::from(std::env::var("GREYCARD_MERGE_OUT").unwrap());
        std::fs::create_dir_all(&out_dir).unwrap();
        let five = std::env::var("GREYCARD_MERGE_FRAMES").as_deref() == Ok("5");
        let (truth_gain, nominal): (Vec<f32>, Vec<f64>) = if five {
            (
                vec![1.0 / 3.8, 1.0, 3.8, 15.5, 60.0],
                vec![0.25, 1.0, 4.0, 16.0, 64.0],
            )
        } else {
            (vec![1.0, 3.8, 15.5], vec![1.0, 4.0, 16.0])
        };
        let reference = truth_gain.len() / 2;
        // `GREYCARD_MERGE_DEMOSAIC=bilinear` as the prototype had it;
        // the edit's default otherwise.
        let mut edit = Edit::default();
        if std::env::var("GREYCARD_MERGE_DEMOSAIC").as_deref() == Ok("bilinear") {
            edit.demosaic = greycard_edit::Demosaic::Bilinear;
        }
        let edit = &edit;
        // `GREYCARD_MERGE_CA=frame` for each frame's own CA fit, `off`
        // for none; the reference's once otherwise.
        let ca = match std::env::var("GREYCARD_MERGE_CA").as_deref() {
            Ok("frame") => CaFits::PerFrame,
            Ok("off") => CaFits::Off,
            _ => CaFits::Once,
        };
        println!("CA {ca:?}");
        let (src, meta) = greycard_core::decode::decode_path_with_metadata(&raw).unwrap();
        let synthetic = Synthetic {
            src: &src,
            meta: &meta,
            gains: &truth_gain,
            nominal: &nominal,
        };
        let out = out_dir.join(if five { "hdr-5.dng" } else { "hdr-3.dng" });
        let _ = std::fs::remove_file(&out);
        let started = Instant::now();
        let report = hdr_of(
            &synthetic,
            &out,
            edit,
            &Options {
                out: Some(out.clone()),
                // `GREYCARD_MERGE_NOISE=assumed`: the weights on the
                // noise the frames were made with, as the prototype's.
                noise: (std::env::var("GREYCARD_MERGE_NOISE").as_deref() == Ok("assumed"))
                    .then_some(Noise::ASSUMED),
                ca,
                ..Default::default()
            },
            &mut crate::walk::NoModels,
            &|_: Step| {},
        )
        .unwrap();
        let took = started.elapsed().as_secs_f64();
        let peak = peak_gb();
        println!(
            "frames {}, {}x{}",
            truth_gain.len(),
            report.width,
            report.height
        );
        println!("merge {took:.2} s, peak resident {peak:.2} GB after it");
        for (name, s) in &report.seconds {
            println!("  {name:<10} {s:.2} s");
        }
        for f in &report.frames {
            println!(
                "  {}: EXIF {:?}, measured {:?}, used {:.4}, moved {:?}, noise {:?}",
                f.name,
                f.exif,
                f.measured.map(|m| m.ratio),
                f.exposure,
                f.moved,
                f.noise
            );
        }
        println!(
            "  all clipped {} px; headroom {:.2} stops; profile {:?}; warnings {:?}",
            report.all_clipped, report.headroom_stops, report.noise_profile, report.warnings
        );
        println!(
            "  file {:.0} MB",
            std::fs::metadata(&out).unwrap().len() as f64 / 1e6
        );

        // The truth: the source walked as the merge walks a frame,
        // before any noise was added, on the reference's scale. Its CA
        // is its own fit, or none when the merge had none; beside it,
        // the truth by the reference's fit, which the merge applies
        // when it fits once.
        let gains_of_file = report_gains(&out);
        let white = |ca| MergeInput {
            white: Some(gains_of_file),
            reconstruct: false,
            denoiser: None,
            ca,
            lens: Vec::new(),
        };
        let own = || match ca {
            CaFits::Off => MergeCa::Off,
            _ => MergeCa::Own,
        };
        let (refr, _, _) = walk_frame(
            &synthetic,
            reference,
            edit,
            white(if ca == CaFits::Off {
                MergeCa::Off
            } else {
                MergeCa::Make
            }),
            None,
            &mut crate::walk::NoModels,
        )
        .unwrap();
        let source = One(&src, &meta);
        let (truth, _, _) = walk_frame(
            &source,
            0,
            edit,
            white(own()),
            None,
            &mut crate::walk::NoModels,
        )
        .unwrap();
        let taken = refr.ca_fit.clone().map(|fit| {
            let (t, _, _) = walk_frame(
                &source,
                0,
                edit,
                white(MergeCa::Take(CaFitRef(fit))),
                None,
                &mut crate::walk::NoModels,
            )
            .unwrap();
            t
        });
        let keep: Vec<bool> = truth.mask.iter().map(|m| !m).collect();
        let g = truth_gain[reference];
        let truth_rgb: Vec<f32> = truth.rgb.iter().map(|v| v * g).collect();
        let keep_ref: Vec<bool> = keep.iter().zip(&refr.mask).map(|(k, c)| *k && !c).collect();
        let back = greycard_core::decode::decode_path(&out).unwrap();
        let headroom = report.headroom_stops.exp2();
        let merged: Vec<f32> = back
            .samples
            .to_f32()
            .into_iter()
            .map(|v| v * headroom)
            .collect();
        let gains = truth.gains;
        let e_ref = band_errors(&refr.rgb, &truth_rgb, &keep_ref, gains);
        let e_on_ref = band_errors(&merged, &truth_rgb, &keep_ref, gains);
        let e_merge = band_errors(&merged, &truth_rgb, &keep, gains);
        let e_taken = taken.map(|t| {
            let t: Vec<f32> = t.rgb.iter().map(|v| v * g).collect();
            band_errors(&merged, &t, &keep, gains)
        });
        println!(
            "\n| band | reference alone (px) | merge on the same px | merge (px) | \
             merge against the truth by the reference's fit |"
        );
        println!("|---|---|---|---|---|");
        for i in 0..e_ref.len() {
            let f = |x: f64| {
                if x.is_finite() {
                    format!("{x:.5}")
                } else {
                    "-".into()
                }
            };
            println!(
                "| {} | {} ({}) | {} | {} ({}) | {} |",
                e_ref[i].0,
                f(e_ref[i].1),
                e_ref[i].2,
                f(e_on_ref[i].1),
                f(e_merge[i].1),
                e_merge[i].2,
                e_taken.as_ref().map_or("-".into(), |e| f(e[i].1))
            );
        }
    }

    /// The white the merged file says, as gains.
    fn report_gains(out: &Path) -> [f64; 3] {
        greycard_core::decode::decode_path(out)
            .unwrap()
            .as_shot_coefficients
            .unwrap()
    }

    /// The process's peak resident set, from /proc.
    fn peak_gb() -> f64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap_or_default()
            .lines()
            .find(|l| l.starts_with("VmHWM:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|k| k.parse::<f64>().ok())
            .map_or(0.0, |kb| kb / 1024.0 / 1024.0)
    }

    /// One frame held, as a source.
    struct One<'a>(&'a RawFrame, &'a RawMetadata);

    impl Sources for One<'_> {
        fn len(&self) -> usize {
            1
        }
        fn name(&self, _: usize) -> String {
            "source".into()
        }
        fn head(&self, _: usize) -> Result<Head> {
            Ok(Head {
                make: self.0.make.clone(),
                model: self.0.model.clone(),
                shot: self.0.shot.clone(),
            })
        }
        fn load(&self, _: usize) -> Result<(RawFrame, RawMetadata)> {
            Ok((self.0.clone(), self.1.clone()))
        }
    }

    /// A bracket made from one raw, a frame at a time.
    struct Synthetic<'a> {
        src: &'a RawFrame,
        meta: &'a RawMetadata,
        gains: &'a [f32],
        nominal: &'a [f64],
    }

    impl Sources for Synthetic<'_> {
        fn len(&self) -> usize {
            self.gains.len()
        }
        fn name(&self, i: usize) -> String {
            format!("frame{i}")
        }
        fn head(&self, i: usize) -> Result<Head> {
            let mut shot = self.src.shot.clone();
            shot.exposure_time = shot.exposure_time.map(|t| t * self.nominal[i] as f32);
            Ok(Head {
                make: self.src.make.clone(),
                model: self.src.model.clone(),
                shot,
            })
        }
        fn load(&self, i: usize) -> Result<(RawFrame, RawMetadata)> {
            Ok((
                bracket_frame(self.src, self.gains[i], 17 + i as u64),
                self.meta.clone(),
            ))
        }
    }

    /// A frame of the made-up bracket: every photosite above black
    /// times `gain`, shot and read noise added as `Noise::ASSUMED`
    /// has them, clipped at the white as the sensor would.
    fn bracket_frame(src: &RawFrame, gain: f32, seed: u64) -> RawFrame {
        use rayon::prelude::*;
        let Samples::U16(samples) = &src.samples else {
            panic!("the bracket is made from 16-bit samples");
        };
        let (w, cpp) = (src.width, src.channels);
        let (black, white) = (&src.levels.black, &src.levels.white);
        let noise = Noise::ASSUMED;
        let out: Vec<u16> = samples
            .par_chunks(w * cpp)
            .enumerate()
            .flat_map_iter(|(y, row)| {
                let mut s = seed ^ (y as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let mut rnd = move || {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    (s >> 11) as f64 / (1u64 << 53) as f64
                };
                row.chunks(cpp)
                    .enumerate()
                    .flat_map(|(x, px)| {
                        px.iter()
                            .enumerate()
                            .map(|(ch, v)| {
                                let (b, wl) = (black.at(y, x, ch), white.at(y, x, ch));
                                let n = (f32::from(*v) - b) / (wl - b);
                                let (u1, u2) = (rnd().max(1e-12), rnd());
                                let g = ((-2.0 * u1.ln()).sqrt()
                                    * (std::f64::consts::TAU * u2).cos())
                                    as f32;
                                let sig = n * gain;
                                let n2 = (sig + g * noise.variance(sig).sqrt()).min(1.0);
                                (b + n2 * (wl - b)).round().clamp(0.0, 65535.0) as u16
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut f = src.clone();
        f.samples = Samples::U16(out);
        f
    }

    /// Relative RMS error of `got` against `truth` (camera RGB on the
    /// reference's scale) in bands of the truth's brightest balanced
    /// channel, in stops under the reference's clip, over the pixels
    /// `keep` allows.
    fn band_errors(
        got: &[f32],
        truth: &[f32],
        keep: &[bool],
        gains: [f32; 3],
    ) -> Vec<(String, f64, usize)> {
        use rayon::prelude::*;
        let bands = [
            (-14.0, -10.0),
            (-10.0, -6.0),
            (-6.0, -2.0),
            (-2.0, 0.0),
            (0.0, 1.0),
            (1.0, 4.0),
        ];
        bands
            .iter()
            .map(|&(lo, hi): &(f32, f32)| {
                let (lo_v, hi_v) = (lo.exp2(), hi.exp2());
                let (se, st, n) = (0..keep.len())
                    .into_par_iter()
                    .filter(|&i| keep[i])
                    .filter_map(|i| {
                        let t = &truth[i * 3..i * 3 + 3];
                        let g = &got[i * 3..i * 3 + 3];
                        let b = (0..3).map(|c| t[c] * gains[c]).fold(0.0f32, f32::max);
                        if b < lo_v || b >= hi_v || !g[0].is_finite() {
                            return None;
                        }
                        let e: f64 =
                            (0..3).map(|c| f64::from(g[c] - t[c]).powi(2)).sum::<f64>() / 3.0;
                        Some((e, f64::from(merge::luma(t)), 1usize))
                    })
                    .reduce(|| (0.0, 0.0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2));
                let rel = if n > 0 {
                    (se / n as f64).sqrt() / (st / n as f64)
                } else {
                    f64::NAN
                };
                (format!("{lo:+.0}..{hi:+.0} stops"), rel, n)
            })
            .collect()
    }
}
