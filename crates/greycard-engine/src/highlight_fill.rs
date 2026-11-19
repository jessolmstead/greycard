//! The learned highlight fill as a stage of the walk (notes §719 §1a):
//! core's camera-space fill (`develop::highlight_fill::camera`) on the
//! picture after the demosaic, the bin, or the denoise, before the
//! file's warp, and the frame's range as Highlights and Whites read it
//! from before the fill.
//!
//! The fields are made once from the plain picture and applied to it
//! and, for a learned pair, to the network's half. The range is carried
//! as a shift ([`Filled::range_shift`]): the guide's cells are read on
//! the picture before the fill and again, where it changed, after it,
//! and the guide's tap adds the difference of the two ranges to its
//! own, so the range it hands the finish is the unfilled frame's while
//! the plane's pixels are the filled picture's.

use greycard_core::develop::Prepared;
use greycard_core::develop::highlight_fill::half::PixelCodes;
use greycard_core::develop::highlight_fill::{Camera, FrameCodes, camera_fields};
use greycard_render::finish::GuideCells;

/// What the fill did, for the log and for the walk's [`Meta`](crate::walk::Meta).
#[derive(Debug, Clone, PartialEq)]
pub struct Filled {
    /// The version that ran.
    pub version: &'static str,
    /// The pixels of the plain picture it changed.
    pub changed: usize,
    /// The look scale it ran at: 1 at full size, the bin's at a bin.
    pub look_scale: f64,
    /// The frame's range before the fill less its range after, in the
    /// guide's stops: what the guide's tap adds back. None where the
    /// fill changed nothing, or changed nothing the range reads.
    pub range_shift: Option<[f32; 2]>,
}

/// Runs version `name` on a camera picture `plain` of `prepared`'s size
/// and frame, and on `network`, a learned pair's other half, with the
/// same fields; `bin` is the bin the picture was made at (1: none).
/// The prepared mosaic's samples are let go first, read by nothing
/// after the demosaic, and its cells once read.
///
/// None, the pictures as they were, when the version is not this
/// build's, the prepare read no cells, the bin is not the cells' own,
/// or nothing is clipped enough to fill: each said in the log.
pub fn run(
    name: &'static str,
    prepared: &mut Prepared,
    plain: &mut [f32],
    network: Option<&mut [f32]>,
    bin: usize,
) -> Option<Filled> {
    prepared.samples = Vec::new();
    let Some(version) = greycard_ai::highlight::version(name) else {
        log::warn!("the highlight fill {name} is not this build's; none ran");
        return None;
    };
    let Some(codes) = prepared.clip_codes.take() else {
        log::warn!("the highlight fill: the prepare read no clip codes; none ran");
        return None;
    };
    let (w, h) = (prepared.width, prepared.height);
    let codes = match FrameCodes::new(&codes, bin) {
        Some(c) if c.size() == (w, h) => c,
        c => {
            log::warn!(
                "the highlight fill: codes of {}x{} at a bin of {bin} are {:?} for a {w}x{h} picture; none ran",
                codes.width,
                codes.height,
                c.map(|c| c.size())
            );
            return None;
        }
    };
    let camera = Camera::of(prepared);
    let f = codes.look_scale();
    let Some(fill) = camera_fields(
        plain,
        w,
        h,
        &camera,
        Some(&codes),
        codes.clips(),
        version.weights(),
        &version.look,
        f,
    ) else {
        log::debug!("the highlight fill {name}: nothing clipped enough to fill");
        return None;
    };
    let before = GuideCells::of(w, h, &|x, y| at(plain, w, x, y, &camera));
    let changes = fill.apply(plain, &codes);
    if let Some(network) = network {
        fill.apply(network, &codes);
    }
    let range_shift = (!changes.is_empty())
        .then(|| {
            let mut after = before.clone();
            let plain: &[f32] = plain;
            after.redo(&|x, y| at(plain, w, x, y, &camera), &|xs, ys| {
                changes.any_in(xs, ys)
            });
            let (b, a) = (before.range(), after.range());
            [b[0] - a[0], b[1] - a[1]]
        })
        .filter(|d| d.iter().any(|&v| v != 0.0));
    Some(Filled {
        version: version.name,
        changed: changes.count(),
        look_scale: f,
        range_shift,
    })
}

/// Pixel (`x`, `y`) of camera picture `rgb`, `w` wide, in the working
/// space as `finish`'s matrix makes it: what the guide reads, short of
/// the profile's map and what comes between the fill and the tap.
#[inline]
fn at(rgb: &[f32], w: usize, x: usize, y: usize, camera: &Camera) -> [f32; 3] {
    let p = 3 * (y * w + x);
    camera.working([rgb[p], rgb[p + 1], rgb[p + 2]])
}
