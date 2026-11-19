//! The panorama: frames turned about the lens, walked one at a time to
//! camera RGB through the stage list (`Purpose::MergeInput`, the lens's
//! vignetting in camera space), placed on one canvas by the core's
//! `merge::pano`, cut by its `seam`, blended by its `blend`, and
//! written as a linear DNG beside the first source.
//!
//! Two passes over the frames. The first walks each: its luminance,
//! corrected for the lens's distortion, at half and a quarter of the
//! size; its corners; a reduced copy for the exposure and the seams;
//! and its samples, spilled to the hidden folder that holds the
//! output's name while the merge runs, since where a frame lands is
//! not known until every frame has been placed.
//! Each frame is matched with the ones walked before it that may share
//! a view (its neighbors in the order given, two away, and the last
//! with the first), each pair refined by the homography fit. The frames
//! are then placed, all at once, and the canvas, the gains, the seams,
//! and the crop are settled on the reduced copies. The second pass
//! reads each frame back, resamples it once onto the canvas (the lens's
//! distortion, the turn, and the projection in one map), and adds it
//! to the canvas's bands. Held at once: the bands, one frame, and the
//! reduced copies.
//!
//! The reference, whose view and scale the panorama is in, is the
//! frame in the middle of the turn once the frames are placed, unless
//! one is asked; the frame walked first (the middle of the list) gives
//! every frame its white and CA fit.
//!
//! The file's tag keeps how the frames were laid. Given that
//! ([`Options::geometry`]), as a merge made again is, the first pass
//! only walks and spills, and nothing is matched, fitted, leveled,
//! measured for its gains, or cropped: the frames are resampled,
//! seamed, and blended onto the canvas and crop the tag keeps.
//!
//! The command line's `greycard merge pano` and the window's Merge to
//! Panorama both run [`pano`].

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use greycard_core::develop::lens::LensCorrection;
use greycard_core::dng::{LinearDngOptions, Merged, write_merged_dng};
use greycard_core::merge::blend::{self, Bands};
use greycard_core::merge::features::{self, Corners, Keypoint};
use greycard_core::merge::pano::{
    self as geo, FrameLens, Kernel, Link, Model as Laid, Placement, Rect, Source,
};
use greycard_core::merge::seam::{Composite, Cut};
use greycard_core::merge::{self, Geometry, PairNoise, Recipe};
use greycard_core::output::OnExists;
use greycard_core::raw::{Orientation, RawFrame};
use greycard_core::register::{self, Model};
use greycard_core::{CameraImage, decode::RawMetadata};
use greycard_edit::Edit;
use rayon::prelude::*;

use super::{Files, Progress, Sources, Step, Walked, heads, walk_loaded};
use crate::stages::{CaFitRef, Facts, MergeCa, MergeInput};
use crate::walk::Models;

/// What goes after the first frame's name.
pub const SUFFIX: &str = "-pano";
/// Fewer pairs of corners than this that one homography holds, and two
/// frames are taken to share nothing.
pub const MIN_INLIERS: usize = 16;
/// The most pixels a panorama's canvas, as cropped, is allowed for
/// now: the bands of 100 MP are about 2 GB. Past it a merge in strips
/// would be wanted, which is not built.
pub const CANVAS_CAP: usize = 100_000_000;
/// How much the canvas is reduced for the exposure, the seams, and the
/// crop.
const STEP: usize = 8;
/// The features' plane is a quarter of the frame, the fit's half.
const FEATURES: f64 = 4.0;
const FIT: f64 = 2.0;
/// How far the homography fit may move a pair from where its features
/// put it, in pixels, before it is taken to have gone astray.
const FIT_LEASH: f64 = 40.0;
/// The residual under which a pair's homography fit is taken, on the
/// fit's own scale ([`register::ALIGNED_RESIDUAL`]'s). A stack's frames
/// are one view, and a twentieth is its bound; a handheld turn's are
/// not quite, since the lens moves as the camera turns and the near
/// and the far shift apart (parallax), which one homography cannot
/// take up. The three frames' pairs came to 0.12 to 0.16; a fit that
/// went into the wrong valley comes in past 0.3 and, there, is past
/// the leash too.
const PANO_RESIDUAL: f32 = 0.25;

/// What a panorama is asked.
#[derive(Clone)]
pub struct Options {
    /// The frame whose view the others are turned onto, by its place
    /// among the frames; none for the middle of the turn, found once
    /// the frames are placed (the middle of the list is walked first).
    pub reference: Option<usize>,
    /// Where the file goes; none for beside the first frame, named
    /// after it ([`default_output`]).
    pub out: Option<PathBuf>,
    pub on_exists: OnExists,
    /// The learned denoiser in each frame's demosaic's place.
    pub denoiser: Option<&'static greycard_ai::Model>,
    /// The lens database the profile comes from; none for the user's
    /// own, as the develop has it.
    pub lenses: Option<Arc<greycard_lens::Database>>,
    /// How each frame is resampled onto the canvas.
    pub kernel: Kernel,
    /// The frames laid as a recipe keeps them, in place of the fit:
    /// their turns, the lens, the gains, the canvas, and the crop. For
    /// a merge made again ([`super::again`]); `reference` then says
    /// which frame the recipe's reference is.
    pub geometry: Option<Box<merge::Laid>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            reference: None,
            out: None,
            on_exists: OnExists::default(),
            denoiser: None,
            lenses: None,
            kernel: Kernel::Lanczos3,
            geometry: None,
        }
    }
}

/// What one pair of frames came to.
#[derive(Debug, Clone)]
pub struct PairReport {
    pub a: usize,
    pub b: usize,
    /// Each frame's corners.
    pub corners: (usize, usize),
    pub matches: usize,
    pub inliers: usize,
    /// The inliers' distance from the features' homography, pixels.
    pub feature_rms: f64,
    /// The homography fit's residual (its own units), and how far it
    /// moved the pair from the features', pixels; none when it did not
    /// hold.
    pub fit: Option<(f32, f64)>,
    /// The pair's points' distance once every frame is placed, pixels.
    pub placed_rms: f64,
    pub seconds: f64,
}

/// What a panorama did.
#[derive(Debug, Clone)]
pub struct Report {
    pub out: PathBuf,
    pub reference: usize,
    pub frames: Vec<String>,
    pub pairs: Vec<PairReport>,
    /// The focal length in pixels from EXIF and the sensor's size
    /// (none when not read: a merge made again reads no lens
    /// database), and as fitted; none for a planar panorama of unknown
    /// focal length.
    pub focal: Option<(Option<f64>, f64)>,
    /// The projection, in words.
    pub projection: String,
    /// The field the frames span, degrees across and down.
    pub span: Option<(f64, f64)>,
    /// Each overlapping pair's per-channel median ratio, the later over
    /// the earlier, and its pixels.
    pub ratios: Vec<(usize, usize, [f64; 3], usize)>,
    /// Each frame's gains onto the reference's scale.
    pub gains: Vec<[f64; 3]>,
    pub canvas: (usize, usize),
    pub crop: Rect,
    pub levels: usize,
    pub kernel: Kernel,
    pub seams: Vec<Cut>,
    /// Points on each frame's seam with the frames before it, in the
    /// written picture's pixels: its middle one by height and one five
    /// sixths of the way down; none for the first frame.
    pub seam_points: Vec<Vec<(usize, usize)>>,
    pub headroom_stops: f32,
    pub noise_profile: [f64; 2],
    pub denoised: bool,
    /// The bytes the canvas's bands held.
    pub bands_bytes: usize,
    pub seconds: Vec<(&'static str, f64)>,
    pub warnings: Vec<String>,
    /// Why frames that asked for a learned tier went without it.
    pub learned_skipped: Option<super::LearnedSkipped>,
}

/// `<first frame>-pano.dng`, beside the first frame.
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

/// The frame a panorama is walked from first: the one asked, or the
/// middle of those given. Asked, it is the reference too; otherwise
/// the reference is the middle of the turn, which the merge finds once
/// the frames are placed ([`geo::Model::middle`]).
pub fn reference_of(count: usize, asked: Option<usize>) -> Result<usize> {
    if count < 2 {
        bail!("a panorama wants two frames or more; got {count}");
    }
    match asked {
        Some(r) if r < count => Ok(r),
        Some(r) => bail!(
            "there is no frame {r}: the frames count from 0 to {}",
            count - 1
        ),
        None => Ok((count - 1) / 2),
    }
}

/// The edit a panorama's file starts with: the reference frame's, less
/// what the merge did or what was of one frame's geometry. Less the
/// exposure, the learned denoiser, and the highlight fill, as a
/// bracket's; less the lens
/// section, which the merge ran (its profile, a distortion or an
/// aberration set by hand); less the crop, the turn, the retouch, and
/// the masks, which were placed on the reference frame and mean
/// nothing on the canvas. The global look is kept.
pub fn edit_for_pano(reference: &Edit) -> Edit {
    let mut edit = super::edit_for_merge(reference);
    edit.lens = Default::default();
    edit.geometry = Default::default();
    edit.retouch = Default::default();
    edit.adjustments = Vec::new();
    edit
}

/// Whether frames walked under edit `a` and under edit `b` come out
/// alike for a panorama: the same demosaic, camera profile, lens
/// section, and learned denoiser. What else an edit holds, a merge's
/// walk does not read.
pub fn walks_alike(a: &Edit, b: &Edit) -> bool {
    a.demosaic == b.demosaic
        && a.camera == b.camera
        && a.lens == b.lens
        && a.noise.tier() == b.noise.tier()
}

/// The frames `files` merged into a panorama under `edit`'s demosaic,
/// CA, and lens, and written as a linear DNG beside the first frame
/// unless `options` says where.
pub fn pano(
    files: &[PathBuf],
    edit: &Edit,
    options: &Options,
    models: &mut dyn Models,
    progress: &dyn Progress,
) -> Result<Report> {
    let first = files.first().map(|f| default_output(f)).unwrap_or_default();
    pano_of(&Files(files), &first, edit, options, models, progress)
}

/// A frame's samples, held on disk between the passes.
struct Spill {
    path: PathBuf,
}

impl Spill {
    /// `rgb` written to `path`, made new in the run's own folder, and
    /// removed when it is dropped.
    fn write(path: PathBuf, rgb: &[f32]) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        let spill = Spill { path };
        let mut f = std::io::BufWriter::new(file);
        f.write_all(bytemuck::cast_slice(rgb))?;
        f.flush()?;
        Ok(spill)
    }

    fn read(&self, len: usize) -> Result<Vec<f32>> {
        let mut out = vec![0.0f32; len];
        std::fs::File::open(&self.path)
            .and_then(|mut f| f.read_exact(bytemuck::cast_slice_mut(&mut out)))
            .with_context(|| format!("reading back {}", self.path.display()))?;
        Ok(out)
    }
}

impl Drop for Spill {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// What the first pass keeps of a frame.
struct Kept {
    /// Its luminance, corrected for the lens, at half the size; held
    /// while a frame to come may pair with it.
    plane: Option<Vec<f32>>,
    corners: Vec<Keypoint>,
    /// Its reduced copy, clipped blocks at the clip, and its size.
    reduced: (Vec<f32>, usize, usize),
    spill: Spill,
}

/// [`pano`] over any [`Sources`], written to `options.out` or else
/// `default_out`.
pub fn pano_of(
    sources: &dyn Sources,
    default_out: &Path,
    edit: &Edit,
    options: &Options,
    models: &mut dyn Models,
    progress: &dyn Progress,
) -> Result<Report> {
    let out = match &options.out {
        Some(p) => p.clone(),
        None => default_out.to_path_buf(),
    };
    // The name held, and the frames spilled in its folder: a merge
    // killed midway leaves one folder, which the next merge to the
    // name clears.
    let reserved = super::hold(sources, &out, options.on_exists)?;
    let mut report = laid(
        sources,
        edit,
        options,
        &reserved,
        &reserved.part(),
        models,
        progress,
    )?;
    report.out = reserved.promote(&report.out)?;
    Ok(report)
}

/// [`pano`] written under the name `reserved` holds, as its file
/// `name` there ([`super::Reserved::join`]), and left there for the
/// caller to promote or not: for a caller that may merge the frames
/// again before it keeps one. The report's `out` is that file;
/// `options.out` and `options.on_exists` are the reservation's to
/// settle, and not read.
pub fn pano_under(
    files: &[PathBuf],
    reserved: &super::Reserved,
    name: &str,
    edit: &Edit,
    options: &Options,
    models: &mut dyn Models,
    progress: &dyn Progress,
) -> Result<Report> {
    laid(
        &Files(files),
        edit,
        options,
        reserved,
        &reserved.join(name),
        models,
        progress,
    )
}

/// The panorama of `sources` written to `to`, in the folder `reserved`
/// holds, which also takes its spilled frames.
fn laid(
    sources: &dyn Sources,
    edit: &Edit,
    options: &Options,
    reserved: &super::Reserved,
    to: &Path,
    models: &mut dyn Models,
    progress: &dyn Progress,
) -> Result<Report> {
    let started = Instant::now();
    let mut seconds: Vec<(&'static str, f64)> = Vec::new();
    let n = sources.len();
    let given = options.geometry.as_ref();
    // The frame walked first, whose white and CA fit every frame takes:
    // the one asked, or the middle of the list. The reference, whose
    // view and scale the panorama is in, is settled once the frames
    // are placed.
    let anchor = reference_of(n, given.map(|g| g.anchor).or(options.reference))?;
    if let Some(g) = given {
        let placed = g.placed.len();
        if placed != n || g.gains.len() != n {
            bail!("the recipe places {placed} frames and the merge has {n}");
        }
    }
    // One body: the frames' heads, before anything is read whole.
    heads(sources, Some(anchor))?;
    let names: Vec<String> = (0..n).map(|i| sources.name(i)).collect();
    let mut warnings: Vec<String> = Vec::new();
    // Laid already, the lens is the recipe's: the database is not read.
    let lenses = options.lenses.clone().or_else(|| {
        given.is_none().then_some(())?;
        greycard_lens::Store::user()
            .ok()
            .and_then(|s| s.load())
            .map(|(db, _)| Arc::new(db))
    });

    // The reference first: its white is every frame's, and its lens
    // and size are the panorama's.
    let mut kept: Vec<Option<Kept>> = (0..n).map(|_| None).collect();
    let mut order: Vec<usize> = vec![anchor];
    order.extend((0..n).filter(|&k| k != anchor));
    let mut white: Option<[f64; 3]> = None;
    let mut lens: Option<(FrameLens, Vec<LensCorrection>)> = None;
    let mut size = (0usize, 0usize);
    let mut raw_size = (0usize, 0usize);
    // The reference fits the CA once; every other frame takes its fit,
    // or fits its own where the reference made none.
    let mut ca = MergeCa::Make;
    let mut ref_walked: Option<Walked> = None;
    // Each frame's tags, its samples let go: the reference's go into
    // the file.
    let mut frame_tags: Vec<Option<(RawFrame, RawMetadata)>> = (0..n).map(|_| None).collect();
    let mut denoised = false;
    let mut skipped = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let mut pairs: Vec<PairReport> = Vec::new();
    let mut walk_s = 0.0;
    let mut features_s = 0.0;
    let mut fit_s = 0.0;
    let mut focal_mm: Option<f32> = None;
    // Frames walked, most recent last, whose planes are held.
    let mut recent: Vec<usize> = Vec::new();
    for (step, &k) in order.iter().enumerate() {
        progress.step(Step::Frame {
            index: step + 1,
            of: n,
        });
        let t = Instant::now();
        let (raw, metadata) = sources.load(k)?;
        if k != anchor && (raw.width, raw.height) != raw_size {
            bail!(
                "{} is {}x{} and the reference {}x{}: a panorama wants frames of one size",
                names[k],
                raw.width,
                raw.height,
                raw_size.0,
                raw_size.1
            );
        }
        if k != anchor && raw.shot.focal_length != focal_mm {
            bail!(
                "{} was shot at another focal length than the reference: a panorama wants \
                 frames of one focal length",
                names[k]
            );
        }
        if k == anchor {
            raw_size = (raw.width, raw.height);
            focal_mm = raw.shot.focal_length;
            // The reference's lens is the panorama's: the profile is
            // read from it before it is walked.
            let crop = raw.effective_crop();
            let corrections = match given {
                Some(g) => g.lens.clone(),
                None => lens_of(edit, &raw, (crop.width, crop.height), lenses.as_deref()),
            };
            if corrections.is_empty() {
                let said = match given {
                    Some(_) => "the frames were not corrected for their lens when they were \
                                merged, and are not now"
                        .to_string(),
                    None => uncorrected(edit, &raw, lenses.is_some()),
                };
                log::info!("merge: {said}");
                warnings.push(said);
            }
            lens = Some((
                FrameLens::of(&corrections, crop.width, crop.height),
                corrections,
            ));
        }
        // The vignetting runs in the walk, in camera space, before
        // anything reads the frame; the distortion is the warp's.
        let vignetting: Vec<LensCorrection> = lens
            .as_ref()
            .map(|(_, c)| c.iter().filter_map(vignetting_only).collect())
            .unwrap_or_default();
        let (f, frame, meta) = walk_loaded(
            &names[k],
            raw,
            metadata,
            edit,
            MergeInput {
                white,
                reconstruct: true,
                denoiser: options.denoiser,
                ca: ca.clone(),
                lens: vignetting,
            },
            models,
        )?;
        if let Some(why) = &f.stood_in {
            let said = format!("the learned denoiser stood down: {why}");
            if !warnings.contains(&said) {
                warnings.push(said);
            }
        }
        denoised |= f.denoised;
        skipped.push(f.learned_skipped);
        if k == anchor {
            ca = match &f.ca_fit {
                Some(fit) => MergeCa::Take(CaFitRef(fit.clone())),
                None => MergeCa::Own,
            };
            size = (f.width, f.height);
            white = Some(f.gains.map(f64::from));
            let (fl, _) = lens.as_ref().expect("the reference's lens");
            if (fl.width, fl.height) != size {
                bail!(
                    "{}: the walk's size is not its crop's: a panorama wants a plain raw",
                    names[k]
                );
            }
        } else if (f.width, f.height) != size {
            bail!(
                "{} is {}x{} and the reference {}x{}: a panorama wants frames of one size",
                names[k],
                f.width,
                f.height,
                size.0,
                size.1
            );
        }
        walk_s += t.elapsed().as_secs_f64();
        let (frame_lens, _) = lens.as_ref().expect("the reference's lens");

        let t = Instant::now();
        let (w, h) = size;
        // With the frames laid already, nothing is matched.
        let (plane, corners) = match given {
            Some(_) => (Vec::new(), Vec::new()),
            None => planes_of(&f.rgb, w, h, frame_lens),
        };
        let reduced = {
            let (mut rgb, clipped, rw, rh) = geo::reduce_by(&f.rgb, w, h, STEP);
            for (p, c) in rgb.chunks_mut(3).zip(&clipped) {
                if *c {
                    p.iter_mut().for_each(|v| *v = v.max(merge::CLIP));
                }
            }
            (rgb, rw, rh)
        };
        features_s += t.elapsed().as_secs_f64();

        // The pairs this frame makes with those held: its neighbors in
        // the order given, two away, and the last with the first. None
        // with the frames laid already.
        let mut partners: Vec<usize> = recent.clone();
        if k != anchor && !partners.contains(&anchor) {
            partners.push(anchor);
        }
        if k != 0 && !partners.contains(&0) && kept[0].is_some() {
            partners.push(0);
        }
        partners.retain(|&j| {
            given.is_none() && j != k && {
                let d = j.abs_diff(k);
                d <= 2 || (d == n - 1 && n > 2)
            }
        });
        partners.sort_unstable();
        for j in partners {
            let Some(other) = kept[j].as_ref() else {
                continue;
            };
            let Some(other_plane) = other.plane.as_ref() else {
                continue;
            };
            let (a, b) = (j.min(k), j.max(k));
            let tp = Instant::now();
            // Frame a's to frame b's.
            let (ca, cb, pa, pb) = if a == j {
                (&other.corners, &corners, other_plane, &plane)
            } else {
                (&corners, &other.corners, &plane, other_plane)
            };
            let paired = pair(ca, cb, pa, pb, size);
            let took = tp.elapsed().as_secs_f64();
            fit_s += took;
            match paired {
                Some((link, mut report)) => {
                    report.a = a;
                    report.b = b;
                    report.seconds = took;
                    links.push(Link { a, b, ..link });
                    pairs.push(report);
                }
                None if b - a == 1 => bail!(
                    "{} and {} share no matches: a panorama wants frames that overlap",
                    names[a],
                    names[b]
                ),
                None => {}
            }
        }

        let spill = Spill::write(reserved.join(&format!("{k}.frame")), &f.rgb)?;
        let f = Walked {
            rgb: Vec::new(),
            mask: Vec::new(),
            ..f
        };
        kept[k] = Some(Kept {
            plane: Some(plane),
            corners,
            reduced,
            spill,
        });
        // Planes held: the reference's, the first's, and the last two
        // walked.
        recent.push(k);
        if recent.len() > 2 {
            let gone = recent.remove(0);
            if gone != anchor
                && gone != 0
                && let Some(kp) = kept[gone].as_mut()
            {
                kp.plane = None;
            }
        }
        frame_tags[k] = Some((frame, meta));
        if k == anchor {
            ref_walked = Some(f);
        }
    }
    seconds.push(("walk the frames", walk_s));
    seconds.push(("features", features_s));
    seconds.push(("pairs: match and fit", fit_s));
    for kp in kept.iter_mut().flatten() {
        kp.plane = None;
    }
    if given.is_none()
        && let Some(k) = geo::unjoined(&links, n, anchor)
    {
        bail!(
            "{} shares no matches with the others: a panorama wants frames that overlap",
            names[k]
        );
    }
    let (frame_lens, corrections) = lens.expect("the reference's lens");
    // The file says its lens was corrected only when it was: a profile
    // or a correction set by hand ran on the frames.
    let lens_corrected = !corrections.is_empty();
    let (w, h) = size;
    let ref_walked = ref_walked.expect("the anchor walked");
    let (anchor_frame, _) = frame_tags[anchor].as_ref().expect("the anchor walked");

    // Every frame placed at once.
    progress.step(Step::Placing);
    let t = Instant::now();
    let exif_focal = focal_mm.and_then(|mm| {
        let crop = lenses
            .as_deref()
            .and_then(|db| db.camera(&anchor_frame.make, &anchor_frame.model))
            .map(|c| c.crop_factor)?;
        let diagonal = 43.2666 / f64::from(crop);
        let width_mm = diagonal * w as f64 / (w as f64).hypot(h as f64);
        Some(f64::from(mm) * w as f64 / width_mm)
    });
    let flat = match given {
        Some(g) => matches!(g.placed, merge::Placed::Planar { .. }),
        None => exif_focal.is_none(),
    };
    if flat {
        warnings.push(if given.is_some() {
            // A merge made again reads no lens database, and says what
            // its recipe says.
            "the panorama is laid on a plane, as its recipe says".to_string()
        } else if focal_mm.is_none() {
            format!(
                "{} gives no focal length: the panorama is laid on a plane",
                names[anchor]
            )
        } else {
            format!(
                "the size of the {} {}'s sensor is not known here: the panorama is laid on a \
                 plane",
                anchor_frame.make, anchor_frame.model
            )
        });
    }
    // The frames are placed by the model as the tag keeps it, so a
    // merge made from the tag places them as this one does.
    let (placed, residuals, focal, span, reference) = match (given, exif_focal) {
        (Some(g), _) => {
            let reference = reference_of(n, options.reference)?;
            let span = Placement {
                model: g.placed.model(),
                width: w,
                height: h,
            }
            .span(&frame_lens);
            let focal = match &g.placed {
                merge::Placed::Turned { focal, .. } => Some((exif_focal, *focal)),
                merge::Placed::Planar { .. } => None,
            };
            (g.placed.clone(), Vec::new(), focal, span, reference)
        }
        (None, Some(f0)) => {
            let mut adjusted = geo::adjust_turned(&links, n, anchor, f0, true, (w, h));
            if let Laid::Turned { focal, .. } = &adjusted.model
                && !(0.6..=1.6).contains(&(focal / f0))
            {
                warnings.push(format!(
                    "the frames' fitted focal length ({focal:.0} px) was far from the EXIF's \
                     ({f0:.0} px); the EXIF's is used"
                ));
                adjusted = geo::adjust_turned(&links, n, anchor, f0, false, (w, h));
            }
            // The reference is the middle of the turn unless one was
            // asked, and the frames are turned onto it.
            let reference = options
                .reference
                .unwrap_or_else(|| adjusted.model.middle(w, h));
            let turned = adjusted.model.onto(reference);
            let placement = Placement {
                model: turned.clone(),
                width: w,
                height: h,
            };
            let span = placement.span(&frame_lens).expect("a turned model");
            let projection = geo::projection_for(span.0, span.1).map_err(anyhow::Error::msg)?;
            let Laid::Turned {
                focal, rotations, ..
            } = turned
            else {
                unreachable!("adjusted turned")
            };
            let placed = merge::Placed::of(&geo::centered(
                Laid::Turned {
                    focal,
                    rotations,
                    projection,
                },
                &frame_lens,
            ));
            // Leveled, the field is the panorama's own: a camera
            // tipped as it turned read taller than it is.
            let span = Placement {
                model: placed.model(),
                width: w,
                height: h,
            }
            .span(&frame_lens)
            .unwrap_or(span);
            if projection != geo::Projection::Planar {
                geo::projection_for(span.0, span.1).map_err(anyhow::Error::msg)?;
            }
            (
                placed,
                adjusted.residuals,
                Some((Some(f0), focal)),
                Some(span),
                reference,
            )
        }
        (None, None) => {
            let adjusted = geo::adjust_planar(&links, n, anchor, w, h);
            let reference = options
                .reference
                .unwrap_or_else(|| adjusted.model.middle(w, h));
            (
                merge::Placed::of(&adjusted.model.onto(reference)),
                adjusted.residuals,
                None,
                None,
                reference,
            )
        }
    };
    if reference != anchor {
        log::info!(
            "merge: {} is the middle of the turn and the panorama's reference; {} was walked \
             first",
            names[reference],
            names[anchor]
        );
    }
    let model = placed.model();
    for (p, r) in pairs.iter_mut().zip(&residuals) {
        p.placed_rms = *r;
    }
    let projection = match &model {
        Laid::Turned {
            projection: geo::Projection::Planar,
            ..
        } => "planar".to_string(),
        Laid::Turned {
            projection: geo::Projection::Cylinder(geo::Axis::Down),
            ..
        } => "a cylinder, its axis down the frames".to_string(),
        Laid::Turned {
            projection: geo::Projection::Cylinder(geo::Axis::Across),
            ..
        } => "a cylinder, its axis across the frames".to_string(),
        Laid::Planar { .. } => "planar, by homographies".to_string(),
    };
    let placement = Placement {
        model,
        width: w,
        height: h,
    };
    let (canvas, boxes) = match given {
        Some(g) => (
            g.canvas,
            geo::boxes_on(&placement, &frame_lens, n, &g.canvas)
                .context("the frames could not be laid on the recipe's canvas")?,
        ),
        None => geo::canvas(&placement, &frame_lens, n)
            .context("the frames could not be laid on one canvas")?,
    };
    if canvas
        .width
        .checked_mul(canvas.height)
        .is_none_or(|area| area > 16 * CANVAS_CAP)
    {
        bail!(
            "the frames would lie on a canvas of {}x{}: past what a panorama holds",
            canvas.width,
            canvas.height
        );
    }
    seconds.push(("place", t.elapsed().as_secs_f64()));

    // The reduced canvas: the exposure, the seams, the crop.
    let t = Instant::now();
    let (lw, lh) = (canvas.width.div_ceil(STEP), canvas.height.div_ceil(STEP));
    let mut tiles: Vec<geo::Warped> = (0..n)
        .map(|k| {
            let kp = kept[k].as_ref().expect("walked");
            let b = boxes[k];
            let rect = Rect {
                x: b.x / STEP,
                y: b.y / STEP,
                width: (b.x + b.width).div_ceil(STEP).min(lw) - b.x / STEP,
                height: (b.y + b.height).div_ceil(STEP).min(lh) - b.y / STEP,
            };
            geo::warp(
                &Source {
                    rgb: &kp.reduced.0,
                    width: kp.reduced.1,
                    height: kp.reduced.2,
                    scale: STEP as f64,
                },
                &placement,
                &frame_lens,
                &canvas,
                k,
                rect,
                geo::Resampling {
                    step: STEP as f64,
                    kernel: Kernel::Bilinear,
                    gain: [1.0; 3],
                    margin: 1.5 * STEP as f64,
                },
            )
        })
        .collect();
    for kp in kept.iter_mut().flatten() {
        kp.reduced = (Vec::new(), 0, 0);
    }
    let mut ratios = Vec::new();
    for a in 0..n {
        if given.is_some() {
            break;
        }
        for b in a + 1..n {
            let Some(both) = tiles[a].rect.intersect(&tiles[b].rect) else {
                continue;
            };
            let (pa, pb) = (part(&tiles[a], both), part(&tiles[b], both));
            if let Some((ratio, count)) = geo::overlap_ratio(&pa, &pb, 200) {
                ratios.push((a, b, ratio, count));
            }
        }
    }
    let gains = match given {
        Some(g) => g.gains.clone(),
        None => geo::gains(&ratios, n, reference),
    };
    for (tile, g) in tiles.iter_mut().zip(&gains) {
        for p in tile.rgb.chunks_mut(3) {
            for c in 0..3 {
                p[c] *= g[c] as f32;
            }
        }
    }
    let mut composite = Composite::new(lw, lh);
    let seams: Vec<Cut> = tiles
        .iter()
        .enumerate()
        .map(|(k, tile)| composite.add(k as u16, tile))
        .collect();
    drop(tiles);
    let (inner, crop) = match given {
        Some(g) => {
            let crop = g.crop;
            if crop.width == 0
                || crop.height == 0
                || crop.x + crop.width > canvas.width
                || crop.y + crop.height > canvas.height
            {
                bail!("the recipe's crop is not on its canvas");
            }
            // The reduced rectangle the crop was cut from, near enough
            // for the seams' points.
            let (x, y) = ((crop.x / STEP).max(1) - 1, (crop.y / STEP).max(1) - 1);
            let inner = Rect {
                x,
                y,
                width: ((crop.x + crop.width).div_ceil(STEP) + 1).min(lw) - x,
                height: ((crop.y + crop.height).div_ceil(STEP) + 1).min(lh) - y,
            };
            (inner, crop)
        }
        None => {
            let covered: Vec<bool> = composite.owner.iter().map(Option::is_some).collect();
            let inner = geo::largest_rectangle(&covered, lw, lh);
            if inner.width < 3 || inner.height < 3 {
                bail!(
                    "the frames cover no rectangle together: a panorama wants frames that overlap"
                );
            }
            // A reduced pixel in from each side, so the full canvas's
            // crop is covered throughout.
            let crop = Rect {
                x: (inner.x + 1) * STEP,
                y: (inner.y + 1) * STEP,
                width: ((inner.width - 2) * STEP).min(canvas.width - (inner.x + 1) * STEP),
                height: ((inner.height - 2) * STEP).min(canvas.height - (inner.y + 1) * STEP),
            };
            (inner, crop)
        }
    };
    if crop.area() > CANVAS_CAP {
        bail!(
            "the panorama would be {}x{}, {:.0} MP: a panorama is held to {} MP for now",
            crop.width,
            crop.height,
            crop.area() as f64 / 1e6,
            CANVAS_CAP / 1_000_000
        );
    }
    let seam_points = (0..n)
        .map(|k| seam_point(&composite, k as u16, inner, crop))
        .collect();
    seconds.push(("exposure, seams, and crop", t.elapsed().as_secs_f64()));

    // The second pass: each frame onto the bands.
    let t = Instant::now();
    let levels = blend::levels_for(crop.width, crop.height);
    let a = 1 << (levels - 1);
    let region = Rect {
        x: crop.x / a * a,
        y: crop.y / a * a,
        width: crop.x + crop.width - crop.x / a * a,
        height: crop.y + crop.height - crop.y / a * a,
    };
    let mut bands = Bands::new(region.width, region.height, levels);
    let bands_bytes = bands.bytes();
    let mut noise = PairNoise::default();
    let mut previous: Option<(Rect, Vec<f32>)> = None;
    for k in 0..n {
        progress.step(Step::Blending {
            index: k + 1,
            of: n,
        });
        let b = boxes[k];
        let pad = 2 * a;
        let wanted = Rect {
            x: b.x.saturating_sub(pad),
            y: b.y.saturating_sub(pad),
            width: b.width + 2 * pad,
            height: b.height + 2 * pad,
        };
        let Some(inside) = wanted.intersect(&region) else {
            continue;
        };
        let x = region.x + (inside.x - region.x) / a * a;
        let y = region.y + (inside.y - region.y) / a * a;
        let rect = Rect {
            x,
            y,
            width: inside.x + inside.width - x,
            height: inside.y + inside.height - y,
        };
        let rgb = kept[k].as_ref().expect("walked").spill.read(w * h * 3)?;
        let g = gains[k].map(|v| v as f32);
        let warped = geo::warp(
            &Source {
                rgb: &rgb,
                width: w,
                height: h,
                scale: 1.0,
            },
            &placement,
            &frame_lens,
            &canvas,
            k,
            rect,
            geo::Resampling {
                step: 1.0,
                kernel: options.kernel,
                gain: g,
                margin: 0.0,
            },
        );
        drop(rgb);
        let mut tile = warped.rgb;
        let known: Vec<bool> = tile
            .chunks(3)
            .map(|p| p.iter().all(|v| v.is_finite()))
            .collect();
        // The noise from this frame and the last where they overlap,
        // both resampled as the file is.
        if let Some((prev_rect, prev)) = &previous
            && let Some(both) = prev_rect.intersect(&rect)
        {
            let (pa, pb) = (
                part_of_rgb(prev, *prev_rect, both),
                part_of_rgb(&tile, rect, both),
            );
            let (ma, mb) = (
                merge::clip_mask(&pa, both.width, both.height),
                merge::clip_mask(&pb, both.width, both.height),
            );
            noise.add(&pa, &ma, &pb, &mb, 1.0, 1.0);
        }
        let mask = upsampled_mask(&composite, k as u16, rect, &known);
        let kept_tile = tile.clone();
        blend::fill(&mut tile, &known, rect.width, rect.height);
        bands.add(
            rect.x - region.x,
            rect.y - region.y,
            rect.width,
            rect.height,
            tile,
            mask,
        );
        previous = Some((rect, kept_tile));
    }
    drop(previous);
    let blended = bands.finish();
    let image = {
        let (ox, oy) = (crop.x - region.x, crop.y - region.y);
        let mut data = Vec::with_capacity(crop.area() * 3);
        for j in 0..crop.height {
            let at = ((oy + j) * region.width + ox) * 3;
            data.extend_from_slice(&blended[at..at + crop.width * 3]);
        }
        drop(blended);
        CameraImage::from_data(crop.width, crop.height, data)?
    };
    seconds.push(("resample and blend", t.elapsed().as_secs_f64()));
    drop(kept);

    progress.step(Step::Writing);
    let t = Instant::now();
    let headroom = merge::headroom(&image, ref_walked.gains);
    let sensor = noise.fit().unwrap_or(ref_walked.noise);
    if noise.fit().is_none() {
        warnings.push(
            "too little the frames share held well to measure their noise; the reference's \
             own estimate is written"
                .into(),
        );
    }
    let profile = merge::noise_profile(&[(1.0, sensor)], headroom);
    let (heads, wholes) = super::hashes(sources);
    let tag = merge::Tag {
        kind: "pano".into(),
        frames: names.clone(),
        reference,
        denoised,
        headroom_stops: headroom.log2(),
        lens_corrected,
        heads,
        wholes,
        unread: None,
        recipe: Some(Recipe {
            ca: super::CaFits::Once.name().into(),
            // The tier that ran: one asked that stood down did not.
            denoise: options
                .denoiser
                .filter(|_| denoised)
                .and_then(greycard_ai::tier_of)
                .map(Into::into),
            noise: None,
            deghost: None,
            kernel: None,
            geometry: Geometry::Pano(Box::new(merge::Laid {
                anchor,
                kernel: options.kernel,
                placed,
                lens: corrections.clone(),
                gains: gains.clone(),
                canvas,
                crop,
            })),
        }),
    };
    let preview = merge::preview(&image, &ref_walked.white_balance, merge::PREVIEW_SIDE)?;
    let software = concat!("greycard ", env!("CARGO_PKG_VERSION"));
    let (ref_frame, ref_meta) = frame_tags[reference].take().expect("every frame walked");
    let metadata = without_lens(&ref_meta);
    let dng = LinearDngOptions {
        metadata: Some(&metadata),
        preview: Some(&preview),
        software: Some(software),
        headroom: Some(headroom),
        ..Default::default()
    };
    let written = (|| -> Result<()> {
        let file = std::fs::File::create(to)?;
        let mut writer = std::io::BufWriter::new(file);
        write_merged_dng(
            &mut writer,
            &ref_frame,
            &image,
            ref_walked.gains,
            &dng,
            &Merged {
                noise_profile: profile,
                tag: &tag,
            },
        )?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        Ok(())
    })();
    if let Err(e) = written {
        return Err(e.context(format!("writing {}", reserved.out().display())));
    }
    seconds.push(("write", t.elapsed().as_secs_f64()));
    seconds.push(("total", started.elapsed().as_secs_f64()));
    Ok(Report {
        out: to.to_path_buf(),
        reference,
        frames: names,
        pairs,
        focal,
        projection,
        span,
        ratios,
        gains,
        canvas: (canvas.width, canvas.height),
        crop,
        levels,
        kernel: options.kernel,
        seams,
        seam_points,
        headroom_stops: headroom.log2(),
        noise_profile: profile,
        denoised,
        bands_bytes,
        seconds,
        warnings,
        learned_skipped: super::LearnedSkipped::of_frames(skipped),
    })
}

/// The lens's corrections for frame `frame` (`size`, in the sensor's
/// geometry) under `edit`'s Lens section, as the develop picks them:
/// the database's profile for what the file names or the lens picked,
/// over what the file corrects itself.
fn lens_of(
    edit: &Edit,
    frame: &RawFrame,
    size: (usize, usize),
    db: Option<&greycard_lens::Database>,
) -> Vec<LensCorrection> {
    let facts = Facts::raw(frame, 0);
    let lens = edit
        .lens
        .over_file(facts.file_corrects())
        .over_measured_ca(facts.measures_ca());
    let profile = crate::lens::profile_correction(
        size,
        Orientation::Normal,
        (&frame.make, &frame.model, &frame.shot, true),
        &lens,
        db,
    );
    lens.corrections(profile)
}

/// Why the frames were not corrected for their lens, for the run's
/// notes; `has_database`, whether there was a lens database to look
/// in.
fn uncorrected(edit: &Edit, frame: &RawFrame, has_database: bool) -> String {
    let lens = &edit.lens;
    let adobe = matches!(lens.chosen, greycard_edit::LensChoice::Lcp(_));
    if !lens.enabled {
        "the reference's Lens section is off: the frames were not corrected".into()
    } else if !lens.wants_profile() {
        "the reference's edit wants no lens profile: the frames were not corrected".into()
    } else if !has_database && !adobe {
        "no lens database: the frames were not corrected".into()
    } else {
        match &frame.shot.lens_model {
            Some(name) => format!("no lens profile for {name}: the frames were not corrected"),
            None => "no lens profile for a lens the frames do not name: the frames were not \
                     corrected"
                .into(),
        }
    }
}

/// A correction's vignetting alone; none for one without.
fn vignetting_only(c: &LensCorrection) -> Option<LensCorrection> {
    c.vignetting.map(|v| LensCorrection {
        distortion: None,
        chromatic_aberration: None,
        vignetting: Some(v),
        ..*c
    })
}

/// The camera luminance of `rgb` (`w` by `h`), corrected for `lens`'s
/// distortion, at `1 / scale` of the size: each sample the luminance
/// where its green light fell, bilinearly; not a number off the
/// sensor.
fn corrected_plane(rgb: &[f32], w: usize, h: usize, lens: &FrameLens, scale: f64) -> Vec<f32> {
    let (pw, ph) = ((w as f64 / scale) as usize, (h as f64 / scale) as usize);
    let mut out = vec![f32::NAN; pw * ph];
    out.par_chunks_mut(pw).enumerate().for_each(|(j, row)| {
        for (i, o) in row.iter_mut().enumerate() {
            let (x, y) = ((i as f64 + 0.5) * scale, (j as f64 + 0.5) * scale);
            let (sx, sy) = lens.source(1, x, y);
            let (fx, fy) = (sx - 0.5, sy - 0.5);
            if fx < 0.0 || fy < 0.0 || fx > (w - 1) as f64 || fy > (h - 1) as f64 {
                continue;
            }
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
            let l = |x: usize, y: usize| merge::luma(&rgb[(y * w + x) * 3..(y * w + x) * 3 + 3]);
            let top = l(x0, y0) * (1.0 - tx) + l(x1, y0) * tx;
            let bottom = l(x0, y1) * (1.0 - tx) + l(x1, y1) * tx;
            *o = top * (1.0 - ty) + bottom * ty;
        }
    });
    out
}

/// What a frame's pairs are found on: its luminance corrected for the
/// lens at half the size, and the corners of its log at a quarter.
fn planes_of(rgb: &[f32], w: usize, h: usize, lens: &FrameLens) -> (Vec<f32>, Vec<Keypoint>) {
    let plane = corrected_plane(rgb, w, h, lens, FIT);
    let (qw, qh) = (
        (w as f64 / FEATURES) as usize,
        (h as f64 / FEATURES) as usize,
    );
    let quarter = log_halved(&plane, (w as f64 / FIT) as usize, (h as f64 / FIT) as usize);
    let corners = features::keypoints(&quarter, qw, qh, &Corners::default());
    (plane, corners)
}

/// `plane` (`w` by `h`) halved by two-by-two means, and its log: not a
/// number where any of the four was not.
fn log_halved(plane: &[f32], w: usize, h: usize) -> Vec<f32> {
    let (hw, hh) = (w / 2, h / 2);
    (0..hw * hh)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % hw * 2, i / hw * 2);
            let s = plane[y * w + x]
                + plane[y * w + x + 1]
                + plane[(y + 1) * w + x]
                + plane[(y + 1) * w + x + 1];
            if s.is_finite() {
                (0.25 * s).max(1e-6).log2()
            } else {
                f32::NAN
            }
        })
        .collect()
}

/// Two frames' corners matched, their homography found by RANSAC and
/// refined by the homography fit on their half planes: the link, and
/// what it came to. None when too few corners agree.
fn pair(
    ka: &[Keypoint],
    kb: &[Keypoint],
    plane_a: &[f32],
    plane_b: &[f32],
    (w, h): (usize, usize),
) -> Option<(Link, PairReport)> {
    let m = features::matches(ka, kb);
    let at = |k: &Keypoint| (f64::from(k.x) * FEATURES, f64::from(k.y) * FEATURES);
    let src: Vec<(f64, f64)> = m.iter().map(|(i, _)| at(&ka[*i])).collect();
    let dst: Vec<(f64, f64)> = m.iter().map(|(_, j)| at(&kb[*j])).collect();
    let found = features::ransac(&src, &dst, 4000, 1.5 * FEATURES)?;
    let corners = [
        (0.0, 0.0),
        (w as f64, 0.0),
        (0.0, h as f64),
        (w as f64, h as f64),
    ];
    if found.inliers.len() < MIN_INLIERS
        || found.homography.determinant() <= 0.0
        || corners
            .iter()
            .any(|&(x, y)| found.homography.depth(x, y) <= 0.0)
    {
        return None;
    }
    let mut homography = found.homography;
    // The fit, on the half planes, in their pixel indices.
    let (pw, ph) = ((w as f64 / FIT) as usize, (h as f64 / FIT) as usize);
    let start = homography.at_scale(1.0 / FIT).shifted(-0.5);
    let options = register::Options {
        model: Model::Homography,
        max_levels: 4,
        min_overlap: 0.05,
        ..register::Options::default()
    };
    let fit = register::fit_homography_from(plane_a, plane_b, pw, ph, &options, &start)
        .map_err(|e| log::info!("merge: a pair's homography fit: {e}"))
        .ok()
        .and_then(|fit| {
            let refined = fit.transform.shifted(0.5).at_scale(FIT);
            let moved = geo::grid_points(&homography, w, h, 8)
                .iter()
                .map(|&(p, q)| {
                    let r = refined.apply(p.0, p.1);
                    (r.0 - q.0).hypot(r.1 - q.1)
                })
                .fold(0.0, f64::max);
            let taken = moved <= FIT_LEASH && fit.residual <= PANO_RESIDUAL;
            if !taken {
                log::info!(
                    "merge: a pair's homography fit went {moved:.1} px from its features \
                     (residual {:.4}); not taken",
                    fit.residual
                );
            }
            taken.then_some((refined, fit.residual, moved))
        });
    if let Some((refined, _, _)) = fit {
        homography = refined;
    }
    let points = geo::grid_points(&homography, w, h, 12);
    if points.len() < 4 {
        return None;
    }
    Some((
        Link {
            a: 0,
            b: 0,
            strength: found.inliers.len(),
            points,
            homography,
        },
        PairReport {
            a: 0,
            b: 0,
            corners: (ka.len(), kb.len()),
            matches: m.len(),
            inliers: found.inliers.len(),
            feature_rms: found.rms,
            fit: fit.map(|(_, r, moved)| (r, moved)),
            placed_rms: 0.0,
            seconds: 0.0,
        },
    ))
}

/// The points of frame `k`'s seam with the frames before it, inside
/// the reduced canvas's rectangle `inner`, its middle by height and
/// one five sixths of the way down, in the pixels of the picture
/// cropped to `crop`.
fn seam_point(composite: &Composite, k: u16, inner: Rect, crop: Rect) -> Vec<(usize, usize)> {
    let lw = composite.width;
    let mut points: Vec<(usize, usize)> = Vec::new();
    for y in inner.y + 1..inner.y + inner.height - 1 {
        for x in inner.x + 1..inner.x + inner.width - 1 {
            if composite.owner[y * lw + x] != Some(k) {
                continue;
            }
            let before = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                .iter()
                .any(|&(a, b)| composite.owner[b * lw + a].is_some_and(|o| o < k));
            if before {
                points.push((x, y));
            }
        }
    }
    points.sort_by_key(|p| p.1);
    [points.len() / 2, points.len() * 5 / 6]
        .iter()
        .filter_map(|&i| points.get(i))
        .map(|&(x, y)| (x * STEP + STEP / 2, y * STEP + STEP / 2))
        .filter(|&(fx, fy)| {
            fx >= crop.x && fy >= crop.y && fx < crop.x + crop.width && fy < crop.y + crop.height
        })
        .map(|(fx, fy)| (fx - crop.x, fy - crop.y))
        .collect()
}

/// The part of a tile's RGB over `rect`, inside it.
fn part(tile: &geo::Warped, rect: Rect) -> Vec<f32> {
    part_of_rgb(&tile.rgb, tile.rect, rect)
}

/// The part of interleaved `rgb` laid over `within` that `rect` covers.
fn part_of_rgb(rgb: &[f32], within: Rect, rect: Rect) -> Vec<f32> {
    let mut out = Vec::with_capacity(rect.area() * 3);
    for j in 0..rect.height {
        let at = ((rect.y - within.y + j) * within.width + rect.x - within.x) * 3;
        out.extend_from_slice(&rgb[at..at + rect.width * 3]);
    }
    out
}

/// Frame `k`'s weight over `rect` of the canvas: its share of the
/// reduced canvas's owners, read bilinearly between the reduced pixels'
/// centers, where the frame has samples.
fn upsampled_mask(composite: &Composite, k: u16, rect: Rect, known: &[bool]) -> Vec<f32> {
    let (lw, lh) = (composite.width, composite.height);
    let mine = |x: isize, y: isize| -> f32 {
        let x = x.clamp(0, lw as isize - 1) as usize;
        let y = y.clamp(0, lh as isize - 1) as usize;
        f32::from(u8::from(composite.owner[y * lw + x] == Some(k)))
    };
    let mut out = vec![0.0f32; rect.area()];
    out.par_chunks_mut(rect.width)
        .enumerate()
        .for_each(|(j, row)| {
            let v = (rect.y + j) as f64 + 0.5;
            let fy = v / STEP as f64 - 0.5;
            let y0 = fy.floor();
            let ty = (fy - y0) as f32;
            for (i, o) in row.iter_mut().enumerate() {
                if !known[j * rect.width + i] {
                    continue;
                }
                let u = (rect.x + i) as f64 + 0.5;
                let fx = u / STEP as f64 - 0.5;
                let x0 = fx.floor();
                let tx = (fx - x0) as f32;
                let (x0, y0) = (x0 as isize, y0 as isize);
                let top = mine(x0, y0) * (1.0 - tx) + mine(x0 + 1, y0) * tx;
                let bottom = mine(x0, y0 + 1) * (1.0 - tx) + mine(x0 + 1, y0 + 1) * tx;
                *o = top * (1.0 - ty) + bottom * ty;
            }
        });
    out
}

/// The reference's metadata with nothing of its lens: no lens named, no
/// focal length, no aperture, no focus distance. A panorama's lens was
/// corrected in the merge, or there was no profile to correct it by;
/// either way its canvas is a projection no one frame's profile fits,
/// and the develop's fallback finds a profile from the body and the
/// focal length alone.
fn without_lens(meta: &RawMetadata) -> RawMetadata {
    let mut m = meta.clone();
    m.lens = None;
    let e = &mut m.exif;
    e.lens_spec = None;
    e.lens_make = None;
    e.lens_model = None;
    e.lens_serial_number = None;
    e.focal_length = None;
    e.fnumber = None;
    e.aperture_value = None;
    e.max_aperture_value = None;
    e.subject_distance = None;
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::Step;
    use greycard_core::merge::Noise;
    use greycard_core::merge::testing::{mosaic_of, turned, write_dng_shot};
    use greycard_core::register::Homography;

    const W: usize = 960;
    const H: usize = 640;
    /// 30 mm on a full-frame sensor 960 pixels wide.
    const FOCAL: f64 = 800.0;

    /// A lens database that knows the synthetic body's sensor (full
    /// frame) and its fixed lens, a profile that corrects next to
    /// nothing (a hundredth of a pixel at the corners).
    fn lenses() -> Arc<greycard_lens::Database> {
        let mut db = greycard_lens::Database::default();
        db.read_xml(
            "<lensdatabase version=\"2\"><camera><maker>Synthetic</maker><model>Pano</model>\
             <mount>None</mount><cropfactor>1</cropfactor></camera>\
             <lens><maker>Synthetic</maker><model>Synthetic 30mm</model><mount>None</mount>\
             <cropfactor>1</cropfactor><calibration>\
             <distortion model=\"poly3\" focal=\"30\" k1=\"0.0001\"/>\
             </calibration></lens></lensdatabase>",
        )
        .unwrap();
        Arc::new(db)
    }

    /// The same body with no lens to correct by.
    fn body_only() -> Arc<greycard_lens::Database> {
        let mut db = greycard_lens::Database::default();
        db.read_xml(
            "<lensdatabase version=\"2\"><camera><maker>Synthetic</maker><model>Pano</model>\
             <mount>None</mount><cropfactor>1</cropfactor></camera></lensdatabase>",
        )
        .unwrap();
        Arc::new(db)
    }

    /// Frames of the scene all round the camera, turned by `yaws`, in a
    /// scratch folder for test `what`.
    fn frames(what: &str, yaws: &[f64]) -> (PathBuf, Vec<PathBuf>) {
        let dir = std::env::temp_dir().join(format!("greycard-pano-{what}-{}", std::process::id()));
        greycard_core::testing::remove_dir_retry(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let files = yaws
            .iter()
            .enumerate()
            .map(|(k, &yaw)| {
                let path = dir.join(format!("IMG_{k}.dng"));
                let samples = mosaic_of(W, H, 1.0, Noise::ASSUMED, 40 + k as u64, |x, y, c| {
                    turned(x, y, c, W, H, FOCAL, yaw)
                });
                write_dng_shot(&path, samples, (W, H), (1, 100), 100, "Pano", Some(30));
                path
            })
            .collect();
        (dir, files)
    }

    fn options() -> Options {
        Options {
            lenses: Some(lenses()),
            ..Options::default()
        }
    }

    /// A panorama whose frames asked for a learned tier on a card the
    /// denoiser lost says so (the card's reason, as a value); with no
    /// tier, or the tier run, it says nothing.
    #[test]
    fn a_panorama_says_its_frames_went_without_the_tier() {
        let (dir, files) = frames("off-card", &[-0.45, 0.0, 0.45]);
        let run = |denoiser, models: &mut dyn crate::walk::Models| {
            pano(
                &files,
                &Edit::default(),
                &Options {
                    denoiser,
                    on_exists: greycard_core::output::OnExists::Overwrite,
                    ..options()
                },
                models,
                &|_: Step| {},
            )
            .unwrap()
        };
        crate::ai::set_test_off_card(Some(true));
        let off = run(
            Some(&greycard_ai::DENOISE_BEST),
            &mut crate::testing::PlainDenoiser,
        );
        assert!(!off.denoised);
        assert_eq!(
            off.learned_skipped,
            Some(super::super::LearnedSkipped::OffCard)
        );
        crate::ai::set_test_off_card(Some(false));
        let ran = run(
            Some(&greycard_ai::DENOISE_BEST),
            &mut crate::testing::PlainDenoiser,
        );
        assert!(ran.denoised);
        assert_eq!(ran.learned_skipped, None);
        let none = run(None, &mut crate::walk::NoModels);
        assert_eq!(none.learned_skipped, None);
        crate::ai::set_test_off_card(None);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Three frames turned 26 degrees apart, a field of 114 degrees:
    /// laid on a cylinder, the focal length fitted to the turn's,
    /// every pair placed to a fraction of a pixel, the exposures one,
    /// and written beside the first frame as a float linear DNG of the
    /// body with nothing of its lens, saying it is a panorama with its
    /// lens corrected.
    #[test]
    fn a_turned_set_is_merged_into_a_panorama() {
        let (dir, files) = frames("turned", &[-0.45, 0.0, 0.45]);
        let steps = std::cell::RefCell::new(Vec::new());
        let report = pano(
            &files,
            &Edit::default(),
            &options(),
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
                Step::Placing,
                Step::Blending { index: 1, of: 3 },
                Step::Blending { index: 2, of: 3 },
                Step::Blending { index: 3, of: 3 },
                Step::Writing,
            ]
        );
        assert_eq!(report.out, dir.join("IMG_0-pano.dng"));
        assert_eq!(report.reference, 1);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(
            report.projection.contains("cylinder"),
            "{}",
            report.projection
        );
        let (exif, fitted) = report.focal.unwrap();
        let exif = exif.unwrap();
        assert!((exif - FOCAL).abs() < 0.1, "{exif}");
        assert!((fitted - FOCAL).abs() < 0.01 * FOCAL, "{fitted}");
        let (across, _) = report.span.unwrap();
        assert!((across - 113.5).abs() < 2.0, "{across}");
        // The neighbors; the first and the last share too little.
        assert_eq!(report.pairs.len(), 2, "{:?}", report.pairs);
        for p in &report.pairs {
            assert!(p.placed_rms < 0.3, "{p:?}");
        }
        for g in &report.gains {
            assert!(g.iter().all(|v| (v - 1.0).abs() < 0.01), "{g:?}");
        }
        // The field's width on the cylinder, less the crop's bite at
        // its ends; as tall as the frames' bowed edges leave.
        let around = FOCAL * across.to_radians();
        assert!(
            report.crop.width as f64 > 0.9 * around && report.crop.height > H * 3 / 4,
            "{:?}",
            report.crop
        );

        let back = greycard_core::decode::decode_path(&report.out).unwrap();
        assert_eq!(
            (back.make.as_str(), back.model.as_str()),
            ("Synthetic", "Pano")
        );
        assert_eq!(back.shot.lens_model, None);
        assert_eq!(back.shot.focal_length, None);
        assert_eq!(back.shot.f_number, None);
        assert_eq!(
            (back.width, back.height),
            (report.crop.width, report.crop.height)
        );
        let tag = back.dng.merged.clone().expect("our tag");
        assert_eq!(tag.kind, "pano");
        assert!(tag.lens_corrected);
        // What the develop's list reads of it: the lens stands down.
        assert!(matches!(
            Facts::raw(&back, 0).input,
            crate::stages::InputKind::Raw {
                merged: true,
                lens_corrected: true,
                ..
            }
        ));
        assert_eq!(tag.frames, ["IMG_0.dng", "IMG_1.dng", "IMG_2.dng"]);
        assert!(back.dng.noise_profile.is_some());
        // The spilled frames are gone.
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// A merged file's samples and its tag.
    fn samples_and_tag(file: &Path) -> (Vec<f32>, merge::Tag) {
        let back = greycard_core::decode::decode_path(file).unwrap();
        let greycard_core::raw::Samples::F32(samples) = back.samples else {
            panic!("float samples");
        };
        (samples, back.dng.merged.expect("our tag"))
    }

    /// The panorama keeps how it was laid: made again from its recipe,
    /// with no lens database and nothing matched or fitted, it is the
    /// same picture to the sample, on the same canvas and crop.
    #[test]
    fn a_panorama_made_again_is_placed_to_the_bit() {
        let (dir, files) = frames("again", &[-0.45, 0.0, 0.45]);
        let report = pano(
            &files,
            &Edit::default(),
            &options(),
            &mut crate::walk::NoModels,
            &|_: Step| {},
        )
        .unwrap();
        let (before, tag) = samples_and_tag(&report.out);
        let recipe = tag.recipe.clone().expect("a recipe");
        let Geometry::Pano(laid) = &recipe.geometry else {
            panic!("a panorama's geometry");
        };
        assert_eq!((laid.anchor, tag.reference), (1, 1));
        assert_eq!(laid.kernel, Kernel::Lanczos3);
        assert_eq!(laid.crop, report.crop);
        assert_eq!(laid.gains, report.gains);
        assert_eq!(laid.lens.len(), 1);
        assert!(matches!(laid.placed, merge::Placed::Turned { .. }));

        let out = dir.join("again.dng");
        let steps = std::cell::RefCell::new(Vec::new());
        let super::super::Again::Pano(again) = super::super::again(
            &report.out,
            &super::super::Overrides {
                out: Some(out.clone()),
                ..Default::default()
            },
            &Edit::default(),
            &mut crate::walk::NoModels,
            &|s: Step| steps.borrow_mut().push(s),
        )
        .unwrap() else {
            panic!("a panorama");
        };
        assert_eq!(steps.into_inner().len(), 8);
        assert_eq!(again.out, out);
        assert!(again.pairs.is_empty());
        assert!(again.warnings.is_empty(), "{:?}", again.warnings);
        assert_eq!((again.canvas, again.crop), (report.canvas, report.crop));
        let (after, tag_after) = samples_and_tag(&out);
        assert_eq!(tag_after, tag);
        assert_eq!(before.len(), after.len());
        assert!(
            before
                .iter()
                .zip(&after)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Frames shot right to left with uneven steps: the middle of the
    /// list is the second frame, the middle of the turn the third. The
    /// third is the reference, the frames turned onto it; the second,
    /// walked first, is the recipe's anchor. Asked for, the second is
    /// the reference.
    #[test]
    fn the_reference_is_the_middle_of_the_turn() {
        let (dir, files) = frames("middle", &[0.5, 0.3, 0.0, -0.3]);
        assert_eq!(reference_of(files.len(), None).unwrap(), 1);
        let report = pano(
            &files,
            &Edit::default(),
            &options(),
            &mut crate::walk::NoModels,
            &|_: Step| {},
        )
        .unwrap();
        assert_eq!(report.reference, 2);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(report.gains[2].iter().all(|&g| g == 1.0));
        let (_, tag) = samples_and_tag(&report.out);
        assert_eq!(tag.reference, 2);
        let Some(Recipe {
            geometry: Geometry::Pano(laid),
            ..
        }) = &tag.recipe
        else {
            panic!("a panorama's recipe");
        };
        assert_eq!(laid.anchor, 1);
        let asked = pano(
            &files,
            &Edit::default(),
            &Options {
                reference: Some(1),
                out: Some(dir.join("asked.dng")),
                ..options()
            },
            &mut crate::walk::NoModels,
            &|_: Step| {},
        )
        .unwrap();
        assert_eq!(asked.reference, 1);
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The lens's vignetting runs in the walk, in camera space: a frame
    /// walked with it is brightened at its corners by the profile's
    /// gain there, and left at its middle.
    #[test]
    fn the_vignetting_runs_in_camera_space() {
        let (dir, files) = frames("vignetting", &[0.0]);
        let correction = LensCorrection {
            vignetting: Some(greycard_core::develop::lens::Vignetting {
                k: [-0.5, 0.0, 0.0],
            }),
            ..Default::default()
        };
        let walked = |lens: Vec<LensCorrection>| {
            let (raw, meta) = Files(&files).load(0).unwrap();
            let merge = MergeInput {
                white: None,
                reconstruct: true,
                denoiser: None,
                ca: MergeCa::Own,
                lens,
            };
            walk_loaded(
                "IMG_0.dng",
                raw,
                meta,
                &Edit::default(),
                merge,
                &mut crate::walk::NoModels,
            )
            .unwrap()
            .0
            .rgb
        };
        let (plain, lit) = (walked(Vec::new()), walked(vec![correction]));
        let at = |rgb: &[f32], x: usize, y: usize| rgb[(y * W + x) * 3 + 1];
        let corner = at(&lit, 4, 4) / at(&plain, 4, 4);
        let want = correction.gain_at(4.5, 4.5, W as f32, H as f32);
        assert!(
            (corner - want).abs() < 0.01 * want,
            "{corner} against {want}"
        );
        let middle = at(&lit, W / 2, H / 2) / at(&plain, W / 2, H / 2);
        assert!((middle - 1.0).abs() < 1e-3, "{middle}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Two frames with nothing between them, one frame, and frames of
    /// two bodies are refused, each saying why.
    #[test]
    fn what_is_no_panorama_is_refused() {
        let (dir, files) = frames("refused", &[0.0, 2.0]);
        let run = |files: &[PathBuf]| {
            pano(
                files,
                &Edit::default(),
                &options(),
                &mut crate::walk::NoModels,
                &|_: Step| {},
            )
            .map(|_| ())
            .unwrap_err()
            .to_string()
        };
        assert_eq!(
            run(&files),
            "IMG_0.dng and IMG_1.dng share no matches: a panorama wants frames that overlap"
        );
        assert_eq!(
            run(&files[..1]),
            "a panorama wants two frames or more; got 1"
        );
        let other = dir.join("other.dng");
        write_dng_shot(
            &other,
            mosaic_of(W, H, 1.0, Noise::ASSUMED, 3, |x, y, c| {
                turned(x, y, c, W, H, FOCAL, 0.3)
            }),
            (W, H),
            (1, 100),
            100,
            "Another",
            Some(30),
        );
        assert_eq!(
            run(&[files[0].clone(), other]),
            "a merge wants frames of one body"
        );
        // A frame of another size is refused before it is walked, not
        // by the reference's CA fit.
        let small = dir.join("small.dng");
        write_dng_shot(
            &small,
            mosaic_of(W / 2, H, 1.0, Noise::ASSUMED, 3, |x, y, c| {
                turned(x, y, c, W / 2, H, FOCAL, 0.0)
            }),
            (W / 2, H),
            (1, 100),
            100,
            "Pano",
            Some(30),
        );
        assert_eq!(
            run(&[files[0].clone(), small]),
            format!(
                "small.dng is {}x{H} and the reference {W}x{H}: a panorama wants frames of one size",
                W / 2
            )
        );
        // As is one of another focal length, before its walk too.
        let longer = dir.join("longer.dng");
        write_dng_shot(
            &longer,
            mosaic_of(W, H, 1.0, Noise::ASSUMED, 4, |x, y, c| {
                turned(x, y, c, W, H, FOCAL, 0.3)
            }),
            (W, H),
            (1, 100),
            100,
            "Pano",
            Some(50),
        );
        assert_eq!(
            run(&[files[0].clone(), longer]),
            "longer.dng was shot at another focal length than the reference: a panorama \
             wants frames of one focal length"
        );
        assert!(!dir.join("IMG_0-pano.dng").exists());
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Frames with no profile to correct them by, or with the Lens
    /// section off, are merged uncorrected and say so: the tag does not
    /// say the lens was corrected, so the develop's profile does not
    /// stand down, and the lens is still left out of the file, the
    /// canvas being a projection no one frame's profile fits.
    #[test]
    fn frames_not_corrected_for_their_lens_say_so() {
        let (dir, files) = frames("uncorrected", &[-0.2, 0.2]);
        let mut off = Edit::default();
        off.lens.enabled = false;
        for (edit, lenses, said) in [
            (
                Edit::default(),
                body_only(),
                "no lens profile for Synthetic 35mm: the frames were not corrected",
            ),
            (
                off,
                lenses(),
                "the reference's Lens section is off: the frames were not corrected",
            ),
        ] {
            let report = pano(
                &files,
                &edit,
                &Options {
                    lenses: Some(lenses),
                    on_exists: OnExists::Overwrite,
                    ..Options::default()
                },
                &mut crate::walk::NoModels,
                &|_: Step| {},
            )
            .unwrap();
            assert_eq!(report.warnings, [said]);
            let back = greycard_core::decode::decode_path(&report.out).unwrap();
            let tag = back.dng.merged.clone().expect("our tag");
            assert!(!tag.lens_corrected);
            assert_eq!(back.shot.focal_length, None);
            assert!(matches!(
                Facts::raw(&back, 0).input,
                crate::stages::InputKind::Raw {
                    lens_corrected: false,
                    ..
                }
            ));
        }
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Two merges to one file at once: the second is refused while the
    /// first holds the name, the first's frames are its own, and
    /// nothing of either is left beside the file. A folder a merge
    /// killed midway left is cleared by the next.
    #[test]
    fn two_merges_to_one_file_do_not_share_it() {
        let (dir, files) = frames("race", &[-0.2, 0.2]);
        let out = dir.join("both.dng");
        let (placing, placed) = std::sync::mpsc::channel::<()>();
        let (second_done, done) = std::sync::mpsc::channel::<()>();
        let done = std::sync::Mutex::new(done);
        let options = Options {
            out: Some(out.clone()),
            on_exists: OnExists::Overwrite,
            ..options()
        };
        let (first, second) = std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                // The first waits, its frames spilled, while the second
                // runs.
                let progress = |s: Step| {
                    if s == Step::Placing {
                        placing.send(()).unwrap();
                        done.lock().unwrap().recv().unwrap();
                    }
                };
                pano(
                    &files,
                    &Edit::default(),
                    &options,
                    &mut crate::walk::NoModels,
                    &progress,
                )
            });
            placed.recv().unwrap();
            let reversed = [files[1].clone(), files[0].clone()];
            let second = pano(
                &reversed,
                &Edit::default(),
                &options,
                &mut crate::walk::NoModels,
                &|_: Step| {},
            );
            second_done.send(()).unwrap();
            (first.join().unwrap(), second)
        });
        let first = first.unwrap();
        assert_eq!(
            second.map(|_| ()).unwrap_err().to_string(),
            format!(
                "{} is being written by another merge; if none is running, remove {}",
                out.display(),
                dir.join(".both.dng.merging").display()
            )
        );
        let tag = greycard_core::decode::decode_path(&first.out)
            .unwrap()
            .dng
            .merged
            .expect("our tag");
        assert_eq!(tag.frames, ["IMG_0.dng", "IMG_1.dng"]);
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        // The name free again, a merge to it runs.
        pano(
            &files,
            &Edit::default(),
            &options,
            &mut crate::walk::NoModels,
            &|_: Step| {},
        )
        .unwrap();
        // A merge killed midway left its folder, its frames spilled and
        // its lock free: the next merge clears it and runs.
        let left = dir.join(".both.dng.merging");
        std::fs::create_dir(&left).unwrap();
        std::fs::write(left.join("lock"), b"").unwrap();
        std::fs::write(left.join("0.frame"), vec![0u8; 4096]).unwrap();
        std::fs::write(left.join("merged.part"), b"").unwrap();
        pano(
            &files,
            &Edit::default(),
            &options,
            &mut crate::walk::NoModels,
            &|_: Step| {},
        )
        .unwrap();
        let hidden: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(hidden.is_empty(), "{hidden:?}");
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// The panorama's edit keeps the reference's look and none of what
    /// was placed on one frame or what the merge ran.
    #[test]
    fn a_panoramas_edit_keeps_the_look_alone() {
        let mut reference = Edit::default();
        reference.light.exposure = 0.7;
        reference.light.tone.contrast = 20.0;
        reference.lens.manual = -0.05;
        reference.lens.ca_red = 0.001;
        reference.lens.chosen = greycard_edit::LensChoice::Database("Synthetic 30mm".into());
        reference.geometry.angle = 2.0;
        reference.adjustments.push(Default::default());
        let edit = edit_for_pano(&reference);
        assert_eq!(edit.light.exposure, 0.0);
        assert_eq!(edit.light.tone.contrast, 20.0);
        assert_eq!(edit.lens, greycard_edit::Lens::default());
        assert_eq!(edit.geometry, Default::default());
        assert!(edit.adjustments.is_empty());
        assert!(edit.retouch.is_empty());
    }

    /// A body whose sensor the database does not know gives no focal
    /// length in pixels: the frames are laid on a plane by their
    /// homographies, and it is said.
    #[test]
    fn without_a_sensor_size_the_panorama_is_planar() {
        let (dir, files) = frames("planar", &[-0.2, 0.2]);
        let report = pano(
            &files,
            &Edit::default(),
            &Options {
                lenses: Some(Arc::new(greycard_lens::Database::default())),
                ..Options::default()
            },
            &mut crate::walk::NoModels,
            &|_: Step| {},
        )
        .unwrap();
        assert_eq!(report.projection, "planar, by homographies");
        assert!(report.focal.is_none());
        assert_eq!(
            report.warnings,
            [
                "no lens profile for Synthetic 35mm: the frames were not corrected",
                "the size of the Synthetic Pano's sensor is not known here: the panorama is \
                 laid on a plane"
            ]
        );
        assert!(report.pairs[0].placed_rms < 0.5, "{:?}", report.pairs);
        // Made again, it says what its recipe says, having read no lens
        // database.
        let (before, _) = samples_and_tag(&report.out);
        let super::super::Again::Pano(again) = super::super::again(
            &report.out,
            &super::super::Overrides::default(),
            &Edit::default(),
            &mut crate::walk::NoModels,
            &|_: Step| {},
        )
        .unwrap() else {
            panic!("a panorama");
        };
        assert_eq!(
            again.warnings,
            [
                "the frames were not corrected for their lens when they were merged, and are \
                 not now",
                "the panorama is laid on a plane, as its recipe says"
            ]
        );
        let (after, _) = samples_and_tag(&report.out);
        assert!(
            before
                .iter()
                .zip(&after)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        greycard_core::testing::remove_dir_retry(&dir);
    }

    /// Two frames walked alike for a panorama: the look apart, only the
    /// demosaic, the camera profile, the lens section, and the learned
    /// tier tell them apart.
    #[test]
    fn edits_walk_alike_but_for_what_the_walk_reads() {
        let a = Edit::default();
        let mut b = a.clone();
        b.light.exposure = 1.0;
        b.noise.strength = 0.3;
        assert!(walks_alike(&a, &b));
        let mut c = a.clone();
        c.noise.learned = greycard_edit::Learned::Balanced;
        assert!(!walks_alike(&a, &c));
        let mut d = a.clone();
        d.lens.enabled = !d.lens.enabled;
        assert!(!walks_alike(&a, &d));
    }

    /// The frames a measurement runs on: `GREYCARD_PANO_FRAMES`, paths
    /// separated by spaces, read only; none skips it.
    fn real_frames() -> Option<Vec<PathBuf>> {
        let v = std::env::var("GREYCARD_PANO_FRAMES").ok()?;
        Some(v.split_whitespace().map(PathBuf::from).collect())
    }

    /// Where a measurement writes: `GREYCARD_PANO_OUT`.
    fn out_dir() -> PathBuf {
        let d = PathBuf::from(std::env::var("GREYCARD_PANO_OUT").expect("GREYCARD_PANO_OUT"));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The written panorama's luminance and size.
    fn luminance_of(path: &Path) -> (Vec<f32>, usize, usize) {
        let back = greycard_core::decode::decode_path(path).unwrap();
        let greycard_core::raw::Samples::F32(s) = &back.samples else {
            panic!("float samples");
        };
        let (w, h) = (back.width, back.height);
        (s.chunks(3).map(merge::luma).collect(), w, h)
    }

    /// The mean magnitude of the Laplacian over the mean level, over
    /// columns `x0..x1`: how much fine detail a picture holds there.
    fn detail(l: &[f32], w: usize, h: usize, x0: usize, x1: usize) -> f64 {
        let (mut lap, mut level) = (0.0f64, 0.0f64);
        for y in 1..h - 1 {
            for x in x0.max(1)..x1.min(w - 1) {
                let i = y * w + x;
                let v = 4.0 * l[i] - l[i - 1] - l[i + 1] - l[i - w] - l[i + w];
                lap += f64::from(v.abs());
                level += f64::from(l[i]);
            }
        }
        lap / level.max(1e-12)
    }

    /// The kernels against each other on real frames: each written,
    /// and the fine detail of the result over the overlaps (four
    /// hundred columns about each seam) and over the whole, against
    /// Lanczos's; with the time each took.
    #[test]
    #[ignore = "reads real frames from GREYCARD_PANO_FRAMES"]
    fn measure_the_kernels() {
        let Some(files) = real_frames() else {
            return;
        };
        let dir = out_dir();
        let mut rows = Vec::new();
        for kernel in [Kernel::Lanczos3, Kernel::Cubic, Kernel::Bilinear] {
            let out = dir.join(format!("kernel-{kernel:?}.dng"));
            let report = pano(
                &files,
                &Edit::default(),
                &Options {
                    out: Some(out.clone()),
                    on_exists: OnExists::Overwrite,
                    kernel,
                    ..Options::default()
                },
                &mut crate::walk::NoModels,
                &|_: Step| {},
            )
            .unwrap();
            let blend = report
                .seconds
                .iter()
                .find(|(n, _)| *n == "resample and blend")
                .unwrap()
                .1;
            let (l, w, h) = luminance_of(&out);
            let seams: Vec<usize> = report
                .seam_points
                .iter()
                .filter_map(|p| p.first())
                .map(|p| p.0)
                .collect();
            let overlap: f64 = seams
                .iter()
                .map(|&x| detail(&l, w, h, x.saturating_sub(200), x + 200))
                .sum::<f64>()
                / seams.len().max(1) as f64;
            rows.push((kernel, overlap, detail(&l, w, h, 0, w), blend));
        }
        let (_, o0, a0, _) = rows[0];
        for (kernel, o, a, s) in &rows {
            println!(
                "MEASURE {kernel:?}: detail over the overlaps {o:.5} ({:.3} of Lanczos's), \
                 whole {a:.5} ({:.3}), resample and blend {s:.2} s",
                o / o0,
                a / a0
            );
        }
    }

    /// The per-frame learned denoiser's cost on real frames: the
    /// panorama with each tier against without, and the noise the file
    /// then says.
    #[test]
    #[ignore = "reads real frames from GREYCARD_PANO_FRAMES and runs the learned denoiser"]
    fn measure_the_denoiser() {
        let Some(files) = real_frames() else {
            return;
        };
        let dir = out_dir();
        for tier in [None, Some("fast"), Some("balanced")] {
            let denoiser = tier.map(|t| greycard_ai::denoiser(t).unwrap());
            let mut models = crate::merge::DenoiseModels::new();
            let report = pano(
                &files,
                &Edit::default(),
                &Options {
                    out: Some(dir.join(format!("denoise-{}.dng", tier.unwrap_or("off")))),
                    on_exists: OnExists::Overwrite,
                    denoiser,
                    ..Options::default()
                },
                &mut models,
                &|_: Step| {},
            )
            .unwrap();
            let at = |what: &str| report.seconds.iter().find(|(n, _)| *n == what).unwrap().1;
            println!(
                "MEASURE denoiser {}: walk {:.2} s, total {:.2} s, denoised {}, noise S {:.3e} O {:.3e}, \
                 pairs {:?}, warnings {:?}",
                tier.unwrap_or("off"),
                at("walk the frames"),
                at("total"),
                report.denoised,
                report.noise_profile[0],
                report.noise_profile[1],
                report
                    .pairs
                    .iter()
                    .map(|p| (p.inliers, p.placed_rms))
                    .collect::<Vec<_>>(),
                report.warnings
            );
        }
    }

    /// The prototype's two tiles, made again from one raw
    /// (`GREYCARD_PANO_SOURCE`): tile A a crop, tile B the source turned
    /// 1.5 degrees, keystoned, shifted, and at 0.85 of its exposure;
    /// then a vignette on both, or none. The pair found as a panorama
    /// finds its pairs, the vignette taken out first, as the lens is:
    /// the homography to the truth over the overlap, and the overlap's
    /// gain against the truth's.
    #[test]
    #[ignore = "reads a raw from GREYCARD_PANO_SOURCE"]
    fn the_prototypes_two_tiles() {
        let Ok(source) = std::env::var("GREYCARD_PANO_SOURCE") else {
            return;
        };
        let source = [PathBuf::from(source)];
        let merge = MergeInput {
            white: None,
            reconstruct: true,
            denoiser: None,
            ca: MergeCa::Own,
            lens: Vec::new(),
        };
        let (walked, _, _) = super::super::walk_frame(
            &Files(&source),
            0,
            &Edit::default(),
            merge,
            None,
            &mut crate::walk::NoModels,
        )
        .unwrap();
        let (sw, sh) = (walked.width, walked.height);
        let src = &walked.rgb;
        let (tw, th) = (
            (0.58 * sw as f64).round() as usize,
            (0.86 * sh as f64).round() as usize,
        );
        let oy = (0.07 * sh as f64).round();
        let (cx, cy) = ((tw - 1) as f64 / 2.0, (th - 1) as f64 / 2.0);
        let t = 1.5f64.to_radians();
        let h3 = |m: [f64; 9]| Homography { m };
        let hb = h3([
            1.0,
            0.0,
            (0.4 * sw as f64).round(),
            0.0,
            1.0,
            oy,
            0.0,
            0.0,
            1.0,
        ])
        .compose(&h3([1.0, 0.0, cx, 0.0, 1.0, cy, 0.0, 0.0, 1.0]))
        .compose(&h3([
            t.cos(),
            -t.sin(),
            0.0,
            t.sin(),
            t.cos(),
            0.0,
            0.0,
            0.0,
            1.0,
        ]))
        .compose(&h3([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.2e-5, 0.0, 1.0]))
        .compose(&h3([1.0, 0.0, -cx, 0.0, 1.0, -cy, 0.0, 0.0, 1.0]));
        let gain_b = 0.85f32;
        let sample = |x: f64, y: f64, c: usize| {
            let x = x.clamp(0.0, (sw - 1) as f64);
            let y = y.clamp(0.0, (sh - 1) as f64);
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
            let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
            let p = |a: usize, b: usize| src[(b * sw + a) * 3 + c];
            (p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx) * (1.0 - fy)
                + (p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx) * fy
        };
        let tile_a: Vec<f32> = (0..tw * th)
            .flat_map(|i| {
                let (x, y) = (i % tw, i / tw);
                (0..3).map(move |c| src[((y + oy as usize) * sw + x) * 3 + c])
            })
            .collect();
        let tile_b: Vec<f32> = (0..tw * th)
            .flat_map(|i| {
                let (u, v) = hb.apply((i % tw) as f64, (i / tw) as f64);
                (0..3).map(move |c| sample(u, v, c) * gain_b)
            })
            .collect();
        // A's (x, y) is the source at (x, y + oy); B's (u, v) at hb (u, v):
        // A's indices to B's, then to pixel centers at halves.
        let truth = hb
            .inverse()
            .unwrap()
            .compose(&h3([1.0, 0.0, 0.0, 0.0, 1.0, oy, 0.0, 0.0, 1.0]))
            .shifted(0.5);
        for strength in [0.0f32, 0.45] {
            // The prototype's vignette, 1 - s r^2 to the half diagonal, is
            // a `pa` model of k1 = -s: what a lens profile corrects.
            let lens = LensCorrection {
                vignetting: Some(greycard_core::develop::lens::Vignetting {
                    k: [-strength, 0.0, 0.0],
                }),
                ..Default::default()
            };
            let darkened = |tile: &[f32]| -> Vec<f32> {
                let mut out = tile.to_vec();
                for (i, p) in out.chunks_mut(3).enumerate() {
                    let g = 1.0
                        / lens.gain_at(
                            (i % tw) as f32 + 0.5,
                            (i / tw) as f32 + 0.5,
                            tw as f32,
                            th as f32,
                        );
                    p.iter_mut().for_each(|v| *v *= g);
                }
                out
            };
            let (a, b) = (darkened(&tile_a), darkened(&tile_b));
            for corrected in [false, true] {
                if strength == 0.0 && corrected {
                    continue;
                }
                let fix = |t: &[f32]| -> Vec<f32> {
                    if !corrected {
                        return t.to_vec();
                    }
                    let image = greycard_core::image::WorkingImage {
                        width: tw,
                        height: th,
                        data: t.to_vec(),
                    };
                    greycard_core::develop::lens::correct(&image, &lens).0.data
                };
                let (a, b) = (fix(&a), fix(&b));
                let none = FrameLens::none(tw, th);
                let started = Instant::now();
                let (pa, ka) = planes_of(&a, tw, th, &none);
                let (pb, kb) = planes_of(&b, tw, th, &none);
                let (link, report) = pair(&ka, &kb, &pa, &pb, (tw, th)).expect("a pair");
                let took = started.elapsed().as_secs_f64();
                let worst = geo::grid_points(&truth, tw, th, 12)
                    .iter()
                    .map(|&(p, q)| {
                        let r = link.homography.apply(p.0, p.1);
                        (r.0 - q.0).hypot(r.1 - q.1)
                    })
                    .fold(0.0, f64::max);
                // B through the found homography onto A over the
                // overlap, and the gain between them.
                let mut on_a = vec![f32::NAN; tw * th * 3];
                let back = link.homography;
                for i in 0..tw * th {
                    let (x, y) = ((i % tw) as f64 + 0.5, (i / tw) as f64 + 0.5);
                    let (u, v) = back.apply(x, y);
                    if u < 1.0 || v < 1.0 || u > tw as f64 - 1.0 || v > th as f64 - 1.0 {
                        continue;
                    }
                    for c in 0..3 {
                        on_a[i * 3 + c] = geo::sample(&b, tw, th, c, u, v, Kernel::Cubic);
                    }
                }
                let a_held: Vec<f32> = a
                    .chunks(3)
                    .zip(on_a.chunks(3))
                    .flat_map(|(p, q)| {
                        if q[0].is_finite() {
                            [p[0], p[1], p[2]]
                        } else {
                            [f32::NAN; 3]
                        }
                    })
                    .collect();
                let (ratio, count) = geo::overlap_ratio(&on_a, &a_held, 1000).unwrap();
                println!(
                    "MEASURE tiles {tw}x{th}, vignette {strength}{}: {}/{} corners, {} matches, \
                     {} inliers; fit {:?}; worst {worst:.3} px from the truth over the overlap; \
                     gain {:.4} {:.4} {:.4} from {count} px (truth {:.4}); {took:.2} s",
                    if corrected { ", taken out first" } else { "" },
                    report.corners.0,
                    report.corners.1,
                    report.matches,
                    report.inliers,
                    report.fit,
                    ratio[0],
                    ratio[1],
                    ratio[2],
                    1.0 / gain_b
                );
                if strength == 0.0 || corrected {
                    assert!(worst < 0.5, "{worst}");
                    assert!(
                        ratio
                            .iter()
                            .all(|r| (r * f64::from(gain_b) - 1.0).abs() < 2e-3),
                        "{ratio:?}"
                    );
                }
            }
        }
    }
}
