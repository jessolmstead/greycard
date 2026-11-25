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

/// A 2-D projective transform, row-major: `[a, b, c, d, e, f, g, h, i]`
/// takes `(x, y)` to `((a x + b y + c) / w, (d x + e y + f) / w)` with
/// `w = g x + h y + i`. Coordinates as [`Transform`]'s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography {
    pub m: [f64; 9],
}

impl Default for Homography {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl From<Transform> for Homography {
    fn from(t: Transform) -> Self {
        let m = t.m;
        Self {
            m: [m[0], m[1], m[2], m[3], m[4], m[5], 0.0, 0.0, 1.0],
        }
    }
}

impl Homography {
    pub const IDENTITY: Self = Self {
        m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    };

    #[inline]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.m;
        let w = m[6] * x + m[7] * y + m[8];
        (
            (m[0] * x + m[1] * y + m[2]) / w,
            (m[3] * x + m[4] * y + m[5]) / w,
        )
    }

    /// The denominator at `(x, y)`: positive where the point is in front
    /// of the plane the transform sees it through.
    #[inline]
    pub fn depth(&self, x: f64, y: f64) -> f64 {
        self.m[6] * x + self.m[7] * y + self.m[8]
    }

    /// `self` after `inner`: the transform that applies `inner` first.
    pub fn compose(&self, inner: &Homography) -> Homography {
        let (a, b) = (&self.m, &inner.m);
        let mut m = [0.0; 9];
        for r in 0..3 {
            for c in 0..3 {
                m[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
            }
        }
        Homography { m }.normalized()
    }

    /// The same transform scaled so its last entry is one, when it is
    /// not near zero.
    pub fn normalized(self) -> Homography {
        let s = self.m[8];
        if s.abs() > 1e-12 && s.is_finite() {
            Homography {
                m: self.m.map(|v| v / s),
            }
        } else {
            self
        }
    }

    pub fn determinant(&self) -> f64 {
        let [a, b, c, d, e, f, g, h, i] = self.m;
        a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
    }

    /// `None` when singular.
    pub fn inverse(&self) -> Option<Homography> {
        let det = self.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let [a, b, c, d, e, f, g, h, i] = self.m;
        let r = [
            e * i - f * h,
            c * h - b * i,
            b * f - c * e,
            f * g - d * i,
            a * i - c * g,
            c * d - a * f,
            d * h - e * g,
            b * g - a * h,
            a * e - b * d,
        ];
        Some(
            Homography {
                m: r.map(|v| v / det),
            }
            .normalized(),
        )
    }

    /// The same geometric transform in coordinates scaled by `factor`.
    pub fn at_scale(&self, factor: f64) -> Homography {
        let mut m = self.m;
        m[2] *= factor;
        m[5] *= factor;
        m[6] /= factor;
        m[7] /= factor;
        Homography { m }
    }

    /// The same transform in coordinates shifted by `by`: what a
    /// transform between pixel indices is between pixel centers at
    /// halves, for `by` one half.
    pub fn shifted(&self, by: f64) -> Homography {
        let to = Homography {
            m: [1.0, 0.0, by, 0.0, 1.0, by, 0.0, 0.0, 1.0],
        };
        let from = Homography {
            m: [1.0, 0.0, -by, 0.0, 1.0, -by, 0.0, 0.0, 1.0],
        };
        to.compose(self).compose(&from)
    }

    /// The largest distance this transform and `other` put between the
    /// same point, over the corners of a `width`×`height` frame.
    pub fn corner_distance(&self, other: &Homography, width: usize, height: usize) -> f64 {
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

/// What the fit needs of a transform, an affine's or a homography's.
trait Warp: Copy + Send + Sync {
    fn compose(&self, inner: &Self) -> Self;
    fn inverse(&self) -> Option<Self>;
    fn at_scale(&self, factor: f64) -> Self;
    fn corner_distance(&self, other: &Self, width: usize, height: usize) -> f64;
    /// The scaling and shift into a level's normalized coordinates.
    fn normalizer(scale: f64, cx: f64, cy: f64) -> Self;
    /// The incremental warp a parameter step stands for.
    fn increment(model: Model, dp: &[f64; 8]) -> Self;
    /// `f(i, x, y)` for the `n` pixels of row `y` from column `x0`, at
    /// each one's warped place.
    fn row(&self, y: f64, x0: usize, n: usize, f: impl FnMut(usize, f64, f64));
}

impl Warp for Transform {
    fn compose(&self, inner: &Self) -> Self {
        Transform::compose(self, inner)
    }
    fn inverse(&self) -> Option<Self> {
        Transform::inverse(self)
    }
    fn at_scale(&self, factor: f64) -> Self {
        Transform::at_scale(self, factor)
    }
    fn corner_distance(&self, other: &Self, width: usize, height: usize) -> f64 {
        Transform::corner_distance(self, other, width, height)
    }
    fn normalizer(scale: f64, cx: f64, cy: f64) -> Self {
        Transform {
            m: [scale, 0.0, cx, 0.0, scale, cy],
        }
    }
    fn increment(model: Model, dp: &[f64; 8]) -> Self {
        increment(model, dp)
    }
    #[inline]
    fn row(&self, y: f64, x0: usize, n: usize, mut f: impl FnMut(usize, f64, f64)) {
        // Stepped along the row rather than applied at each pixel.
        let (mut u, mut v) = self.apply(x0 as f64, y);
        for i in 0..n {
            f(i, u, v);
            u += self.m[0];
            v += self.m[3];
        }
    }
}

impl Warp for Homography {
    fn compose(&self, inner: &Self) -> Self {
        Homography::compose(self, inner)
    }
    fn inverse(&self) -> Option<Self> {
        Homography::inverse(self)
    }
    fn at_scale(&self, factor: f64) -> Self {
        Homography::at_scale(self, factor)
    }
    fn corner_distance(&self, other: &Self, width: usize, height: usize) -> f64 {
        Homography::corner_distance(self, other, width, height)
    }
    fn normalizer(scale: f64, cx: f64, cy: f64) -> Self {
        Homography {
            m: [scale, 0.0, cx, 0.0, scale, cy, 0.0, 0.0, 1.0],
        }
    }
    fn increment(model: Model, dp: &[f64; 8]) -> Self {
        match model {
            Model::Homography => Homography {
                m: [
                    1.0 + dp[0],
                    dp[2],
                    dp[4],
                    dp[1],
                    1.0 + dp[3],
                    dp[5],
                    dp[6],
                    dp[7],
                    1.0,
                ],
            },
            other => increment(other, dp).into(),
        }
    }
    #[inline]
    fn row(&self, y: f64, x0: usize, n: usize, mut f: impl FnMut(usize, f64, f64)) {
        for i in 0..n {
            let (u, v) = self.apply((x0 + i) as f64, y);
            f(i, u, v);
        }
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
    /// Eight: the affine's six and the two of a perspective, a plane
    /// seen from another direction. What a camera turned about its lens
    /// does to a frame, and the keystone an affine cannot take out.
    /// Fitted by [`fit_homography_from`] onto a [`Homography`].
    Homography,
}

impl Model {
    pub fn parameters(&self) -> usize {
        match self {
            Model::Translation => 2,
            Model::Similarity => 4,
            Model::Affine => 6,
            Model::Homography => 8,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Model::Translation => "translation",
            Model::Similarity => "similarity",
            Model::Affine => "affine",
            Model::Homography => "homography",
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
            "homography" => Ok(Model::Homography),
            other => Err(format!(
                "want translation, similarity, affine, or homography, not {other}"
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
    /// What the gradient fit's answer is refined by, if anything. When
    /// a refinement is used, the fit's residual and overlap are scored
    /// again at the refined transform.
    pub refine: Refine,
}

/// What a fit's answer is refined by once the gradient fit has found
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Refine {
    /// Nothing: the gradient fit's answer is the answer.
    #[default]
    None,
    /// The shifts of small patches read off their phase, which a
    /// difference in blur between the frames does not move
    /// ([`refine_by_phase`]): for frames that differ in focus.
    Phase,
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
            refine: Refine::None,
        }
    }
}

/// What a fit came to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit<T = Transform> {
    /// Reference coordinates to moving coordinates: the moving frame
    /// sampled at `transform.apply(x, y)` is the reference's `(x, y)`.
    pub transform: T,
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

impl<T> Fit<T> {
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
    if options.model == Model::Homography {
        return Err(Error::Unsupported(
            "a homography is fitted by fit_homography_from".to_string(),
        ));
    }
    let mut fit = fit_warp(reference, moving, width, height, options, initial)?;
    if options.refine == Refine::Phase {
        match refine_by_phase(
            reference,
            moving,
            width,
            height,
            options.model,
            &fit.transform,
        ) {
            Ok(phased) => {
                // The residual a caller gates on is the placing's it
                // gets, so it is scored again where the refinement put
                // the frame; the gradient fit's would vouch for a
                // transform that is no longer the answer.
                if let Some((residual, overlap)) =
                    scored_at(reference, moving, width, height, options, &phased.transform)
                {
                    log::debug!(
                        "register: refined by the phase in {} passes, {} patches agreeing to \
                         {:.3} px; residual {:.4} -> {residual:.4}",
                        phased.passes,
                        phased.patches,
                        phased.agreement.miss,
                        fit.residual
                    );
                    fit.transform = phased.transform;
                    fit.residual = residual;
                    fit.overlap = overlap;
                }
            }
            Err(why) => log::debug!("register: the phase refinement not used: {why:?}"),
        }
    }
    Ok(fit)
}

/// The residual and overlap a fit would report for `t`: the error over
/// the overlap at `options.finest_level`, scored as [`fit_from`] scores
/// its own answer. None when the overlap is under `options.min_overlap`
/// or a plane has nothing to score.
fn scored_at(
    reference: &[f32],
    moving: &[f32],
    width: usize,
    height: usize,
    options: &Options,
    t: &Transform,
) -> Option<(f32, f32)> {
    let mut a = Plane {
        data: log_plane(reference, options.floor_stops),
        width,
        height,
    };
    let mut b = Plane {
        data: log_plane(moving, options.floor_stops),
        width,
        height,
    };
    // Down to the finest level the fit descends to, as its pyramid does.
    let mut level = 0;
    while level < options.finest_level
        && level + 1 < options.max_levels.max(1)
        && a.width.min(a.height) / 2 >= options.min_side.max(3)
    {
        a = downsample(&a);
        b = downsample(&b);
        level += 1;
    }
    let (reference_mean, deviation) = moments(&a);
    let (moving_mean, _) = moments(&b);
    if !(deviation.is_finite() && deviation as f32 >= options.min_contrast) {
        return None;
    }
    // Each log plane let go once standardized: three full planes at the
    // most, under what the fit's own pyramids held.
    let template = standardized(&a, reference_mean, deviation);
    drop(a);
    let moved = standardized(&b, moving_mean, deviation);
    drop(b);
    let scored = score(&template, &moved, &t.at_scale(0.5f64.powi(level as i32)));
    let countable = ((template.width - 2) * (template.height - 2)) as f32;
    let overlap = scored.count as f32 / countable;
    if overlap < options.min_overlap {
        return None;
    }
    let count = scored.count.max(1) as f64;
    let variance = (scored.sse - scored.error_sum * scored.error_sum / count) / count;
    Some((variance.max(0.0).sqrt() as f32, overlap))
}

/// [`fit_from`] onto a [`Homography`], for frames a turn of the camera
/// put a keystone between: a panorama's neighbors, started from what
/// their features said. `options.model` may be any; the homography's
/// eight parameters are fitted only under [`Model::Homography`].
pub fn fit_homography_from(
    reference: &[f32],
    moving: &[f32],
    width: usize,
    height: usize,
    options: &Options,
    initial: &Homography,
) -> Result<Fit<Homography>> {
    fit_warp(reference, moving, width, height, options, initial)
}

fn fit_warp<W: Warp + std::fmt::Debug>(
    reference: &[f32],
    moving: &[f32],
    width: usize,
    height: usize,
    options: &Options,
    initial: &W,
) -> Result<Fit<W>> {
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
            "the transform to start from is singular: {initial:?}"
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
    downsample_view(&src.data, src.width, src.height)
}

/// [`downsample`] of a plane held as a slice.
fn downsample_view(data: &[f32], w: usize, h: usize) -> Plane {
    const TAPS: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let ow = w.div_ceil(2);
    let oh = h.div_ceil(2);
    let mut rows = vec![0.0f32; ow * h];
    rows.par_chunks_mut(ow)
        .zip(data.par_chunks(w))
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
struct LevelResult<W> {
    transform: W,
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
    hessian: [f64; PACKED],
    /// The steepest-descent parameter updates.
    b: [f64; MOST],
    /// The steepest-descent images' own sums.
    sd_sum: [f64; MOST],
    error_sum: f64,
    sse: f64,
    count: u64,
}

impl Accumulated {
    const ZERO: Self = Self {
        hessian: [0.0; PACKED],
        b: [0.0; MOST],
        sd_sum: [0.0; MOST],
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
    fn system(&self, n: usize) -> ([f64; PACKED], [f64; MOST], f64) {
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

/// The most parameters a model has: the homography's.
const MOST: usize = 8;
/// The packed upper triangle of a Hessian of that many.
const PACKED: usize = MOST * (MOST + 1) / 2;

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
fn steepest_descent(model: Model, gx: f64, gy: f64, x: f64, y: f64, sd: &mut [f64; MOST]) {
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
        Model::Homography => {
            sd[0] = gx * x;
            sd[1] = gy * x;
            sd[2] = gx * y;
            sd[3] = gy * y;
            sd[4] = gx;
            sd[5] = gy;
            let r = gx * x + gy * y;
            sd[6] = -x * r;
            sd[7] = -y * r;
        }
    }
}

/// The incremental warp a parameter step stands for, in the centered
/// and scaled coordinates the Jacobian was taken in. A homography's
/// step is [`Homography`]'s own; here it is taken as its affine part.
fn increment(model: Model, dp: &[f64; MOST]) -> Transform {
    match model {
        Model::Translation => Transform {
            m: [1.0, 0.0, dp[0], 0.0, 1.0, dp[1]],
        },
        Model::Similarity => Transform {
            m: [1.0 + dp[0], -dp[1], dp[2], dp[1], 1.0 + dp[0], dp[3]],
        },
        Model::Affine | Model::Homography => Transform {
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
fn accumulate<W: Warp>(template: &Plane, moving: &Plane, t: &W, model: Model) -> Accumulated {
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
            let mut sd = [0.0f64; MOST];
            let yn = (y as f64 - cy) / scale;
            t.row(y as f64, 1, w - 2, |at, u, v| {
                let x = at + 1;
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
            });
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
fn score<W: Warp>(template: &Plane, moving: &Plane, t: &W) -> Score {
    let (w, h) = (template.width, template.height);
    let rows: Vec<(f64, f64, u64)> = (1..h - 1)
        .into_par_iter()
        .map(|y| {
            let mut sse = 0.0;
            let mut error_sum = 0.0;
            let mut count = 0;
            t.row(y as f64, 1, w - 2, |i, u, v| {
                if let Some(warped) = sample(moving, u, v) {
                    let e = (warped - template.data[y * w + i + 1]) as f64;
                    error_sum += e;
                    sse += e * e;
                    count += 1;
                }
            });
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
fn run_level<W: Warp>(
    template: &Plane,
    moving: &Plane,
    start: W,
    options: &Options,
) -> LevelResult<W> {
    let (cx, cy) = center(template);
    let scale = norm_scale(template);
    let from_normalized = W::normalizer(scale, cx, cy);
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
        let step = W::increment(options.model, &dp);
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
fn solve(packed: &[f64; PACKED], b: &[f64; MOST], n: usize) -> Option<[f64; MOST]> {
    let mut h = [[0.0f64; MOST]; MOST];
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
    let mut l = [[0.0f64; MOST]; MOST];
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
    let mut y = [0.0f64; MOST];
    for i in 0..n {
        let mut sum = b[i];
        for p in 0..i {
            sum -= l[i][p] * y[p];
        }
        y[i] = sum / l[i][i];
    }
    let mut x = [0.0f64; MOST];
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

/// How a warp resamples a frame: every warp here and the panorama's
/// canvas take one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kernel {
    Bilinear,
    /// Catmull-Rom, four taps an axis.
    Cubic,
    /// Lanczos with three lobes, six taps an axis: what a merge
    /// resamples a frame it keeps with, since bilinear softens more
    /// than a merge's weighting gains back. It rings at a hard edge,
    /// a few percent of the step; a merge clamps the warp to the
    /// frame's range ([`sample_range`]).
    Lanczos3,
}

impl Kernel {
    /// Its name in a merge's tag.
    pub fn name(self) -> &'static str {
        match self {
            Kernel::Bilinear => "bilinear",
            Kernel::Cubic => "cubic",
            Kernel::Lanczos3 => "lanczos3",
        }
    }

    /// The kernel of that name.
    pub fn named(name: &str) -> Option<Kernel> {
        [Kernel::Bilinear, Kernel::Cubic, Kernel::Lanczos3]
            .into_iter()
            .find(|k| k.name() == name)
    }

    /// Taps a side.
    pub fn reach(self) -> isize {
        match self {
            Kernel::Bilinear => 1,
            Kernel::Cubic => 2,
            Kernel::Lanczos3 => 3,
        }
    }

    /// The weights of the taps about a position `t` (0 to 1) past the
    /// sample before it: the taps from `1 - reach` to `reach` samples
    /// on. Lanczos's from three sines a position rather than two a tap:
    /// `sin(pi (t - o)) = (-1)^o sin(pi t)`, and the third-period sine
    /// of `t - o` by the angle-difference rule.
    #[inline]
    pub(crate) fn taps(self, t: f64) -> [f64; 6] {
        let mut w = [0.0f64; 6];
        let reach = self.reach();
        if self == Kernel::Lanczos3 {
            use std::f64::consts::PI;
            // cos and sin of pi o / 3, o from -2 to 3.
            const COS: [f64; 6] = [-0.5, 0.5, 1.0, 0.5, -0.5, -1.0];
            const SIN: [f64; 6] = [
                -0.866_025_403_784_438_6,
                -0.866_025_403_784_438_6,
                0.0,
                0.866_025_403_784_438_6,
                0.866_025_403_784_438_6,
                0.0,
            ];
            let s1 = (PI * t).sin();
            let (s3, c3) = (PI * t / 3.0).sin_cos();
            for (i, w) in w.iter_mut().enumerate() {
                let o = i as f64 - 2.0;
                let a = t - o;
                if a.abs() < 1e-9 {
                    *w = 1.0;
                    continue;
                }
                let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
                let third = s3 * COS[i] - c3 * SIN[i];
                *w = 3.0 * sign * s1 * third / (PI * PI * a * a);
            }
            return w;
        }
        for (i, w) in w.iter_mut().enumerate().take((2 * reach) as usize) {
            *w = self.weight(t - (i as isize - reach + 1) as f64);
        }
        w
    }

    #[inline]
    pub(crate) fn weight(self, t: f64) -> f64 {
        let a = t.abs();
        match self {
            Kernel::Bilinear => (1.0 - a).max(0.0),
            Kernel::Cubic => {
                if a < 1.0 {
                    1.5 * a * a * a - 2.5 * a * a + 1.0
                } else if a < 2.0 {
                    -0.5 * a * a * a + 2.5 * a * a - 4.0 * a + 2.0
                } else {
                    0.0
                }
            }
            Kernel::Lanczos3 => {
                if a < 1e-9 {
                    1.0
                } else if a < 3.0 {
                    let p = std::f64::consts::PI * a;
                    3.0 * p.sin() * (p / 3.0).sin() / (p * p)
                } else {
                    0.0
                }
            }
        }
    }

    /// The taps about `t` as `f32`, normalized to sum to one, so a
    /// flat picture stays flat whatever the position.
    #[inline]
    fn normalized(self, t: f64) -> [f32; 6] {
        let w = self.taps(t);
        let n = (2 * self.reach()) as usize;
        let sum: f64 = w[..n].iter().sum();
        let mut out = [0.0f32; 6];
        for (o, w) in out.iter_mut().zip(&w[..n]) {
            *o = (w / sum) as f32;
        }
        out
    }
}

/// The least and the most finite sample of `src`: what a merge clamps
/// a frame it warped with a ringing kernel to ([`clamp_samples`]), so
/// a ring at a hard edge puts no sample below the frame's darkest or
/// over its brightest. `(0, 0)` for a source with none.
pub fn sample_range(src: &[f32]) -> (f32, f32) {
    let (lo, hi) = src
        .par_iter()
        .filter(|v| v.is_finite())
        .fold(
            || (f32::INFINITY, f32::NEG_INFINITY),
            |(lo, hi), &v| (lo.min(v), hi.max(v)),
        )
        .reduce(
            || (f32::INFINITY, f32::NEG_INFINITY),
            |a, b| (a.0.min(b.0), a.1.max(b.1)),
        );
    if lo <= hi { (lo, hi) } else { (0.0, 0.0) }
}

/// Every sample of `samples` held to `range`; one not a number (an
/// [`Edge::Fill`] of it) stays as it is.
pub fn clamp_samples(samples: &mut [f32], range: (f32, f32)) {
    let (lo, hi) = range;
    samples.par_iter_mut().for_each(|v| *v = v.clamp(lo, hi));
}

/// `src` resampled through `t` by `kernel`: the output's `(x, y)` is
/// `src` at `t.apply(x, y)`.
///
/// With the transform a [`fit`] returned, this is the moving frame put
/// into the reference's frame, ready to merge.
///
/// An output with no width or no height is empty; a source with no
/// width or no height puts [`Edge`] everywhere, since every sample of
/// it is outside.
#[allow(clippy::too_many_arguments)]
pub fn warp_plane(
    src: &[f32],
    width: usize,
    height: usize,
    out_width: usize,
    out_height: usize,
    t: &Transform,
    edge: Edge,
    kernel: Kernel,
) -> Vec<f32> {
    assert_eq!(src.len(), width * height);
    warp_interleaved(
        src,
        (width, height, 1),
        (out_width, out_height),
        t,
        edge,
        kernel,
    )
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
    kernel: Kernel,
) -> Vec<f32> {
    assert_eq!(src.len(), width * height * channels);
    warp_interleaved(
        src,
        (width, height, channels),
        (width, height),
        t,
        edge,
        kernel,
    )
}

/// [`warp_plane`] for a working-space image, at its own size.
pub fn warp_image(src: &WorkingImage, t: &Transform, edge: Edge, kernel: Kernel) -> WorkingImage {
    let data = warp_interleaved(
        &src.data,
        (src.width, src.height, WorkingImage::CHANNELS),
        (src.width, src.height),
        t,
        edge,
        kernel,
    );
    WorkingImage {
        width: src.width,
        height: src.height,
        data,
    }
}

/// [`warp_plane`] for a camera-space image, at its own size: where a
/// stack is merged.
pub fn warp_camera_image(
    src: &CameraImage,
    t: &Transform,
    edge: Edge,
    kernel: Kernel,
) -> CameraImage {
    let data = warp_interleaved(
        &src.data,
        (src.width, src.height, CameraImage::CHANNELS),
        (src.width, src.height),
        t,
        edge,
        kernel,
    );
    CameraImage {
        width: src.width,
        height: src.height,
        data,
    }
}

/// `size` is the source's width, height, and channels; `out` the
/// warped picture's width and height.
fn warp_interleaved(
    src: &[f32],
    size: (usize, usize, usize),
    out: (usize, usize),
    t: &Transform,
    edge: Edge,
    kernel: Kernel,
) -> Vec<f32> {
    let (width, height, channels) = size;
    let (out_width, out_height) = out;
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
    let wide = kernel != Kernel::Bilinear;
    out.par_chunks_mut(out_width * channels)
        .enumerate()
        .for_each(|(y, line)| {
            let (mut u, mut v) = t.apply(0.0, y as f64);
            for px in line.chunks_mut(channels) {
                let inside =
                    u >= 0.0 && v >= 0.0 && u <= (width - 1) as f64 && v <= (height - 1) as f64;
                match (inside, edge) {
                    (false, Edge::Fill(value)) => px.fill(value),
                    _ if wide => {
                        let cu = u.clamp(0.0, (width - 1) as f64);
                        let cv = v.clamp(0.0, (height - 1) as f64);
                        wide_sample(src, size, (cu, cv), kernel, px);
                    }
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

/// One pixel of `src` (`size` its width, height, and channels) at
/// `at`, inside it, by a kernel wider than bilinear: the 2-D sum of
/// the separable kernel's taps, the taps past the edge held to it.
/// Direct for any transform; a translation's taps are the same at
/// every pixel, and computing them each time costs a few trig calls a
/// pixel, which measured under a second on a 45 MP frame.
// Grid code: the indices are the mathematics.
#[allow(clippy::needless_range_loop)]
#[inline]
fn wide_sample(
    src: &[f32],
    size: (usize, usize, usize),
    at: (f64, f64),
    kernel: Kernel,
    px: &mut [f32],
) {
    let (width, height, channels) = size;
    let (u, v) = at;
    let (x0, y0) = (u.floor(), v.floor());
    let (wx, wy) = (kernel.normalized(u - x0), kernel.normalized(v - y0));
    let reach = kernel.reach();
    let n = (2 * reach) as usize;
    let (ix, iy) = (x0 as isize - reach + 1, y0 as isize - reach + 1);
    let (w, h) = (width as isize, height as isize);
    let mut cols = [0usize; 6];
    let mut rows = [0usize; 6];
    for k in 0..n {
        cols[k] = (ix + k as isize).clamp(0, w - 1) as usize * channels;
        rows[k] = (iy + k as isize).clamp(0, h - 1) as usize * width * channels;
    }
    for (c, o) in px.iter_mut().enumerate() {
        let mut sum = 0.0f32;
        for j in 0..n {
            let row = &src[rows[j] + c..];
            let mut acc = 0.0f32;
            for i in 0..n {
                acc += wx[i] * row[cols[i]];
            }
            sum += wy[j] * acc;
        }
        *o = sum;
    }
}

/// The smaller of the two patch sizes [`refine_by_phase`] reads, at
/// the level it works on; the other is twice it. Each is read up to an
/// eighth of its side in cycles: periods of eight of the level's
/// pixels and longer.
const PHASE_SIDE: usize = 32;
/// The level it works on is the first whose short side is no longer
/// than this.
const PHASE_SIDE_MOST: usize = 2048;
/// Most times the patches are read and the transform corrected: each
/// pass takes out about half of what is left (a window does not move
/// with the content, so a patch reads a shift short), more once the
/// passes are stretched, and settling takes two passes running. A
/// refinement that has not settled by then is not used.
const PHASE_ROUNDS: usize = 16;
/// A pass that moves the corners by less than this, in the working
/// level's pixels, has settled. Not a hundredth: the passes jitter by
/// a few hundredths as patches come into the overlap and leave it, and
/// a refinement held to less would never settle; measured on the
/// synthetic stacks, the jitter is 0.02 to 0.07 once the approach is
/// done.
const PHASE_SETTLED: f64 = 0.05;
/// The most the patches may miss the transform fitted to them, at the
/// median, in the working level's pixels, for the refinement to be
/// used: past it the frames are not related by one transform of the
/// model's kind (a turn of the camera about a point other than its
/// lens, a scene with depth seen from two places), and which answer
/// the refinement settles on depends on which patches it weighs most.
/// See [`refine_by_phase`] for how it was set.
pub const PHASE_MISS_MOST: f64 = 0.4;
/// The most the patches may miss at the plain median, every patch
/// counted once, in the working level's pixels. See
/// [`refine_by_phase`] for how it was set.
pub const PHASE_PLAIN_MOST: f64 = 3.0;
/// The fewest patches of equal weight, `(Σw)² / Σw²`, that must carry
/// the refinement. See [`refine_by_phase`] for how it was set.
pub const PHASE_EFFECTIVE_LEAST: f64 = 8.0;
/// A sample at or above this share of its frame's largest is taken as
/// clipped, and a patch holding more than a sixty-fourth of them is
/// not read: a clipped highlight's edge moves with the exposure, not
/// with the frame.
const PHASE_CLIPPED: f32 = 0.98;

/// A plane borrowed: the phase reads its levels without copying the
/// full resolution.
#[derive(Clone, Copy)]
struct View<'a> {
    data: &'a [f32],
    width: usize,
    height: usize,
}

impl<'a> View<'a> {
    fn of(plane: &'a Plane) -> Self {
        Self {
            data: &plane.data,
            width: plane.width,
            height: plane.height,
        }
    }
}

/// One patch's shift, by the slope of its cross-power phase: where the
/// content of `moving` sits against `reference`'s, as `(dx, dy)` with
/// the moving patch's content at `x + d` being the reference's at `x`,
/// and how far to believe it. None for a patch with nothing in it, or
/// with clipped samples (at or over `clip`, the reference's and the
/// moving frame's) in it.
///
/// Why the phase: a symmetric blur's transfer function is real, so it
/// scales a frequency's amplitude and leaves its phase where it was,
/// and two patches that differ in defocus by any amount of it still
/// have cross-power phases that are a plane through the origin, its
/// slope the shift. A gradient fit has no such property (see
/// [`refine_by_phase`]). The patch is taken in linear light, as a
/// lens's blur acts, divided by its own mean so that an exposure
/// difference, a gain, is nothing, and under a Hann window. A blur
/// whose transfer function turns negative (a disc's past its first
/// zero) flips a band's phase by a half turn; the second and third
/// passes leave out any bin more than a quarter turn off the plane.
///
/// **What it can read.** The first pass fits the bins up to 2.5 cycles
/// a patch, and the phase of the highest of them wraps once the shift
/// passes a fifth of the patch's side: past that the slope it finds is
/// an alias. On a repeating pattern the alias is a whole period off,
/// and every patch agrees on it, so the patches alone cannot tell; it
/// is the gradient fit's answer, which [`refine_by_phase`] starts
/// from and may move only by a fraction of a patch, that keeps a
/// refinement from walking a period away (see its guard).
///
/// **How far to believe it.** The weight is the square of the patches'
/// coherence at the shift found: the cross power's projection onto the
/// fitted plane over the two patches' own power, 1 for one picture in
/// both, near 0 for noise or for two pictures with nothing in common.
/// It does not grow with the patch's contrast: a dark patch's noise, or
/// the edge of a highlight, has no more say than any other patch that
/// agrees as well.
///
/// `SIDE` is the patch's side, `K1` one more than the highest frequency
/// read (`SIDE / 8 + 1`), and `NX` the count of horizontal frequencies
/// (`2 K1 - 1`): three numbers for one, as arrays want them.
fn phase_shift<const SIDE: usize, const K1: usize, const NX: usize>(
    reference: View,
    moving: View,
    x0: usize,
    y0: usize,
    clip: (f32, f32),
) -> Option<(f64, f64, f64)> {
    use std::f64::consts::PI;
    const { assert!(K1 == SIDE / 8 + 1 && NX == 2 * K1 - 1) };
    let k = K1 - 1;
    let w = reference.width;
    let window: [f64; SIDE] =
        std::array::from_fn(|i| 0.5 - 0.5 * (2.0 * PI * (i as f64 + 0.5) / SIDE as f64).cos());
    let twiddle: [[(f64, f64); SIDE]; K1] = std::array::from_fn(|f| {
        std::array::from_fn(|n| {
            let t = -2.0 * PI * (f * n) as f64 / SIDE as f64;
            (t.cos(), t.sin())
        })
    });
    let patch = |p: View, clip: f32| -> Option<Vec<f64>> {
        let mut v: Vec<f64> = (0..SIDE * SIDE)
            .map(|k| p.data[(y0 + k / SIDE) * w + x0 + k % SIDE] as f64)
            .collect();
        let clipped = v.iter().filter(|&&s| s >= clip as f64).count();
        if clipped * 64 > v.len() {
            return None;
        }
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        if !(mean > 0.0 && mean.is_finite()) {
            return None;
        }
        for (k, x) in v.iter_mut().enumerate() {
            *x = (*x / mean - 1.0) * window[k % SIDE] * window[k / SIDE];
        }
        Some(v)
    };
    // The transform at |kx| <= k, 0 <= ky <= k: rows, then columns. A
    // negative frequency's twiddle is the conjugate of its positive's.
    let transform = |p: &[f64]| -> [[(f64, f64); NX]; K1] {
        let mut rows = [[(0.0, 0.0); NX]; SIDE];
        for (y, row) in rows.iter_mut().enumerate() {
            let line = &p[y * SIDE..(y + 1) * SIDE];
            for (j, out) in row.iter_mut().enumerate() {
                let kx = j as isize - k as isize;
                let (mut re, mut im) = (0.0, 0.0);
                for (v, (c, s)) in line.iter().zip(&twiddle[kx.unsigned_abs()]) {
                    re += v * c;
                    im += v * s;
                }
                *out = (re, if kx < 0 { -im } else { im });
            }
        }
        let mut out = [[(0.0, 0.0); NX]; K1];
        for (ky, line) in out.iter_mut().enumerate() {
            for (j, o) in line.iter_mut().enumerate() {
                let (mut re, mut im) = (0.0, 0.0);
                for (row, (c, s)) in rows.iter().zip(&twiddle[ky]) {
                    let (r, i) = row[j];
                    re += r * c - i * s;
                    im += r * s + i * c;
                }
                *o = (re, im);
            }
        }
        out
    };
    let (a, b) = (
        transform(&patch(reference, clip.0)?),
        transform(&patch(moving, clip.1)?),
    );
    // Each bin of the half plane: its angular frequency, the phase of
    // A conj(B), and the cross power's magnitude, which weighs it; and
    // the two patches' own power over the same bins.
    let mut bins = Vec::with_capacity(K1 * NX);
    let (mut power_a, mut power_b) = (0.0f64, 0.0f64);
    for ky in 0..K1 {
        for (j, (&(ar, ai), &(br, bi))) in a[ky].iter().zip(&b[ky]).enumerate() {
            let kx = j as isize - k as isize;
            if ky == 0 && kx <= 0 {
                continue;
            }
            power_a += ar * ar + ai * ai;
            power_b += br * br + bi * bi;
            let (cr, ci) = (ar * br + ai * bi, ai * br - ar * bi);
            let m = cr.hypot(ci);
            if m > 0.0 && m.is_finite() {
                bins.push((
                    2.0 * PI * kx as f64 / SIDE as f64,
                    2.0 * PI * ky as f64 / SIDE as f64,
                    ci.atan2(cr),
                    m,
                ));
            }
        }
    }
    // The plane through the origin by weighted least squares, three
    // times: the lowest frequencies alone, then every bin on the last
    // pass's residual, wrapped, a bin a quarter turn off left out.
    let lowest = 2.0 * PI * 2.5 / SIDE as f64;
    let mut d = (0.0f64, 0.0f64);
    let mut information = 0.0;
    for pass in 0..3 {
        let (mut sxx, mut sxy, mut syy, mut bx, mut by) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for &(gx, gy, phase, m) in &bins {
            if pass == 0 && gx.hypot(gy) > lowest {
                continue;
            }
            let mut r = phase - (gx * d.0 + gy * d.1);
            r -= 2.0 * PI * (r / (2.0 * PI)).round();
            if pass > 0 && r.abs() > PI / 2.0 {
                continue;
            }
            sxx += m * gx * gx;
            sxy += m * gx * gy;
            syy += m * gy * gy;
            bx += m * gx * r;
            by += m * gy * r;
        }
        let det = sxx * syy - sxy * sxy;
        if !(det > 0.0 && det.is_finite()) {
            return None;
        }
        d.0 += (syy * bx - sxy * by) / det;
        d.1 += (sxx * by - sxy * bx) / det;
        information = det.sqrt();
    }
    // The coherence at the plane found.
    let along: f64 = bins
        .iter()
        .map(|&(gx, gy, phase, m)| m * (phase - (gx * d.0 + gy * d.1)).cos())
        .sum();
    let coherence = (along / (power_a * power_b).sqrt()).clamp(0.0, 1.0);
    // The information the slope had, which grows with the patch's
    // contrast and is what a sharp patch is worth, times the square of
    // its coherence, which a patch of noise or of two different
    // pictures does not have whatever its contrast. Not capped: a cap
    // at the ninetieth percentile, or at 25 times the median, takes
    // from the sharp patches the say they earned and costs the
    // synthetic stacks half again of their error at the corners.
    let weight = information * coherence * coherence;
    (weight.is_finite() && weight > 0.0).then_some((d.0, d.1, weight))
}

/// One patch's reading: its middle, its shift, and its weight.
type Shift = (f64, f64, f64, f64, f64);

/// Every patch of `SIDE` (overlapping by half) that the moving frame,
/// placed by `t`, covers, and its shift: one past the frame's edge
/// holds the clamp's smear.
fn shifts<const SIDE: usize, const K1: usize, const NX: usize>(
    reference: View,
    warped: View,
    t: &Transform,
    clip: (f32, f32),
) -> Vec<Shift> {
    let (w, h) = (reference.width, reference.height);
    if w < SIDE || h < SIDE {
        return Vec::new();
    }
    let stride = SIDE / 2;
    let starts = |size: usize| (0..=(size - SIDE) / stride).map(move |i| i * stride);
    let places: Vec<(usize, usize)> = starts(h)
        .flat_map(|y| starts(w).map(move |x| (x, y)))
        .collect();
    let inside = |u: f64, v: f64| {
        let (u, v) = t.apply(u, v);
        u >= 0.0 && v >= 0.0 && u <= (w - 1) as f64 && v <= (h - 1) as f64
    };
    let middle = (SIDE - 1) as f64 / 2.0;
    places
        .par_iter()
        .filter(|&&(x, y)| {
            let (x0, y0) = (x as f64, y as f64);
            let (x1, y1) = (x0 + (SIDE - 1) as f64, y0 + (SIDE - 1) as f64);
            inside(x0, y0) && inside(x1, y0) && inside(x0, y1) && inside(x1, y1)
        })
        .filter_map(|&(x, y)| {
            let (dx, dy, weight) = phase_shift::<SIDE, K1, NX>(reference, warped, x, y, clip)?;
            Some((x as f64 + middle, y as f64 + middle, dx, dy, weight))
        })
        .collect()
}

/// The transform near the identity, of `model`'s kind, that takes each
/// patch's middle `p` to `p + d`: weighted least squares over the
/// patches, reweighted five times by Huber's rule on each patch's
/// miss, so that a patch the phase read wrongly (a moving subject, a
/// region with nothing in common) has a bounded say. With it, how far
/// the patches are from agreeing on that transform ([`Agreement`]).
/// None when the patches do not hold the model.
fn transform_of_shifts(
    field: &[Shift],
    model: Model,
    width: usize,
    height: usize,
) -> Option<(Transform, Agreement)> {
    let n = model.parameters().min(6);
    if field.len() * 2 < n * 4 {
        return None;
    }
    let (cx, cy) = ((width - 1) as f64 / 2.0, (height - 1) as f64 / 2.0);
    let scale = 0.5 * (width as f64).hypot(height as f64);
    // The rows of the design for one patch, in centered and scaled
    // coordinates: d = J p.
    let rows = |x: f64, y: f64| -> [[f64; MOST]; 2] {
        let (x, y) = ((x - cx) / scale, (y - cy) / scale);
        let mut r = [[0.0; MOST]; 2];
        match model {
            Model::Translation => {
                r[0][0] = 1.0;
                r[1][1] = 1.0;
            }
            Model::Similarity => {
                r[0][..4].copy_from_slice(&[x, -y, 1.0, 0.0]);
                r[1][..4].copy_from_slice(&[y, x, 0.0, 1.0]);
            }
            Model::Affine | Model::Homography => {
                r[0][..6].copy_from_slice(&[x, 0.0, y, 0.0, 1.0, 0.0]);
                r[1][..6].copy_from_slice(&[0.0, x, 0.0, y, 0.0, 1.0]);
            }
        }
        r
    };
    let misses_at = |p: &[f64; MOST]| -> Vec<f64> {
        field
            .iter()
            .map(|&(x, y, dx, dy, _)| {
                let r = rows(x, y);
                let at = |row: &[f64; MOST]| row.iter().zip(p).map(|(a, b)| a * b).sum::<f64>();
                (dx - at(&r[0])).hypot(dy - at(&r[1]))
            })
            .collect()
    };
    let mut robust = vec![1.0f64; field.len()];
    let mut p = [0.0f64; MOST];
    for _ in 0..6 {
        let mut packed = [0.0f64; PACKED];
        let mut b = [0.0f64; MOST];
        for (&(x, y, dx, dy, weight), r) in field.iter().zip(&robust) {
            let c = weight * r;
            for (row, d) in rows(x, y).iter().zip([dx, dy]) {
                let mut k = 0;
                for i in 0..n {
                    b[i] += c * row[i] * d;
                    for j in i..n {
                        packed[k] += c * row[i] * row[j];
                        k += 1;
                    }
                }
            }
        }
        p = solve(&packed, &b, n)?;
        let misses = misses_at(&p);
        let mut sorted = misses.clone();
        sorted.sort_by(f64::total_cmp);
        // A fiftieth of a pixel at least, so a field that fits
        // exactly does not reweigh on its rounding.
        let k = 1.5 * (1.4826 * sorted[sorted.len() / 2]).max(0.02);
        for (r, miss) in robust.iter_mut().zip(&misses) {
            *r = if *miss <= k { 1.0 } else { k / miss };
        }
    }
    // The weighted median of the misses, by the patches' own weights,
    // and the plain one, every patch counted once.
    let misses = misses_at(&p);
    let plain = {
        let mut sorted = misses.clone();
        sorted.sort_by(f64::total_cmp);
        sorted[sorted.len() / 2]
    };
    let (sum, squares) = field
        .iter()
        .fold((0.0, 0.0), |(s, q), f| (s + f.4, q + f.4 * f.4));
    let effective = if squares > 0.0 {
        sum * sum / squares
    } else {
        0.0
    };
    let mut weighed: Vec<(f64, f64)> = misses.into_iter().zip(field.iter().map(|s| s.4)).collect();
    weighed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = weighed.iter().map(|m| m.1).sum();
    let mut seen = 0.0;
    let mut miss = weighed.last().map_or(0.0, |m| m.0);
    for &(m, weight) in &weighed {
        seen += weight;
        if seen >= total / 2.0 {
            miss = m;
            break;
        }
    }
    // Back to pixels: x + d(x), the linear part over the scale.
    let (a, b, c, d, tx, ty) = match model {
        Model::Translation => (0.0, 0.0, 0.0, 0.0, p[0], p[1]),
        Model::Similarity => (p[0], -p[1], p[1], p[0], p[2], p[3]),
        Model::Affine | Model::Homography => (p[0], p[2], p[1], p[3], p[4], p[5]),
    };
    let (a, b, c, d) = (1.0 + a / scale, b / scale, c / scale, 1.0 + d / scale);
    Some((
        Transform {
            m: [
                a,
                b,
                cx + tx - a * cx - b * cy,
                c,
                d,
                cy + ty - c * cx - d * cy,
            ],
        },
        Agreement {
            miss,
            plain,
            effective,
        },
    ))
}

/// How far the patches are from agreeing on the transform fitted to
/// them, in the plane's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Agreement {
    /// The median of the patches' misses, each patch weighed as the fit
    /// weighed it.
    pub miss: f64,
    /// The median of the misses with every patch counted once: the
    /// weighted one alone is the miss of whichever patch holds half the
    /// weight, and one strong patch on a moving leaf can hold that much
    /// and agree with itself.
    pub plain: f64,
    /// How many patches of equal weight the field is worth, `(Σw)² / Σw²`:
    /// the whole count when the weights are even, one when one patch
    /// holds them all.
    pub effective: f64,
}

/// How far a transform moves the four corners of a `w` by `h` plane,
/// x and y each.
fn corner_moves(t: &Transform, w: usize, h: usize) -> [f64; 8] {
    let (x1, y1) = ((w - 1) as f64, (h - 1) as f64);
    let mut out = [0.0; 8];
    for (k, (x, y)) in [(0.0, 0.0), (x1, 0.0), (0.0, y1), (x1, y1)]
        .into_iter()
        .enumerate()
    {
        let (dx, dy) = t.displacement(x, y);
        out[2 * k] = dx;
        out[2 * k + 1] = dy;
    }
    out
}

/// Why the guard turns away a refinement whose patches came to
/// `agreement`, or None when it is kept.
fn refused(agreement: &Agreement, most_miss: f64) -> Option<Unrefined> {
    if agreement.effective < PHASE_EFFECTIVE_LEAST {
        return Some(Unrefined::Few {
            effective: agreement.effective,
        });
    }
    (agreement.miss > most_miss || agreement.plain > PHASE_PLAIN_MOST).then_some(
        Unrefined::Disagree {
            miss: agreement.miss,
            plain: agreement.plain,
        },
    )
}

/// What [`refine_by_phase`] came to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Phased {
    /// The refined transform, in the planes' own pixels.
    pub transform: Transform,
    /// How many times the patches were read.
    pub passes: usize,
    /// How well the patches agreed on the transform fitted to them at
    /// the last pass, in the working level's pixels: what the guard
    /// reads.
    pub agreement: Agreement,
    /// How many patches the last pass read.
    pub patches: usize,
    /// The working level's pixels per full-resolution pixel: 1, a half,
    /// a quarter.
    pub level: f64,
}

/// Why [`refine_by_phase`] did not refine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Unrefined {
    /// The planes are not the size given, or too small for two patches.
    Size,
    /// Too few patches had anything in them to hold the model.
    Patches,
    /// The passes ran out before the corrections settled.
    Unsettled { passes: usize },
    /// The patches settled, but do not agree on one transform: their
    /// weighted median miss is over [`PHASE_MISS_MOST`] or their plain
    /// one over [`PHASE_PLAIN_MOST`].
    Disagree { miss: f64, plain: f64 },
    /// The patches settled, but too few of them carry the weight: fewer
    /// than [`PHASE_EFFECTIVE_LEAST`] of equal weight.
    Few { effective: f64 },
}

/// A transform a gradient fit found between two frames that differ in
/// focus, corrected by the shifts of small patches read off their
/// phase: what a focus stack's links are refined by
/// ([`Refine::Phase`]).
///
/// **Why.** A gradient fit's answer moves with a difference in blur
/// between its frames. Between two neighbors of a focus stack the blur
/// differs everywhere, by an amount that runs across the picture with
/// the scene's depth, and the fit reads part of that as motion: a
/// shift, a turn, a magnification. It is small for one link, a tenth
/// to a half of a pixel at the corners of a 480-pixel frame, but it is
/// the same between every pair of neighbors, since each sees the same
/// scene with the focus a step further on, so down a chain it adds up
/// rather than averaging out. A patch's phase does not move with a
/// symmetric blur at all (see [`phase_shift`]), so the shifts the
/// patches report are the geometry's, and a transform fitted to them,
/// robustly, has no such bias. The rest is the blur's change across a
/// patch, which is not a convolution and which the phase cannot be
/// blind to.
///
/// **The guard.** It refines and does not replace, and it is not used
/// unless it earned it. The patches read shifts up to a fifth of their
/// side, so the gradient fit's reach is still what finds the frame.
/// The passes must settle, two running under [`PHASE_SETTLED`], within
/// [`PHASE_ROUNDS`]. Then the patches must agree on one transform, three
/// ways, all in the working level's pixels: their weighted median miss
/// under [`PHASE_MISS_MOST`] (0.4); their plain median miss, every
/// patch counted once, under [`PHASE_PLAIN_MOST`] (3); and the weight
/// spread over at least [`PHASE_EFFECTIVE_LEAST`] patches' worth (8),
/// so that no one strong patch, a sunlit edge on a moving leaf, holds
/// half the weight and passes the weighted median on its own miss. Two
/// frames that no transform of the model's kind relates (a handheld
/// pair turned about a point ahead of the lens, whose near and far
/// parts moved differently) settle on an answer that depends on which
/// patches weigh most, and the gradient fit's, which weighs every pixel
/// alike, is the better of the two there: it is kept.
///
/// Where the bounds sit, measured. The synthetic stacks' links that
/// are kept: weighted miss 0.04 to 0.4, plain 0.11 to 2.8 (the high
/// end on the depth bed, whose defocused and flat patches read little
/// and weigh little, and which the plain bound has to let through), and
/// 9 to 100 patches' worth (12 at the least on the texture beds; 20
/// would turn half of those away). Real handheld pairs at one focus, at
/// half a 24-megapixel frame: three kept, at 0.15 to 0.34 weighted,
/// 0.24 to 0.44 plain, 60 to 86 patches' worth, where the phase and the
/// gradient fit agree to a tenth of a pixel; four turned away at 0.71
/// to 3.8 weighted, among them the one the refinement used to take 5 px
/// from the gradient fit's answer to a worse place (forward then back
/// 4.95 px against 0.09). The plain bound is loose: on the beds it
/// cannot be tighter than the depth bed allows, and it is the patch
/// count that stops one strong patch.
///
/// **What the guard gives up.**
/// - Stacks of two to five frames, and macro stacks of coarse steps,
///   are mostly turned away: their neighbors are a large step of
///   defocus apart, past what the phase is blind to. No gain there, and
///   no loss: the gradient fit's placing, to the bit.
/// - A chain can mix kept and turned-away links. Each kept link
///   replaces the gradient fit's bias with a smaller one of the same
///   sign, so the mix is never worse than either, but the chain's error
///   is the turned-away links' sum.
/// - A repeating pattern of 8 to 13 working pixels (a fifth of the
///   patches' sides, down to where they wrap): when the gradient fit's
///   answer is more than half a period off, the patches settle on the
///   alias and agree on it, and nothing here can tell.
/// - No real focus stack has been through it: the real pairs measured
///   are at one focus, and the gain is shown only on synthetic stacks
///   and on a real frame blurred and moved by a known amount.
///
/// It runs on the first pyramid level whose short side is at most 2048
/// pixels, with patches of 32 and of 64 pixels a side overlapping by
/// half; on a 24-megapixel frame that is about two thirds of a second
/// a link. The levels are read straight from the planes given: nothing
/// is copied at full resolution.
pub fn refine_by_phase(
    reference: &[f32],
    moving: &[f32],
    width: usize,
    height: usize,
    model: Model,
    initial: &Transform,
) -> std::result::Result<Phased, Unrefined> {
    refine_by_phase_within(
        reference,
        moving,
        width,
        height,
        model,
        initial,
        PHASE_MISS_MOST,
    )
}

/// [`refine_by_phase`] with the guard's miss given: for a measurement
/// of what the guard turns away.
pub(crate) fn refine_by_phase_within(
    reference: &[f32],
    moving: &[f32],
    width: usize,
    height: usize,
    model: Model,
    initial: &Transform,
    most_miss: f64,
) -> std::result::Result<Phased, Unrefined> {
    if reference.len() != width * height || moving.len() != width * height {
        return Err(Unrefined::Size);
    }
    let full = (
        View {
            data: reference,
            width,
            height,
        },
        View {
            data: moving,
            width,
            height,
        },
    );
    // Down to the level it works on, in linear light. A level is
    // made only when the one above is too big.
    let mut levels: Option<(Plane, Plane)> = None;
    let mut factor = 1.0f64;
    while width.min(height) as f64 / factor > PHASE_SIDE_MOST as f64 {
        let (a, b) = match &levels {
            None => (full.0, full.1),
            Some((a, b)) => (View::of(a), View::of(b)),
        };
        levels = Some((
            downsample_view(a.data, a.width, a.height),
            downsample_view(b.data, b.width, b.height),
        ));
        factor *= 2.0;
    }
    let (a, b) = match &levels {
        None => full,
        Some((a, b)) => (View::of(a), View::of(b)),
    };
    let (w, h) = (a.width, a.height);
    if w < PHASE_SIDE * 2 || h < PHASE_SIDE * 2 {
        return Err(Unrefined::Size);
    }
    let peak = |v: View| v.data.iter().copied().fold(0.0f32, f32::max) * PHASE_CLIPPED;
    let clip = (peak(a), peak(b));
    let mut t = initial.at_scale(1.0 / factor);
    let mut last: Option<[f64; 8]> = None;
    let mut was_settled = false;
    for pass in 1..=PHASE_ROUNDS {
        let data = warp_plane(b.data, w, h, w, h, &t, Edge::Clamp, Kernel::Cubic);
        let warped = View {
            data: &data,
            width: w,
            height: h,
        };
        let mut field = shifts::<PHASE_SIDE, { PHASE_SIDE / 8 + 1 }, { PHASE_SIDE / 4 + 1 }>(
            a, warped, &t, clip,
        );
        field.extend(shifts::<
            { 2 * PHASE_SIDE },
            { PHASE_SIDE / 4 + 1 },
            { PHASE_SIDE / 2 + 1 },
        >(a, warped, &t, clip));
        let (correction, agreement) =
            transform_of_shifts(&field, model, w, h).ok_or(Unrefined::Patches)?;
        let moved = correction.corner_distance(&Transform::IDENTITY, w, h);
        // A patch reads a shift short, by a share that depends on what
        // is in it and on how blurred it is (its window does not move
        // with the content), so the passes close in geometrically. The
        // share is read off the last two passes, the corners' moves
        // projected one on the other, and the correction stretched by
        // what is still to come: `1 / (1 - r)` for a pass that left `r`
        // of what was there. Not when the passes turn or grow: that is
        // no geometric approach, and stretching it would run away.
        let corners = corner_moves(&correction, w, h);
        let stretch = match &last {
            Some(before) => {
                let along: f64 = corners.iter().zip(before).map(|(a, b)| a * b).sum();
                let size: f64 = before.iter().map(|b| b * b).sum();
                let r = if size > 0.0 { along / size } else { 0.0 };
                if r > 0.0 && r < 1.0 {
                    1.0 / (1.0 - r.min(0.8))
                } else {
                    1.0
                }
            }
            None => 1.0,
        };
        last = Some(corners);
        // Settled when two passes running move less than the bound. A
        // step under the bound goes in as read, never stretched (a
        // stretched jitter would not settle), and the pass after it reads
        // where it put the frame, so no step is left unchecked.
        let settled = moved < PHASE_SETTLED && was_settled;
        was_settled = moved < PHASE_SETTLED;
        if settled {
            t = t.compose(&correction);
            if let Some(why) = refused(&agreement, most_miss) {
                return Err(why);
            }
            return Ok(Phased {
                transform: t.at_scale(factor),
                passes: pass,
                agreement,
                patches: field.len(),
                level: 1.0 / factor,
            });
        }
        let mut stretched = correction;
        let stretch = if was_settled { 1.0 } else { stretch };
        for (k, v) in stretched.m.iter_mut().enumerate() {
            let identity = Transform::IDENTITY.m[k];
            *v = identity + (*v - identity) * stretch;
        }
        t = t.compose(&stretched);
    }
    Err(Unrefined::Unsettled {
        passes: PHASE_ROUNDS,
    })
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

    /// A keystone (a turn of the camera about the lens, seen on a
    /// plane) with a shift: the homography fit takes it out to a few
    /// hundredths of a pixel from the features' rough start; the affine
    /// fit, from the same start, cannot.
    #[test]
    fn a_homography_takes_out_a_keystone() {
        let (w, h) = (512usize, 384usize);
        let about = middle(w, h);
        let keystone = Homography {
            m: [1.0, 0.01, 0.0, -0.004, 1.0, 0.0, 1.2e-4, -6e-5, 1.0],
        };
        let truth = Homography::from(Transform::translation(about[0] + 6.5, about[1] - 3.25))
            .compose(&keystone)
            .compose(&Homography::from(Transform::translation(
                -about[0], -about[1],
            )));
        let r = reference(w, h);
        let back = truth.inverse().unwrap();
        let m = plane_of(w, h, |x, y| {
            let (u, v) = back.apply(x, y);
            texture(u, v)
        });
        // What features would start from: a pixel out at the corners.
        let start = Homography::from(Transform::translation(7.2, -2.6))
            .compose(&truth.compose(&Homography::from(Transform::translation(-7.0, 3.0))));
        let homography = Options {
            model: Model::Homography,
            max_levels: 3,
            ..Options::default()
        };
        let got = fit_homography_from(&r, &m, w, h, &homography, &start).expect("a fit");
        let e = truth.corner_distance(&got.transform, w, h);
        assert!(e < 0.05, "{e} px, residual {}", got.residual);
        assert!(got.residual < 0.01, "residual {}", got.residual);

        // The affine, from the shift: half a pixel and more off at a
        // corner, however it is fitted.
        let affine = Options {
            model: Model::Affine,
            ..Options::default()
        };
        let got =
            fit_from(&r, &m, w, h, &affine, &Transform::translation(6.5, -3.25)).expect("a fit");
        let e = truth.corner_distance(&Homography::from(got.transform), w, h);
        assert!(e > 0.5, "{e} px");
        // And the affine entry point refuses the homography's model.
        assert!(fit(&r, &m, w, h, &homography).is_err());
    }

    #[test]
    fn the_homography_composes_inverts_and_scales() {
        let a = Homography {
            m: [1.1, 0.05, 3.0, -0.02, 0.95, -4.0, 1e-4, 2e-4, 1.0],
        };
        let round = a.compose(&a.inverse().unwrap());
        assert!(round.corner_distance(&Homography::IDENTITY, 100, 100) < 1e-9);
        let (x, y) = (30.0, 17.0);
        let (ax, ay) = a.apply(x, y);
        let (hx, hy) = a.at_scale(0.5).apply(x / 2.0, y / 2.0);
        assert!((hx - ax / 2.0).abs() < 1e-9 && (hy - ay / 2.0).abs() < 1e-9);
        // Between pixel centers at halves and indices.
        let (sx, sy) = a.shifted(-0.5).apply(x - 0.5, y - 0.5);
        assert!((sx - (ax - 0.5)).abs() < 1e-9 && (sy - (ay - 0.5)).abs() < 1e-9);
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
        assert!(warp_plane(&src, 2, 2, 0, 5, &t, Edge::Clamp, Kernel::Bilinear).is_empty());
        assert!(warp_plane(&src, 2, 2, 5, 0, &t, Edge::Clamp, Kernel::Bilinear).is_empty());
        // Nothing to sample: every output pixel is outside.
        assert_eq!(
            warp_plane(&[], 0, 0, 2, 2, &t, Edge::Fill(-3.0), Kernel::Bilinear),
            vec![-3.0; 4]
        );
        assert_eq!(
            warp_plane(&[], 0, 0, 2, 2, &t, Edge::Clamp, Kernel::Bilinear),
            vec![0.0; 4]
        );
        let empty = WorkingImage::new(0, 0);
        assert!(
            warp_image(&empty, &t, Edge::Clamp, Kernel::Bilinear)
                .data
                .is_empty()
        );
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
        let out = warp_plane(
            &ramp,
            w,
            h,
            w,
            h,
            &t,
            Edge::Fill(f32::NAN),
            Kernel::Bilinear,
        );
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
        let filled = warp_plane(&src, w, h, w, h, &away, Edge::Fill(-1.0), Kernel::Bilinear);
        assert!(filled.iter().all(|v| *v == -1.0));
        let clamped = warp_plane(&src, w, h, w, h, &away, Edge::Clamp, Kernel::Bilinear);
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
            let aligned = warp_plane(&m, w, h, w, h, t, Edge::Clamp, Kernel::Bilinear);
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
        let back = warp_plane(&r, w, h, w, h, &there, Edge::Clamp, Kernel::Bilinear);
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
            Kernel::Bilinear,
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
        let out = warp_image(&image, &t, Edge::Clamp, Kernel::Bilinear);
        assert_eq!(out.pixel(5, 5), [7.0, 8.0, 15.0]);
        let camera = CameraImage::from_data(w, h, image.data.clone()).expect("a camera image");
        let out = warp_camera_image(&camera, &t, Edge::Clamp, Kernel::Bilinear);
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
            let _ = warp_plane(
                &m,
                w,
                h,
                w,
                h,
                &Transform::IDENTITY,
                Edge::Clamp,
                Kernel::Bilinear,
            );
            let start = std::time::Instant::now();
            let warped = warp_plane(
                &m,
                w,
                h,
                w,
                h,
                &Transform::similarity(1.004, 0.4f64.to_radians(), middle(w, h), [12.0, -8.0]),
                Edge::Clamp,
                Kernel::Bilinear,
            );
            println!(
                "{w}x{h} warp of one plane: {:>7.0} ms ({} samples)",
                start.elapsed().as_secs_f64() * 1000.0,
                warped.len()
            );
        }
    }

    /// A grating across `x` of `f` cycles a pixel, its phase moved by
    /// `shift` pixels, `w` by `h`.
    fn grating(w: usize, h: usize, f: f64, shift: f64) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let x = (i % w) as f64 + shift;
                (0.5 + 0.4 * (std::f64::consts::TAU * f * x).sin()) as f32
            })
            .collect()
    }

    /// The amplitude of the grating of `f` in `plane` over its middle,
    /// against the true one: the share of the contrast a warp kept.
    fn kept(plane: &[f32], w: usize, h: usize, f: f64) -> f64 {
        let (mut s, mut c, mut n) = (0.0, 0.0, 0.0);
        for y in h / 4..3 * h / 4 {
            for x in 16..w - 16 {
                let a = std::f64::consts::TAU * f * x as f64;
                let v = f64::from(plane[y * w + x]) - 0.5;
                s += v * a.sin();
                c += v * a.cos();
                n += 1.0;
            }
        }
        2.0 * (s * s + c * c).sqrt() / n / 0.4
    }

    /// A grating drawn half a pixel off each way and warped back: the
    /// contrast Lanczos-3 keeps at a quarter and a half of Nyquist
    /// against bilinear's (which is `cos(pi f)` at a half-pixel phase),
    /// and the ring a step makes under it, as a share of the step.
    #[test]
    fn lanczos_keeps_what_bilinear_softens_and_rings_a_little() {
        let (w, h) = (128usize, 32usize);
        let back = Transform::translation(0.5, 0.5);
        let mut said = Vec::new();
        for f in [0.125, 0.25] {
            let off = grating(w, h, f, -0.5);
            let k = |kernel| {
                kept(
                    &warp_plane(&off, w, h, w, h, &back, Edge::Clamp, kernel),
                    w,
                    h,
                    f,
                )
            };
            let (bilinear, lanczos) = (k(Kernel::Bilinear), k(Kernel::Lanczos3));
            said.push(format!(
                "at {f} cycles a pixel: bilinear keeps {bilinear:.3}, Lanczos-3 {lanczos:.3}"
            ));
            let cos = (std::f64::consts::PI * f).cos();
            assert!((bilinear - cos).abs() < 0.01, "{bilinear} against {cos}");
            // Lanczos-3's response rises a little over one below
            // half of Nyquist.
            assert!(lanczos > 0.97 && lanczos < 1.05, "{lanczos}");
        }
        // A step from 0.1 to 0.9, half a pixel off.
        let step: Vec<f32> = (0..w * h)
            .map(|i| if (i % w) < w / 2 { 0.1 } else { 0.9 })
            .collect();
        let out = warp_plane(&step, w, h, w, h, &back, Edge::Clamp, Kernel::Lanczos3);
        let (lo, hi) = out
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let ring = f64::from((hi - 0.9).max(0.1 - lo)) / 0.8;
        said.push(format!(
            "a hard step's ring under Lanczos-3: {:.2}% of the step",
            ring * 100.0
        ));
        // The same step as a lens draws it, over about a pixel.
        let soft: Vec<f32> = (0..w * h)
            .map(|i| {
                let d = (i % w) as f64 - (w / 2) as f64 + 0.5;
                (0.1 + 0.8 / (1.0 + (-d / 0.5).exp())) as f32
            })
            .collect();
        let out_soft = warp_plane(&soft, w, h, w, h, &back, Edge::Clamp, Kernel::Lanczos3);
        let (slo, shi) = out_soft
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let (tlo, thi) = sample_range(&soft);
        let soft_ring = f64::from((shi - thi).max(tlo - slo).max(0.0)) / 0.8;
        said.push(format!(
            "a soft step's ring under Lanczos-3: {:.2}% of the step",
            soft_ring * 100.0
        ));
        println!("{}", said.join("\n"));
        assert!(ring > 0.0 && ring < 0.15, "{ring}");
        assert!(soft_ring < 0.03, "{soft_ring}");
        // Held to the source's range, nothing is past it.
        let mut held = out.clone();
        clamp_samples(&mut held, sample_range(&step));
        assert!(held.iter().all(|v| (0.1..=0.9).contains(v)));
    }

    /// Lanczos-3 puts a flat picture back flat, an integer move exactly,
    /// and an edge's fill where the point is outside.
    #[test]
    fn lanczos_is_flat_exact_on_the_grid_and_fills_outside() {
        let (w, h) = (40usize, 30usize);
        let flat = vec![0.37f32; w * h * 3];
        let t = Transform::similarity(1.01, 0.3f64.to_radians(), [20.0, 15.0], [0.3, -0.7]);
        let out = warp_samples(&flat, w, h, 3, &t, Edge::Clamp, Kernel::Lanczos3);
        assert!(out.iter().all(|v| (v - 0.37).abs() < 1e-5));
        let r = reference(w, h);
        let moved = warp_plane(
            &r,
            w,
            h,
            w,
            h,
            &Transform::translation(3.0, -2.0),
            Edge::Fill(f32::NAN),
            Kernel::Lanczos3,
        );
        for y in 0..h - 2 {
            for x in 0..w - 3 {
                if y < 2 {
                    assert!(moved[y * w + x].is_nan());
                    continue;
                }
                assert_eq!(moved[y * w + x], r[(y - 2) * w + x + 3]);
            }
        }
        let mut with_nan = vec![2.0f32, f32::NAN, -1.0, 5.0];
        assert_eq!(sample_range(&with_nan), (-1.0, 5.0));
        clamp_samples(&mut with_nan, (0.0, 3.0));
        assert_eq!(with_nan[0], 2.0);
        assert!(with_nan[1].is_nan());
        assert_eq!((with_nan[2], with_nan[3]), (0.0, 3.0));
        assert_eq!(sample_range(&[f32::NAN]), (0.0, 0.0));
    }

    /// The time a 45 MP frame of three channels takes to warp by a
    /// similarity, bilinear and Lanczos-3, on the threads rayon has.
    /// Ignored: a timing, not a check. `RAYON_NUM_THREADS=4 cargo test
    /// --release -p greycard-core -- --ignored --nocapture
    /// a_45_mp_warp`.
    #[test]
    #[ignore]
    fn a_45_mp_warp() {
        let (w, h) = (8256usize, 5504usize);
        let src: Vec<f32> = (0..w * h * 3)
            .map(|i| ((i * 7919) % 1000) as f32 / 1000.0)
            .collect();
        let t = Transform::similarity(1.002, 0.5f64.to_radians(), middle(w, h), [2.0, -1.5]);
        for kernel in [
            Kernel::Bilinear,
            Kernel::Lanczos3,
            Kernel::Bilinear,
            Kernel::Lanczos3,
        ] {
            let start = std::time::Instant::now();
            let out = warp_samples(&src, w, h, 3, &t, Edge::Fill(f32::NAN), kernel);
            println!(
                "{w}x{h}x3 {}: {:.2} s ({} threads)",
                kernel.name(),
                start.elapsed().as_secs_f64(),
                rayon::current_num_threads()
            );
            drop(out);
        }
    }

    /// A plane convolved with `taps` (offset, offset, weight), the
    /// taps clamped at the border.
    fn convolved(
        src: &[f32],
        width: usize,
        height: usize,
        taps: &[(isize, isize, f32)],
    ) -> Vec<f32> {
        (0..width * height)
            .into_par_iter()
            .map(|k| {
                let (x, y) = ((k % width) as isize, (k / width) as isize);
                taps.iter()
                    .map(|&(dx, dy, v)| {
                        let xx = (x + dx).clamp(0, width as isize - 1) as usize;
                        let yy = (y + dy).clamp(0, height as isize - 1) as usize;
                        v * src[yy * width + xx]
                    })
                    .sum()
            })
            .collect()
    }

    fn gaussian(sigma: f64) -> Vec<(isize, isize, f32)> {
        let r = (3.0 * sigma).ceil() as isize;
        let mut taps = Vec::new();
        for dy in -r..=r {
            for dx in -r..=r {
                let d2 = (dx * dx + dy * dy) as f64;
                taps.push((dx, dy, (-d2 / (2.0 * sigma * sigma)).exp() as f32));
            }
        }
        normalized(taps)
    }

    /// A lens's out-of-focus disc: its transfer function goes negative
    /// past the first zero.
    fn disc(radius: f64) -> Vec<(isize, isize, f32)> {
        let r = radius.ceil() as isize;
        let mut taps = Vec::new();
        for dy in -r..=r {
            for dx in -r..=r {
                if (dx as f64).hypot(dy as f64) <= radius {
                    taps.push((dx, dy, 1.0));
                }
            }
        }
        normalized(taps)
    }

    /// A cat's eye, the disc cut by the lens's rim off axis: not
    /// symmetric, so its transfer function has a phase. Moved so that
    /// its centroid is on the pixel, as a blur that does not shift the
    /// picture.
    fn cats_eye(radius: f64, cut: f64) -> Vec<(isize, isize, f32)> {
        let r = radius.ceil() as isize + 1;
        let mut points = Vec::new();
        for dy in -r..=r {
            for dx in -r..=r {
                let (x, y) = (dx as f64, dy as f64);
                if x.hypot(y) <= radius && (x - cut * radius).hypot(y) <= radius {
                    points.push((x, y));
                }
            }
        }
        let cx = points.iter().map(|p| p.0).sum::<f64>() / points.len() as f64;
        let mut taps = std::collections::BTreeMap::<(isize, isize), f32>::new();
        for (x, y) in points {
            let x = x - cx;
            let (x0, f) = (x.floor(), (x - x.floor()) as f32);
            *taps.entry((x0 as isize, y as isize)).or_default() += 1.0 - f;
            *taps.entry((x0 as isize + 1, y as isize)).or_default() += f;
        }
        normalized(taps.into_iter().map(|((x, y), v)| (x, y, v)).collect())
    }

    fn normalized(taps: Vec<(isize, isize, f32)>) -> Vec<(isize, isize, f32)> {
        let sum: f32 = taps.iter().map(|t| t.2).sum();
        taps.into_iter().map(|(x, y, v)| (x, y, v / sum)).collect()
    }

    /// A defocus that grows away from a column, as a scene's depth runs
    /// across a frame: the Gaussians of 0 to 8 pixels blended by `x`,
    /// the deviation `steep` times the distance from `focus` (a share
    /// of the width).
    fn ramped(src: &[f32], width: usize, height: usize, focus: f64, steep: f64) -> Vec<f32> {
        let sigmas = [0.0, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0];
        let levels: Vec<Vec<f32>> = sigmas
            .iter()
            .map(|&s| {
                if s == 0.0 {
                    src.to_vec()
                } else {
                    convolved(src, width, height, &gaussian(s))
                }
            })
            .collect();
        (0..width * height)
            .map(|k| {
                let x = (k % width) as f64 / width as f64;
                let s = (steep * (x - focus).abs()).min(8.0);
                let i = sigmas.iter().rposition(|&v| v <= s).unwrap_or(0);
                let j = (i + 1).min(sigmas.len() - 1);
                let t = if j > i {
                    ((s - sigmas[i]) / (sigmas[j] - sigmas[i])) as f32
                } else {
                    0.0
                };
                levels[i][k] * (1.0 - t) + levels[j][k] * t
            })
            .collect()
    }

    /// Two frames of one picture under different blurs, the second
    /// moved by `truth`: the reference blurred as `a`, the moving frame
    /// as `b` and then taken through the truth's inverse, so the fit
    /// should come back with the truth.
    fn blurred_pair(
        width: usize,
        height: usize,
        a: impl Fn(&[f32]) -> Vec<f32>,
        b: impl Fn(&[f32]) -> Vec<f32>,
        truth: &Transform,
    ) -> (Vec<f32>, Vec<f32>) {
        let sharp = reference(width, height);
        let moving = warp_plane(
            &b(&sharp),
            width,
            height,
            width,
            height,
            &truth.inverse().expect("a truth"),
            Edge::Clamp,
            Kernel::Cubic,
        );
        (a(&sharp), moving)
    }

    fn similarity_options(refine: Refine) -> Options {
        Options {
            model: Model::Similarity,
            refine,
            ..Default::default()
        }
    }

    /// The known-answer construction: one picture under two different
    /// blurs, a Gaussian, a lens's disc, an off-axis cat's eye, and a
    /// defocus ramping across the frame, the second frame moved by a
    /// known similarity. The gradient fit alone reads a third to three
    /// fifths of a pixel of the blurs' difference as motion at the
    /// corners (0.59 under the Gaussian, 0.54 the disc, 0.39 the cat's
    /// eye, 0.35 the ramps); the refinement leaves 0.04, 0.02, 0.01, and
    /// 0.16. A ramp steep enough to change the blur by a pixel across a
    /// patch is past what the phase is blind to, and the guard turns
    /// such a pair away more often than not.
    #[test]
    fn the_phase_refinement_is_blind_to_a_difference_in_blur() {
        let (width, height) = (480usize, 320);
        let truth = Transform::similarity(
            1.004,
            0.08f64.to_radians(),
            middle(width, height),
            [2.3, -1.6],
        );
        let same = |p: &[f32]| p.to_vec();
        type Blur<'a> = Box<dyn Fn(&[f32]) -> Vec<f32> + 'a>;
        let cases: Vec<(&str, Blur, Blur, f64)> = vec![
            (
                "a Gaussian of 3",
                Box::new(same),
                Box::new(|p| convolved(p, width, height, &gaussian(3.0))),
                0.15,
            ),
            (
                "a disc of 4",
                Box::new(same),
                Box::new(|p| convolved(p, width, height, &disc(4.0))),
                0.15,
            ),
            (
                "a cat's eye of 4",
                Box::new(same),
                Box::new(|p| convolved(p, width, height, &cats_eye(4.0, 0.6))),
                0.15,
            ),
            (
                "ramps of defocus across the frame",
                Box::new(|p| ramped(p, width, height, 0.35, 8.0)),
                Box::new(|p| ramped(p, width, height, 0.65, 8.0)),
                0.25,
            ),
        ];
        for (name, a, b, most) in cases {
            let (reference, moving) = blurred_pair(width, height, a, b, &truth);
            let off = |refine| {
                let fit = fit(
                    &reference,
                    &moving,
                    width,
                    height,
                    &similarity_options(refine),
                )
                .expect("a fit");
                error(&fit, &truth, width, height)
            };
            let (gradient, phase) = (off(Refine::None), off(Refine::Phase));
            println!("{name}: the gradient fit {gradient:.3} px off, refined {phase:.3}");
            assert!(phase < most, "{name}: refined {phase:.3} px off");
            assert!(
                gradient > most,
                "{name}: the gradient fit {gradient:.3} px off"
            );
        }
    }

    /// A pure shift, no blur, from the identity: found to a few
    /// hundredths of a pixel.
    #[test]
    fn the_phase_refinement_finds_a_known_shift() {
        let (width, height) = (480usize, 320);
        let truth = Transform::translation(1.7, -0.9);
        let moving = moved(width, height, &truth);
        let phased = refine_by_phase(
            &reference(width, height),
            &moving,
            width,
            height,
            Model::Similarity,
            &Transform::IDENTITY,
        )
        .expect("refined");
        let off = phased.transform.corner_distance(&truth, width, height);
        println!("{phased:?}: {off:.4} px off");
        assert!(off < 0.05, "{off} px off");
        assert!(phased.agreement.miss < 0.05, "{}", phased.agreement.miss);
    }

    /// A frame with nothing in it: no patch has a phase to read.
    #[test]
    fn a_featureless_frame_is_not_refined() {
        let (width, height) = (480usize, 320);
        let flat = vec![0.4f32; width * height];
        let got = refine_by_phase(
            &flat,
            &flat,
            width,
            height,
            Model::Similarity,
            &Transform::IDENTITY,
        );
        assert_eq!(got, Err(Unrefined::Patches));
    }

    /// Started further off than a patch can read, a fifth of its side
    /// and more, the refinement does not hand back a wrong answer: it
    /// finds the truth or it says it did not settle or did not agree.
    #[test]
    fn a_start_past_the_patches_reach_is_not_refined_wrongly() {
        let (width, height) = (480usize, 320);
        let truth = Transform::IDENTITY;
        let sharp = reference(width, height);
        for off in [8.0, 14.0, 20.0, 40.0] {
            let got = refine_by_phase(
                &sharp,
                &sharp,
                width,
                height,
                Model::Similarity,
                &Transform::translation(off, 0.0),
            );
            println!("started {off} px off: {got:?}");
            if let Ok(phased) = got {
                let left = phased.transform.corner_distance(&truth, width, height);
                assert!(left < 0.05, "started {off} px off, ended {left:.3} px off");
            }
        }
    }

    /// A repeating pattern: a grating of 24-pixel period over the
    /// texture's faint copy, moved by a fraction of the period. Started
    /// within a patch's reach, the refinement finds it. A start a whole
    /// period off is the same picture to every patch, and only the
    /// gradient fit's reach, which the refinement starts from, can tell
    /// it apart; the refinement keeps whichever period it was handed.
    #[test]
    fn a_repeating_pattern_is_refined_within_its_reach() {
        let (width, height) = (480usize, 320);
        let pattern = |x: f64, y: f64| {
            let period = 24.0;
            let w = 2.0 * std::f64::consts::PI / period;
            (0.5 + 0.3 * (w * x).sin() * (w * y).sin()) as f32 + 0.1 * texture(x, y)
        };
        let truth = Transform::translation(3.1, 1.2);
        let back = truth.inverse().expect("invertible");
        let a = plane_of(width, height, pattern);
        let b = plane_of(width, height, |x, y| {
            let (u, v) = back.apply(x, y);
            pattern(u, v)
        });
        let phased = refine_by_phase(
            &a,
            &b,
            width,
            height,
            Model::Similarity,
            &Transform::translation(1.0, 0.0),
        )
        .expect("refined");
        let off = phased.transform.corner_distance(&truth, width, height);
        assert!(off < 0.1, "{off} px off");
    }

    /// One strong patch, on a moving leaf or a sunlit edge, holding most
    /// of the weight and moving its own way, against two hundred weaker
    /// ones that agree with each other: the fit leans to the strong patch,
    /// the weighted median miss is about its own, 0.32, and alone would
    /// let the refinement through. The patch count (1.2 patches' worth)
    /// and the plain median (3.2) see it.
    #[test]
    fn one_strong_patch_cannot_pass_the_guard_alone() {
        let (w, h) = (960usize, 640usize);
        let mut field: Vec<Shift> = Vec::new();
        for j in 0..10 {
            for i in 0..20 {
                let (x, y) = (24.0 + 48.0 * i as f64, 32.0 + 64.0 * j as f64);
                // A few hundredths of a pixel of reading noise.
                let wobble = 0.03 * ((i * 7 + j * 13) % 5) as f64 - 0.06;
                field.push((x, y, 0.3 + wobble, -0.2 - wobble, 1.0));
            }
        }
        field.push((480.0, 320.0, 3.0, 2.0, 2000.0));
        let (_, agreement) =
            transform_of_shifts(&field, Model::Similarity, w, h).expect("a transform");
        // The weighted median alone would keep it.
        assert!(agreement.miss < PHASE_MISS_MOST, "{agreement:?}");
        assert!(
            refused(&agreement, PHASE_MISS_MOST).is_some(),
            "{agreement:?} kept"
        );
        // And the same field without the strong patch is kept.
        field.pop();
        let (_, agreement) =
            transform_of_shifts(&field, Model::Similarity, w, h).expect("a transform");
        assert_eq!(refused(&agreement, PHASE_MISS_MOST), None, "{agreement:?}");
    }
}
