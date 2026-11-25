//! Focus stacking: a sharpness map per frame, a Laplacian pyramid blend.
//!
//! A focus stack is a sequence of frames of one still scene, each
//! focused at a different distance, and the picture wanted is the one
//! that is sharp everywhere. This takes those frames, puts them on top
//! of one another with [`crate::register`], measures how sharp each is
//! at every pixel, and merges them band by band so that each band
//! comes from whichever frame holds it sharpest.
//!
//! Written from two papers, nothing ported. The pyramid is Burt and
//! Adelson, "The Laplacian Pyramid as a Compact Image Code" (IEEE
//! Trans. Communications 31(4), 1983): the same binomial `[1 4 6 4
//! 1]/16` reduce the registration pyramid uses, and the matching
//! expand, so that a frame taken apart and put back together is
//! itself. The weighting is Mertens, Kautz and Van Reeth, "Exposure
//! Fusion" (Pacific Graphics 2007): the per-frame weights are carried
//! down a Gaussian pyramid of their own and applied level by level,
//! which is what makes a seam invisible — a weight map that switches
//! over one pixel at full resolution has switched over half the frame
//! by the coarsest level, so the low frequencies cross fade over a
//! distance and the high ones do not have to.
//!
//! **The reference.** Every frame is registered onto one of them with
//! a similarity — shift, rotation and one scale — because focus
//! breathing changes the magnification, so even a stack from a tripod
//! is not a stack of frames of one size. The default reference is the
//! middle frame ([`Reference::Middle`]) and not the sharpest. A focus
//! stack is ordered by focus distance and the breathing is monotonic
//! in it, so the middle frame is the one nearest in magnification to
//! all the others, and the registration's reach is spent on the
//! smallest transforms it can be. The sharpest frame is an arbitrary
//! point in that sequence and is as likely as not to be at one end of
//! it. [`Reference::Sharpest`] is there for a caller who wants the
//! output's geometry to be the sharpest frame's, and
//! [`Reference::Index`] for one who knows.
//!
//! **Neighbor to neighbor, not each to the reference.** The fits are
//! chained outward from the reference: frame `r+2` is fitted against
//! frame `r+1`, and its transform onto the reference is the
//! composition. That is not a saving, it is the only thing that works.
//! A focus stack's frames differ by defocus, and defocus is a real
//! difference in the picture and not a difference in where it is: the
//! ends of a stack have next to no structure in common, one being
//! sharp exactly where the other is a smear, and a gradient fit
//! between them has little to hold on to. Measured on the synthetic
//! stack the tests build, five frames fitted straight onto the middle
//! one come back with the scale wrong in sign on two of them, at
//! residuals of 0.36 to 0.58; fitted to their neighbors the same
//! five links are 0.18 to 0.21 and every magnification comes back
//! within 0.004 of the truth, in the right direction. It is not
//! exact: the frames are built a step of 0.4% apart and the fits
//! recover between three fifths and three quarters of that, the rest
//! being the defocus pulling on a gradient fit that has no robust
//! weighting. The cost of the chain is that a link's error
//! accumulates down it,
//! which for the tens of frames a focus stack has is a small price,
//! and it is the trade every focus stacker makes.
//!
//! A frame whose link fails is dropped, and the chain carries on from
//! the last frame that held rather than stopping: the next frame is
//! fitted against that one, two focus steps away instead of one.
//!
//! **What counts as a failed link.** Not
//! [`crate::register::Fit::aligned`], which is calibrated for a frame
//! against a warped copy of itself and wants a residual under a
//! twentieth. Two frames of a focus stack never reach that, because
//! the defocus between them is a difference no transform takes out
//! and the residual counts it. The gate here is
//! [`Options::max_residual`], which the links of a real stack sit
//! well under and two pictures of different scenes — about 1.4, §119
//! — sit well over.
//!
//! **The sharpness map.** The local energy of the Laplacian, smoothed
//! over a small window. It is taken on the *log* of the luminance, as
//! the registration's fit is and for the same reason: there the
//! Laplacian is a relative contrast rather than an absolute one, so a
//! sharp edge in the shadows counts as much as a sharp edge in the
//! light, and a frame's own exposure cannot tilt the comparison. The
//! floor under the log is [`crate::register`]'s, taken from the
//! frame's median, which keeps one dead photosite from becoming the
//! sharpest thing in the picture.
//!
//! **Halos**, which §71 named as the quality problem: where a sharp
//! near edge sits over a blurred far one, the frames focused far hold
//! the near object's out-of-focus disc, a wide soft glow spread over
//! the background. A merge that switches from the near frame to the
//! far frame across the silhouette pulls that glow in beside the
//! sharp edge, and it reads as a halo following the outline.
//!
//! The pyramid is what is done about it, and on the measure the tests
//! take it is most of the answer. A frame's weight map says "this
//! frame, here" at full resolution; carried down a Gaussian pyramid it
//! says it less and less sharply, so a coarse band — which is where
//! the glow is — is never selected from one frame over a small region.
//! The fixture is a sharp bar over a background defocused by eight
//! pixels, the far-focused frame carrying the bar's own out-of-focus
//! disc over the background at the coverage a Gaussian gives it, and
//! the halo is the background's local mean in the thirty pixels beyond
//! the silhouette over its level further out, as a fraction of the
//! step across the silhouette. Merged with the same weights and no
//! pyramid at all it is +13.3%. With the pyramid it is +3.4%, and the
//! depth is what does it: one level 13.3%, two 7.9%, three 3.4%, four
//! 3.4%. It stops improving once the pyramid is deeper than the blur.
//!
//! [`Options::selectivity`] does not touch the halo — across
//! exponents from 0 to 8 it moves it by two tenths of a percent, and
//! it should not, since the glow is low frequency and lives in the top
//! of the pyramid, which is not a band and has no contrast to select
//! on. What it does is keep the detail, which is the other half of the
//! quality problem and the reason it is on by default. See its own
//! note.
//!
//! **Faint texture.** The weights come from the full-resolution
//! Laplacian, and where the picture has only a faint, fine texture
//! that measure is the sensor noise's, the same in every frame: the
//! weights spread over the whole stack and the merge averages frames
//! focused at other depths over the texture. On the bed's eleven
//! frames at base ISO it kept a quarter of a 1.5% texture, where the
//! frame focused there keeps three quarters. So the placing pass also
//! gathers, per 16 px block and counted across the stack, each frame's
//! energy a level and two down the pyramid, where the texture stands
//! clear of the noise; and the blend weighs each frame's three finest
//! bands by how near that frame comes to the block's sharpest, where
//! the sharpest stands far enough above the block's median, measured
//! against each frame's own finest band, to be believed against pure
//! noise. The same bed keeps 0.57 of the texture now; a flat field, where no block is believed, is averaged
//! as it was. See `prior`. The coarser bands and the top keep the
//! frames' own weights, so the seams and the halo are the pyramid's as
//! below. Stacks of fewer than eight frames, and a selectivity of 0,
//! merge as they did.
//!
//! **A bright edge's spill.** A frame focused away from a blown
//! highlight holds its blur disc, whose rim is a hard edge in that frame
//! wherever the disc is clipped, and the weights took it for detail: a
//! blown specular came out ringed with the defocused frames' rims and a
//! dark ring between them. Once a placed frame has a clip, the placing
//! pass also keeps, at quarter size, the darkest frame's level at each
//! place and the brightest that frame holds within reach; in the blend,
//! a frame much brighter than the darkest, its excess joined to such a
//! source and itself at the clip where the darkest is not, is spilling,
//! and its weights are held down there at every level. Only at the
//! clip: a glow under it, or a dark object with a bright ground leaking
//! in, is left as it was. See `spill`. At any count over one; at a
//! selectivity of 0 neither this nor the prior is heard, and a stack
//! with no clip merges as it did.
//!
//! What was considered and is not here: shrinking each frame's weight
//! map by a morphological erosion the width of the blur, so that a
//! frame is never trusted right up to the edge of where it is sharp.
//! It would take the halo further down and it would eat real detail
//! at every edge narrower than the erosion, and the width it wants is
//! the blur radius, which is not known — so it was not written. The
//! honest version of it is a depth map, which is its own project.
//!
//! Tried and measured: an
//! exponent on the sharpness map itself, to make the full-resolution
//! weights pick a frame more decisively. At 1.5 it is a rounding error
//! better and at 2 and above the merge falls apart, one frame winning
//! whole regions it should be sharing; the error against an all-sharp
//! frame goes from 0.011 to 0.030 to 0.049. The map is already a
//! squared quantity — it is the energy of a Laplacian, not its
//! magnitude — which is most of why another power is too much.
//!
//! **Coverage.** A frame warped onto the reference does not cover all
//! of it; the shift and the scale leave a margin. The result says how
//! many frames covered each pixel ([`StackReport::coverage`]), so a
//! caller can crop to where the whole stack was rather than to where
//! only the reference was. The reference covers everything, so the
//! count is never zero and the merge never has a hole.
//!
//! Everything here is CPU. A stack is a one-off, as §71 said. On this
//! machine it costs 0.55 to 0.7 s a frame at 24 megapixels, run to
//! run, and about 1.5 s at 45: five frames of 6000x4000 in 2.7 to
//! 3.9 s, ten in 5.4 to 7.0, roughly linear in the count with a
//! fraction of a second of fixed cost for the accumulators. Not a
//! viewport concern, and no case yet for a GPU version.
//!
//! **Memory.** The merge's own share is the three accumulator pyramids
//! — the weighted bands, the weights and the unweighted bands the
//! flat-region fallback needs — and one frame's pyramid at a time,
//! with the prior's block counts beside them (about 1.6 bytes a pixel,
//! whatever the count) and, in a stack with a clip, the spill's six
//! quarter-size planes (about 1.5 bytes).
//! Nothing else is held across frames: the luminance planes are taken
//! and dropped as they are used, three at a time in the chain and one
//! at a time everywhere else, and the Laplacian pyramid takes the
//! warped frame rather than copying it. [`stack`] and [`stack_camera`]
//! are handed every frame and so hold them all, 12 bytes a pixel a
//! frame; [`stack_camera_from`] reads them, once to score and place
//! them and once to blend them, and holds none, so a stack of a
//! hundred frames needs what a stack of two does.

use rayon::prelude::*;

use crate::error::{Error, Result};
use crate::image::{CameraImage, WorkingImage};
use crate::register::{self, Edge, Fit, Kernel, Transform};
use crate::sharpness;

#[cfg(test)]
mod bed;
#[cfg(test)]
mod breathing;
#[cfg(test)]
mod halo_bed;
#[cfg(test)]
mod in_memory;
mod prior;
#[cfg(test)]
mod scene_bed;
mod spill;

/// Which frame the others are put onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reference {
    /// The middle of the sequence: the frame nearest in magnification
    /// to all the others, since focus breathing runs one way through
    /// a stack. The default.
    #[default]
    Middle,
    /// The frame with the most Laplacian contrast over the whole
    /// picture.
    Sharpest,
    /// This frame, by its position in the slice.
    Index(usize),
}

/// How a stack is merged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub reference: Reference,
    /// How each frame is put onto the reference. A similarity by
    /// default: focus breathing is a change of scale, so a
    /// translation is not enough. Each link refined by the phase of
    /// small patches ([`register::Refine::Phase`]), which a difference
    /// in defocus does not move: the gradient fit alone reads part of
    /// one as motion, the same way at every link, and down a chain it
    /// adds up.
    pub register: register::Options,
    /// Radius, in pixels, of the window the Laplacian energy is
    /// averaged over to make the sharpness map. Small enough to follow
    /// a subject's outline, large enough that noise does not decide.
    pub sharpness_radius: usize,
    /// Most pyramid levels.
    pub max_levels: usize,
    /// The coarsest level keeps at least this many pixels on its short
    /// side.
    pub min_side: usize,
    /// The residual a link between two frames may have and still be
    /// believed. It is not [`crate::register::ALIGNED_RESIDUAL`]:
    /// that threshold is for two frames of one picture, and two
    /// frames of a focus stack differ by a defocus no transform takes
    /// out. See the module's note.
    pub max_residual: f32,
    /// How far a link may move a frame's corners, as a fraction of
    /// the frame's diagonal, before it is refused however good its
    /// residual. A focus stack is one camera that did not go
    /// anywhere; a fit that says otherwise found something else.
    pub max_motion: f64,
    /// How much a frame's contrast *in a band* adds to its weight in
    /// that band: 0 is Mertens's weighted average of the bands, and
    /// larger leans toward taking each band from the frame with the
    /// most to say in it, in the manner of Burt and Kolczynski.
    ///
    /// It is what keeps the detail. At 0 the weights come from the
    /// full-resolution map alone, and a frame that is merely blurred
    /// rather than featureless still holds a fifth of the weight where
    /// another frame is sharp, so the merged band is a fifth of the
    /// way back toward blurred: measured on the synthetic stack, the
    /// merge reaches 83% to 90% of the best frame's sharpness in that
    /// frame's own band. At 2 it reaches 92% to 95%, at 4, 94% to 97%.
    ///
    /// What it costs is the averaging. Where every frame is equally
    /// sharp — most of a stack, most of the time — a weight that
    /// follows the band's contrast follows the noise in it, and the
    /// merge stops being an average of `n` frames. With white noise a
    /// three-hundredth of the range on every frame, the merged frame's
    /// own Laplacian energy goes from 0.0156 at 0 to 0.0200 at 2 and
    /// 0.0243 at 8, against 0.0319 for a single frame: at 2 the stack
    /// is still worth two and a half frames of averaging, at 8 barely
    /// one. Two is the default for that reason.
    ///
    /// At 0 the stack's prior (see the module's note on faint texture)
    /// stands down too, so 0 stays the plain weighted average of the
    /// full-resolution map; above 0 the prior weighs the finest bands
    /// beside it.
    pub selectivity: f32,
    /// Radius the band contrast is smoothed over before the
    /// selectivity exponent, in that level's own pixels, so the
    /// selection within a band is not made pixel by pixel. Between 0
    /// and 4 it changes nothing measurable; 2 is a middle.
    pub band_radius: usize,
    /// Added to the band contrast before the exponent, in the samples'
    /// own units: where every frame is flat it is what makes them
    /// weigh the same rather than divide by nothing.
    pub band_floor: f32,
    /// The weight a frame keeps where it has no sharpness at all, as a
    /// fraction of that frame's mean sharpness. It is what makes a
    /// flat sky the average of the stack — which is also its
    /// noise divided by the root of the count — rather than a coin
    /// toss between frames.
    pub weight_floor: f32,
    /// Whether a frame's weights are held down where it holds the
    /// clipped spill of a blown highlight another frame keeps in place
    /// (the module's note on a bright edge's spill). On by default;
    /// heard only above a selectivity of 0. Off, a stack of fewer than
    /// eight frames merges as it did before it.
    pub spill: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            reference: Reference::Middle,
            register: register::Options {
                model: register::Model::Similarity,
                refine: register::Refine::Phase,
                ..Default::default()
            },
            sharpness_radius: 4,
            max_levels: 12,
            min_side: 32,
            max_residual: 0.6,
            max_motion: 0.05,
            selectivity: 2.0,
            band_radius: 2,
            band_floor: 1e-4,
            weight_floor: 0.01,
            spill: true,
        }
    }
}

/// Why a frame is not in the merge.
#[derive(Debug, Clone, PartialEq)]
pub enum Dropped {
    /// The fit could not be made at all: no structure, no overlap, a
    /// singular step.
    NoFit(String),
    /// A fit was made and is not to be believed: its residual is over
    /// [`Options::max_residual`].
    NotAligned {
        residual: f32,
        overlap: f32,
        threshold: f32,
    },
    /// A fit was made and moves the frame further than a stack's
    /// frames move: over [`Options::max_motion`] of the diagonal.
    TooFar { pixels: f64, limit: f64 },
}

impl std::fmt::Display for Dropped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dropped::NoFit(why) => write!(f, "no fit: {why}"),
            Dropped::NotAligned {
                residual,
                overlap,
                threshold,
            } => write!(
                f,
                "not aligned: residual {residual:.4} over {:.1}% of the frame, \
                 against a threshold of {threshold:.2}",
                overlap * 100.0,
            ),
            Dropped::TooFar { pixels, limit } => write!(
                f,
                "the fit moves the frame {pixels:.0} px, past the {limit:.0} px \
                 a stack's frames move"
            ),
        }
    }
}

/// What became of one frame of the stack.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameReport {
    /// Its position in the slice handed to [`stack`].
    pub index: usize,
    /// Whether it is the frame the others were put onto.
    pub reference: bool,
    /// The root-mean-square Laplacian of its log luminance: contrast
    /// per pixel, in stops, over the whole frame. Higher is sharper,
    /// and it is what [`Reference::Sharpest`] compares.
    pub sharpness: f32,
    /// The frame this one was fitted against: its neighbor toward
    /// the reference, or the nearest frame that way whose own link
    /// held. None for the reference.
    pub against: Option<usize>,
    /// That fit. Its residual is the number that says whether to
    /// believe the link; its transform is onto `against`, not onto
    /// the reference.
    pub fit: Option<Fit>,
    /// Onto the reference: this frame's links composed down the
    /// chain. The identity for the reference, and for a frame that
    /// was dropped.
    pub transform: Transform,
    /// Why it is not in the merge; none when it is.
    pub dropped: Option<Dropped>,
}

impl FrameReport {
    /// Whether this frame is in the merged picture.
    pub fn used(&self) -> bool {
        self.dropped.is_none()
    }
}

/// What a merge did, beside the picture.
#[derive(Debug, Clone, PartialEq)]
pub struct StackReport {
    pub width: usize,
    pub height: usize,
    /// Which frame the others were put onto.
    pub reference: usize,
    /// One per frame handed in, in that order.
    pub frames: Vec<FrameReport>,
    /// Pyramid levels the blend used.
    pub levels: usize,
    /// How many frames covered each pixel of the result, row-major.
    /// Never zero: the reference covers all of itself.
    pub coverage: Vec<u16>,
    /// Whether the placing pass heard a placed frame at its clip, and
    /// gathered the spill ([`Options::spill`]): what a stack made again
    /// is told, so that one without is not read for it.
    pub clipped: bool,
}

impl StackReport {
    /// How many frames are in the merge.
    pub fn used(&self) -> usize {
        self.frames.iter().filter(|f| f.used()).count()
    }

    /// Where every frame in the merge landed: the region a caller
    /// should crop to if it wants no pixel that only some of the stack
    /// saw.
    pub fn fully_covered(&self) -> Vec<bool> {
        let all = self.used() as u16;
        self.coverage.iter().map(|&c| c >= all).collect()
    }

    /// The fraction of the frame every frame in the merge covered.
    pub fn full_coverage_fraction(&self) -> f32 {
        if self.coverage.is_empty() {
            return 0.0;
        }
        let all = self.used() as u16;
        let n = self.coverage.iter().filter(|&&c| c >= all).count();
        n as f32 / self.coverage.len() as f32
    }
}

/// Merge developed working-space frames of one scene into the picture
/// that is sharp everywhere.
///
/// The frames must all be the same size — one camera, one crop — and
/// there must be at least one. The result is in the reference frame's
/// geometry.
pub fn stack(frames: &[WorkingImage], options: &Options) -> Result<(WorkingImage, StackReport)> {
    let (width, height) = one_size(frames.iter().map(|f| (f.width, f.height)))?;
    let samples: Vec<&[f32]> = frames.iter().map(|f| f.data.as_slice()).collect();
    let (data, report) = merge(&samples, width, height, WORKING_LUMA, options)?;
    Ok((WorkingImage::from_data(width, height, data)?, report))
}

/// [`stack`] one step earlier in the pipeline: camera-space frames,
/// straight from [`crate::demosaic`], merged in the space a linear DNG
/// stores, so the result can be written as one.
///
/// The merge is the same. What differs is the weighting the luminance
/// is taken with, everywhere it is taken — the registration's planes,
/// the frame scores and the sharpness maps alike: the mosaic's own,
/// green counted twice, since camera RGB has no colorimetry on it yet
/// and the three channels are not balanced against each other.
pub fn stack_camera(
    frames: &[CameraImage],
    options: &Options,
) -> Result<(CameraImage, StackReport)> {
    let (width, height) = one_size(frames.iter().map(|f| (f.width, f.height)))?;
    let samples: Vec<&[f32]> = frames.iter().map(|f| f.data.as_slice()).collect();
    let (data, report) = merge(&samples, width, height, CAMERA_LUMA, options)?;
    Ok((CameraImage::from_data(width, height, data)?, report))
}

/// Where a merge read from its frames ([`stack_camera_from`]) is, for
/// a caller to say. Said before the frame it names is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The `index`th frame read of `of` (from 1), in the first pass:
    /// scored, and placed against its neighbor unless the reference is
    /// [`Reference::Sharpest`], which wants every score first.
    Reading { index: usize, of: usize },
    /// The frames read are being placed onto the reference: at once
    /// for most references, a second pass for the sharpest.
    Placing,
    /// The `index`th frame of the `of` the merge uses is being read
    /// again and blended in.
    Blending { index: usize, of: usize },
}

/// [`stack_camera`] with the frames read as the merge wants them rather
/// than held: `count` frames, frame `k` given by `read(k)` as often as
/// the merge asks for it, the same samples each time. What it holds
/// does not grow with the count: one frame's working set and the
/// accumulators, and of each frame no pixel, only its score, its fit,
/// and its transform.
///
/// Each frame is read twice: once to score it and place it, once to
/// blend it, in its own order, so that the sums come out as
/// [`stack_camera`]'s do, bit for bit. With [`Reference::Sharpest`] it
/// is read three times, since the reference is not known until every
/// frame is scored. `step` is told each read before it is made.
pub fn stack_camera_from<E: From<Error>>(
    count: usize,
    mut read: impl FnMut(usize) -> std::result::Result<CameraImage, E>,
    mut step: impl FnMut(Step),
    options: &Options,
) -> std::result::Result<(CameraImage, StackReport), E> {
    let mut read = |k: usize| read(k).map(|f| (f.width, f.height, f.data));
    let (width, height, data, report) =
        merge_from(count, CAMERA_LUMA, options, &mut read, &mut step)?;
    Ok((CameraImage::from_data(width, height, data)?, report))
}

/// [`stack_camera_from`] with every frame's place given rather than
/// found: `placed[k]` is frame `k`'s transform onto frame `reference`,
/// none for a frame left out. Nothing is scored or fitted; each frame
/// the merge uses is blended where it is put, so the picture is the one
/// [`stack_camera_from`] made, to the bit, when the places are the ones
/// it found. For a stack made again from what it kept.
///
/// Above a selectivity of 0 (with [`Options::spill`] on and `clipped`,
/// what [`StackReport::clipped`] said of the first merge, or at least
/// `prior::MIN_FRAMES`, 8, frames placed, where the stack's prior has a
/// say) what the placing pass gathered is gathered again as it gathered
/// it: each frame the merge uses read in the chain's order, each read
/// said as [`Step::Reading`] of the frames used, then
/// [`Step::Placing`], then the blends. Such a frame is read twice, and
/// its first read should be what the placing pass read. Otherwise each
/// is read once, to be blended. The report's
/// scores and fits are left empty; a frame left out is
/// [`Dropped::NoFit`].
pub fn stack_camera_placed<E: From<Error>>(
    reference: usize,
    placed: &[Option<Transform>],
    clipped: bool,
    mut read: impl FnMut(usize) -> std::result::Result<CameraImage, E>,
    mut step: impl FnMut(Step),
    options: &Options,
) -> std::result::Result<(CameraImage, StackReport), E> {
    let n = placed.len();
    if placed.get(reference).is_none_or(Option::is_none) {
        return Err(Error::Unsupported(format!(
            "frame {reference} is no placed frame of a stack of {n}"
        ))
        .into());
    }
    let mut read = |k: usize| read(k).map(|f| (f.width, f.height, f.data));
    let mut frames = Frames {
        read: &mut read,
        first: None,
    };
    let used: Vec<(usize, Transform)> = placed
        .iter()
        .enumerate()
        .filter_map(|(k, t)| Some((k, (*t)?)))
        .collect();
    // What the placing pass heard, as it heard it: the same frames in
    // the same order, the reference and then the chain's, each measured
    // as read and put by its transform. Only the placing pass's read
    // tells a clip, and the blend's may be another picture (a caller's
    // highlights rebuilt over the clip), so a stack whose first merge
    // heard none (`clipped`) is not read for the spill.
    let mut listen = Listen::of(options, used.len());
    listen.spill &= clipped;
    let (heard, clipped) = if listen.any() {
        gather_placed(&mut frames, reference, placed, options, listen, &mut step)?
    } else {
        (Heard::default(), false)
    };
    let mut blend: Option<Blend> = None;
    for (i, &(k, transform)) in used.iter().enumerate() {
        step(Step::Blending {
            index: i + 1,
            of: used.len(),
        });
        let (w, h, frame) = frames.get(k)?;
        blend
            .get_or_insert_with(|| Blend::new(w, h, options))
            .add_measured(frame, transform, CAMERA_LUMA, options, &heard);
    }
    let blend = blend.expect("the reference is placed");
    let (width, height, levels) = (blend.width, blend.height, blend.sizes.len());
    let (out, coverage) = blend.finish();
    let report = StackReport {
        clipped,
        width,
        height,
        reference,
        frames: placed
            .iter()
            .enumerate()
            .map(|(index, t)| FrameReport {
                index,
                reference: index == reference,
                sharpness: 0.0,
                against: None,
                fit: None,
                transform: t.unwrap_or(Transform::IDENTITY),
                dropped: t
                    .is_none()
                    .then(|| Dropped::NoFit("left out when it was first stacked".into())),
            })
            .collect(),
        levels,
        coverage,
    };
    Ok((CameraImage::from_data(width, height, out)?, report))
}

/// The working space's luminance weights, as
/// [`crate::register::luminance`] has them.
const WORKING_LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// A mosaic's own weighting, as [`crate::register::camera_luminance`]
/// has it: green twice, since camera RGB has no colorimetry yet.
const CAMERA_LUMA: [f32; 3] = [0.25, 0.5, 0.25];

/// One plane from interleaved RGB.
fn luminance_of(samples: &[f32], luma: [f32; 3]) -> Vec<f32> {
    samples
        .par_chunks(CHANNELS)
        .map(|px| luma[0] * px[0] + luma[1] * px[1] + luma[2] * px[2])
        .collect()
}

fn one_size(sizes: impl Iterator<Item = (usize, usize)>) -> Result<(usize, usize)> {
    let mut first = None;
    for (n, size) in sizes.enumerate() {
        match first {
            None => first = Some(size),
            Some(one) if one == size => {}
            Some((w, h)) => {
                return Err(Error::Unsupported(format!(
                    "a stack wants frames of one size: frame 0 is {w}x{h} and frame {n} is {}x{}",
                    size.0, size.1
                )));
            }
        }
    }
    let (w, h) = first.ok_or_else(|| Error::Unsupported("a stack of no frames".into()))?;
    if w < 3 || h < 3 {
        return Err(Error::Unsupported(format!("{w}x{h} is too small to stack")));
    }
    Ok((w, h))
}

const CHANNELS: usize = 3;

/// A merge of frames held by the caller: [`merge_from`] reading each
/// from the slice, a copy at a time.
fn merge(
    frames: &[&[f32]],
    width: usize,
    height: usize,
    luma: [f32; 3],
    options: &Options,
) -> Result<(Vec<f32>, StackReport)> {
    for (n, f) in frames.iter().enumerate() {
        if f.len() != width * height * CHANNELS {
            return Err(Error::Unsupported(format!(
                "frame {n} has {} samples, not the {} a {width}x{height} RGB frame has",
                f.len(),
                width * height * CHANNELS
            )));
        }
    }
    let mut read = |k: usize| Ok::<_, Error>((width, height, frames[k].to_vec()));
    let (_, _, data, report) = merge_from(frames.len(), luma, options, &mut read, &mut |_| {})?;
    Ok((data, report))
}

/// A frame as read: its width, its height, and its interleaved RGB.
type Read = (usize, usize, Vec<f32>);

/// The frames as the merge reads them, each checked against the first
/// read for its size.
struct Frames<'a, E> {
    read: &'a mut dyn FnMut(usize) -> std::result::Result<Read, E>,
    /// The first frame read, and its size.
    first: Option<(usize, usize, usize)>,
}

impl<E: From<Error>> Frames<'_, E> {
    fn get(&mut self, index: usize) -> std::result::Result<(usize, usize, Vec<f32>), E> {
        let (w, h, data) = (self.read)(index)?;
        match self.first {
            None => {
                if w < 3 || h < 3 {
                    return Err(Error::Unsupported(format!("{w}x{h} is too small to stack")).into());
                }
                self.first = Some((index, w, h));
            }
            Some((_, fw, fh)) if (fw, fh) == (w, h) => {}
            Some((first, fw, fh)) => {
                return Err(Error::Unsupported(format!(
                    "a stack wants frames of one size: frame {first} is {fw}x{fh} and frame \
                     {index} is {w}x{h}"
                ))
                .into());
            }
        }
        if data.len() != w * h * CHANNELS {
            return Err(Error::Unsupported(format!(
                "frame {index} has {} samples, not the {} a {w}x{h} RGB frame has",
                data.len(),
                w * h * CHANNELS
            ))
            .into());
        }
        Ok((w, h, data))
    }
}

/// Which of the placing pass's words on the frames a merge gathers: the
/// prior's ([`prior`]) and the spill's ([`spill`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Listen {
    prior: bool,
    spill: bool,
}

impl Listen {
    /// What a merge of `placed` frames under `options` gathers. Neither
    /// at a selectivity of 0, the plain weighted average that setting
    /// promises; the prior only in a stack long enough for a median.
    fn of(options: &Options, placed: usize) -> Self {
        let selective = options.selectivity > 0.0;
        Self {
            prior: selective && placed >= prior::MIN_FRAMES,
            spill: selective && options.spill && placed >= 2,
        }
    }

    fn any(self) -> bool {
        self.prior || self.spill
    }
}

/// What the placing pass took of one frame for the blend.
#[derive(Default)]
struct Taken {
    /// The frame's index in the stack.
    index: usize,
    blocks: Option<Vec<prior::Blocks>>,
    /// Whether the frame is at its own clip anywhere.
    clips: bool,
    coarse: Option<spill::Coarse>,
}

/// The spill's part of the gathering. Only a frame at the clip is held
/// ([`spill`]), so its planes are gathered only once one is read: until
/// then the frames are listed, and the first at the clip has those
/// before it read again, in their order, for the spill alone. What is
/// gathered is what gathering every frame would gather, whichever frame
/// clips first: the same frames in the same order, and the prior's
/// skips the same, since a frame without a clip has none.
enum Spilling {
    /// Not listened for.
    Deaf,
    /// No frame at the clip yet: those gathered, by index and place.
    Waiting(Vec<(usize, Transform)>),
    Heard(spill::Gathered),
}

/// The placing pass's words, gathered a frame at a time.
struct Gathering {
    width: usize,
    height: usize,
    prior: Option<prior::Gathered>,
    spill: Spilling,
}

impl Gathering {
    fn new(width: usize, height: usize, listen: Listen) -> Self {
        Self {
            width,
            height,
            prior: listen.prior.then(|| prior::Gathered::new(width, height)),
            spill: if listen.spill {
                Spilling::Waiting(Vec::new())
            } else {
                Spilling::Deaf
            },
        }
    }

    /// Whether the spill is being gathered: every frame's planes wanted.
    fn hearing(&self) -> bool {
        matches!(self.spill, Spilling::Heard(_))
    }

    /// A frame at the clip is read: the spill gathered from here on, and
    /// the frames gathered before it, by index and place, to be read
    /// again for it ([`Gathering::again`]) before the frame is added.
    fn wake(&mut self) -> Vec<(usize, Transform)> {
        match &mut self.spill {
            Spilling::Waiting(seen) => {
                let seen = std::mem::take(seen);
                self.spill = Spilling::Heard(spill::Gathered::new(self.width, self.height));
                seen
            }
            _ => Vec::new(),
        }
    }

    /// A frame gathered before the spill woke, read again: the spill's
    /// part of it, the prior's having been taken (with no skips, since
    /// a frame without a clip has none).
    fn again(&mut self, coarse: &spill::Coarse, onto: &Transform) {
        if let Spilling::Heard(s) = &mut self.spill {
            let _ = s.add(coarse, onto);
        }
    }

    /// Frame `index`, taken, put by `onto`.
    fn add(&mut self, index: usize, taken: Taken, onto: &Transform) {
        // The spill first: it says which of the frame's blocks the
        // prior is to believe.
        let roles = match (&mut self.spill, &taken.coarse) {
            (Spilling::Heard(s), Some(d)) => Some(s.add(d, onto)),
            (Spilling::Waiting(seen), _) => {
                seen.push((index, *onto));
                None
            }
            _ => None,
        };
        if let Some(p) = &mut self.prior {
            p.add(&taken.blocks.unwrap_or_default(), onto, roles.as_deref());
        }
    }

    /// What was heard, once `placed` frames are placed: no spill where
    /// no frame was at the clip.
    fn finish(self, placed: usize) -> Heard {
        Heard {
            prior: found(self.prior, placed),
            spill: match self.spill {
                Spilling::Heard(s) if placed >= 2 => Some(s.finish()),
                _ => None,
            },
        }
    }
}

/// The frames placed since a placed frame woke the spill, added: the
/// frames placed before it read again for the spill first.
fn settle<E: From<Error>>(
    frames: &mut Frames<'_, E>,
    gathering: &std::cell::RefCell<Option<Gathering>>,
    pending: &std::cell::RefCell<Vec<(Taken, Transform)>>,
    luma: [f32; 3],
    options: &Options,
) -> std::result::Result<(), E> {
    let pending = pending.take();
    let mut gathering = gathering.borrow_mut();
    let Some(g) = gathering.as_mut() else {
        return Ok(());
    };
    for (taken, onto) in pending {
        if taken.clips {
            for (j, before) in g.wake() {
                let (w, h, frame) = frames.get(j)?;
                let lum = luminance_of(&frame, luma);
                drop(frame);
                g.again(&coarse_of(&lum, w, h, options), &before);
            }
        }
        g.add(taken.index, taken, &onto);
    }
    Ok(())
}

/// The placing pass gathered again for frames put by `placed` onto
/// `reference`: each read in the chain's order, said as
/// [`Step::Reading`], measured, and added; the first frame at its clip
/// has those before it read again for the spill, unsaid.
fn gather_placed<E: From<Error>>(
    frames: &mut Frames<'_, E>,
    reference: usize,
    placed: &[Option<Transform>],
    options: &Options,
    listen: Listen,
    step: &mut dyn FnMut(Step),
) -> std::result::Result<(Heard, bool), E> {
    let n = placed.len();
    let used = placed.iter().filter(|p| p.is_some()).count();
    let mut gathering: Option<Gathering> = None;
    let order = std::iter::once(reference).chain(chain_orders(n, reference).into_iter().flatten());
    let order = order.filter_map(|k| Some((k, placed[k]?)));
    for (i, (k, transform)) in order.enumerate() {
        step(Step::Reading {
            index: i + 1,
            of: used,
        });
        let (w, h, frame) = frames.get(k)?;
        let lum = luminance_of(&frame, CAMERA_LUMA);
        drop(frame);
        let g = gathering.get_or_insert_with(|| Gathering::new(w, h, listen));
        let (_, taken) = measure(&lum, w, h, options, listen, g.hearing());
        drop(lum);
        if taken.clips {
            for (j, onto) in g.wake() {
                let (w, h, frame) = frames.get(j)?;
                let lum = luminance_of(&frame, CAMERA_LUMA);
                drop(frame);
                g.again(&coarse_of(&lum, w, h, options), &onto);
            }
        }
        g.add(k, taken, &transform);
    }
    step(Step::Placing);
    let clipped = gathering.as_ref().is_some_and(Gathering::hearing);
    Ok((
        gathering.map(|g| g.finish(used)).unwrap_or_default(),
        clipped,
    ))
}

/// What the blend is told of the stack.
#[derive(Default)]
struct Heard {
    prior: Option<prior::Prior>,
    spill: Option<spill::Spill>,
}

/// The frame's score (the RMS Laplacian of its log luminance, which
/// every frame gets whatever else happens: it picks the reference, and
/// a caller is shown it), and what the placing pass takes of it for
/// `listen`; the spill's planes only where the frame is at its clip or
/// `hearing`, the spill being gathered already.
fn measure(
    lum: &[f32],
    width: usize,
    height: usize,
    options: &Options,
    listen: Listen,
    hearing: bool,
) -> (f32, Taken) {
    let log = register::log_plane(lum, options.register.floor_stops);
    let energy = sharpness::laplacian_energy(&log, width, height);
    let score = (energy.iter().map(|&e| e as f64).sum::<f64>() / energy.len() as f64).sqrt() as f32;
    let clips = listen.spill && spill::clips(&log, width, height);
    let taken = Taken {
        blocks: listen
            .prior
            .then(|| prior::blocks(&log, &energy, width, height)),
        clips,
        coarse: (listen.spill && (hearing || clips))
            .then(|| spill::coarse(&log, &energy, width, height, None, clips)),
        ..Taken::default()
    };
    (score, taken)
}

/// A frame read again for the spill alone: its coarse planes as
/// [`measure`] takes them.
fn coarse_of(lum: &[f32], width: usize, height: usize, options: &Options) -> spill::Coarse {
    let log = register::log_plane(lum, options.register.floor_stops);
    let energy = sharpness::laplacian_energy(&log, width, height);
    spill::coarse(
        &log,
        &energy,
        width,
        height,
        None,
        spill::clips(&log, width, height),
    )
}

/// The merge, its frames read rather than held.
///
/// The first pass reads every frame in the chain's order — the
/// reference, then outward from it one way and then the other — scoring
/// each and fitting it against the last frame that held, so no more
/// than three luminance planes are live: the reference's, kept for the
/// second direction, the anchor's, and the frame's being fitted. The
/// second pass reads the frames the merge uses in their own order and
/// blends each into the accumulators as it comes, which is the order
/// the sums were always taken in. Nothing of a frame survives its read
/// but its score and its link.
fn merge_from<E: From<Error>>(
    count: usize,
    luma: [f32; 3],
    options: &Options,
    read: &mut dyn FnMut(usize) -> std::result::Result<Read, E>,
    step: &mut dyn FnMut(Step),
) -> std::result::Result<Merged, E> {
    if count == 0 {
        return Err(Error::Unsupported("a stack of no frames".into()).into());
    }
    let mut frames = Frames { read, first: None };
    let n = count;
    let mut sharpness = vec![0.0f32; n];
    let mut reads = 0usize;

    let sharpest = options.reference == Reference::Sharpest;
    // What the placing pass gathers; at a selectivity of 0 the merge is
    // the plain weighted average that setting promises, and nothing is
    // gathered or used.
    let gathering: std::cell::RefCell<Option<Gathering>> = std::cell::RefCell::new(None);
    // The placing pass's words, only where they can be used: above a
    // selectivity of 0, and the prior in a stack long enough for a
    // median.
    let listen = Listen::of(options, n);
    let reference = match options.reference {
        Reference::Middle => n / 2,
        Reference::Index(i) => {
            if i >= n {
                return Err(Error::Unsupported(format!(
                    "frame {i} asked for as the reference of a stack of {n}"
                ))
                .into());
            }
            i
        }
        Reference::Sharpest => {
            // A pass of its own: the chain starts from the reference,
            // and the reference is the best score.
            for (i, s) in sharpness.iter_mut().enumerate() {
                reads += 1;
                step(Step::Reading {
                    index: reads,
                    of: n,
                });
                let (w, h, frame) = frames.get(i)?;
                let lum = luminance_of(&frame, luma);
                drop(frame);
                *s = measure(&lum, w, h, options, Listen::default(), false).0;
            }
            sharpness
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(i, _)| i)
                .unwrap_or(0)
        }
    };
    if sharpest {
        step(Step::Placing);
    }

    // A placed frame at its clip wakes the spill, and the frames placed
    // before it are read again for it, which the fit's reads cannot
    // wait on: it is held here, with any placed after it, until the
    // next read or the fit's end. Only a placed frame wakes it, as in a
    // stack made again, which reads only those.
    let pending: std::cell::RefCell<Vec<(Taken, Transform)>> = std::cell::RefCell::new(Vec::new());
    let place = |taken: Taken, onto: &Transform| {
        if let Some(g) = gathering.borrow_mut().as_mut() {
            let mut pending = pending.borrow_mut();
            if !pending.is_empty() || (taken.clips && !g.hearing()) {
                pending.push((taken, *onto));
            } else {
                g.add(taken.index, taken, onto);
            }
        }
    };
    // Each frame's luminance as the chain reaches it, scored on the way
    // unless the scoring pass has been.
    type Measured = (usize, usize, Vec<f32>, Taken);
    let mut lum_of = |i: usize| -> std::result::Result<Measured, E> {
        settle(&mut frames, &gathering, &pending, luma, options)?;
        if !sharpest {
            reads += 1;
            step(Step::Reading {
                index: reads,
                of: n,
            });
        }
        let (w, h, frame) = frames.get(i)?;
        let lum = luminance_of(&frame, luma);
        drop(frame);
        let hearing = gathering.borrow().as_ref().is_some_and(Gathering::hearing);
        let (s, mut taken) = measure(&lum, w, h, options, listen, hearing);
        taken.index = i;
        if !sharpest {
            sharpness[i] = s;
        }
        Ok((w, h, lum, taken))
    };
    let (width, height, reference_lum, reference_taken) = lum_of(reference)?;
    if listen.any() {
        gathering.replace(Some(Gathering::new(width, height, listen)));
        place(reference_taken, &Transform::IDENTITY);
    }
    let chain = fit_chain(
        n,
        width,
        height,
        reference,
        &reference_lum,
        options,
        |i| lum_of(i).map(|(_, _, lum, taken)| (lum, taken)),
        place,
    )?;
    drop(reference_lum);
    settle(&mut frames, &gathering, &pending, luma, options)?;
    let placed = chain.dropped.iter().filter(|d| d.is_none()).count();
    let gathering = gathering.into_inner();
    let clipped = gathering.as_ref().is_some_and(Gathering::hearing);
    let heard = gathering.map(|g| g.finish(placed)).unwrap_or_default();
    if !sharpest {
        step(Step::Placing);
    }

    let reports: Vec<FrameReport> = (0..n)
        .map(|index| FrameReport {
            index,
            reference: index == reference,
            sharpness: sharpness[index],
            against: chain.against[index],
            fit: chain.fits[index],
            transform: chain.onto[index],
            dropped: chain.dropped[index].clone(),
        })
        .collect();

    let mut blend = Blend::new(width, height, options);
    let used = reports.iter().filter(|r| r.used()).count();
    for (k, report) in reports.iter().filter(|r| r.used()).enumerate() {
        step(Step::Blending {
            index: k + 1,
            of: used,
        });
        let (_, _, frame) = frames.get(report.index)?;
        blend.add_measured(frame, report.transform, luma, options, &heard);
    }
    let levels = blend.sizes.len();
    let (out, coverage) = blend.finish();

    Ok((
        width,
        height,
        out,
        StackReport {
            width,
            height,
            reference,
            frames: reports,
            levels,
            coverage,
            clipped,
        },
    ))
}

/// What [`merge_from`] gives: the size, the merged samples, and the
/// report.
type Merged = (usize, usize, Vec<f32>, StackReport);

/// The prior from the evidence gathered, when `placed` frames are
/// enough for a median.
fn found(evidence: Option<prior::Gathered>, placed: usize) -> Option<prior::Prior> {
    let prior = evidence
        .filter(|_| placed >= prior::MIN_FRAMES)
        .map(prior::Gathered::finish);
    if let Some(p) = &prior {
        log::debug!(
            "stack: the stack's prior has a say in {:.1}% of the picture",
            p.trusted() * 100.0
        );
    }
    prior
}

/// The blend's accumulators, one frame added at a time.
///
/// Unnormalized weights, renormalized level by level against the
/// weight that actually landed there. Each frame's full-resolution
/// weight map is carried down its own Gaussian pyramid and used as it
/// comes, and the sum of the weights is accumulated beside the sum of
/// the weighted bands; the division happens once, at the end.
///
/// This is *not* Mertens's order, which normalizes the maps against
/// each other at full resolution and then builds the pyramids, so that
/// every level's weights already sum to one. The two are not the same
/// thing: the effective weight here is `G(w_i) / sum_j G(w_j)` where
/// Mertens's is `G(w_i / sum_j w_j)`, and a blur of a ratio is not the
/// ratio of blurs. Measured on the synthetic stack, the effective
/// per-frame weights part company by up to 0.18 at levels 1 to 3.
///
/// It is kept because what the seam argument needs is true of both and
/// the differences do not matter. At level 0 they are identical, the
/// pyramid being the identity there, so the detail is selected exactly
/// as Mertens selects it. At every level the result is a convex
/// combination of the frames' own bands — nonnegative weights over
/// their own sum — so nothing can overshoot and the brightness is a
/// weighted mean of the frames' and not a scaling of it. Both carry the
/// weight map down a pyramid, which is the whole of why a seam is soft.
/// And once `selectivity` is on, the per-level weights are not the
/// pyramid of any full-resolution map at all, so normalizing first is
/// not even defined for them.
///
/// What it buys is one pass over the frames and one accumulator,
/// instead of every frame's weight map held at once to normalize them
/// against each other — which is also what lets the frames be read one
/// at a time.
struct Blend {
    width: usize,
    height: usize,
    sizes: Vec<(usize, usize)>,
    bands: Vec<Vec<f32>>,
    weights: Vec<Vec<f32>>,
    /// The same bands with no weights on them, for the pixels where
    /// every frame's weight is zero. That happens: the weight floor is
    /// a fraction of the frame's own mean sharpness, and a region that
    /// is flat in every frame has no sharpness for the floor to be a
    /// fraction of. Without this, such a region divides zero by nothing
    /// and comes out black.
    plain: Vec<Vec<f32>>,
    merged: usize,
    coverage: Vec<u16>,
    ones: Vec<f32>,
}

impl Blend {
    fn new(width: usize, height: usize, options: &Options) -> Self {
        let sizes = pyramid_sizes(width, height, options);
        Self {
            width,
            height,
            bands: sizes
                .iter()
                .map(|&(w, h)| vec![0.0; w * h * CHANNELS])
                .collect(),
            weights: sizes.iter().map(|&(w, h)| vec![0.0; w * h]).collect(),
            plain: sizes
                .iter()
                .map(|&(w, h)| vec![0.0; w * h * CHANNELS])
                .collect(),
            sizes,
            merged: 0,
            coverage: vec![0u16; width * height],
            ones: vec![1.0f32; width * height],
        }
    }

    /// [`Blend::add`] with the frame's multiplier under `prior`, its
    /// blocks taken as the placing pass took them: on the frame as read
    /// and placed by its transform, since a warped frame's finest band
    /// is softened by the warp, which would tilt the ratio.
    fn add_measured(
        &mut self,
        frame: Vec<f32>,
        transform: Transform,
        luma: [f32; 3],
        options: &Options,
        heard: &Heard,
    ) {
        let (w, h) = (self.width, self.height);
        // The spill's word only on a frame it can hold: most frames of
        // most stacks have no clip, and are spared its measuring.
        let spill = heard.spill.as_ref().filter(|s| {
            let from = s.held_from();
            frame
                .par_chunks(CHANNELS)
                .any(|px| luma[0] * px[0] + luma[1] * px[1] + luma[2] * px[2] >= from)
        });
        let (multiplier, damping) = if heard.prior.is_some() || spill.is_some() {
            let log =
                register::log_plane(&luminance_of(&frame, luma), options.register.floor_stops);
            let energy = sharpness::laplacian_energy(&log, w, h);
            let multiplier = heard
                .prior
                .as_ref()
                .map(|p| p.multiplier(&prior::blocks(&log, &energy, w, h), &transform));
            // Held under the brightest the placing pass saw, which may
            // have read the frame clipped where this read rebuilt it.
            let damping = spill.and_then(|s| {
                let coarse = spill::coarse(&log, &energy, w, h, Some(s.ceiling()), true);
                s.multiplier(&coarse, &transform)
            });
            (multiplier, damping)
        } else {
            (None, None)
        };
        self.add(
            frame,
            transform,
            luma,
            options,
            heard.prior.as_ref().zip(multiplier.as_deref()),
            heard.spill.as_ref().zip(damping.as_deref()),
        );
    }

    /// One frame, taken, in the reference's geometry by `transform`.
    fn add(
        &mut self,
        frame: Vec<f32>,
        transform: Transform,
        luma: [f32; 3],
        options: &Options,
        prior: Option<(&prior::Prior, &[f32])>,
        spill: Option<(&spill::Spill, &[f32])>,
    ) {
        let (width, height) = (self.width, self.height);
        let sizes = &self.sizes;
        let levels = sizes.len();
        let identity = transform == Transform::IDENTITY;

        // The frame in the reference's geometry, and where it reached.
        // The picture is warped with the edge held rather than filled,
        // so that what hangs over the side is a continuation and not a
        // black cliff the pyramid would spread down every level; the
        // mask is what actually keeps it out of the merge.
        // Taken, and the Laplacian pyramid takes the warped frame as
        // its base rather than copying it: at 45 megapixels a copy is
        // half a gigabyte live beside the pyramid it is being copied
        // into. A frame that moved is let go once it is warped.
        // Lanczos-3, which keeps the detail a stack is for; its ring at
        // a hard edge is held to the frame's own range.
        let warped: Vec<f32> = if identity {
            frame
        } else {
            let mut warped = register::warp_samples(
                &frame,
                width,
                height,
                CHANNELS,
                &transform,
                Edge::Clamp,
                Kernel::Lanczos3,
            );
            register::clamp_samples(&mut warped, register::sample_range(&frame));
            drop(frame);
            warped
        };
        let mask = if identity {
            vec![1.0f32; width * height]
        } else {
            register::warp_plane(
                &self.ones,
                width,
                height,
                width,
                height,
                &transform,
                Edge::Fill(0.0),
                Kernel::Bilinear,
            )
        };
        // A frame covers a pixel when it contributes to it at all,
        // which is the same test its weight makes: the mask multiplies
        // the weight, so anything above zero is in the merge. The
        // bilinear ramp means the outermost row or column of a warped
        // frame counts as covered on a fraction of a sample.
        for (c, m) in self.coverage.iter_mut().zip(&mask) {
            if *m > 0.0 {
                *c += 1;
            }
        }

        // The same weighting the registration and the frame scores
        // used, so a camera-space stack is measured in camera-space
        // luminance throughout and a working-space one in the working
        // space's.
        let lum = luminance_of(&warped, luma);
        let log = register::log_plane(&lum, options.register.floor_stops);
        drop(lum);
        let map = sharpness::of_plane(&log, width, height, options.sharpness_radius);

        let mean = (map.iter().map(|&v| v as f64).sum::<f64>() / map.len().max(1) as f64) as f32;
        let floor = mean * options.weight_floor.max(0.0);
        let weight: Vec<f32> = map
            .par_iter()
            .zip(mask.par_iter())
            .map(|(&s, &m)| (s + floor) * m)
            .collect();
        drop(map);
        drop(mask);
        drop(log);

        let laplacian = laplacian_pyramid(warped, sizes, CHANNELS);
        // (`warped` is gone: it is the pyramid's base now.)
        let mut gaussian = gaussian_pyramid(weight, sizes);
        // The stack's word on this frame, at the levels where the
        // frame's own fine energy is the noise's (see `prior`).
        if let Some((prior, multiplier)) = prior {
            for (k, level) in gaussian
                .iter_mut()
                .enumerate()
                .take(levels - 1)
                .take(prior::BANDS)
            {
                prior.apply(multiplier, level, sizes[k].0, k);
            }
        }
        if let Some((spill, damping)) = spill {
            for (k, level) in gaussian.iter_mut().enumerate() {
                spill.apply(damping, level, sizes[k].0, k);
            }
        }

        for k in 0..levels {
            let (lw, lh) = sizes[k];
            // The top of the pyramid is not a band, it is the
            // picture's own low frequency: nothing to select on, so it
            // is the plain weighted average and the merge keeps its
            // brightness.
            let level_weight: Vec<f32> = if k + 1 == levels || options.selectivity <= 0.0 {
                gaussian[k].clone()
            } else {
                let magnitude: Vec<f32> = laplacian[k]
                    .par_chunks(CHANNELS)
                    .map(|px| px.iter().map(|v| v.abs()).sum::<f32>() / CHANNELS as f32)
                    .collect();
                let contrast = sharpness::box_blur(&magnitude, lw, lh, options.band_radius);
                let floor = options.band_floor;
                // Squaring is the default and `powf` is not cheap over
                // a pyramid a frame.
                let square = options.selectivity == 2.0;
                let exponent = options.selectivity;
                gaussian[k]
                    .par_iter()
                    .zip(contrast.par_iter())
                    .map(|(&g, &c)| {
                        let t = c + floor;
                        g * if square { t * t } else { t.powf(exponent) }
                    })
                    .collect()
            };
            self.bands[k]
                .par_chunks_mut(CHANNELS)
                .zip(laplacian[k].par_chunks(CHANNELS))
                .zip(level_weight.par_iter())
                .for_each(|((acc, band), &w)| {
                    for (a, b) in acc.iter_mut().zip(band) {
                        *a += w * b;
                    }
                });
            self.weights[k]
                .par_iter_mut()
                .zip(level_weight.par_iter())
                .for_each(|(s, &w)| *s += w);
            self.plain[k]
                .par_iter_mut()
                .zip(laplacian[k].par_iter())
                .for_each(|(a, b)| *a += b);
        }
        self.merged += 1;
    }

    /// The merged samples and the coverage count.
    ///
    /// Each band normalized by the weight that went into it, then
    /// collapsed from the top. Where no weight went into it at all —
    /// every frame flat there — the plain mean of the frames' own bands
    /// stands in, which is what a merge with nothing to choose between
    /// should give and what dividing by nothing would not.
    fn finish(self) -> (Vec<f32>, Vec<u16>) {
        let Self {
            sizes,
            mut bands,
            weights,
            plain,
            merged,
            coverage,
            ones,
            ..
        } = self;
        drop(ones);
        let levels = sizes.len();
        let share = 1.0 / merged.max(1) as f32;
        for k in 0..levels {
            bands[k]
                .par_chunks_mut(CHANNELS)
                .zip(weights[k].par_iter())
                .zip(plain[k].par_chunks(CHANNELS))
                .for_each(|((px, &w), flat)| {
                    // Below the smallest normal float the ratio is not
                    // a ratio any more: the numerator has underflowed
                    // with the denominator.
                    if w > f32::MIN_POSITIVE {
                        let inv = 1.0 / w;
                        for v in px.iter_mut() {
                            *v *= inv;
                        }
                    } else {
                        for (v, f) in px.iter_mut().zip(flat) {
                            *v = f * share;
                        }
                    }
                });
        }
        drop(plain);
        drop(weights);
        let mut out = bands.pop().expect("a pyramid has a top");
        for k in (0..levels - 1).rev() {
            let (w, h) = sizes[k];
            let (cw, ch) = sizes[k + 1];
            let up = expand(&out, cw, ch, w, h, CHANNELS);
            out = bands.pop().expect("one band a level");
            out.par_iter_mut()
                .zip(up.par_iter())
                .for_each(|(o, u)| *o += u);
        }
        (out, coverage)
    }
}

/// Every frame's link to its neighbor, chained outward from the
/// reference, and those links composed onto the reference.
struct Chain {
    against: Vec<Option<usize>>,
    fits: Vec<Option<Fit>>,
    onto: Vec<Transform>,
    dropped: Vec<Option<Dropped>>,
}

/// The chain over `n` frames, each frame's luminance asked of `lum` as
/// the chain reaches it: up from the reference, then down. Two planes
/// are held at a time beside the reference's, which both directions
/// start from: the frame the chain has reached and the one being
/// fitted to it.
#[allow(clippy::too_many_arguments)]
fn fit_chain<E, T>(
    n: usize,
    width: usize,
    height: usize,
    reference: usize,
    reference_lum: &[f32],
    options: &Options,
    mut lum: impl FnMut(usize) -> std::result::Result<(Vec<f32>, T), E>,
    mut placed: impl FnMut(T, &Transform),
) -> std::result::Result<Chain, E> {
    let mut chain = Chain {
        against: vec![None; n],
        fits: vec![None; n],
        onto: vec![Transform::IDENTITY; n],
        dropped: vec![None; n],
    };
    let limit = options.max_motion * (width as f64).hypot(height as f64);
    for order in chain_orders(n, reference) {
        // The last frame this way whose link held, and the link that
        // got there: one focus step looks much like the next, so the
        // previous answer is most of the next one's. No plane of its
        // own while it is the reference.
        let mut anchor = reference;
        let mut anchor_lum: Option<Vec<f32>> = None;
        let mut seed = Transform::IDENTITY;
        for i in order {
            chain.against[i] = Some(anchor);
            let (moving, extra) = lum(i)?;
            match register::fit_from(
                anchor_lum.as_deref().unwrap_or(reference_lum),
                &moving,
                width,
                height,
                &options.register,
                &seed,
            ) {
                Ok(fit) => {
                    let moved = fit
                        .transform
                        .corner_distance(&Transform::IDENTITY, width, height);
                    chain.fits[i] = Some(fit);
                    if fit.residual > options.max_residual {
                        chain.dropped[i] = Some(Dropped::NotAligned {
                            residual: fit.residual,
                            overlap: fit.overlap,
                            threshold: options.max_residual,
                        });
                    } else if moved > limit {
                        chain.dropped[i] = Some(Dropped::TooFar {
                            pixels: moved,
                            limit,
                        });
                    } else {
                        chain.onto[i] = fit.transform.compose(&chain.onto[anchor]);
                        placed(extra, &chain.onto[i]);
                        seed = fit.transform;
                        anchor = i;
                        anchor_lum = Some(moving);
                    }
                }
                Err(e) => chain.dropped[i] = Some(Dropped::NoFit(e.to_string())),
            }
        }
    }
    Ok(chain)
}

/// The order the chain reaches the frames of `n` from `reference` in:
/// up from it, then down.
fn chain_orders(n: usize, reference: usize) -> [Vec<usize>; 2] {
    [(reference + 1..n).collect(), (0..reference).rev().collect()]
}

/// The level sizes, full resolution first.
fn pyramid_sizes(width: usize, height: usize, options: &Options) -> Vec<(usize, usize)> {
    let mut sizes = vec![(width, height)];
    while sizes.len() < options.max_levels.max(1) {
        let (w, h) = *sizes.last().expect("a pyramid has a base");
        let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
        if nw.min(nh) < options.min_side.max(3) || (nw, nh) == (w, h) {
            break;
        }
        sizes.push((nw, nh));
    }
    sizes
}

/// Gaussian pyramid of a one-channel plane; the base is taken, not
/// copied.
fn gaussian_pyramid(base: Vec<f32>, sizes: &[(usize, usize)]) -> Vec<Vec<f32>> {
    let mut levels = vec![base];
    for k in 1..sizes.len() {
        let (w, h) = sizes[k - 1];
        levels.push(reduce(levels.last().expect("a base"), w, h, 1));
    }
    levels
}

/// Laplacian pyramid of an interleaved image: each level the
/// difference between that Gaussian level and the expansion of the
/// next, and the top the Gaussian top itself. The base is taken, not
/// copied.
fn laplacian_pyramid(base: Vec<f32>, sizes: &[(usize, usize)], channels: usize) -> Vec<Vec<f32>> {
    let mut gaussian: Vec<Vec<f32>> = Vec::with_capacity(sizes.len());
    gaussian.push(base);
    for k in 1..sizes.len() {
        let (w, h) = sizes[k - 1];
        gaussian.push(reduce(gaussian.last().expect("a base"), w, h, channels));
    }
    let top = gaussian.len() - 1;
    for k in 0..top {
        let (w, h) = sizes[k];
        let (cw, ch) = sizes[k + 1];
        let up = expand(&gaussian[k + 1], cw, ch, w, h, channels);
        gaussian[k]
            .par_iter_mut()
            .zip(up.par_iter())
            .for_each(|(g, u)| *g -= u);
    }
    gaussian
}

/// Blur by the binomial `[1 4 6 4 1]/16` and keep every other sample,
/// separably, the taps clamped at the border. The kernel is the
/// registration pyramid's; this one carries any number of interleaved
/// channels.
pub(crate) fn reduce(src: &[f32], width: usize, height: usize, channels: usize) -> Vec<f32> {
    const TAPS: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let ow = width.div_ceil(2);
    let oh = height.div_ceil(2);
    let mut rows = vec![0.0f32; ow * height * channels];
    rows.par_chunks_mut(ow * channels)
        .zip(src.par_chunks(width * channels))
        .for_each(|(out, row)| {
            for x in 0..ow {
                let c = 2 * x;
                for (k, tap) in TAPS.iter().enumerate() {
                    let i = (c + k).saturating_sub(2).min(width - 1);
                    for p in 0..channels {
                        out[x * channels + p] += tap * row[i * channels + p];
                    }
                }
            }
        });
    let mut out = vec![0.0f32; ow * oh * channels];
    out.par_chunks_mut(ow * channels)
        .enumerate()
        .for_each(|(y, line)| {
            let c = 2 * y;
            for (k, tap) in TAPS.iter().enumerate() {
                let j = (c + k).saturating_sub(2).min(height - 1);
                let row = &rows[j * ow * channels..(j + 1) * ow * channels];
                for (o, s) in line.iter_mut().zip(row) {
                    *o += tap * s;
                }
            }
        });
    out
}

/// The matching expansion: samples doubled and filled in with the same
/// binomial, to exactly the parent's size, so that a level minus the
/// expansion of the one above it and back again is itself.
pub(crate) fn expand(
    src: &[f32],
    width: usize,
    height: usize,
    out_width: usize,
    out_height: usize,
    channels: usize,
) -> Vec<f32> {
    let mut rows = vec![0.0f32; out_width * height * channels];
    rows.par_chunks_mut(out_width * channels)
        .zip(src.par_chunks(width * channels))
        .for_each(|(out, row)| {
            for x in 0..out_width {
                let c = x / 2;
                let at = |i: usize| {
                    let i = i.min(width - 1) * channels;
                    &row[i..i + channels]
                };
                if x % 2 == 0 {
                    let (a, b, d) = (at(c.saturating_sub(1)), at(c), at(c + 1));
                    for p in 0..channels {
                        out[x * channels + p] = (a[p] + 6.0 * b[p] + d[p]) / 8.0;
                    }
                } else {
                    let (a, b) = (at(c), at(c + 1));
                    for p in 0..channels {
                        out[x * channels + p] = 0.5 * (a[p] + b[p]);
                    }
                }
            }
        });
    let mut out = vec![0.0f32; out_width * out_height * channels];
    let span = out_width * channels;
    out.par_chunks_mut(span).enumerate().for_each(|(y, line)| {
        let c = y / 2;
        let at = |j: usize| {
            let j = j.min(height - 1) * span;
            &rows[j..j + span]
        };
        if y % 2 == 0 {
            let (a, b, d) = (at(c.saturating_sub(1)), at(c), at(c + 1));
            for i in 0..span {
                line[i] = (a[i] + 6.0 * b[i] + d[i]) / 8.0;
            }
        } else {
            let (a, b) = (at(c), at(c + 1));
            for i in 0..span {
                line[i] = 0.5 * (a[i] + b[i]);
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spill gathered late, from the first frame at its clip with
    /// those before it read again, is the spill gathered from the first
    /// frame, wherever in the order the clipped frame comes, and so are
    /// the prior's skips; with no frame at its clip, nothing is heard.
    #[test]
    fn the_spill_gathered_late_is_the_spill_gathered_from_the_first() {
        let scene = halo_bed::Scene::new(halo_bed::Object::Bright, 9);
        let (w, h) = (scene.width, scene.height);
        let options = Options::default();
        let listen = Listen {
            prior: true,
            spill: true,
        };
        let lums: Vec<Vec<f32>> = scene
            .frames()
            .iter()
            .map(|f| luminance_of(&f.data, WORKING_LUMA))
            .collect();
        let gather = |lums: &[Vec<f32>], eager: bool| {
            let mut g = Gathering::new(w, h, listen);
            if eager {
                g.wake();
            }
            let mut blocks = Vec::new();
            for (k, lum) in lums.iter().enumerate() {
                let (_, taken) = measure(lum, w, h, &options, listen, g.hearing());
                if taken.clips {
                    for (j, onto) in g.wake() {
                        g.again(&coarse_of(&lums[j], w, h, &options), &onto);
                    }
                }
                let only = Listen {
                    prior: true,
                    spill: false,
                };
                blocks.push(measure(lum, w, h, &options, only, false).1.blocks);
                g.add(k, taken, &Transform::IDENTITY);
            }
            let heard = g.finish(lums.len());
            let prior = heard.prior.expect("a prior");
            let multipliers: Vec<Vec<u32>> = blocks
                .iter()
                .map(|b| {
                    let m = prior.multiplier(b.as_deref().expect("blocks"), &Transform::IDENTITY);
                    m.iter().map(|v| v.to_bits()).collect()
                })
                .collect();
            (heard.spill, multipliers)
        };
        let (none, _) = gather(&lums, false);
        assert!(none.is_none());
        for late in [0, 4, 8] {
            // Frame `late` alone clipped, under its disc's level.
            let mut lums = lums.clone();
            let top = lums[late].iter().copied().fold(0.0f32, f32::max);
            for v in &mut lums[late] {
                *v = v.min(0.7 * top);
            }
            let (lazy, lazy_prior) = gather(&lums, false);
            let (eager, eager_prior) = gather(&lums, true);
            assert!(lazy.is_some(), "{late}");
            assert!(lazy == eager, "{late}");
            assert!(lazy_prior == eager_prior, "{late}");
        }
    }

    /// A lattice value at `(ix, iy)`, 0 to 1, from the coordinates
    /// alone, so every run sees the same picture.
    fn lattice(ix: i64, iy: i64) -> f64 {
        let mut h = (ix as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
        h ^= h >> 29;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 32;
        (h >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Value noise on a lattice of `cell` pixels, smoothstepped.
    fn octave(x: f64, y: f64, cell: f64) -> f64 {
        let (gx, gy) = (x / cell, y / cell);
        let (i, j) = (gx.floor(), gy.floor());
        let (fx, fy) = (gx - i, gy - j);
        let ease = |t: f64| t * t * (3.0 - 2.0 * t);
        let (sx, sy) = (ease(fx), ease(fy));
        let (i, j) = (i as i64, j as i64);
        let a = lattice(i, j) + sx * (lattice(i + 1, j) - lattice(i, j));
        let b = lattice(i, j + 1) + sx * (lattice(i + 1, j + 1) - lattice(i, j + 1));
        a + sy * (b - a)
    }

    const CELLS: [f64; 5] = [160.0, 80.0, 40.0, 20.0, 10.0];

    /// A texture with structure at five scales, defocused by `sigma`.
    ///
    /// The defocus is done in the frequency domain rather than by
    /// convolution: each octave is a band around one spatial period,
    /// so a Gaussian of deviation `sigma` multiplies it by that
    /// Gaussian's own response there. Two properties earn it. The
    /// coarse octaves come through untouched whatever the blur, which
    /// is what lets a test compare two frames' low frequencies
    /// exactly. And the texture is defined at a point, so a frame can
    /// be taken through any transform without a second resampling.
    fn texture(x: f64, y: f64, sigma: f64) -> f32 {
        let mut v = 0.6;
        let mut amplitude = 0.35;
        for cell in CELLS {
            let response =
                (-2.0 * std::f64::consts::PI.powi(2) * sigma * sigma / (cell * cell)).exp();
            v += amplitude * response * (octave(x, y, cell) - 0.5);
            amplitude *= 0.85;
        }
        v as f32
    }

    /// A gray RGB frame, one sample a pixel taken from `f`.
    fn frame_of(width: usize, height: usize, f: impl Fn(f64, f64) -> f32 + Sync) -> WorkingImage {
        let data: Vec<f32> = (0..width * height)
            .into_par_iter()
            .flat_map_iter(|k| {
                let v = f((k % width) as f64, (k / width) as f64);
                [v, v, v]
            })
            .collect();
        WorkingImage::from_data(width, height, data).expect("a frame")
    }

    fn middle(width: usize, height: usize) -> [f64; 2] {
        [(width - 1) as f64 / 2.0, (height - 1) as f64 / 2.0]
    }

    /// The green channel of an image as a plane.
    fn plane(image: &WorkingImage) -> Vec<f32> {
        image.data.chunks(3).map(|px| px[1]).collect()
    }

    /// Mean Laplacian energy of the log luminance over a box: the
    /// measure the module reports, restricted so a border the warps
    /// did not reach cannot decide it.
    fn energy_over(image: &WorkingImage, x0: usize, x1: usize, margin: usize) -> f32 {
        let (w, h) = (image.width, image.height);
        let log = register::log_plane(&plane(image), 14.0);
        let energy = sharpness::laplacian_energy(&log, w, h);
        let mut sum = 0.0f64;
        let mut n = 0u64;
        for y in margin..h - margin {
            for x in x0..x1 {
                sum += energy[y * w + x] as f64;
                n += 1;
            }
        }
        (sum / n as f64).sqrt() as f32
    }

    fn sharpness(image: &WorkingImage, margin: usize) -> f32 {
        energy_over(image, margin, image.width - margin, margin)
    }

    /// How far an image is from the truth, over the interior.
    fn error_against(image: &WorkingImage, truth: &WorkingImage, margin: usize) -> f32 {
        let (w, h) = (image.width, image.height);
        let mut sum = 0.0f64;
        let mut n = 0u64;
        for y in margin..h - margin {
            for x in margin..w - margin {
                let d = (image.pixel(x, y)[1] - truth.pixel(x, y)[1]) as f64;
                sum += d * d;
                n += 1;
            }
        }
        (sum / n as f64).sqrt() as f32
    }

    /// A stack of `n` frames of a scene whose depth runs across `x`:
    /// frame `k` is sharp in its own band and blurred either side of
    /// it, and each frame sees the scene through a small similarity,
    /// as focus breathing and a nudged tripod leave it.
    ///
    /// Returns the frames and, in the middle frame's geometry, the
    /// picture that is sharp everywhere: what the merge is after.
    fn depth_stack(width: usize, height: usize, n: usize) -> (Vec<WorkingImage>, WorkingImage) {
        let reference = n / 2;
        let bands = n as f64;
        let frames = (0..n)
            .map(|k| {
                let step = k as f64 - reference as f64;
                let scene = Transform::similarity(
                    1.0 + 0.004 * step,
                    (0.15 * step).to_radians(),
                    middle(width, height),
                    [1.5 * step, -step],
                );
                // The middle of this frame's sharp band, in scene x,
                // and how fast the defocus grows either side of it: a
                // tenth of the frame's width per unit of deviation, so
                // that the stack is the same stack at any size.
                let focus = (k as f64 + 0.5) / bands * width as f64;
                let ramp = width as f64 / 10.0;
                frame_of(width, height, move |x, y| {
                    let (u, v) = scene.apply(x, y);
                    texture(u, v, (u - focus).abs() / ramp)
                })
            })
            .collect();
        let truth = frame_of(width, height, |x, y| texture(x, y, 0.0));
        (frames, truth)
    }

    #[test]
    fn a_depth_stack_comes_out_sharper_than_any_frame_of_it() {
        let (width, height) = (400usize, 300);
        let (frames, truth) = depth_stack(width, height, 5);
        let (merged, report) = stack(&frames, &Options::default()).expect("a stack");
        assert_eq!(report.reference, 2);
        assert_eq!(report.used(), 5, "{:#?}", report.frames);
        assert!(report.frames.iter().all(|f| f.dropped.is_none()));

        // Every link came in well under the gate, and every frame's
        // magnification comes back in the right direction and within
        // 0.004 of the truth. Not to a part in a thousand: the frames
        // are built 0.4% apart and the fit recovers three fifths to
        // three quarters of that, the defocus pulling on the rest.
        for f in &report.frames {
            if let Some(fit) = f.fit {
                assert!(fit.residual < 0.4, "frame {}: {:?}", f.index, fit);
            }
            let truth = 1.0 / (1.0 + 0.004 * (f.index as f64 - 2.0));
            let got = f.transform.scale();
            println!(
                "frame {} against {:?}: link residual {:.4}, scale onto the reference \
                 {got:.5} against {truth:.5}",
                f.index,
                f.against,
                f.fit.map(|x| x.residual).unwrap_or(0.0),
            );
            assert!(
                (got - truth).abs() < 0.004,
                "frame {}: scale {got} against {truth}",
                f.index
            );
        }

        let margin = 24;
        let merged_sharpness = sharpness(&merged, margin);
        let each: Vec<f32> = frames.iter().map(|f| sharpness(f, margin)).collect();
        let sharpest_frame = each.iter().copied().fold(0.0f32, f32::max);
        let truth_sharpness = sharpness(&truth, margin);
        println!(
            "sharpness: frames {each:?}, merged {merged_sharpness:.4}, \
             all-sharp truth {truth_sharpness:.4}"
        );
        // Sharper than any frame that went in, by the measure the
        // module itself reports, and within reach of a frame that was
        // never defocused at all.
        assert!(
            merged_sharpness > sharpest_frame * 1.25,
            "merged {merged_sharpness} against the sharpest frame {sharpest_frame}"
        );
        assert!(
            merged_sharpness > truth_sharpness * 0.8,
            "merged {merged_sharpness} against an all-sharp frame's {truth_sharpness}"
        );

        // And nearer the all-sharp picture than any frame is.
        let merged_error = error_against(&merged, &truth, margin);
        let errors: Vec<f32> = frames
            .iter()
            .map(|f| error_against(f, &truth, margin))
            .collect();
        let closest = errors.iter().copied().fold(f32::MAX, f32::min);
        println!("error against the truth: frames {errors:?}, merged {merged_error:.5}");
        assert!(
            merged_error < closest * 0.6,
            "merged {merged_error} against the nearest frame's {closest}"
        );

        // Sharper everywhere, not only on average. Two claims, and
        // the second is the one that means "everywhere": in each
        // frame's own band the merge comes within a tenth of that
        // frame, and the merge's *worst* band beats every frame's best
        // band but one — which is to say there is nowhere the merge is
        // as soft as every frame is somewhere.
        let mut merged_worst = f32::MAX;
        let mut frame_bests: Vec<f32> = vec![0.0; frames.len()];
        for k in 0..5 {
            let x0 = (k * width / 5).max(margin);
            let x1 = ((k + 1) * width / 5).min(width - margin);
            let here = energy_over(&merged, x0, x1, margin);
            merged_worst = merged_worst.min(here);
            let mut best_here = 0.0f32;
            for (i, f) in frames.iter().enumerate() {
                let there = energy_over(f, x0, x1, margin);
                frame_bests[i] = frame_bests[i].max(there);
                best_here = best_here.max(there);
            }
            println!("band {k}: merged {here:.4}, best frame there {best_here:.4}");
            assert!(
                here > best_here * 0.85,
                "band {k}: merged {here} against the best frame's {best_here}"
            );
        }
        let frame_worsts: Vec<f32> = frames
            .iter()
            .map(|f| {
                (0..5)
                    .map(|k| {
                        let x0 = (k * width / 5).max(margin);
                        let x1 = ((k + 1) * width / 5).min(width - margin);
                        energy_over(f, x0, x1, margin)
                    })
                    .fold(f32::MAX, f32::min)
            })
            .collect();
        let best_worst = frame_worsts.iter().copied().fold(0.0f32, f32::max);
        println!(
            "the merge's softest band is {merged_worst:.4}; the frames' softest are \
             {frame_worsts:?}"
        );
        assert!(
            merged_worst > best_worst * 1.5,
            "the merge's softest band {merged_worst} against the frames' best softest \
             {best_worst}"
        );
    }

    /// A stack of sharp frames each seen through a known similarity, as
    /// a hand steps a focus: each frame's similarity comes back within a
    /// quarter of a pixel at the corners (measured: 0.03 and 0.11, the
    /// stack's fit as it was; the kernel does not enter it), and the
    /// frames resampled by Lanczos-3 merge back to the scene.
    #[test]
    fn a_stacks_known_similarity_is_recovered() {
        let (width, height) = (400usize, 300);
        let scenes: Vec<Transform> = (0..3)
            .map(|k| {
                let step = k as f64 - 1.0;
                Transform::similarity(
                    1.0 + 0.002 * step,
                    (0.5 * step).to_radians(),
                    middle(width, height),
                    [2.0 * step, -1.5 * step],
                )
            })
            .collect();
        let frames: Vec<WorkingImage> = scenes
            .iter()
            .map(|scene| {
                let scene = *scene;
                frame_of(width, height, move |x, y| {
                    let (u, v) = scene.apply(x, y);
                    texture(u, v, 0.0)
                })
            })
            .collect();
        let (merged, report) = stack(&frames, &Options::default()).expect("a stack");
        assert_eq!((report.reference, report.used()), (1, 3));
        for f in &report.frames {
            let truth = scenes[f.index].inverse().unwrap();
            let off = f.transform.corner_distance(&truth, width, height);
            println!("frame {}: {off:.3} px at the corners", f.index);
            assert!(off < 0.25, "frame {}: {off} px", f.index);
        }
        let truth = frame_of(width, height, |x, y| texture(x, y, 0.0));
        let error = error_against(&merged, &truth, 24);
        println!("a sharp stack merged back to {error:.5} of the scene");
        assert!(error < 0.01, "{error}");
    }

    /// Two frames, each sharp in one half, and one of them a twentieth
    /// brighter than the other. A merge that switched frames at the
    /// boundary would leave that twentieth as a step; the pyramid has
    /// to spread it over a distance.
    #[test]
    fn a_band_boundary_leaves_no_step() {
        let (width, height) = (400usize, 300);
        const OFFSET: f32 = 0.05;
        const BLUR: f64 = 3.0;
        let edge = width / 2;
        let left = frame_of(width, height, move |x, y| {
            texture(x, y, if (x as usize) < edge { 0.0 } else { BLUR })
        });
        let right = frame_of(width, height, move |x, y| {
            texture(x, y, if (x as usize) < edge { BLUR } else { 0.0 }) + OFFSET
        });
        let (merged, report) = stack(&[left.clone(), right], &Options::default()).expect("a stack");
        assert_eq!(report.used(), 2);

        // What is left of `merged - left` once the columns are
        // averaged is the offset: 0 where the left frame won, the
        // whole offset where the right frame did. Averaged over the
        // columns and no further, it still carries a couple of
        // hundredths of the two frames' own difference in texture —
        // the fine octaves the defocus attenuated do not cancel and do
        // not average away over three hundred rows — so the profile is
        // smoothed over thirteen pixels before its slope is taken. The
        // crossing is tens of pixels wide, so the smoothing does not
        // reach it.
        let raw: Vec<f32> = (0..width)
            .map(|x| {
                let mut sum = 0.0f64;
                for y in 0..height {
                    sum += (merged.pixel(x, y)[1] - left.pixel(x, y)[1]) as f64;
                }
                (sum / height as f64) as f32
            })
            .collect();
        let profile: Vec<f32> = (0..width)
            .map(|x| {
                let lo = x.saturating_sub(6);
                let hi = (x + 7).min(width);
                raw[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
            })
            .collect();
        let mean = |r: std::ops::Range<usize>| {
            let n = r.len();
            profile[r].iter().sum::<f32>() / n as f32
        };
        let step = profile[40..width - 40]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        let low = mean(40..edge - 60);
        let high = mean(edge + 60..width - 40);
        println!(
            "seam: the offset crosses from {low:.4} to {high:.4}, \
             the steepest pixel {step:.5} ({:.0} px to cross)",
            OFFSET / step
        );
        // Both ends are reached, so the blend really did switch
        // frames. The right half does not reach the whole offset and
        // should not: the offset is a constant, a constant lives only
        // in the top of the pyramid, and at the top of the pyramid the
        // two frames are equally sharp and are averaged, which is the
        // right answer for every part of a focus stack that is not
        // about detail. Placed exactly where they are, the right half
        // comes to 0.36 of the offset. As the chain places them it is
        // 0.57: the gradient fit's placing, turned a third of a degree
        // by the two halves' difference in blur, since the two frames
        // are a whole step of defocus apart and the phase's refinement
        // is turned away on them. The bound below is the chain's as it
        // is: a placing that took the turn out would come to 0.36 and
        // fail it, and the bound should then come down to 0.25.
        assert!(
            low < 0.15 * OFFSET,
            "the left frame did not win its half: {low}"
        );
        assert!(
            high > 0.4 * OFFSET,
            "the right frame did not win its half: {high}"
        );
        // And what crossing there is takes tens of pixels, not one.
        assert!(
            step < OFFSET / 12.0,
            "the offset crosses in {:.0} px, which is a step",
            OFFSET / step
        );
        // Nothing overshoots either side of the two levels.
        let lowest = profile[40..width - 40]
            .iter()
            .copied()
            .fold(f32::MAX, f32::min);
        let highest = profile[40..width - 40]
            .iter()
            .copied()
            .fold(f32::MIN, f32::max);
        assert!(
            lowest > -0.15 * OFFSET && highest < 1.15 * OFFSET,
            "the crossing overshoots: {lowest} to {highest}"
        );
    }

    /// A frame of another scene cannot be registered, and is said so
    /// and left out rather than merged into the picture wrong.
    #[test]
    fn a_frame_that_will_not_register_is_dropped_and_reported() {
        let (width, height) = (400usize, 300);
        let (mut frames, _) = depth_stack(width, height, 3);
        // Another picture entirely: the texture read a long way off,
        // which shares its statistics and none of its structure.
        let stranger = frame_of(width, height, |x, y| texture(x + 9000.0, y + 7000.0, 0.0));
        frames.push(stranger);

        let options = Options {
            reference: Reference::Index(1),
            ..Default::default()
        };
        let (merged, report) = stack(&frames, &options).expect("a stack");
        assert_eq!(report.reference, 1);
        assert_eq!(report.used(), 3, "{:#?}", report.frames);
        let dropped = &report.frames[3];
        assert!(dropped.dropped.is_some(), "{dropped:?}");
        println!("dropped: {}", dropped.dropped.as_ref().expect("a reason"));
        assert!(report.frames[..3].iter().all(|f| f.dropped.is_none()));

        // And the picture is the one the three good frames make: the
        // stranger did not get in.
        let (without, _) = stack(&frames[..3], &options).expect("a stack of the three");
        assert_eq!(merged.data, without.data);
    }

    /// One frame in is that frame out: the weights cancel, and the
    /// pyramid taken apart and put back together is itself.
    #[test]
    fn one_frame_in_is_that_frame_out() {
        let (width, height) = (301usize, 197);
        let one = frame_of(width, height, |x, y| texture(x, y, 0.0));
        let (merged, report) =
            stack(std::slice::from_ref(&one), &Options::default()).expect("a stack of one");
        assert_eq!(report.reference, 0);
        assert_eq!(report.used(), 1);
        assert!(report.frames[0].fit.is_none());
        assert!(report.coverage.iter().all(|&c| c == 1));
        assert_eq!(report.full_coverage_fraction(), 1.0);
        let worst = merged
            .data
            .iter()
            .zip(&one.data)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        println!("one frame in, {worst:.3e} out; {} levels", report.levels);
        assert!(worst < 1e-5, "the pyramid did not round trip: {worst}");
    }

    /// The halo §71 names: a sharp near edge over a blurred far
    /// background. The frames focused far carry the near object's
    /// out-of-focus disc spread over the background, and a merge that
    /// is not careful brings it in beside the silhouette.
    ///
    /// Returns the frames, the picture with no halo in it, and where
    /// the bar's right edge is.
    fn depth_edge(
        width: usize,
        height: usize,
        sigma: f64,
    ) -> (Vec<WorkingImage>, WorkingImage, usize) {
        let (x0, x1) = (width / 3, 2 * width / 3);
        let bar = move |x: f64| {
            if x >= x0 as f64 && x < x1 as f64 {
                1.0
            } else {
                0.0
            }
        };
        // The background, and a foreground half again as bright.
        let back = |x: f64, y: f64, s: f64| texture(x, y, s) * 0.4;
        let front = |x: f64, y: f64, s: f64| texture(x + 3000.0, y, s) * 1.1;
        // Abramowitz and Stegun 7.1.26: enough of an error function
        // for a test fixture.
        let erf = |t: f64| {
            let s = t.signum();
            let t = t.abs();
            let u = 1.0 / (1.0 + 0.327_591_1 * t);
            let y = 1.0
                - (((((1.061_405_429 * u - 1.453_152_027) * u) + 1.421_413_741) * u
                    - 0.284_496_736)
                    * u
                    + 0.254_829_592)
                    * u
                    * (-t * t).exp();
            s * y
        };
        // A defocused bar's coverage at x: the Gaussian's integral
        // over it, which is the alpha its disc has there.
        let alpha = move |x: f64| {
            let n = |t: f64| 0.5 * (1.0 + erf(t / (sigma * std::f64::consts::SQRT_2)));
            (n(x1 as f64 - x) - n(x0 as f64 - x)).clamp(0.0, 1.0)
        };
        // Near focus: the foreground sharp and hard edged, the
        // background defocused behind it.
        let near = frame_of(width, height, move |x, y| {
            let b = bar(x) as f32;
            b * front(x, y, 0.0) + (1.0 - b) * back(x, y, sigma)
        });
        // Far focus: the background sharp, and the defocused
        // foreground laid over it with the coverage its disc has.
        let far = frame_of(width, height, move |x, y| {
            let a = alpha(x) as f32;
            a * front(x, y, sigma) + (1.0 - a) * back(x, y, 0.0)
        });
        let truth = frame_of(width, height, move |x, y| {
            let b = bar(x) as f32;
            b * front(x, y, 0.0) + (1.0 - b) * back(x, y, 0.0)
        });
        (vec![near, far], truth, x1)
    }

    /// The halo beside the silhouette, as a fraction of the step
    /// across it: the background's local mean in the thirty pixels
    /// beyond the bar, over its own level further out.
    ///
    /// The baseline is taken from the frame itself rather than
    /// assumed zero, because a defocused texture's column mean is not
    /// its sharp one's: a few percent of the step is in the fixture
    /// whatever the merge does, and it is not the halo.
    fn halo(merged: &WorkingImage, truth: &WorkingImage, edge: usize) -> f32 {
        let (width, height) = (merged.width, merged.height);
        let profile: Vec<f32> = (edge..width)
            .map(|x| {
                let mut sum = 0.0f64;
                for y in 8..height - 8 {
                    sum += (merged.pixel(x, y)[1] - truth.pixel(x, y)[1]) as f64;
                }
                (sum / (height - 16) as f64) as f32
            })
            .collect();
        let far = &profile[40..70];
        let baseline = far.iter().sum::<f32>() / far.len() as f32;
        // Past the first few pixels, where the bar's own hard edge is
        // resampled and every merge has the same trouble with it.
        let worst = profile[6..36]
            .iter()
            .map(|v| v - baseline)
            .fold(0.0f32, |a, b| if b.abs() > a.abs() { b } else { a });
        // The step across the silhouette, in the same units.
        let mean = |image: &WorkingImage, x0: usize, x1: usize| {
            let mut sum = 0.0f64;
            for y in 8..height - 8 {
                for x in x0..x1 {
                    sum += image.pixel(x, y)[1] as f64;
                }
            }
            sum / ((height - 16) * (x1 - x0)) as f64
        };
        let step = mean(truth, edge - 40, edge - 10) - mean(truth, edge + 40, edge + 70);
        (worst as f64 / step) as f32
    }

    /// The halo §71 names, and what the pyramid is worth against it:
    /// the same weights applied with no pyramid at all is the baseline.
    #[test]
    fn a_depth_edge_leaves_little_halo() {
        let (width, height) = (400usize, 300);
        let (frames, truth, edge) = depth_edge(width, height, 8.0);
        // Neither with the spill (`spill`), which takes some of the halo
        // whatever the pyramid: this is the pyramid's share.
        let flat = Options {
            max_levels: 1,
            spill: false,
            ..Default::default()
        };
        let (merged, report) = stack(&frames, &flat).expect("a stack with no pyramid");
        assert_eq!(report.used(), 2, "{:#?}", report.frames);
        let without = halo(&merged, &truth, edge);
        let pyramid = Options {
            spill: false,
            ..Default::default()
        };
        let (merged, report) = stack(&frames, &pyramid).expect("a stack");
        assert_eq!(report.used(), 2, "{:#?}", report.frames);
        let with = halo(&merged, &truth, edge);
        let (merged, _) = stack(&frames, &Options::default()).expect("a stack");
        let spilled = halo(&merged, &truth, edge);
        println!(
            "halo beyond the silhouette: {:+.1}% of the step with no pyramid, \
             {:+.1}% with {} levels, {:+.1}% with the spill held down too",
            without * 100.0,
            with * 100.0,
            report.levels,
            spilled * 100.0
        );
        assert!(
            without > 0.05,
            "the fixture has no halo to take out: {without}"
        );
        // The fit reads the near frame's growing discs as a scale of
        // about 2% and warps it; under Lanczos-3 that warp no longer
        // softens the no-pyramid baseline into a larger halo (13% under
        // bilinear, 9% now), so the pyramid's share of it is measured
        // against a smaller one: 4.3% of 9.1%.
        assert!(
            with.abs() < 0.05 && with < without * 0.5,
            "the blend leaves a halo of {:.1}% of the step, against {:.1}% with no \
             pyramid",
            with * 100.0,
            without * 100.0
        );
        assert!(
            spilled.abs() <= with.abs() + 0.002,
            "the spill held down leaves a halo of {:.1}% of the step, against {:.1}%",
            spilled * 100.0,
            with * 100.0
        );
    }

    #[test]
    fn frames_of_two_sizes_are_refused() {
        let a = frame_of(16, 16, |x, y| texture(x, y, 0.0));
        let b = frame_of(16, 15, |x, y| texture(x, y, 0.0));
        let e = stack(&[a, b], &Options::default()).expect_err("two sizes");
        assert!(matches!(e, Error::Unsupported(_)), "{e}");
        let e = stack(&[], &Options::default()).expect_err("no frames");
        assert!(matches!(e, Error::Unsupported(_)), "{e}");
    }

    /// A region that is flat in every frame has no sharpness, and the
    /// weight floor is a fraction of a frame's own mean sharpness, so
    /// there is nothing to weigh the frames by there. The merge has to
    /// fall back to their plain mean rather than divide by nothing.
    ///
    /// The value matters. `log2(0.25)` is exactly -2, so the Laplacian
    /// of the log is exactly zero and so is every weight; a value
    /// whose log2 is not exact leaves a few ulps of energy behind and
    /// hides the fault. The size matters too: at 400x300 the pyramid
    /// is four levels deep, where a merge that came out black would
    /// come out black at every one of them.
    #[test]
    fn a_region_flat_in_every_frame_is_their_mean_and_not_black() {
        let (width, height) = (400usize, 300);
        let one = frame_of(width, height, |_, _| 0.25);
        let (merged, report) = stack(&[one.clone(), one.clone()], &Options::default())
            .expect("a stack of two flat frames");
        assert!(report.levels >= 4, "{} levels", report.levels);
        let worst = merged
            .data
            .iter()
            .map(|v| (v - 0.25).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "a flat frame came back {worst} off");

        // And with the floor turned off entirely, which is a setting
        // the options offer: half the picture textured, half of it the
        // same exact flat value.
        let edge = width / 2;
        let half = |sigma: f64| {
            frame_of(width, height, move |x, y| {
                if (x as usize) < edge {
                    texture(x, y, sigma)
                } else {
                    0.25
                }
            })
        };
        let options = Options {
            weight_floor: 0.0,
            ..Default::default()
        };
        let (merged, _) = stack(&[half(0.0), half(3.0)], &options).expect("a stack");
        let mut worst = 0.0f32;
        for y in 8..height - 8 {
            for x in edge + 8..width - 8 {
                worst = worst.max((merged.pixel(x, y)[1] - 0.25).abs());
            }
        }
        // Not zero: the pyramid carries the textured half's low
        // frequencies a little way across the boundary, which is the
        // blend doing its job. A tenth of a percent of the value, as
        // against the whole of it that dividing by nothing would cost.
        println!("the flat half is {worst:.5} off its own value with no weight floor");
        assert!(
            worst < 5e-3,
            "the flat half came back {worst} off with no weight floor"
        );
    }

    /// The camera-space entry measures camera luminance everywhere it
    /// measures luminance, which is what its own doc promises.
    ///
    /// Two halves to it. Both weightings sum to one, so on a gray
    /// frame they are the same plane and the two entries must agree
    /// exactly. On a frame whose detail is all in the green — which is
    /// what an unbalanced mosaic's RGB looks like — they weigh that
    /// green differently, 0.678 against 0.5, and the scores must part.
    #[test]
    fn a_camera_stack_measures_camera_luminance() {
        let (width, height) = (200usize, 150);
        let gray: Vec<WorkingImage> = [0.0f64, 2.0]
            .iter()
            .map(|&sigma| frame_of(width, height, move |x, y| texture(x, y, sigma)))
            .collect();
        let as_camera: Vec<CameraImage> = gray
            .iter()
            .map(|f| CameraImage::from_data(width, height, f.data.clone()).expect("a frame"))
            .collect();
        let (working, wr) = stack(&gray, &Options::default()).expect("a working stack");
        let (camera, cr) = stack_camera(&as_camera, &Options::default()).expect("a camera stack");
        // Not bit-identical: the camera weights are powers of two and
        // sum to exactly one on a gray pixel, the working space's do
        // not, so the two planes differ by an ulp before anything else
        // happens. Everything after that is the same arithmetic.
        let worst = working
            .data
            .iter()
            .zip(&camera.data)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        println!(
            "gray frames: the two entries differ by {worst:.2e}, scores {:.6} and {:.6}",
            wr.frames[0].sharpness, cr.frames[0].sharpness
        );
        assert!(worst < 1e-5, "gray frames came out {worst} apart");
        assert!((wr.frames[0].sharpness - cr.frames[0].sharpness).abs() < 1e-5);

        // Detail in the green alone, red and blue flat.
        let green: Vec<CameraImage> = [0.0f64, 2.0]
            .iter()
            .map(|&sigma| {
                let data: Vec<f32> = (0..width * height)
                    .into_par_iter()
                    .flat_map_iter(|k| {
                        let v = texture((k % width) as f64, (k / width) as f64, sigma);
                        [0.30f32, v, 0.20]
                    })
                    .collect();
                CameraImage::from_data(width, height, data).expect("a frame")
            })
            .collect();
        let as_working: Vec<WorkingImage> = green
            .iter()
            .map(|f| WorkingImage::from_data(width, height, f.data.clone()).expect("a frame"))
            .collect();
        let (_, cr) = stack_camera(&green, &Options::default()).expect("a camera stack");
        let (_, wr) = stack(&as_working, &Options::default()).expect("a working stack");
        assert!(cr.frames[0].sharpness > 0.0 && wr.frames[0].sharpness > 0.0);
        println!(
            "green-only frames: camera score {:.6}, working score {:.6}",
            cr.frames[0].sharpness, wr.frames[0].sharpness
        );
        assert!(
            (cr.frames[0].sharpness - wr.frames[0].sharpness).abs() > 1e-4,
            "the two weightings gave the same score on a green-only frame: {} and {}",
            cr.frames[0].sharpness,
            wr.frames[0].sharpness
        );
    }

    /// A fit that finds the picture but says the camera moved further
    /// than a stack's camera moves is refused on the transform, not on
    /// the residual.
    #[test]
    fn a_frame_the_fit_puts_too_far_away_is_dropped() {
        let (width, height) = (400usize, 300);
        // A twentieth of the diagonal is 25 px; this is 40, and the
        // pyramid finds it, so the residual will be excellent.
        let moved = frame_of(width, height, |x, y| texture(x + 40.0, y, 0.0));
        let still = frame_of(width, height, |x, y| texture(x, y, 0.0));
        let (_, report) = stack(&[moved, still], &Options::default()).expect("a stack");
        assert_eq!(report.reference, 1);
        let dropped = report.frames[0]
            .dropped
            .as_ref()
            .expect("the frame should be dropped");
        println!("dropped: {dropped}");
        match dropped {
            Dropped::TooFar { pixels, limit } => {
                assert!((*pixels - 40.0).abs() < 1.0, "{pixels} px");
                assert!((*limit - 25.0).abs() < 0.1, "{limit} px");
            }
            other => panic!("wanted TooFar, got {other}"),
        }
        // On the residual alone it would have been believed.
        let fit = report.frames[0].fit.expect("a fit was made");
        assert!(fit.residual < 0.05, "{fit:?}");
        assert_eq!(report.used(), 1);
    }

    #[test]
    fn the_sharpest_frame_can_be_the_reference() {
        let (width, height) = (200usize, 150);
        let frames = vec![
            frame_of(width, height, |x, y| texture(x, y, 4.0)),
            frame_of(width, height, |x, y| texture(x, y, 0.0)),
            frame_of(width, height, |x, y| texture(x, y, 2.0)),
        ];
        let options = Options {
            reference: Reference::Sharpest,
            ..Default::default()
        };
        let (_, report) = stack(&frames, &options).expect("a stack");
        assert_eq!(report.reference, 1);
        assert_eq!(
            Reference::Middle,
            Options::default().reference,
            "the default is the middle frame"
        );
    }

    /// What the merge that held every frame gave: the report whole,
    /// across references, selectivities, a dropped frame, and both
    /// luminance weightings, and the samples by their bits where the
    /// stack is too short for the prior (which the held merge
    /// predates); where it is not, they differ.
    #[test]
    fn the_merge_in_passes_is_the_merge_that_held_every_frame() {
        let (width, height) = (240usize, 180);
        let (five, _) = depth_stack(width, height, 5);
        let (mut four, _) = depth_stack(width, height, 4);
        let (nine, _) = depth_stack(width, height, 9);
        // A frame of another scene: its link fails, and the chain goes
        // on from the last frame that held.
        four[1] = frame_of(width, height, |x, y| texture(x + 913.0, y + 377.0, 0.0));
        // The held merge predates the spill (`spill`), which is off here
        // so the two can be held to each other.
        let base = Options {
            spill: false,
            ..Options::default()
        };
        let cases = [
            ("five, middle", &five, base),
            (
                "five, sharpest",
                &five,
                Options {
                    reference: Reference::Sharpest,
                    ..base
                },
            ),
            (
                "five, frame 0",
                &five,
                Options {
                    reference: Reference::Index(0),
                    ..base
                },
            ),
            (
                "five, frame 4, selectivity 3",
                &five,
                Options {
                    reference: Reference::Index(4),
                    selectivity: 3.0,
                    ..base
                },
            ),
            (
                "five, selectivity 0",
                &five,
                Options {
                    selectivity: 0.0,
                    ..base
                },
            ),
            ("four, one dropped", &four, base),
            ("nine, middle", &nine, base),
            (
                "nine, selectivity 0",
                &nine,
                Options {
                    selectivity: 0.0,
                    ..base
                },
            ),
        ];
        for (name, frames, options) in cases {
            // The nine-frame cases ask only whether the prior speaks;
            // one weighting is enough for that.
            let lumas: &[[f32; 3]] = if frames.len() == 9 {
                &[WORKING_LUMA]
            } else {
                &[WORKING_LUMA, CAMERA_LUMA]
            };
            for &luma in lumas {
                let samples: Vec<&[f32]> = frames.iter().map(|f| f.data.as_slice()).collect();
                let (want, want_report) =
                    in_memory::merge(&samples, width, height, luma, &options).expect("held");
                let (got, got_report) =
                    merge(&samples, width, height, luma, &options).expect("in passes");
                assert_eq!(got_report, want_report, "{name}");
                // The held merge predates the stack's prior, which
                // speaks from eight frames up and above a selectivity
                // of 0; without it the samples are the same to the bit.
                let same = got
                    .iter()
                    .map(|v| v.to_bits())
                    .eq(want.iter().map(|v| v.to_bits()));
                assert_eq!(
                    same,
                    got_report.used() < prior::MIN_FRAMES || options.selectivity <= 0.0,
                    "{name}: {} frames used, the samples {}",
                    got_report.used(),
                    if same { "the same" } else { "differ" }
                );
            }
        }
        assert_eq!(
            stack(&four, &base).unwrap().1.used(),
            3,
            "the fixture drops a frame"
        );
    }

    /// Read rather than handed over: the same merge, each frame read
    /// once to place it and once to blend it, said in that order; with
    /// the sharpest frame as the reference, a scoring pass first.
    #[test]
    fn a_stack_read_frame_by_frame_is_the_stack_handed_over() {
        let (width, height) = (240usize, 180);
        let (frames, _) = depth_stack(width, height, 5);
        let camera: Vec<CameraImage> = frames
            .iter()
            .map(|f| CameraImage::from_data(width, height, f.data.clone()).unwrap())
            .collect();
        for (reference, passes) in [(Reference::Middle, 2), (Reference::Sharpest, 3)] {
            let options = Options {
                reference,
                ..Options::default()
            };
            let (want, want_report) = stack_camera(&camera, &options).expect("handed over");
            let mut reads = vec![0usize; camera.len()];
            let mut steps = Vec::new();
            let (got, got_report) = stack_camera_from(
                camera.len(),
                |k| {
                    reads[k] += 1;
                    Ok::<_, Error>(camera[k].clone())
                },
                |s| steps.push(s),
                &options,
            )
            .expect("read");
            assert_eq!(got_report, want_report);
            assert!(
                got.data
                    .iter()
                    .map(|v| v.to_bits())
                    .eq(want.data.iter().map(|v| v.to_bits()))
            );
            assert_eq!(reads, vec![passes; camera.len()], "{reference:?}");
            let n = camera.len();
            let mut said: Vec<Step> = (1..=n)
                .map(|index| Step::Reading { index, of: n })
                .collect();
            said.push(Step::Placing);
            said.extend((1..=n).map(|index| Step::Blending { index, of: n }));
            assert_eq!(steps, said, "{reference:?}");
        }
    }

    /// A stack long enough for the prior, made again from the places
    /// it found: the prior's evidence read again in the chain's order,
    /// each frame used read twice and the one dropped not at all, and
    /// the picture the same to the bit.
    #[test]
    fn a_stack_placed_again_hears_its_prior_again() {
        let (width, height) = (240usize, 180);
        let (mut frames, _) = depth_stack(width, height, 10);
        // A frame of another scene, dropped, and the chain goes on.
        frames[8] = frame_of(width, height, |x, y| texture(x + 913.0, y + 377.0, 0.0));
        let camera: Vec<CameraImage> = frames
            .iter()
            .map(|f| CameraImage::from_data(width, height, f.data.clone()).unwrap())
            .collect();
        let options = Options {
            reference: Reference::Index(3),
            ..Options::default()
        };
        assert!(options.selectivity > 0.0);
        let (first, report) = stack_camera(&camera, &options).expect("first");
        let used = report.used();
        assert!(used >= prior::MIN_FRAMES && used < camera.len(), "{used}");
        // The prior speaks: the merge that predates it gives another
        // picture.
        let samples: Vec<&[f32]> = camera.iter().map(|f| f.data.as_slice()).collect();
        let (held, _) =
            in_memory::merge(&samples, width, height, CAMERA_LUMA, &options).expect("held");
        assert!(first.data.iter().zip(&held).any(|(a, b)| a != b));

        let placed: Vec<_> = report
            .frames
            .iter()
            .map(|f| f.used().then_some(f.transform))
            .collect();
        let mut reads = vec![0usize; camera.len()];
        let mut order = Vec::new();
        let mut steps = Vec::new();
        let (again, again_report) = stack_camera_placed(
            3,
            &placed,
            report.clipped,
            |k| {
                reads[k] += 1;
                order.push(k);
                Ok::<_, Error>(camera[k].clone())
            },
            |s| steps.push(s),
            &options,
        )
        .expect("again");
        assert!(
            again
                .data
                .iter()
                .map(|v| v.to_bits())
                .eq(first.data.iter().map(|v| v.to_bits()))
        );
        assert_eq!(again_report.coverage, report.coverage);
        let twice: Vec<usize> = placed.iter().map(|p| 2 * p.is_some() as usize).collect();
        assert_eq!(reads, twice);
        let used_order: Vec<usize> = [3, 4, 5, 6, 7, 9, 2, 1, 0]
            .into_iter()
            .filter(|&k| placed[k].is_some())
            .collect();
        assert_eq!(order[..used], used_order[..], "the chain's order");
        let mut said: Vec<Step> = (1..=used)
            .map(|index| Step::Reading { index, of: used })
            .collect();
        said.push(Step::Placing);
        said.extend((1..=used).map(|index| Step::Blending { index, of: used }));
        assert_eq!(steps, said);

        // At a selectivity of 0 the prior is not heard, and each frame
        // is read once.
        let flat = Options {
            selectivity: 0.0,
            ..options
        };
        let (first, report) = stack_camera(&camera, &flat).expect("first");
        let placed: Vec<_> = report
            .frames
            .iter()
            .map(|f| f.used().then_some(f.transform))
            .collect();
        let mut reads = 0;
        let (again, _) = stack_camera_placed(
            3,
            &placed,
            report.clipped,
            |k| {
                reads += 1;
                Ok::<_, Error>(camera[k].clone())
            },
            |_| {},
            &flat,
        )
        .expect("again");
        assert_eq!(first.data, again.data);
        assert_eq!(reads, report.used());
    }

    /// A frame that fails to read on the second pass ends the merge
    /// with its error; one of another size is refused by name.
    #[test]
    fn a_frame_that_fails_its_second_read_ends_the_merge() {
        let (width, height) = (120usize, 90);
        let (frames, _) = depth_stack(width, height, 3);
        let camera: Vec<CameraImage> = frames
            .iter()
            .map(|f| CameraImage::from_data(width, height, f.data.clone()).unwrap())
            .collect();
        let mut reads = 0;
        let e = stack_camera_from(
            3,
            |k| {
                reads += 1;
                if reads > 3 {
                    Err(Error::Decode("gone".into()))
                } else {
                    Ok(camera[k].clone())
                }
            },
            |_| {},
            &Options::default(),
        )
        .expect_err("cut short");
        assert!(matches!(e, Error::Decode(_)), "{e}");
        let small = CameraImage::from_data(60, 45, vec![0.5; 60 * 45 * 3]).unwrap();
        let e = stack_camera_from(
            3,
            |k| {
                Ok::<_, Error>(if k == 2 {
                    small.clone()
                } else {
                    camera[k].clone()
                })
            },
            |_| {},
            &Options::default(),
        )
        .expect_err("two sizes");
        assert!(
            e.to_string()
                .contains("frame 1 is 120x90 and frame 2 is 60x45"),
            "{e}"
        );
        let none = stack_camera_from(
            0,
            |_| Ok::<_, Error>(small.clone()),
            |_| {},
            &Options::default(),
        );
        assert!(matches!(none, Err(Error::Unsupported(_))));
    }

    /// Not a check, a measurement: `cargo test --release -p
    /// greycard-core -- --ignored --nocapture timing_of_a_stack`.
    #[test]
    #[ignore = "a timing, not a test"]
    fn timing_of_a_stack_at_twenty_four_megapixels() {
        /// The high-water mark of this process's resident set, in
        /// gigabytes, as the kernel has counted it so far. Linux only;
        /// this is a measurement, not a check.
        fn peak_gb() -> f64 {
            std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|s| {
                    s.lines()
                        .find(|l| l.starts_with("VmHWM:"))
                        .and_then(|l| l.split_whitespace().nth(1))
                        .and_then(|k| k.parse::<f64>().ok())
                })
                .map(|kb| kb / 1_048_576.0)
                .unwrap_or(f64::NAN)
        }
        let (width, height) = (6000usize, 4000);
        // The first stack of a run pays for the pages of every plane
        // it allocates, and the allocator only has those pages once a
        // full-size stack has asked for them: warming at 512 pixels
        // left the five-frame run a quarter slower than the ten-frame
        // one that followed it.
        let (warm, _) = depth_stack(width, height, 2);
        let _ = stack(&warm, &Options::default());
        drop(warm);
        for (width, height, n) in [
            (width, height, 5usize),
            (width, height, 10),
            (8192, 5464, 10),
        ] {
            let built = std::time::Instant::now();
            let (frames, _) = depth_stack(width, height, n);
            let built = built.elapsed();
            let start = std::time::Instant::now();
            let (merged, report) = stack(&frames, &Options::default()).expect("a stack");
            let took = start.elapsed();
            println!(
                "{n} frames of {width}x{height}: {:>6.2} s ({:.2} s a frame), {} levels, \
                 reference {}, {} used, every frame over {:.1}% of the picture \
                 (the frames took {:.1} s to build; merged {} samples; peak resident                  {:.1} GB)",
                took.as_secs_f64(),
                took.as_secs_f64() / n as f64,
                report.levels,
                report.reference,
                report.used(),
                report.full_coverage_fraction() * 100.0,
                built.as_secs_f64(),
                merged.data.len(),
                peak_gb(),
            );
            for f in &report.frames {
                println!(
                    "  frame {} sharpness {:.4}{}",
                    f.index,
                    f.sharpness,
                    match (&f.fit, &f.dropped) {
                        (_, Some(d)) => format!(" DROPPED, {d}"),
                        (Some(fit), None) => format!(
                            ", residual {:.4} over {:.1}%, scale {:.6}, {} iterations",
                            fit.residual,
                            fit.overlap * 100.0,
                            fit.transform.scale(),
                            fit.iterations
                        ),
                        (None, None) => " (the reference)".into(),
                    }
                );
            }
        }
    }

    /// Not a check, a measurement of what the spill costs: frames read
    /// in passes (`stack_camera_from`), each a copy of one held at 24
    /// megapixels, so the merge's own working set is what grows; with
    /// `GREYCARD_SPILL=0` the spill is off, and with
    /// `GREYCARD_SPILL_CLIP=1` every frame has a blown square, so the
    /// spill is gathered and every frame measured. Run each way in its own
    /// process for the peak: `GREYCARD_SPILL=1 cargo test --release -p
    /// greycard-core -- --ignored --nocapture timing_of_the_spill`.
    #[test]
    #[ignore = "a timing, not a test"]
    fn timing_of_the_spill() {
        let said = std::env::var("GREYCARD_SPILL").unwrap_or_default();
        let spill = said != "0";
        let blown = std::env::var("GREYCARD_SPILL_CLIP").is_ok_and(|v| v == "1");
        let peak_mb = || {
            std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|s| {
                    s.lines()
                        .find(|l| l.starts_with("VmHWM:"))
                        .and_then(|l| l.split_whitespace().nth(1))
                        .and_then(|k| k.parse::<f64>().ok())
                })
                .map_or(f64::NAN, |kb| kb / 1024.0)
        };
        let options = Options {
            spill,
            ..Options::default()
        };
        for n in [5usize, 10] {
            let (frames, _) = depth_stack(6000, 4000, n);
            let camera: Vec<CameraImage> = frames
                .into_iter()
                .map(|mut f| {
                    if blown {
                        for y in 1800..2000 {
                            let row = &mut f.data[(y * 6000 + 2900) * 3..(y * 6000 + 3100) * 3];
                            row.fill(4.0);
                        }
                    }
                    CameraImage::from_data(f.width, f.height, f.data).unwrap()
                })
                .collect();
            let held = peak_mb();
            let start = std::time::Instant::now();
            let _ = stack_camera_from(n, |k| Ok::<_, Error>(camera[k].clone()), |_| {}, &options)
                .expect("a stack");
            println!(
                "spill {said:?}, blown {blown}: {n} frames of 6000x4000 in passes, {:.2} s; peak {:.0} MB, \
                 {:.0} MB over the frames held",
                start.elapsed().as_secs_f64(),
                peak_mb(),
                peak_mb() - held
            );
        }
    }
}
