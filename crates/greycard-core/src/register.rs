//! Registration: the transform that puts one frame on top of another.
//!
//! Frames in a stack — a handheld bracket, a focus stack, the
//! refinement pass of a panorama — differ by a small transform: a
//! shift, a little rotation, a magnification the focus breathing
//! changed. This fits that transform directly from the pixels, by
//! inverse-compositional Lucas-Kanade over a Gaussian pyramid, with no
//! feature detection and no correspondences. Coarse levels catch the
//! large part of the motion, fine levels settle the fraction of a
//! pixel.
//!
//! Written from Baker and Matthews, "Lucas-Kanade 20 Years On: A
//! Unifying Framework" (IJCV 56(3), 2004), §3.2: the update is
//! computed on the template, so the Jacobian is taken once, on the
//! template, at the identity, instead of on the moving frame at every
//! step. What is not saved is the Hessian. The textbook
//! inverse-compositional algorithm inverts it once per level; here it
//! is accumulated again every iteration, because the pixels that go
//! into it are the ones that land inside the moving frame, and which
//! those are changes as the warp does. Holding a stale Hessian over a
//! changing pixel set is what makes a partly overlapping pair
//! converge to the wrong place, so the pass stays.
//!
//! Nothing here is ported from another project.
//!
//! **Exposure.** A bracket's frames differ by stops, which an ordinary
//! sum-of-squares fit reads as motion. So the fit does not run on the
//! luminance, it runs on the log of it, and three things then make it
//! blind to the difference.
//!
//! Each pyramid level has its own mean taken off. A frame one stop
//! darker is, in the log, the same picture minus one — an additive
//! constant, which the mean removes exactly, and exactly at every
//! level, because a constant survives a blur and a decimation
//! unchanged.
//!
//! The log's dark floor is set from each frame's own median rather
//! than at an absolute value ([`Options::floor_stops`]), so it lands
//! on the same scene luminance in both frames and the difference stays
//! constant down there too.
//!
//! And whatever offset is left — the mean is taken over the whole
//! frame, but two frames only mostly overlap — is eliminated from the
//! normal equations rather than solved for, which is the Schur
//! complement of a brightness parameter the fit never has to carry.
//!
//! What is *not* per-frame is the deviation the levels are divided by:
//! that is the reference's, for both. Dividing each frame by its own
//! would put a gain between them wherever their content differs, at
//! the edge a shift brings in or under one frame's vignetting, and a
//! gain is a mismatch the fit pays for with motion. One shared divisor
//! is only a change of units.
//!
//! **What a caller gets.** [`Fit::residual`] is the root-mean-square
//! difference over the overlap in those normalized units: 0 for an
//! exact match, about 1.4 for two frames with nothing in common (two
//! unit-deviation planes that do not correlate). That is the number
//! that separates a good fit from a failed one, and
//! [`Fit::aligned`] applies the threshold so a caller has one thing
//! to look at. [`Fit::converged`] is not that thing: it says the
//! iteration stopped moving, which a fit that walked into the wrong
//! valley also does. A frame with no structure to fit — a blank sky,
//! a lens cap — is not given a confident transform at all: it comes
//! back as an error.
//!
//! **How far it reaches.** Cold, from the identity, the pyramid finds
//! a shift of about a seventh of the frame's width. Measured on the
//! synthetic texture at three frame sizes, it holds at fifteen percent
//! of the width and is gone by eighteen; one level alone holds under a
//! tenth, which is what the pyramid is there to buy. A seventh of a
//! 6000-pixel frame is 850 pixels, far more than a handheld bracket
//! produces.
//!
//! It is a fraction of the width rather than a number of pixels
//! because the coarsest level is a fixed size
//! ([`Options::min_side`]): what a level can capture is set by the
//! size of the detail in it, and every doubling on the way down
//! multiplies that by two. Which is also why the pyramid must be
//! allowed to run out at `min_side` and not at
//! [`Options::max_levels`] — a cap that bites first costs reach on
//! exactly the largest sensors, so the default is set high enough not
//! to. A frame with no large structure, or one whose overlap is
//! partial, reaches less; a caller who knows roughly where the frame
//! went should say so with [`fit_from`] rather than reach further.
//!
//! **What it does not do.** There is no robust weighting in the normal
//! equations: every pixel in the overlap counts the same, so a moving
//! subject, a specular flare or a bright speck pulls on the fit in
//! proportion to its gradient. Only the log's floor is defended
//! against outliers, and only so that one hot photosite cannot black
//! out the rest of the picture. Deghosting and per-pixel flow belong
//! to the merge, not here.
//!
//! Everything here is CPU and single-image; a stack is a one-off, not
//! a viewport concern.

use rayon::prelude::*;

use crate::error::{Error, Result};
use crate::image::{CameraImage, WorkingImage};

/// The working space's luminance weights, as the tone and haze
/// operations use them.
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// A 2-D affine transform, row-major: `[a, b, tx, c, d, ty]` takes
/// `(x, y)` to `(a x + b y + tx, c x + d y + ty)`.
///
/// Coordinates are pixel centers: the sample at column `i`, row `j` sits
/// at `(i, j)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub m: [f64; 6],
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
    };

    pub fn translation(tx: f64, ty: f64) -> Self {
        Self {
            m: [1.0, 0.0, tx, 0.0, 1.0, ty],
        }
    }

    /// A uniform `scale` and a rotation of `angle` radians about
    /// `about`, then a shift of `translate`.
    pub fn similarity(scale: f64, angle: f64, about: [f64; 2], translate: [f64; 2]) -> Self {
        let (s, c) = angle.sin_cos();
        let (a, b) = (scale * c, -scale * s);
        let (cx, cy) = (about[0], about[1]);
        Self {
            m: [
                a,
                b,
                cx + translate[0] - a * cx - b * cy,
                -b,
                a,
                cy + translate[1] + b * cx - a * cy,
            ],
        }
    }

    #[inline]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.m;
        (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5])
    }

    /// `self` after `inner`: the transform that applies `inner` first.
    pub fn compose(&self, inner: &Transform) -> Transform {
        let (a, b) = (&self.m, &inner.m);
        Transform {
            m: [
                a[0] * b[0] + a[1] * b[3],
                a[0] * b[1] + a[1] * b[4],
                a[0] * b[2] + a[1] * b[5] + a[2],
                a[3] * b[0] + a[4] * b[3],
                a[3] * b[1] + a[4] * b[4],
                a[3] * b[2] + a[4] * b[5] + a[5],
            ],
        }
    }

    pub fn determinant(&self) -> f64 {
        self.m[0] * self.m[4] - self.m[1] * self.m[3]
    }

    /// `None` when the linear part is singular.
    pub fn inverse(&self) -> Option<Transform> {
        let det = self.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let m = &self.m;
        let (a, b, c, d) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
        Some(Transform {
            m: [a, b, -(a * m[2] + b * m[5]), c, d, -(c * m[2] + d * m[5])],
        })
    }

    /// The same geometric transform written in coordinates scaled by
    /// `factor`: half resolution is 0.5.
    pub fn at_scale(&self, factor: f64) -> Transform {
        let mut t = *self;
        t.m[2] *= factor;
        t.m[5] *= factor;
        t
    }

    /// The scale the transform applies: exact for a similarity, the
    /// square root of the area factor otherwise.
    pub fn scale(&self) -> f64 {
        self.determinant().abs().sqrt()
    }

    /// The rotation in radians: exact for a similarity, the angle the
    /// x axis turns through otherwise.
    pub fn rotation(&self) -> f64 {
        self.m[3].atan2(self.m[0])
    }

    /// How far the transform moves the point `(x, y)`.
    pub fn displacement(&self, x: f64, y: f64) -> (f64, f64) {
        let (u, v) = self.apply(x, y);
        (u - x, v - y)
    }

    /// The largest distance this transform and `other` put between the
    /// same point, over the corners of a `width`×`height` frame. A
    /// frame with no width or no height is one point, the origin.
    pub fn corner_distance(&self, other: &Transform, width: usize, height: usize) -> f64 {
        let (w, h) = ((width.max(1) - 1) as f64, (height.max(1) - 1) as f64);
        [[0.0, 0.0], [w, 0.0], [0.0, h], [w, h]]
            .iter()
            .map(|p| {
                let (ax, ay) = self.apply(p[0], p[1]);
                let (bx, by) = other.apply(p[0], p[1]);
                (ax - bx).hypot(ay - by)
            })
            .fold(0.0, f64::max)
    }
}

/// Which transform is fitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// A shift: two parameters. A tripod that was nudged.
    Translation,
    /// Shift, rotation and one scale: four parameters. Handheld
    /// frames, and focus breathing.
    Similarity,
    /// Six parameters: shift, rotation, two scales and a shear. What a
    /// distant scene's small viewpoint change looks like.
    Affine,
}

impl Model {
    pub fn parameters(&self) -> usize {
        match self {
            Model::Translation => 2,
            Model::Similarity => 4,
            Model::Affine => 6,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Model::Translation => "translation",
            Model::Similarity => "similarity",
            Model::Affine => "affine",
        }
    }
}

impl std::fmt::Display for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for Model {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "translation" => Ok(Model::Translation),
            "similarity" => Ok(Model::Similarity),
            "affine" => Ok(Model::Affine),
            other => Err(format!(
                "want translation, similarity or affine, not {other}"
            )),
        }
    }
}

/// How the fit is run.
#[derive(Debug, Clone, Copy, PartialEq)]
///
/// Every field that could be out of range is clamped rather than
/// refused: `max_levels` and `max_iterations` to at least one,
/// `min_side` to at least three, and `finest_level` to the coarsest
/// level the pyramid actually has. [`Fit::levels`] and [`Fit::finest`]
/// report what the fit ran with.
pub struct Options {
    pub model: Model,
    /// Most pyramid levels, full resolution counted as one. The
    /// default is high enough that [`Options::min_side`] is what ends
    /// the pyramid at any sensor size in use.
    pub max_levels: usize,
    /// The coarsest level keeps at least this many pixels on its short
    /// side. Smaller than about this and a level has no motion left to
    /// tell, and its border eats the rest.
    pub min_side: usize,
    /// The finest level the fit descends to: 0 is full resolution, 1
    /// half, 2 a quarter. Stopping early costs accuracy and saves most
    /// of the time, since the finest level is three quarters of the work.
    pub finest_level: usize,
    /// Iterations a level may take before it gives up.
    pub max_iterations: usize,
    /// A level has converged when an iteration moves the frame's
    /// corners by less than this many of that level's pixels.
    pub epsilon: f64,
    /// Fraction of the reference that must land inside the moving
    /// frame for the fit to be believed.
    pub min_overlap: f32,
    /// Deviation of the log luminance, in stops, below which a level
    /// is taken to have no structure to fit.
    pub min_contrast: f32,
    /// How far below the frame's median luminance the log's floor
    /// sits, in stops. Fourteen is a sensor's dynamic range and then
    /// some: what is under it is noise, and lifting it to the floor
    /// keeps the log of a black pixel finite without touching anything
    /// the fit can use.
    pub floor_stops: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            model: Model::Similarity,
            max_levels: 12,
            min_side: 32,
            finest_level: 0,
            max_iterations: 30,
            epsilon: 0.01,
            min_overlap: 0.25,
            min_contrast: 1e-3,
            floor_stops: 14.0,
        }
    }
}

/// What a fit came to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    /// Reference coordinates to moving coordinates: the moving frame
    /// sampled at `transform.apply(x, y)` is the reference's `(x, y)`.
    pub transform: Transform,
    /// Root-mean-square difference over the overlap, in the normalized
    /// log units the fit runs on: 0 for an exact match, about 1.4 for
    /// frames with nothing in common.
    ///
    /// A root mean square, with what that implies: one wild sample
    /// carries into it. A photosite reading a billion times the white
    /// level takes this from 0.002 to 0.18 on a half-megapixel frame,
    /// while leaving the transform where it was, because a single
    /// pixel has to be `residual * sqrt(pixels)` deviations out to
    /// matter and on a 24-megapixel frame it never is. A poor residual
    /// under a transform that looks reasonable is a reason to look at
    /// the frames, not only at the fit.
    pub residual: f32,
    /// Fraction of the reference that landed inside the moving frame.
    pub overlap: f32,
    /// Pyramid levels built.
    pub levels: usize,
    /// The finest level the fit actually descended to, which is
    /// [`Options::finest_level`] clamped to the pyramid's depth.
    pub finest: usize,
    /// The coarsest level's size.
    pub coarsest: (usize, usize),
    /// Iterations over all levels.
    pub iterations: usize,
    /// Whether the finest level stopped because the step had got small
    /// or the error had stopped falling, rather than because it ran
    /// out of iterations.
    ///
    /// This is a stopping criterion and not a verdict. Gauss-Newton
    /// stops just as contentedly at the bottom of the wrong valley: a
    /// shift past what the pyramid can capture comes back converged,
    /// with most of the frame overlapping, and entirely wrong.
    /// [`Fit::aligned`] is the verdict; this says only that no more
    /// iterations would have helped.
    pub converged: bool,
}

/// The residual under which [`Fit::aligned`] believes a fit.
///
/// The scale is the fit's own: the deviation of the difference between
/// two unit-deviation planes. Zero is an exact match; about 1.4 is two
/// pictures with nothing in common. Measured, a frame laid on a warped
/// copy of itself comes in between 0.002 and 0.03 — the upper end
/// being a real 45-megapixel pair, where eight-bit quantization and
/// clipped highlights are the difference — and a fit that walked into
/// the wrong valley comes in above 0.5. A twentieth sits between the
/// two with a decade of room either side.
pub const ALIGNED_RESIDUAL: f32 = 0.05;

impl Fit {
    /// Whether to use this transform.
    ///
    /// The one thing a caller has to check: the residual is under
    /// [`ALIGNED_RESIDUAL`]. A fit that fails this is not a rough
    /// alignment to be refined, it is an alignment that was not found,
    /// and a stack should drop the frame or ask for a guess
    /// ([`fit_from`]) rather than merge through it.
    ///
    /// What it does not promise is sub-pixel accuracy. It separates a
    /// fit that found the picture from one that went somewhere else,
    /// and a fit can sit just inside the threshold and still be a
    /// pixel out — a single level asked for a shift at the edge of
    /// what it can capture managed 1.4 px at a residual of 0.043. Only
    /// a residual near the floor, a few thousandths, says the frames
    /// are on top of each other to a fraction of a pixel.
    pub fn aligned(&self) -> bool {
        self.residual < ALIGNED_RESIDUAL
    }
}

/// The luminance of a working-space image, for [`fit`].
pub fn luminance(image: &WorkingImage) -> Vec<f32> {
    image
        .data
        .par_chunks(WorkingImage::CHANNELS)
        .map(|px| LUMA[0] * px[0] + LUMA[1] * px[1] + LUMA[2] * px[2])
        .collect()
}

/// The luminance of a camera-space image: the mosaic's own weighting,
/// green counted twice, since camera RGB has no colorimetry yet.
pub fn camera_luminance(image: &CameraImage) -> Vec<f32> {
    image
        .data
        .par_chunks(CameraImage::CHANNELS)
        .map(|px| 0.25 * px[0] + 0.5 * px[1] + 0.25 * px[2])
        .collect()
}

/// Fit the transform that takes `reference` coordinates to `moving`
/// coordinates, from the identity.
///
/// Both planes are the same `width`×`height` — stack frames from one
/// camera — and hold linear luminance; see the module's note on what
/// the fit actually runs on.
pub fn fit(
    reference: &[f32],
    moving: &[f32],
    width: usize,
    height: usize,
    options: &Options,
) -> Result<Fit> {
    fit_from(
        reference,
        moving,
        width,
        height,
        options,
        &Transform::IDENTITY,
    )
}

/// [`fit`], started from a transform already known roughly: the
/// previous frame's fit in a long stack, or a shift read off the
/// metadata.
pub fn fit_from(
    reference: &[f32],
    moving: &[f32],
    width: usize,
    height: usize,
    options: &Options,
    initial: &Transform,
) -> Result<Fit> {
    if reference.len() != width * height || moving.len() != width * height {
        return Err(Error::Unsupported(format!(
            "registration wants two {width}x{height} planes, got {} and {}",
            reference.len(),
            moving.len()
        )));
    }
    if width < 3 || height < 3 {
        return Err(Error::NoFit(format!(
            "{width}x{height} is too small to fit"
        )));
    }
    if initial.inverse().is_none() {
        return Err(Error::NoFit(format!(
            "the transform to start from is singular: {:?}",
            initial.m
        )));
    }

    let a = pyramid(
        log_plane(reference, options.floor_stops),
        width,
        height,
        options,
    );
    let b = pyramid(
        log_plane(moving, options.floor_stops),
        width,
        height,
        options,
    );
    let levels = a.len();
    let finest = options.finest_level.min(levels - 1);

    // Level coordinates are the full-resolution ones halved per level:
    // the decimation keeps every other sample, so level l's pixel i is
    // level 0's pixel i * 2^l, with no half-pixel offset to carry.
    let mut t = initial.at_scale(0.5f64.powi((levels - 1) as i32));
    let mut iterations = 0;
    let mut converged = false;
    // The finest level's standardized planes, kept so the score at the
    // end does not build them a second time.
    let mut finest_planes = None;

    for level in (finest..levels).rev() {
        if level < levels - 1 {
            t = t.at_scale(2.0);
        }
        let (reference_mean, contrast) = moments(&a[level]);
        let (moving_mean, moving_contrast) = moments(&b[level]);
        if !contrast.is_finite()
            || !moving_contrast.is_finite()
            || (contrast as f32) < options.min_contrast
            || (moving_contrast as f32) < options.min_contrast
        {
            if level == finest {
                return Err(Error::NoFit(format!(
                    "the frames vary by {contrast:.2e} and {moving_contrast:.2e} stops: nothing to fit"
                )));
            }
            // A level too blurred to say anything is skipped, not failed.
            continue;
        }
        let template = standardized(&a[level], reference_mean, contrast);
        let moved = standardized(&b[level], moving_mean, contrast);
        let step = run_level(&template, &moved, t, options);
        t = step.transform;
        iterations += step.iterations;
        if level == finest {
            converged = step.converged;
            finest_planes = Some((template, moved));
        }
    }

    let (template, moved) = match finest_planes {
        Some(planes) => planes,
        // Only if the finest level was skipped, which cannot happen:
        // a skipped finest level is the error above.
        None => {
            return Err(Error::NoFit(
                "the finest level had nothing to fit".to_string(),
            ));
        }
    };
    let scored = score(&template, &moved, &t);
    let countable = ((template.width - 2) * (template.height - 2)) as f32;
    let overlap = scored.count as f32 / countable;
    if overlap < options.min_overlap {
        return Err(Error::NoFit(format!(
            "the frames overlap over {:.0}% of the reference, under the {:.0}% asked for",
            overlap * 100.0,
            options.min_overlap * 100.0
        )));
    }
    let count = scored.count.max(1) as f64;
    let variance = (scored.sse - scored.error_sum * scored.error_sum / count) / count;
    let residual = variance.max(0.0).sqrt() as f32;

    Ok(Fit {
        transform: t.at_scale(2f64.powi(finest as i32)),
        residual,
        overlap,
        levels,
        finest,
        coarsest: (a[levels - 1].width, a[levels - 1].height),
        iterations,
        converged,
    })
}

/// A single-channel image.
struct Plane {
    data: Vec<f32>,
    width: usize,
    height: usize,
}

/// The log of the luminance in stops, floored `floor_stops` below the
/// frame's median, so that two exposures of one scene differ by a
/// constant everywhere, the floor included.
///
/// The median and not the mean. A mean is an outlier's to move: one
/// hot photosite at a thousand times the white level lifts it far
/// enough that the floor swallows the picture, and the fit is then
/// looking at a flat plane with a speck on it. Measured, before this:
/// one such pixel in a 512x384 frame moved the fit by 0.78 px. A
/// median does not notice it.
///
/// That is the only outlier defense in the module. The normal
/// equations weight every pixel alike, so a speck still pulls on the
/// fit through its own gradient — it just can no longer take the rest
/// of the frame with it.
///
/// A sample at or below zero is dark, and goes to the floor with the
/// rest of the dark. A sample that is not finite is not data at all,
/// and takes the median instead: the one value that says nothing,
/// contributing neither an error nor a gradient nor a speck for the
/// blur to spread down the pyramid. Putting it on the floor would make
/// it a black dot fourteen stops out, which is a feature, and a loud one.
pub(crate) fn log_plane(src: &[f32], floor_stops: f32) -> Vec<f32> {
    let mut logs: Vec<f32> = src
        .par_iter()
        .map(|v| {
            if v.is_finite() {
                // Zero and below have no log; the floor is applied
                // below, once there is a median to take it from.
                v.max(0.0).log2()
            } else {
                f32::NAN
            }
        })
        .collect();
    // No finite sample at all: one flat plane, which the contrast test
    // refuses a moment later.
    let Some(middle) = median(&logs) else {
        return vec![0.0; src.len()];
    };
    let floor = middle - floor_stops;
    logs.par_iter_mut().for_each(|l| {
        *l = if l.is_nan() { middle } else { l.max(floor) };
    });
    logs
}

/// The median of the finite values, to a quarter of a stop, by
/// histogram: one pass, and exact enough for a floor fourteen stops
/// under it.
///
/// A quarter of a stop is also a bin width that a change of exposure
/// by whole stops lands on exactly, so two frames a stop apart get
/// medians exactly a stop apart and floors that clamp the same
/// samples.
fn median(logs: &[f32]) -> Option<f32> {
    const LOW: f32 = -96.0;
    const STEP: f32 = 0.25;
    const BINS: usize = 768;
    let parts: Vec<[u32; BINS]> = logs
        .par_chunks(1 << 16)
        .map(|chunk| {
            let mut bins = [0u32; BINS];
            for v in chunk {
                // NaN for a sample that had no log, and the negative
                // infinity log2(0) gives: neither is a value.
                if !v.is_finite() {
                    continue;
                }
                // A float cast saturates, so a log far below the range
                // lands in the first bin rather than wrapping.
                let k = ((v - LOW) / STEP) as usize;
                bins[k.min(BINS - 1)] += 1;
            }
            bins
        })
        .collect();
    let mut bins = [0u64; BINS];
    for part in &parts {
        for (b, p) in bins.iter_mut().zip(part) {
            *b += *p as u64;
        }
    }
    let total: u64 = bins.iter().sum();
    if total == 0 {
        return None;
    }
    let half = total.div_ceil(2);
    let mut seen = 0u64;
    for (k, count) in bins.iter().enumerate() {
        seen += count;
        if seen >= half {
            return Some(LOW + (k as f32 + 0.5) * STEP);
        }
    }
    None
}

/// The pyramid, level 0 first. The base is taken, not copied.
fn pyramid(base: Vec<f32>, width: usize, height: usize, options: &Options) -> Vec<Plane> {
    let mut levels = vec![Plane {
        data: base,
        width,
        height,
    }];
    while levels.len() < options.max_levels.max(1) {
        let top = levels.last().expect("a pyramid has a base");
        if top.width.min(top.height) / 2 < options.min_side.max(3) {
            break;
        }
        levels.push(downsample(top));
    }
    levels
}

/// Blur by the binomial `[1 4 6 4 1] / 16` and keep every other
/// sample, separably, the taps clamped at the border.
fn downsample(src: &Plane) -> Plane {
    const TAPS: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let (w, h) = (src.width, src.height);
    let ow = w.div_ceil(2);
    let oh = h.div_ceil(2);
    let mut rows = vec![0.0f32; ow * h];
    rows.par_chunks_mut(ow)
        .zip(src.data.par_chunks(w))
        .for_each(|(out, row)| {
            for (x, o) in out.iter_mut().enumerate() {
                let c = 2 * x;
                let mut sum = 0.0;
                for (k, tap) in TAPS.iter().enumerate() {
                    let i = (c + k).saturating_sub(2).min(w - 1);
                    sum += tap * row[i];
                }
                *o = sum;
            }
        });
    let mut out = vec![0.0f32; ow * oh];
    out.par_chunks_mut(ow).enumerate().for_each(|(y, line)| {
        let c = 2 * y;
        for (x, o) in line.iter_mut().enumerate() {
            let mut sum = 0.0;
            for (k, tap) in TAPS.iter().enumerate() {
                let j = (c + k).saturating_sub(2).min(h - 1);
                sum += tap * rows[j * ow + x];
            }
            *o = sum;
        }
    });
    Plane {
        data: out,
        width: ow,
        height: oh,
    }
}

/// A level's mean and deviation, in stops.
fn moments(src: &Plane) -> (f64, f64) {
    let n = src.data.len() as f64;
    let mean = src.data.iter().map(|v| *v as f64).sum::<f64>() / n;
    let var = src
        .data
        .iter()
        .map(|v| {
            let d = *v as f64 - mean;
            d * d
        })
        .sum::<f64>()
        / n;
    (mean, var.sqrt())
}

/// The level with its own mean taken off and a *shared* deviation
/// divided out.
///
/// The mean is what carries the exposure difference away, and it has
/// to be each frame's own. The deviation is the reference's for both
/// frames: dividing each by its own would put a gain between them
/// wherever their content is not quite the same — the edges a shift
/// brings in, a lens's vignetting — and a gain is a mismatch the fit
/// would try to pay for with motion. One shared divisor is only a
/// change of units.
fn standardized(src: &Plane, mean: f64, deviation: f64) -> Plane {
    let scale = if deviation.is_finite() && deviation > 0.0 {
        1.0 / deviation
    } else {
        0.0
    };
    let data = src
        .data
        .par_iter()
        .map(|v| ((*v as f64 - mean) * scale) as f32)
        .collect();
    Plane {
        data,
        width: src.width,
        height: src.height,
    }
}

/// One pyramid level's worth of iteration.
struct LevelResult {
    transform: Transform,
    iterations: usize,
    converged: bool,
}

/// What one pass over the template collected.
///
/// The sums of the steepest-descent images and of the error are kept
/// alongside their products because the fit has one parameter more
/// than the model does: a constant added to the whole frame, which in
/// the log is whatever exposure difference the mean did not already
/// take off. It is eliminated rather than solved for — subtracting the
/// outer products of the sums from the normal equations is that
/// parameter's Schur complement — so a stop left over costs nothing
/// and, more to the point, moves nothing.
#[derive(Clone, Copy)]
struct Accumulated {
    /// The Hessian's upper triangle, packed by rows.
    hessian: [f64; 21],
    /// The steepest-descent parameter updates.
    b: [f64; 6],
    /// The steepest-descent images' own sums.
    sd_sum: [f64; 6],
    error_sum: f64,
    sse: f64,
    count: u64,
}

impl Accumulated {
    const ZERO: Self = Self {
        hessian: [0.0; 21],
        b: [0.0; 6],
        sd_sum: [0.0; 6],
        error_sum: 0.0,
        sse: 0.0,
        count: 0,
    };

    fn add(&mut self, other: &Self) {
        for (a, b) in self.hessian.iter_mut().zip(&other.hessian) {
            *a += b;
        }
        for (a, b) in self.b.iter_mut().zip(&other.b) {
            *a += b;
        }
        for (a, b) in self.sd_sum.iter_mut().zip(&other.sd_sum) {
            *a += b;
        }
        self.error_sum += other.error_sum;
        self.sse += other.sse;
        self.count += other.count;
    }

    /// The normal equations with that constant eliminated, and the
    /// error's variance about its own mean.
    ///
    /// Matrix code: the indices are the mathematics, and the packed
    /// triangle's `k` walks with them.
    #[allow(clippy::needless_range_loop)]
    fn system(&self, n: usize) -> ([f64; 21], [f64; 6], f64) {
        let count = self.count.max(1) as f64;
        let mut hessian = self.hessian;
        let mut b = self.b;
        let mut k = 0;
        for i in 0..n {
            b[i] -= self.sd_sum[i] * self.error_sum / count;
            for j in i..n {
                hessian[k] -= self.sd_sum[i] * self.sd_sum[j] / count;
                k += 1;
            }
        }
        let variance = (self.sse - self.error_sum * self.error_sum / count) / count;
        (hessian, b, variance.max(0.0))
    }
}

/// What a fit scored: the error and how much of the frame it was over.
struct Score {
    sse: f64,
    error_sum: f64,
    count: u64,
}

#[inline]
fn sample(plane: &Plane, x: f64, y: f64) -> Option<f32> {
    let (w, h) = (plane.width, plane.height);
    if !(x >= 0.0 && y >= 0.0 && x <= (w - 1) as f64 && y <= (h - 1) as f64) {
        return None;
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = (x - x0) as f32;
    let fy = (y - y0) as f32;
    let i = x0 as usize;
    let j = y0 as usize;
    let i1 = (i + 1).min(w - 1);
    let j1 = (j + 1).min(h - 1);
    let row0 = j * w;
    let row1 = j1 * w;
    let a = plane.data[row0 + i] + fx * (plane.data[row0 + i1] - plane.data[row0 + i]);
    let b = plane.data[row1 + i] + fx * (plane.data[row1 + i1] - plane.data[row1 + i]);
    Some(a + fy * (b - a))
}

/// The steepest-descent row at a pixel: the template's gradient
/// through the warp's Jacobian, in coordinates centered and scaled so
/// that every column is of the same order and the Hessian stays
/// conditioned.
#[inline]
fn steepest_descent(model: Model, gx: f64, gy: f64, x: f64, y: f64, sd: &mut [f64; 6]) {
    match model {
        Model::Translation => {
            sd[0] = gx;
            sd[1] = gy;
        }
        Model::Similarity => {
            sd[0] = gx * x + gy * y;
            sd[1] = -gx * y + gy * x;
            sd[2] = gx;
            sd[3] = gy;
        }
        Model::Affine => {
            sd[0] = gx * x;
            sd[1] = gy * x;
            sd[2] = gx * y;
            sd[3] = gy * y;
            sd[4] = gx;
            sd[5] = gy;
        }
    }
}

/// The incremental warp a parameter step stands for, in the centered
/// and scaled coordinates the Jacobian was taken in.
fn increment(model: Model, dp: &[f64; 6]) -> Transform {
    match model {
        Model::Translation => Transform {
            m: [1.0, 0.0, dp[0], 0.0, 1.0, dp[1]],
        },
        Model::Similarity => Transform {
            m: [1.0 + dp[0], -dp[1], dp[2], dp[1], 1.0 + dp[0], dp[3]],
        },
        Model::Affine => Transform {
            m: [1.0 + dp[0], dp[2], dp[4], dp[1], 1.0 + dp[3], dp[5]],
        },
    }
}

/// One pass over the template at the current transform.
/// One pass over the template at the current transform: the normal
/// equations, the error and how many pixels went into them.
///
/// Two things here are done again every iteration that the textbook
/// inverse-compositional algorithm does once per level, and both are
/// deliberate.
///
/// The Hessian, because the pixels that go into it are the ones whose
/// warped position lands inside the moving frame, and which those are
/// changes as the warp does. A Hessian taken over one set of pixels
/// and applied to a gradient taken over another is a wrong step, and
/// the more of the frame hangs over the edge the wronger it is.
///
/// The steepest-descent images, because measuring said so. They do not
/// depend on the warp, so they can be built once per level and read —
/// six planes a pixel for an affine, which at 24 megapixels is 576 MB
/// that the accumulation then streams through. Done that way a
/// 6000x4000 affine fit took 211 ms against 160 ms for recomputing
/// them here, and with the allocation in play it wandered past a
/// second. The template's neighbors are in cache already, because the
/// pass reads the template anyway; a plane six times its size is not.
fn accumulate(template: &Plane, moving: &Plane, t: &Transform, model: Model) -> Accumulated {
    let (w, h) = (template.width, template.height);
    let (cx, cy) = center(template);
    let scale = norm_scale(template);
    let n = model.parameters();
    // The gradient the Jacobian wants is with respect to the centered
    // and scaled coordinate, not the pixel: a central difference is
    // half the neighbors' difference, and a normalized unit is `scale`
    // pixels.
    let gradient = 0.5 * scale;
    let rows: Vec<Accumulated> = (1..h - 1)
        .into_par_iter()
        .map(|y| {
            let mut acc = Accumulated::ZERO;
            let (mut u, mut v) = t.apply(1.0, y as f64);
            let mut sd = [0.0f64; 6];
            let yn = (y as f64 - cy) / scale;
            for x in 1..w - 1 {
                if let Some(warped) = sample(moving, u, v) {
                    let row = y * w + x;
                    let e = (warped - template.data[row]) as f64;
                    let gx = gradient * (template.data[row + 1] - template.data[row - 1]) as f64;
                    let gy = gradient * (template.data[row + w] - template.data[row - w]) as f64;
                    let xn = (x as f64 - cx) / scale;
                    steepest_descent(model, gx, gy, xn, yn, &mut sd);
                    let mut k = 0;
                    for i in 0..n {
                        let si = sd[i];
                        acc.b[i] += si * e;
                        acc.sd_sum[i] += si;
                        for s in sd.iter().take(n).skip(i) {
                            acc.hessian[k] += si * s;
                            k += 1;
                        }
                    }
                    acc.error_sum += e;
                    acc.sse += e * e;
                    acc.count += 1;
                }
                u += t.m[0];
                v += t.m[3];
            }
            acc
        })
        .collect();
    // Summed in row order, so the answer does not depend on how the
    // rows were split across threads.
    let mut total = Accumulated::ZERO;
    for row in &rows {
        total.add(row);
    }
    total
}

/// The error alone, without the Hessian: what a fit is judged by.
fn score(template: &Plane, moving: &Plane, t: &Transform) -> Score {
    let (w, h) = (template.width, template.height);
    let rows: Vec<(f64, f64, u64)> = (1..h - 1)
        .into_par_iter()
        .map(|y| {
            let (mut u, mut v) = t.apply(1.0, y as f64);
            let mut sse = 0.0;
            let mut error_sum = 0.0;
            let mut count = 0;
            for x in 1..w - 1 {
                if let Some(warped) = sample(moving, u, v) {
                    let e = (warped - template.data[y * w + x]) as f64;
                    error_sum += e;
                    sse += e * e;
                    count += 1;
                }
                u += t.m[0];
                v += t.m[3];
            }
            (sse, error_sum, count)
        })
        .collect();
    let mut total = Score {
        sse: 0.0,
        error_sum: 0.0,
        count: 0,
    };
    for (sse, error_sum, count) in rows {
        total.sse += sse;
        total.error_sum += error_sum;
        total.count += count;
    }
    total
}

fn center(plane: &Plane) -> (f64, f64) {
    (
        (plane.width - 1) as f64 / 2.0,
        (plane.height - 1) as f64 / 2.0,
    )
}

fn norm_scale(plane: &Plane) -> f64 {
    0.5 * ((plane.width as f64).hypot(plane.height as f64))
}

/// Gauss-Newton on one level, inverse-compositionally: the step is
/// found in the template's frame and composed onto the running
/// transform as its inverse.
fn run_level(template: &Plane, moving: &Plane, start: Transform, options: &Options) -> LevelResult {
    let (cx, cy) = center(template);
    let scale = norm_scale(template);
    let from_normalized = Transform {
        m: [scale, 0.0, cx, 0.0, scale, cy],
    };
    let to_normalized = from_normalized
        .inverse()
        .expect("the centering map is invertible");

    let mut t = start;
    let mut best = start;
    let mut best_sse = f64::INFINITY;
    let mut iterations = 0;
    let mut converged = false;
    let cap = options.max_iterations.max(1);

    for _ in 0..cap {
        iterations += 1;
        let acc = accumulate(template, moving, &t, options.model);
        if acc.count == 0 {
            t = best;
            break;
        }
        let (hessian, b, sse) = acc.system(options.model.parameters());
        // Hazard, for whoever reads this next: the two variances being
        // compared are means over the pixels that landed inside the
        // moving frame, and that set changes with the warp. A step
        // that moves the frame off its neighbor can in principle lower
        // the mean while raising the total, and this test would then
        // accept it. It wants a common set of pixels, or the total
        // over a fixed denominator, to be airtight. It has not been
        // made to fire — the levels above have the warp close enough
        // by the time the overlap moves at all — so it is written down
        // rather than worked around.
        if sse > best_sse {
            // The step made it worse: the one before it was the answer.
            t = best;
            converged = true;
            break;
        }
        best = t;
        best_sse = sse;
        let Some(dp) = solve(&hessian, &b, options.model.parameters()) else {
            converged = true;
            break;
        };
        let step = increment(options.model, &dp);
        let Some(back) = from_normalized
            .compose(&step)
            .compose(&to_normalized)
            .inverse()
        else {
            converged = true;
            break;
        };
        let next = t.compose(&back);
        let moved = next.corner_distance(&t, template.width, template.height);
        if !moved.is_finite() {
            break;
        }
        t = next;
        if moved < options.epsilon {
            converged = true;
            break;
        }
    }
    LevelResult {
        transform: t,
        iterations,
        converged,
    }
}

/// Solve `H dp = b` for a symmetric positive-definite `H` given as its
/// packed upper triangle, by Cholesky. `None` when the normal
/// equations are singular or near it, which is what a frame with
/// nothing to fit produces.
// Matrix code: the indices are the mathematics, and naming them away
// would only hide it.
#[allow(clippy::needless_range_loop)]
fn solve(packed: &[f64; 21], b: &[f64; 6], n: usize) -> Option<[f64; 6]> {
    let mut h = [[0.0f64; 6]; 6];
    let mut k = 0;
    let mut largest = 0.0f64;
    for i in 0..n {
        for j in i..n {
            h[i][j] = packed[k];
            h[j][i] = packed[k];
            k += 1;
        }
        largest = largest.max(h[i][i]);
    }
    if largest <= 0.0 || !largest.is_finite() {
        return None;
    }
    // Cholesky, in place in the lower triangle.
    let mut l = [[0.0f64; 6]; 6];
    for i in 0..n {
        for j in 0..=i {
            let mut sum = h[i][j];
            for p in 0..j {
                sum -= l[i][p] * l[j][p];
            }
            if i == j {
                if sum <= 1e-10 * largest {
                    return None;
                }
                l[i][i] = sum.sqrt();
            } else {
                l[i][j] = sum / l[j][j];
            }
        }
    }
    let mut y = [0.0f64; 6];
    for i in 0..n {
        let mut sum = b[i];
        for p in 0..i {
            sum -= l[i][p] * y[p];
        }
        y[i] = sum / l[i][i];
    }
    let mut x = [0.0f64; 6];
    for i in (0..n).rev() {
        let mut sum = y[i];
        for p in i + 1..n {
            sum -= l[p][i] * x[p];
        }
        x[i] = sum / l[i][i];
    }
    if x.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some(x)
}

/// What a sample outside the source becomes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edge {
    /// This value.
    Fill(f32),
    /// The nearest sample the source has.
    Clamp,
}

/// `src` resampled through `t`, bilinearly: the output's `(x, y)` is
/// `src` at `t.apply(x, y)`.
///
/// With the transform a [`fit`] returned, this is the moving frame put
/// into the reference's frame, ready to merge.
///
/// An output with no width or no height is empty; a source with no
/// width or no height puts [`Edge`] everywhere, since every sample of
/// it is outside.
pub fn warp_plane(
    src: &[f32],
    width: usize,
    height: usize,
    out_width: usize,
    out_height: usize,
    t: &Transform,
    edge: Edge,
) -> Vec<f32> {
    assert_eq!(src.len(), width * height);
    warp_interleaved(src, width, height, 1, out_width, out_height, t, edge)
}

/// [`warp_plane`] for interleaved samples of any channel count, at
/// the source's own size: what a merge warps a frame with when it
/// does not care which space the samples are in.
pub fn warp_samples(
    src: &[f32],
    width: usize,
    height: usize,
    channels: usize,
    t: &Transform,
    edge: Edge,
) -> Vec<f32> {
    assert_eq!(src.len(), width * height * channels);
    warp_interleaved(src, width, height, channels, width, height, t, edge)
}

/// [`warp_plane`] for a working-space image, at its own size.
pub fn warp_image(src: &WorkingImage, t: &Transform, edge: Edge) -> WorkingImage {
    let data = warp_interleaved(
        &src.data,
        src.width,
        src.height,
        WorkingImage::CHANNELS,
        src.width,
        src.height,
        t,
        edge,
    );
    WorkingImage {
        width: src.width,
        height: src.height,
        data,
    }
}

/// [`warp_plane`] for a camera-space image, at its own size: where a
/// stack is merged.
pub fn warp_camera_image(src: &CameraImage, t: &Transform, edge: Edge) -> CameraImage {
    let data = warp_interleaved(
        &src.data,
        src.width,
        src.height,
        CameraImage::CHANNELS,
        src.width,
        src.height,
        t,
        edge,
    );
    CameraImage {
        width: src.width,
        height: src.height,
        data,
    }
}

#[allow(clippy::too_many_arguments)]
fn warp_interleaved(
    src: &[f32],
    width: usize,
    height: usize,
    channels: usize,
    out_width: usize,
    out_height: usize,
    t: &Transform,
    edge: Edge,
) -> Vec<f32> {
    assert_eq!(src.len(), width * height * channels);
    if out_width == 0 || out_height == 0 {
        return Vec::new();
    }
    if width == 0 || height == 0 {
        // Every sample is outside a frame with no pixels in it.
        let outside = match edge {
            Edge::Fill(value) => value,
            Edge::Clamp => 0.0,
        };
        return vec![outside; out_width * out_height * channels];
    }
    let mut out = vec![0.0f32; out_width * out_height * channels];
    out.par_chunks_mut(out_width * channels)
        .enumerate()
        .for_each(|(y, line)| {
            let (mut u, mut v) = t.apply(0.0, y as f64);
            for px in line.chunks_mut(channels) {
                let inside =
                    u >= 0.0 && v >= 0.0 && u <= (width - 1) as f64 && v <= (height - 1) as f64;
                match (inside, edge) {
                    (false, Edge::Fill(value)) => px.fill(value),
                    _ => {
                        let cu = u.clamp(0.0, (width - 1) as f64);
                        let cv = v.clamp(0.0, (height - 1) as f64);
                        let x0 = cu.floor();
                        let y0 = cv.floor();
                        let fx = (cu - x0) as f32;
                        let fy = (cv - y0) as f32;
                        let i = x0 as usize;
                        let j = y0 as usize;
                        let i1 = (i + 1).min(width - 1);
                        let j1 = (j + 1).min(height - 1);
                        let (r0, r1) = (j * width, j1 * width);
                        for (c, o) in px.iter_mut().enumerate() {
                            let p00 = src[(r0 + i) * channels + c];
                            let p01 = src[(r0 + i1) * channels + c];
                            let p10 = src[(r1 + i) * channels + c];
                            let p11 = src[(r1 + i1) * channels + c];
                            let a = p00 + fx * (p01 - p00);
                            let b = p10 + fx * (p11 - p10);
                            *o = a + fy * (b - a);
                        }
                    }
                }
                u += t.m[0];
                v += t.m[3];
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lattice value at `(ix, iy)`, 0 to 1: the texture's
    /// randomness, from the coordinates alone, so every run and every
    /// level sees the same picture.
    fn lattice(ix: i64, iy: i64) -> f64 {
        let mut h = (ix as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
        h ^= h >> 29;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 32;
        (h >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Value noise on a lattice of `cell` pixels, smoothstepped so it
    /// has a gradient everywhere.
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

    /// A texture with structure at five scales, from about 160 pixels
    /// down to ten, aperiodic at every one of them, its octaves
    /// falling off slowly enough that the finest carries most of the
    /// gradient.
    ///
    /// All three properties are the point. No period, so a coarse
    /// level cannot mistake one feature for the next. Fine detail
    /// dominating, so the full-resolution fit has the narrow capture
    /// range a real frame gives it and the pyramid has something to
    /// do. And nothing under about ten pixels, because a texture with
    /// energy at the sampling limit is resampled differently on two
    /// grids that are not parallel, which is a mismatch no transform
    /// can take out and would be measured here as the fit's error.
    /// Always positive, so it stands in for a linear luminance.
    fn texture(x: f64, y: f64) -> f32 {
        let mut v = 0.6;
        let mut amplitude = 0.35;
        let mut cell = 160.0;
        for _ in 0..5 {
            v += amplitude * (octave(x, y, cell) - 0.5);
            amplitude *= 0.85;
            cell /= 2.0;
        }
        v as f32
    }

    /// The texture on a grid, each sample the mean over the pixel's
    /// own footprint, three by three, as a sensor integrates over its
    /// photosites.
    fn plane_of(width: usize, height: usize, f: impl Fn(f64, f64) -> f32 + Sync) -> Vec<f32> {
        const OFFSETS: [f64; 3] = [-1.0 / 3.0, 0.0, 1.0 / 3.0];
        (0..width * height)
            .into_par_iter()
            .map(|k| {
                let (x, y) = ((k % width) as f64, (k / width) as f64);
                let mut sum = 0.0;
                for dy in OFFSETS {
                    for dx in OFFSETS {
                        sum += f(x + dx, y + dy);
                    }
                }
                sum / 9.0
            })
            .collect()
    }

    /// The reference frame: the texture on the grid.
    fn reference(width: usize, height: usize) -> Vec<f32> {
        plane_of(width, height, texture)
    }

    /// The frame whose fit against [`reference`] is `t`, taken from
    /// the texture itself rather than resampled from the reference, so
    /// what the tests measure is the fit and not an interpolation.
    fn moved(width: usize, height: usize, t: &Transform) -> Vec<f32> {
        let back = t.inverse().expect("an invertible truth");
        plane_of(width, height, |x, y| {
            let (u, v) = back.apply(x, y);
            texture(u, v)
        })
    }

    fn middle(width: usize, height: usize) -> [f64; 2] {
        [(width - 1) as f64 / 2.0, (height - 1) as f64 / 2.0]
    }

    /// A shear and two scales about the frame's middle, with a shift.
    fn affine_truth(width: usize, height: usize) -> Transform {
        let about = middle(width, height);
        let linear = Transform {
            m: [1.006, 0.013, 0.0, -0.009, 0.994, 0.0],
        };
        Transform::translation(about[0] + 3.5, about[1] - 2.25)
            .compose(&linear)
            .compose(&Transform::translation(-about[0], -about[1]))
    }

    /// How far a fit is from the truth, in pixels, at the worst corner.
    fn error(fit: &Fit, truth: &Transform, width: usize, height: usize) -> f64 {
        truth.corner_distance(&fit.transform, width, height)
    }

    #[test]
    fn the_transform_composes_and_inverts() {
        let a = Transform::similarity(1.3, 0.4, [10.0, -3.0], [2.0, 5.0]);
        let b = Transform {
            m: [0.9, 0.05, -4.0, -0.02, 1.1, 7.0],
        };
        let round = a.compose(&a.inverse().expect("invertible"));
        assert!(round.corner_distance(&Transform::IDENTITY, 100, 100) < 1e-9);
        // Composition is application in order: a after b.
        let (x, y) = (13.0, -7.0);
        let (bx, by) = b.apply(x, y);
        let (abx, aby) = a.apply(bx, by);
        let (cx, cy) = a.compose(&b).apply(x, y);
        assert!((abx - cx).abs() < 1e-9 && (aby - cy).abs() < 1e-9);
        // A similarity reports back the scale and angle it was built from.
        assert!((a.scale() - 1.3).abs() < 1e-12);
        assert!((a.rotation() - 0.4).abs() < 1e-12);
        // The same transform in half-resolution coordinates takes a
        // point to half of where it did.
        let (ax, ay) = a.apply(x, y);
        let (hx, hy) = a.at_scale(0.5).apply(x / 2.0, y / 2.0);
        assert!((hx - ax / 2.0).abs() < 1e-9 && (hy - ay / 2.0).abs() < 1e-9);
        // A singular transform has no inverse rather than an infinity.
        assert!(
            Transform {
                m: [1.0, 2.0, 0.0, 2.0, 4.0, 0.0]
            }
            .inverse()
            .is_none()
        );
    }

    #[test]
    fn recovers_a_sub_pixel_translation() {
        let (w, h) = (512usize, 384usize);
        let truth = Transform::translation(7.35, -4.2);
        let r = reference(w, h);
        let m = moved(w, h, &truth);
        for model in [Model::Translation, Model::Similarity, Model::Affine] {
            let options = Options {
                model,
                ..Options::default()
            };
            let got = fit(&r, &m, w, h, &options).expect("a fit");
            let e = error(&got, &truth, w, h);
            assert!(e < 0.02, "{model}: {e} px, residual {}", got.residual);
            assert!(got.residual < 0.01, "{model}: residual {}", got.residual);
            assert!(got.converged, "{model} ran out of iterations");
        }
    }

    #[test]
    fn recovers_a_rotation_and_a_scale() {
        let (w, h) = (512usize, 384usize);
        let truth = Transform::similarity(1.03, 3.0f64.to_radians(), middle(w, h), [5.0, -2.5]);
        let r = reference(w, h);
        let m = moved(w, h, &truth);
        let got = fit(&r, &m, w, h, &Options::default()).expect("a fit");
        let e = error(&got, &truth, w, h);
        assert!(e < 0.02, "{e} px, residual {}", got.residual);
        assert!((got.transform.scale() - 1.03).abs() < 1e-4, "{got:?}");
        assert!(
            (got.transform.rotation().to_degrees() - 3.0).abs() < 2e-3,
            "{got:?}"
        );
    }

    #[test]
    fn recovers_an_affine_a_similarity_cannot() {
        let (w, h) = (512usize, 384usize);
        let truth = affine_truth(w, h);
        let r = reference(w, h);
        let m = moved(w, h, &truth);
        let options = Options {
            model: Model::Affine,
            ..Options::default()
        };
        let got = fit(&r, &m, w, h, &options).expect("a fit");
        let e = error(&got, &truth, w, h);
        // A fifth of a pixel, against a few thousandths for a
        // translation or a rotation: a shear of over a percent
        // resamples the two frames differently enough that the best
        // sum of squares is not quite at the truth. It is the
        // resampling's error and not the fit's — the same answer comes
        // back from the truth as a starting point, and from a single
        // level with no pyramid at all.
        assert!(e < 0.5, "{e} px, residual {}", got.residual);
        // Both halves of that claim, since they are what separates a
        // resampling bias from a wrong Jacobian: starting from the
        // truth lands in the same place, and so does a single level
        // with no pyramid under it.
        let warm = fit_from(&r, &m, w, h, &options, &truth).expect("a fit");
        assert!(
            warm.transform.corner_distance(&got.transform, w, h) < 0.02,
            "cold {:?} warm {:?}",
            got.transform,
            warm.transform
        );
        let alone = Options {
            max_levels: 1,
            ..options
        };
        let one = fit_from(&r, &m, w, h, &alone, &truth).expect("a fit");
        assert!(
            one.transform.corner_distance(&got.transform, w, h) < 0.02,
            "pyramid {:?} one level {:?}",
            got.transform,
            one.transform
        );
        // The similarity has no shear to give, and says so in its
        // residual as well as in its answer.
        let similarity = fit(&r, &m, w, h, &Options::default()).expect("a fit");
        assert!(
            error(&similarity, &truth, w, h) > 5.0 * e,
            "the similarity did nearly as well as the affine"
        );
        assert!(
            similarity.residual > 3.0 * got.residual,
            "affine {} similarity {}",
            got.residual,
            similarity.residual
        );
    }

    #[test]
    fn stops_of_exposure_do_not_move_the_fit() {
        let (w, h) = (512usize, 384usize);
        let truth = Transform::similarity(1.01, 1.5f64.to_radians(), middle(w, h), [4.4, 2.2]);
        let r = reference(w, h);
        let m = moved(w, h, &truth);
        let even = fit(&r, &m, w, h, &Options::default()).expect("a fit");
        for (name, gain) in [
            ("a stop down", 0.5f32),
            ("a stop up", 2.0),
            ("three down", 0.125),
        ] {
            let frame: Vec<f32> = m.iter().map(|v| v * gain).collect();
            let got = fit(&r, &frame, w, h, &Options::default()).expect("a fit");
            let e = error(&got, &truth, w, h);
            assert!(e < 0.02, "{name}: {e} px, residual {}", got.residual);
            // And it is the same fit, not merely a good one: a stop is
            // an additive constant in the log, which the mean takes
            // off and the eliminated offset mops up after it.
            let apart = got.transform.corner_distance(&even.transform, w, h);
            assert!(
                apart < 1e-4,
                "{name}: {apart} px from the even-exposure fit"
            );
        }
    }

    #[test]
    fn a_shift_past_the_finest_levels_reach_needs_the_pyramid() {
        // Twelve percent of the width. One level holds under a tenth
        // and is nowhere near this; the pyramid holds to fifteen
        // percent, so both sides of the comparison have room.
        let (w, h) = (768usize, 576usize);
        let truth = Transform::translation(73.7, -55.3);
        let r = reference(w, h);
        let m = moved(w, h, &truth);
        let full = fit(&r, &m, w, h, &Options::default()).expect("a fit");
        assert!(error(&full, &truth, w, h) < 0.05, "{:?}", full.transform);
        assert!(full.aligned(), "residual {}", full.residual);
        let alone = Options {
            max_levels: 1,
            ..Options::default()
        };
        let one = fit(&r, &m, w, h, &alone).expect("a fit of sorts");
        assert!(
            error(&one, &truth, w, h) > 20.0,
            "one level found it after all: {one:?}"
        );
        // And a caller can tell the two apart without knowing the
        // truth — but only by the residual, which is what
        // [`Fit::aligned`] looks at.
        assert!(!one.aligned(), "residual {}", one.residual);
        assert!(
            one.residual > 10.0 * full.residual,
            "good {} bad {}",
            full.residual,
            one.residual
        );
    }

    #[test]
    fn a_converged_fit_can_still_be_the_wrong_answer() {
        // Why `converged` is not the thing to check. Asked for a shift
        // of twenty-eight percent of the width, well past what the
        // pyramid can capture, the fit walks into another valley,
        // settles there to its own satisfaction, and reports two
        // thirds of the frame overlapping. Only the residual says what
        // happened.
        let (w, h) = (512usize, 384usize);
        let truth = Transform::translation(114.7, -86.0);
        let r = reference(w, h);
        let m = moved(w, h, &truth);
        let got = fit(&r, &m, w, h, &Options::default()).expect("a fit of sorts");
        assert!(error(&got, &truth, w, h) > 50.0, "{got:?}");
        assert!(
            got.converged && got.overlap > 0.5,
            "the hazard this guards has gone away: {got:?}"
        );
        assert!(!got.aligned(), "residual {}", got.residual);
    }

    #[test]
    fn the_pyramid_reaches_a_tenth_of_the_frames_width() {
        // What the module doc promises, at two frame shapes. The
        // measured limit is half again this; twelve percent is the
        // number with margin under it.
        for (w, h) in [(512usize, 384usize), (768, 576)] {
            let r = reference(w, h);
            let shift = 0.12 * w as f64;
            for direction in [(0.8, -0.6), (-0.6, 0.8), (1.0, 0.0)] {
                let truth = Transform::translation(shift * direction.0, shift * direction.1);
                let m = moved(w, h, &truth);
                let got = fit(&r, &m, w, h, &Options::default()).expect("a fit");
                let e = error(&got, &truth, w, h);
                // A fifth of a pixel: this test is about reach, and a
                // shift near the edge of it lands on the picture
                // rather than dead on the pixel. The accuracy tests
                // ask for thousandths, from shifts well inside.
                assert!(
                    e < 0.2 && got.aligned(),
                    "{w}x{h} shifted {shift:.0} px by {direction:?}: {e} px, \
                     residual {}",
                    got.residual
                );
            }
        }
    }

    #[test]
    fn an_infinity_in_a_frame_does_not_pass_for_an_alignment() {
        // Before the log took its floor from a median and skipped what
        // has no log, one non-finite sample took the mean to infinity,
        // the floor with it, and every sample to the floor: two flat
        // planes, a singular system, and a clean identity with a
        // residual of zero over all of the frame — a failure wearing
        // the face of a perfect fit.
        let (w, h) = (512usize, 384usize);
        let truth = Transform::translation(7.35, -4.2);
        let mut r = reference(w, h);
        let mut m = moved(w, h, &truth);
        for (name, poison) in [
            ("an infinity", f32::INFINITY),
            ("a negative infinity", f32::NEG_INFINITY),
            ("a NaN", f32::NAN),
        ] {
            let (was_r, was_m) = (r[100 * w + 100], m[200 * w + 300]);
            r[100 * w + 100] = poison;
            m[200 * w + 300] = poison;
            let got = fit(&r, &m, w, h, &Options::default()).expect("a fit");
            // One bad sample is one dark speck, and the fit is the
            // same fit.
            let e = error(&got, &truth, w, h);
            assert!(e < 0.02, "{name}: {e} px, residual {}", got.residual);
            assert!(got.aligned(), "{name}: residual {}", got.residual);
            assert!(got.residual > 0.0, "{name}: a residual of exactly zero");
            r[100 * w + 100] = was_r;
            m[200 * w + 300] = was_m;
        }
        // A frame that is nothing but infinities has no log at all,
        // and is refused rather than fitted.
        let all_bad = vec![f32::INFINITY; w * h];
        let err = fit(&r, &all_bad, w, h, &Options::default()).expect_err("no fit");
        assert!(matches!(err, Error::NoFit(_)), "{err}");
    }

    #[test]
    fn a_hot_photosite_does_not_move_the_fit() {
        // A single sample a billion times the white level. With the
        // log's floor taken from the mean this moved the fit 0.78 px
        // and the residual to 0.18; from the median it does neither.
        let (w, h) = (512usize, 384usize);
        let truth = Transform::translation(7.35, -4.2);
        let r = reference(w, h);
        let m = moved(w, h, &truth);
        let clean = fit(&r, &m, w, h, &Options::default()).expect("a fit");
        let mut hot = r.clone();
        hot[137 * w + 251] = 1e9;
        let got = fit(&hot, &m, w, h, &Options::default()).expect("a fit");
        let e = error(&got, &truth, w, h);
        assert!(e < 0.05, "{e} px, residual {}", got.residual);
        // The residual is another matter, and this is the record of
        // it: a root mean square over half a megapixel, with one
        // sample thirty stops out, goes from 0.002 to about 0.18 —
        // enough to fail `aligned` on a fit that is in fact exact.
        // Nothing here defends against that except the frame being
        // large: the same pixel in 24 megapixels moves the residual by
        // a fiftieth as much. A caller seeing a poor residual under a
        // sane transform should look at its inputs.
        assert!(got.residual > clean.residual, "{got:?}");
        assert!(got.residual < 0.3, "{got:?}");
    }

    #[test]
    fn a_warp_with_nothing_to_warp_is_empty_rather_than_a_panic() {
        let src = vec![1.0f32, 2.0, 3.0, 4.0];
        let t = Transform::translation(1.0, 1.0);
        // No output asked for.
        assert!(warp_plane(&src, 2, 2, 0, 5, &t, Edge::Clamp).is_empty());
        assert!(warp_plane(&src, 2, 2, 5, 0, &t, Edge::Clamp).is_empty());
        // Nothing to sample: every output pixel is outside.
        assert_eq!(
            warp_plane(&[], 0, 0, 2, 2, &t, Edge::Fill(-3.0)),
            vec![-3.0; 4]
        );
        assert_eq!(warp_plane(&[], 0, 0, 2, 2, &t, Edge::Clamp), vec![0.0; 4]);
        let empty = WorkingImage::new(0, 0);
        assert!(warp_image(&empty, &t, Edge::Clamp).data.is_empty());
        // And a frame with no corners is one point.
        assert_eq!(t.corner_distance(&Transform::IDENTITY, 0, 0), 2.0f64.sqrt());
    }

    #[test]
    fn a_singular_start_is_refused() {
        let (w, h) = (256usize, 256usize);
        let r = reference(w, h);
        let flat = Transform {
            m: [1.0, 2.0, 0.0, 2.0, 4.0, 0.0],
        };
        let err = fit_from(&r, &r, w, h, &Options::default(), &flat).expect_err("no fit");
        assert!(matches!(err, Error::NoFit(_)), "{err}");
    }

    #[test]
    fn the_options_that_are_clamped_are_reported() {
        let (w, h) = (512usize, 384usize);
        let r = reference(w, h);
        let m = moved(w, h, &Transform::translation(3.0, -2.0));
        // A pyramid deeper than the frame allows, a finest level under
        // the coarsest, and no iterations: all clamped, and the fit
        // says what it ran with.
        let options = Options {
            max_levels: 99,
            finest_level: 99,
            max_iterations: 0,
            min_side: 0,
            ..Options::default()
        };
        let got = fit(&r, &m, w, h, &options).expect("a fit");
        assert_eq!(got.finest, got.levels - 1);
        assert!(got.levels > 1 && got.levels < 99);
        // The default descends to full resolution and says so.
        let got = fit(&r, &m, w, h, &Options::default()).expect("a fit");
        assert_eq!(got.finest, 0);
    }

    #[test]
    fn a_flat_frame_is_a_failure_not_a_transform() {
        let (w, h) = (256usize, 256usize);
        let flat = vec![0.4f32; w * h];
        let err = fit(&flat, &flat, w, h, &Options::default()).expect_err("no fit");
        assert!(matches!(err, Error::NoFit(_)), "{err}");
        // A frame flat but for a whisper of dither is the same answer:
        // there is nothing in it to fit.
        let dithered: Vec<f32> = (0..w * h)
            .map(|k| 0.4 + ((k * 2654435761usize) % 17) as f32 * 1e-8)
            .collect();
        let err = fit(&dithered, &flat, w, h, &Options::default()).expect_err("no fit");
        assert!(matches!(err, Error::NoFit(_)), "{err}");
        // A textured reference against a blank frame fails too, rather
        // than reporting a confident transform onto nothing.
        let err = fit(&reference(w, h), &flat, w, h, &Options::default()).expect_err("no fit");
        assert!(matches!(err, Error::NoFit(_)), "{err}");
        // Frames too small to hold a pyramid level, and planes that
        // are not the size they say, are refused rather than indexed.
        assert!(fit(&[0.0; 4], &[0.0; 4], 2, 2, &Options::default()).is_err());
        assert!(fit(&[0.0; 4], &[0.0; 9], 3, 3, &Options::default()).is_err());
    }

    #[test]
    fn unrelated_frames_leave_a_residual_a_caller_can_see() {
        let (w, h) = (512usize, 384usize);
        let r = reference(w, h);
        // A different picture: the same texture read from somewhere
        // else entirely, and turned.
        let other = plane_of(w, h, |x, y| texture(4000.0 - y, 7000.0 + x));
        let poor = fit(&r, &other, w, h, &Options::default()).expect("some fit");
        assert!(poor.residual > 0.5, "{}", poor.residual);
        let truth = Transform::translation(3.5, 1.25);
        let honest = fit(&r, &moved(w, h, &truth), w, h, &Options::default()).expect("a fit");
        assert!(honest.residual < 0.02, "{}", honest.residual);
    }

    #[test]
    fn a_warp_reproduces_a_ramp_exactly() {
        // Bilinear resampling is exact on a plane linear in x and y, so
        // the warp can be held to the arithmetic rather than a tolerance.
        let (w, h) = (64usize, 48usize);
        let ramp = plane_of(w, h, |x, y| (0.25 * x + 0.5 * y + 3.0) as f32);
        let t = Transform::similarity(0.9, 0.3, [20.0, 20.0], [4.0, -2.0]);
        let out = warp_plane(&ramp, w, h, w, h, &t, Edge::Fill(f32::NAN));
        let mut checked = 0;
        for y in 0..h {
            for x in 0..w {
                let v = out[y * w + x];
                if v.is_nan() {
                    continue;
                }
                let (u, vv) = t.apply(x as f64, y as f64);
                let want = 0.25 * u + 0.5 * vv + 3.0;
                assert!((v as f64 - want).abs() < 2e-5, "{x},{y}: {v} want {want}");
                checked += 1;
            }
        }
        assert!(checked > w * h / 2, "the warp landed almost nowhere");
    }

    #[test]
    fn a_warp_fills_or_clamps_outside_the_frame() {
        let (w, h) = (16usize, 16usize);
        let src = plane_of(w, h, |x, y| (x + y) as f32);
        let away = Transform::translation(-100.0, 0.0);
        let filled = warp_plane(&src, w, h, w, h, &away, Edge::Fill(-1.0));
        assert!(filled.iter().all(|v| *v == -1.0));
        let clamped = warp_plane(&src, w, h, w, h, &away, Edge::Clamp);
        assert!(clamped[0].abs() < 1e-6);
        assert!((clamped[5 * w] - 5.0).abs() < 1e-6);
    }

    #[test]
    fn the_fit_warps_the_moving_frame_back_onto_the_reference() {
        let (w, h) = (512usize, 384usize);
        let truth = Transform::similarity(1.02, 2.0f64.to_radians(), middle(w, h), [6.0, -3.0]);
        let r = reference(w, h);
        let m = moved(w, h, &truth);
        let got = fit(&r, &m, w, h, &Options::default()).expect("a fit");
        // Over the interior, where the resampling has neighbors on
        // every side, the warped frame is the reference's own picture.
        let difference = |t: &Transform| {
            let aligned = warp_plane(&m, w, h, w, h, t, Edge::Clamp);
            let mut sse = 0.0f64;
            let mut n = 0u32;
            for y in 24..h - 24 {
                for x in 24..w - 24 {
                    sse += ((aligned[y * w + x] - r[y * w + x]) as f64).powi(2);
                    n += 1;
                }
            }
            (sse / n as f64).sqrt()
        };
        let aligned = difference(&got.transform);
        // Half a pixel out is the scale the answer is measured
        // against: the alignment has to be several times better than
        // that, not merely small.
        let nudged = difference(&Transform::translation(0.5, 0.0).compose(&got.transform));
        assert!(
            aligned * 4.0 < nudged,
            "aligned {aligned}, half a pixel out {nudged}"
        );
        // And the inverse takes the reference the other way.
        let there = got.transform.inverse().expect("invertible");
        let back = warp_plane(&r, w, h, w, h, &there, Edge::Clamp);
        let (cx, cy) = (w / 2, h / 2);
        assert!((back[cy * w + cx] - m[cy * w + cx]).abs() < 5e-3);
    }

    #[test]
    fn a_resampled_frame_with_a_missing_border_still_fits() {
        // The moving frame made the way a real one arrives: resampled,
        // with nothing outside the original's edge.
        let (w, h) = (512usize, 384usize);
        let truth = Transform::similarity(1.015, 2.5f64.to_radians(), middle(w, h), [9.0, -6.0]);
        let r = reference(w, h);
        let m = warp_plane(
            &r,
            w,
            h,
            w,
            h,
            &truth.inverse().expect("invertible"),
            Edge::Clamp,
        );
        let got = fit(&r, &m, w, h, &Options::default()).expect("a fit");
        let e = error(&got, &truth, w, h);
        assert!(e < 0.1, "{e} px, residual {}", got.residual);
        assert!(got.overlap > 0.9, "{}", got.overlap);
    }

    #[test]
    fn stopping_short_of_full_resolution_costs_a_tenth_of_a_pixel() {
        let (w, h) = (512usize, 384usize);
        let truth = Transform::similarity(1.01, 1.0f64.to_radians(), middle(w, h), [5.0, -4.0]);
        let r = reference(w, h);
        let m = moved(w, h, &truth);
        let mut last = 0.0;
        for finest in [0usize, 1, 2] {
            let options = Options {
                finest_level: finest,
                ..Options::default()
            };
            let got = fit(&r, &m, w, h, &options).expect("a fit");
            let e = error(&got, &truth, w, h);
            assert!(e < 0.1, "at level {finest}: {e} px");
            assert!(e >= last, "level {finest} beat the finer level");
            last = e;
        }
    }

    #[test]
    fn a_guess_saves_the_pyramid_the_journey() {
        let (w, h) = (512usize, 384usize);
        let truth = Transform::translation(51.2, -38.4);
        let r = reference(w, h);
        let m = moved(w, h, &truth);
        // A single level cannot find this shift cold; from a guess
        // twenty pixels out, it can.
        let alone = Options {
            max_levels: 1,
            ..Options::default()
        };
        let guess = Transform::translation(45.0, -30.0);
        let got = fit_from(&r, &m, w, h, &alone, &guess).expect("a fit");
        assert!(error(&got, &truth, w, h) < 0.05, "{got:?}");
    }

    #[test]
    fn an_image_warps_its_channels_together() {
        let (w, h) = (32usize, 24usize);
        let mut image = WorkingImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                image.data[i] = x as f32;
                image.data[i + 1] = y as f32;
                image.data[i + 2] = (x + y) as f32;
            }
        }
        let t = Transform::translation(2.0, 3.0);
        let out = warp_image(&image, &t, Edge::Clamp);
        assert_eq!(out.pixel(5, 5), [7.0, 8.0, 15.0]);
        let camera = CameraImage::from_data(w, h, image.data.clone()).expect("a camera image");
        let out = warp_camera_image(&camera, &t, Edge::Clamp);
        assert_eq!(out.pixel(5, 5), [7.0, 8.0, 15.0]);
    }

    #[test]
    fn luminance_weights_the_channels() {
        let mut image = WorkingImage::new(2, 1);
        image.data.copy_from_slice(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let l = luminance(&image);
        assert!((l[0] - LUMA[0]).abs() < 1e-6 && (l[1] - LUMA[1]).abs() < 1e-6);
        let camera = CameraImage::from_data(2, 1, image.data.clone()).expect("a camera image");
        let l = camera_luminance(&camera);
        assert!((l[0] - 0.25).abs() < 1e-6 && (l[1] - 0.5).abs() < 1e-6);
    }

    /// Not a check, a measurement: `cargo test --release -p
    /// greycard-core -- --ignored --nocapture timing_at`.
    #[test]
    #[ignore = "a timing, not a test"]
    fn timing_at_six_and_twenty_four_megapixels() {
        for (w, h) in [(3000usize, 2000usize), (6000, 4000)] {
            let r = reference(w, h);
            for model in [Model::Translation, Model::Similarity, Model::Affine] {
                // Each model is given a transform it can hold, so the
                // timings are of fits that converge.
                let truth = match model {
                    Model::Translation => Transform::translation(12.0, -8.0),
                    _ => Transform::similarity(
                        1.004,
                        0.4f64.to_radians(),
                        middle(w, h),
                        [12.0, -8.0],
                    ),
                };
                let m = moved(w, h, &truth);
                // The first fit of a run pays for the pages of every
                // plane it allocates; that is not what is being timed.
                let _ = fit(&r, &m, w, h, &Options::default());
                for finest in [0usize, 1] {
                    let options = Options {
                        model,
                        finest_level: finest,
                        ..Options::default()
                    };
                    let start = std::time::Instant::now();
                    let got = fit(&r, &m, w, h, &options).expect("a fit");
                    let took = start.elapsed();
                    println!(
                        "{w}x{h} {model:>11} to level {finest}: {:>7.0} ms, {} levels down to \
                         {}x{}, {} iterations, residual {:.4}, error {:.4} px",
                        took.as_secs_f64() * 1000.0,
                        got.levels,
                        got.coarsest.0,
                        got.coarsest.1,
                        got.iterations,
                        got.residual,
                        error(&got, &truth, w, h),
                    );
                }
            }
            // The first warp pays for the output's pages; the
            // second is the one worth printing.
            let m = moved(w, h, &Transform::translation(12.0, -8.0));
            let _ = warp_plane(&m, w, h, w, h, &Transform::IDENTITY, Edge::Clamp);
            let start = std::time::Instant::now();
            let warped = warp_plane(
                &m,
                w,
                h,
                w,
                h,
                &Transform::similarity(1.004, 0.4f64.to_radians(), middle(w, h), [12.0, -8.0]),
                Edge::Clamp,
            );
            println!(
                "{w}x{h} warp of one plane: {:>7.0} ms ({} samples)",
                start.elapsed().as_secs_f64() * 1000.0,
                warped.len()
            );
        }
    }
}
