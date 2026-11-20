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
//!
//! The fields outlive the stage ([`KeptFill`], kept with the walk's
//! points): a walk whose steps up to the fill are the ones they were
//! made by applies them again rather than make them, the same bits; and
//! the window, while a white balance moves, applies the last ones
//! through the new white as a stand-in ([`stand_in`]) until the fill is
//! made again.

use std::sync::Arc;

use greycard_core::develop::Prepared;
use greycard_core::develop::highlight_fill::half::PixelCodes;
use greycard_core::develop::highlight_fill::{
    Camera, CameraFill, FrameCodes, GaveWay, camera_fields_until,
};
use greycard_render::finish::GuideCells;

use crate::stages::Step;
use crate::walk::Place;

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
    /// How the fields were had.
    pub how: How,
}

/// How a walk had the fields it applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// Made by this walk.
    Made,
    /// Kept from a walk of the same steps up to the fill, in the same
    /// places: the fields this walk would have made.
    Kept,
    /// The last fields, made under another white balance, standing in
    /// for the fill until it is made again: the window's alone.
    StandIn,
}

/// One fill's fields, kept past the stage that made them, with the
/// steps up to and including the fill and where each ran: what they are
/// the fields of. Its own frame's alone: a [`Kept`](crate::walk::Kept)
/// lets go of everything it holds for another input.
#[derive(Debug)]
pub struct KeptFill {
    /// The walk's steps through the fill.
    pub steps: Vec<Step>,
    /// Where each of them ran.
    pub places: Vec<Place>,
    /// The version that made them.
    pub version: &'static str,
    /// The bin the picture was made at, and its size: the fields'.
    pub bin: usize,
    pub size: (usize, usize),
    pub fill: CameraFill,
}

impl KeptFill {
    /// The bytes it holds: the fields, and its steps' list.
    pub fn bytes(&self) -> usize {
        self.fill.bytes()
            + std::mem::size_of::<Self>()
            + self.steps.capacity() * std::mem::size_of::<Step>()
            + self.places.capacity() * std::mem::size_of::<Place>()
    }

    /// Whether these are the fields a walk of `steps` (through its
    /// fill), run at `places`, would make: the same steps, each where it
    /// ran then. Two pictures of those are the same to the bit, and so
    /// are their fields.
    pub fn is_for(&self, steps: &[Step], places: &[Place]) -> bool {
        self.steps == steps && self.places == places
    }
}

/// The last fields, standing in for the fill in one walk of the
/// window's: a step of their own, which no list of an edit makes, so a
/// point a stand-in walk kept could never serve the fill's list or the
/// list without it. Equal only to itself (the very fields).
#[derive(Clone)]
pub struct StandIn(pub Arc<KeptFill>);

impl PartialEq for StandIn {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for StandIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "StandIn({} at {}x{})",
            self.0.version, self.0.size.0, self.0.size.1
        )
    }
}

/// Runs version `name` on a camera picture `plain` of `prepared`'s size
/// and frame, and on `network`, a learned pair's other half, with the
/// same fields; `bin` is the bin the picture was made at (1: none).
/// The prepared mosaic's samples are let go first, read by nothing
/// after the demosaic, and its cells once read. `give_way` is asked
/// between the fill's parts; told to stop, it returns [`GaveWay`] with
/// the pictures as they were.
///
/// None, the pictures as they were, when the version is not this
/// build's, the prepare read no cells, the bin is not the cells' own,
/// or nothing is clipped enough to fill: each said in the log. Else
/// what it did, and its fields.
pub fn run(
    name: &'static str,
    prepared: &mut Prepared,
    plain: &mut [f32],
    network: Option<&mut [f32]>,
    bin: usize,
    give_way: &dyn Fn() -> bool,
) -> Result<Option<(Filled, CameraFill)>, GaveWay> {
    prepared.samples = Vec::new();
    let Some(version) = greycard_ai::highlight::version(name) else {
        log::warn!("the highlight fill {name} is not this build's; none ran");
        return Ok(None);
    };
    let Some(codes) = prepared.clip_codes.take() else {
        log::warn!("the highlight fill: the prepare read no clip codes; none ran");
        return Ok(None);
    };
    let (w, h) = (prepared.width, prepared.height);
    let Some(frame) = frame_codes(&codes, bin, (w, h)) else {
        return Ok(None);
    };
    let camera = Camera::of(prepared);
    let f = frame.look_scale();
    let Some(fill) = camera_fields_until(
        plain,
        w,
        h,
        &camera,
        Some(&frame),
        frame.clips(),
        version.weights(),
        &version.look,
        f,
        give_way,
    )?
    else {
        log::debug!("the highlight fill {name}: nothing clipped enough to fill");
        return Ok(None);
    };
    let filled = apply(
        &fill,
        version.name,
        &frame,
        &camera,
        plain,
        network,
        How::Made,
    );
    Ok(Some((filled, fill)))
}

/// Applies `kept`, fields a walk of the same steps made, as [`run`]
/// would have made and applied them: the same pixels, the same range.
/// None, the pictures as they were, where the prepare read no cells or
/// they are not the fields' size.
pub fn reapply(
    kept: &KeptFill,
    prepared: &mut Prepared,
    plain: &mut [f32],
    network: Option<&mut [f32]>,
    bin: usize,
) -> Option<Filled> {
    prepared.samples = Vec::new();
    let codes = prepared.clip_codes.take()?;
    let frame = sized(&codes, bin, prepared, kept)?;
    let camera = Camera::of(prepared);
    Some(apply(
        &kept.fill,
        kept.version,
        &frame,
        &camera,
        plain,
        network,
        How::Kept,
    ))
}

/// The last fields `kept` applied through this frame's camera, standing
/// in for the fill under another white balance
/// ([`CameraFill::through`]): the cells read again by this walk's
/// prepare, so the pixels are the ones a fill made again would read.
/// None, the pictures as they were, where the prepare read no cells,
/// they are not the fields' size, or the new matrix has no inverse.
pub fn stand_in(
    kept: &KeptFill,
    prepared: &mut Prepared,
    plain: &mut [f32],
    network: Option<&mut [f32]>,
    bin: usize,
) -> Option<Filled> {
    prepared.samples = Vec::new();
    let codes = prepared.clip_codes.take()?;
    let frame = sized(&codes, bin, prepared, kept)?;
    let camera = Camera::of(prepared);
    let fill = kept.fill.through(&camera)?;
    Some(apply(
        &fill,
        kept.version,
        &frame,
        &camera,
        plain,
        network,
        How::StandIn,
    ))
}

/// The codes of `prepared`'s picture at `bin`, when it is the size
/// `kept`'s fields are for.
fn sized<'a>(
    codes: &'a greycard_core::develop::clipcodes::ClipCodes,
    bin: usize,
    prepared: &Prepared,
    kept: &KeptFill,
) -> Option<FrameCodes<'a>> {
    let size = (prepared.width, prepared.height);
    let frame = frame_codes(codes, bin, size)?;
    let fields = (kept.fill.fields().width(), kept.fill.fields().height());
    if bin != kept.bin || size != kept.size || fields != size {
        log::warn!(
            "the highlight fill's kept fields are for {}x{} at a bin of {}, the picture {}x{} at {bin}; none applied",
            kept.size.0,
            kept.size.1,
            kept.bin,
            size.0,
            size.1
        );
        return None;
    }
    Some(frame)
}

/// The codes for a `size` picture at `bin`, or why not in the log.
fn frame_codes(
    codes: &greycard_core::develop::clipcodes::ClipCodes,
    bin: usize,
    (w, h): (usize, usize),
) -> Option<FrameCodes<'_>> {
    match FrameCodes::new(codes, bin) {
        Some(c) if c.size() == (w, h) => Some(c),
        c => {
            log::warn!(
                "the highlight fill: codes of {}x{} at a bin of {bin} are {:?} for a {w}x{h} picture; none ran",
                codes.width,
                codes.height,
                c.map(|c| c.size())
            );
            None
        }
    }
}

/// `fill` applied to `plain` and `network`, with the range's shift read
/// on `plain` before and after through `camera`.
fn apply(
    fill: &CameraFill,
    version: &'static str,
    codes: &FrameCodes<'_>,
    camera: &Camera,
    plain: &mut [f32],
    network: Option<&mut [f32]>,
    how: How,
) -> Filled {
    let (w, h) = codes.size();
    let before = GuideCells::of(w, h, &|x, y| at(plain, w, x, y, camera));
    let changes = fill.apply(plain, codes);
    if let Some(network) = network {
        fill.apply(network, codes);
    }
    let range_shift = (!changes.is_empty())
        .then(|| {
            let mut after = before.clone();
            let plain: &[f32] = plain;
            after.redo(&|x, y| at(plain, w, x, y, camera), &|xs, ys| {
                changes.any_in(xs, ys)
            });
            let (b, a) = (before.range(), after.range());
            [b[0] - a[0], b[1] - a[1]]
        })
        .filter(|d| d.iter().any(|&v| v != 0.0));
    Filled {
        version,
        changed: changes.count(),
        look_scale: codes.look_scale(),
        range_shift,
        how,
    }
}

/// Pixel (`x`, `y`) of camera picture `rgb`, `w` wide, in the working
/// space as `finish`'s matrix makes it: what the guide reads, short of
/// the profile's map and what comes between the fill and the tap.
#[inline]
fn at(rgb: &[f32], w: usize, x: usize, y: usize, camera: &Camera) -> [f32; 3] {
    let p = 3 * (y * w + x);
    camera.working([rgb[p], rgb[p + 1], rgb[p + 2]])
}
