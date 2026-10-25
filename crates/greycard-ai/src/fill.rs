//! Fill: a hole made up from what is around it, by LaMa. The model
//! takes any height and width that are multiples of 8, so the caller
//! cuts a window round the hole at the picture's own pixels
//! ([`window`]) and the window is filled at them, as long as no pixel
//! of the hole lies deeper than [`DEPTH`] from a known one: past that
//! LaMa's answer goes to a dark smear, so a deeper hole is filled at
//! the scale that brings it to [`DEPTH`] and taken back up
//! ([`filled`]). A window over [`MAX`] on a side is filled in
//! overlapped tiles ([`tiled`]), blended across the overlap. The WebGPU
//! provider does not report a device out of memory, it answers with
//! finite garbage; what keeps a small card clear of that is the cap,
//! and every answer is checked where it can be, outside the hole
//! ([`implausible`]).

use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use crate::image::Rgbf;
use crate::registry::FILL;
use crate::runtime::{self, Error, Loaded, Provider, Result};
use crate::store::Store;

/// The longest side the model is asked at once, on every provider:
/// about 2 GB of a card's memory and 0.2 s on a recent one, 3.5 GB and
/// 1.6 s on the CPU. At 2048 a card needs 9 GB and the CPU 14 GB, for
/// no less time than four tiles of 1024.
pub const MAX: usize = 1024;

/// How far a pixel outside the hole may come back from what went in:
/// the export composites the picture back there exactly.
const OUTSIDE: f32 = 1e-3;

/// How far neighboring tiles overlap, at the least.
pub const OVERLAP: usize = 64;

/// The context round a hole at the least, each side.
const MARGIN: usize = 32;

/// How deep into a hole, in the model's pixels, LaMa fills well: the
/// farthest a hole pixel may lie from a known one. Measured on a sky
/// and a shore: clean to 64, a ghost of what was removed at 96, a dark
/// smear from 128.
pub const DEPTH: f32 = 64.0;

/// The model's sizes are multiples of this (three halvings).
const STEP: usize = 8;

/// The smallest side the export takes.
const SMALLEST: usize = 32;

/// The size the session is warmed up at.
const WARM: usize = 512;

/// A window of the picture, in its pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Window {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// The window a hole whose bounding box is `hole` and whose [`depth`]
/// is `depth` is filled in, in a frame of `frame_width` ×
/// `frame_height`: the box with half its longer side again on every
/// side for context, but no more than twice the depth, so a long thin
/// stroke is not filled with a frame's worth round it; within the
/// frame, its sides grown to multiples of 8 where the frame has room.
pub fn window(hole: Window, depth: f32, frame_width: usize, frame_height: usize) -> Window {
    let margin = (hole.width.max(hole.height) / 2)
        .min((2.0 * depth).ceil() as usize)
        .max(MARGIN);
    let (x, width) = span(hole.x, hole.width, margin, frame_width);
    let (y, height) = span(hole.y, hole.height, margin, frame_height);
    Window {
        x,
        y,
        width,
        height,
    }
}

/// One axis of [`window`]: `start..start + len` grown by `margin`
/// each side within `0..frame`, then to a multiple of [`STEP`], to the
/// far side first and then the near one.
fn span(start: usize, len: usize, margin: usize, frame: usize) -> (usize, usize) {
    let start = start.min(frame);
    let end = (start + len).min(frame);
    let mut a = start.saturating_sub(margin);
    let mut b = (end + margin).min(frame);
    let want = (b - a).next_multiple_of(STEP).min(frame);
    let grow = want - (b - a);
    let right = grow.min(frame - b);
    b += right;
    a -= grow - right;
    (a, b - a)
}

/// How deep the hole goes: the farthest any pixel where `hole` is
/// nonzero lies from one where it is zero, in pixels (a chamfer
/// distance, within a few percent of the true one). Past the window's
/// edge counts as neither, so a hole at the frame's edge is as deep as
/// its distance from the known side.
pub fn depth(hole: &[f32], width: usize, height: usize) -> f32 {
    // Steps of 3 straight and 4 diagonal, a third of a pixel each.
    const FAR: u32 = u32::MAX / 2;
    let mut d: Vec<u32> = hole
        .iter()
        .map(|&h| if h > 0.0 { FAR } else { 0 })
        .collect();
    let at = |d: &[u32], x: isize, y: isize| -> u32 {
        if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
            FAR
        } else {
            d[y as usize * width + x as usize]
        }
    };
    for y in 0..height as isize {
        for x in 0..width as isize {
            let i = y as usize * width + x as usize;
            if d[i] == 0 {
                continue;
            }
            let best = (at(&d, x - 1, y) + 3)
                .min(at(&d, x, y - 1) + 3)
                .min(at(&d, x - 1, y - 1) + 4)
                .min(at(&d, x + 1, y - 1) + 4);
            d[i] = d[i].min(best);
        }
    }
    for y in (0..height as isize).rev() {
        for x in (0..width as isize).rev() {
            let i = y as usize * width + x as usize;
            if d[i] == 0 {
                continue;
            }
            let best = (at(&d, x + 1, y) + 3)
                .min(at(&d, x, y + 1) + 3)
                .min(at(&d, x + 1, y + 1) + 4)
                .min(at(&d, x - 1, y + 1) + 4);
            d[i] = d[i].min(best);
        }
    }
    // No known pixel at all: as deep as the window is long.
    if d.iter().any(|&v| v >= FAR) {
        return width.max(height) as f32;
    }
    d.iter().copied().max().unwrap_or(0) as f32 / 3.0
}

/// The scale a hole `depth` deep is filled at: the picture's own
/// pixels to [`DEPTH`], less past it.
pub fn scale_for(depth: f32) -> f32 {
    if depth <= DEPTH { 1.0 } else { DEPTH / depth }
}

/// `image` taken down to `width` × `height` by the mean of what each
/// new pixel covers, so the grain averages rather than aliases.
fn shrink(image: &Rgbf, width: usize, height: usize) -> Rgbf {
    let (sx, sy) = (
        image.width as f32 / width as f32,
        image.height as f32 / height as f32,
    );
    let mut data = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        let (y0, y1) = cover(y, sy, image.height);
        for x in 0..width {
            let (x0, x1) = cover(x, sx, image.width);
            let mut sum = [0.0f32; 3];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let p = image.at(xx, yy);
                    for c in 0..3 {
                        sum[c] += p[c];
                    }
                }
            }
            let n = ((x1 - x0) * (y1 - y0)) as f32;
            data.extend(sum.map(|v| v / n));
        }
    }
    Rgbf::new(width, height, data)
}

/// The hole taken down as [`shrink`] takes the picture: a new pixel
/// is in the hole when any pixel it covers is.
fn shrink_hole(hole: &[f32], w: usize, h: usize, width: usize, height: usize) -> Vec<f32> {
    let (sx, sy) = (w as f32 / width as f32, h as f32 / height as f32);
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        let (y0, y1) = cover(y, sy, h);
        for x in 0..width {
            let (x0, x1) = cover(x, sx, w);
            let any = (y0..y1).any(|yy| hole[yy * w + x0..yy * w + x1].iter().any(|&v| v > 0.0));
            out.push(if any { 1.0 } else { 0.0 });
        }
    }
    out
}

/// The source pixels new pixel `i` covers at `scale` source pixels a
/// new one, at least one.
fn cover(i: usize, scale: f32, len: usize) -> (usize, usize) {
    let a = ((i as f32 * scale) as usize).min(len - 1);
    let b = (((i + 1) as f32 * scale).ceil() as usize).clamp(a + 1, len);
    (a, b)
}

/// What [`filled`] made.
pub struct Filled {
    /// The window, at its own size, the hole made up.
    pub image: Rgbf,
    /// How many tiles the model ran.
    pub tiles: usize,
    /// The scale the model saw the window at.
    pub scale: f32,
    /// The window's size at that scale.
    pub size: (usize, usize),
}

/// `image` with the pixels where `hole` is nonzero made up by `run`:
/// at the picture's own pixels when the hole's [`depth`], `depth`, is
/// no more than [`DEPTH`], else taken down until it is, filled, and
/// taken back up under the hole alone, the rest of the window as it
/// was. In tiles of no side over `max` either way ([`tiled`]).
pub fn filled(
    image: &Rgbf,
    hole: &[f32],
    depth: f32,
    max: usize,
    run: impl FnMut(&Rgbf, &[f32]) -> Result<Rgbf>,
) -> Result<Filled> {
    let (w, h) = (image.width, image.height);
    if hole.len() != w * h {
        return Err(Error::Shape(format!(
            "a hole of {} for a window of {w}x{h}",
            hole.len()
        )));
    }
    let scale = scale_for(depth);
    if scale >= 1.0 {
        let (image, tiles) = tiled(image, hole, max, run)?;
        return Ok(Filled {
            image,
            tiles,
            scale: 1.0,
            size: (w, h),
        });
    }
    let sw = ((w as f32 * scale).round() as usize).max(1);
    let sh = ((h as f32 * scale).round() as usize).max(1);
    let small = shrink(image, sw, sh);
    let small_hole = shrink_hole(hole, w, h, sw, sh);
    let (answer, tiles) = tiled(&small, &small_hole, max, run)?;
    let back = answer.resampled(w, h);
    let mut out = image.clone();
    for (i, &m) in hole.iter().enumerate() {
        if m > 0.0 {
            out.data[i * 3..i * 3 + 3].copy_from_slice(&back.data[i * 3..i * 3 + 3]);
        }
    }
    Ok(Filled {
        image: out,
        tiles,
        scale: sw as f32 / w as f32,
        size: (sw, sh),
    })
}

/// The tiles along one axis of `len`: one when it fits in `max`,
/// else as few as cover it with [`OVERLAP`] between neighbors, each
/// the same multiple of 8, spread evenly. `max` is a multiple of 8.
fn spans(len: usize, max: usize) -> Vec<(usize, usize)> {
    if len <= max {
        return vec![(0, len)];
    }
    let n = (len - OVERLAP).div_ceil(max - OVERLAP);
    let size = (len + (n - 1) * OVERLAP)
        .div_ceil(n)
        .next_multiple_of(STEP)
        .min(max);
    (0..n)
        .map(|i| ((i * (len - size) + (n - 1) / 2) / (n - 1), size))
        .collect()
}

/// The tiles a window of `width` × `height` is filled in, no side
/// over `max`, in order: rows top to bottom, left to right within a
/// row.
pub fn tiles(width: usize, height: usize, max: usize) -> Vec<Window> {
    let (xs, ys) = (spans(width, max), spans(height, max));
    ys.iter()
        .flat_map(|&(y, h)| {
            xs.iter().map(move |&(x, w)| Window {
                x,
                y,
                width: w,
                height: h,
            })
        })
        .collect()
}

/// A tile's weight at `p` along one axis of `spans`, tile `i`: 1 in
/// its own part, ramping across each overlap so that neighbors' sum
/// to 1.
fn ramp(spans: &[(usize, usize)], i: usize, p: usize) -> f32 {
    let (start, size) = spans[i];
    let end = start + size;
    let center = p as f32 + 0.5;
    let mut w = 1.0f32;
    if i > 0 {
        let prev_end = spans[i - 1].0 + spans[i - 1].1;
        if p < prev_end {
            w = w.min((center - start as f32) / (prev_end - start) as f32);
        }
    }
    if i + 1 < spans.len() {
        let next = spans[i + 1].0;
        if p >= next {
            w = w.min((end as f32 - center) / (end - next) as f32);
        }
    }
    w.clamp(0.0, 1.0)
}

/// `image` with the pixels where `hole` is nonzero made up by `run`,
/// one tile of no side over `max` at a time ([`tiles`]): each tile
/// blended into the next across their overlap, and the part of an
/// overlap an earlier tile already holds the most of handed to the
/// later one as known, so the two continue each other. A tile with no
/// hole in it is not run. Answers the image and how many tiles ran.
pub fn tiled(
    image: &Rgbf,
    hole: &[f32],
    max: usize,
    mut run: impl FnMut(&Rgbf, &[f32]) -> Result<Rgbf>,
) -> Result<(Rgbf, usize)> {
    let (w, h) = (image.width, image.height);
    if hole.len() != w * h {
        return Err(Error::Shape(format!(
            "a hole of {} for a window of {w}x{h}",
            hole.len()
        )));
    }
    let (xs, ys) = (spans(w, max), spans(h, max));
    if xs.len() == 1 && ys.len() == 1 {
        return Ok((run(image, hole)?, 1));
    }
    let mut acc = vec![0.0f32; w * h * 3];
    let mut weight = vec![0.0f32; w * h];
    let mut ran = 0;
    for (j, &(ty, th)) in ys.iter().enumerate() {
        for (i, &(tx, tw)) in xs.iter().enumerate() {
            let mut data = Vec::with_capacity(tw * th * 3);
            let mut part = Vec::with_capacity(tw * th);
            for y in ty..ty + th {
                for x in tx..tx + tw {
                    let k = y * w + x;
                    if weight[k] >= 0.5 {
                        let s = 1.0 / weight[k];
                        data.extend([acc[k * 3] * s, acc[k * 3 + 1] * s, acc[k * 3 + 2] * s]);
                        part.push(0.0);
                    } else {
                        data.extend_from_slice(&image.data[k * 3..k * 3 + 3]);
                        part.push(hole[k]);
                    }
                }
            }
            let input = Rgbf::new(tw, th, data);
            let out = if part.iter().any(|&v| v > 0.0) {
                ran += 1;
                let out = run(&input, &part)?;
                if out.width != tw || out.height != th {
                    return Err(Error::Shape(format!(
                        "a tile of {tw}x{th} came back {}x{}",
                        out.width, out.height
                    )));
                }
                out
            } else {
                input
            };
            for y in 0..th {
                let wy = ramp(&ys, j, ty + y);
                for x in 0..tw {
                    let wt = wy * ramp(&xs, i, tx + x);
                    let k = (ty + y) * w + tx + x;
                    let o = (y * tw + x) * 3;
                    for c in 0..3 {
                        acc[k * 3 + c] += wt * out.data[o + c];
                    }
                    weight[k] += wt;
                }
            }
        }
    }
    let data = acc
        .as_chunks::<3>()
        .0
        .iter()
        .zip(&weight)
        .flat_map(|(v, &wt)| {
            let s = if wt > 0.0 { 1.0 / wt } else { 0.0 };
            [v[0] * s, v[1] * s, v[2] * s]
        })
        .collect();
    Ok((Rgbf::new(w, h, data), ran))
}

/// Why an answer `out` to `input` with `hole` cannot be right, if it
/// cannot: a pixel outside the hole that did not come back as it went
/// in (both in 0 to 1). What the hole holds is not judged: a fill may
/// rightly be far lighter or darker than its surround (dust on a lamp),
/// and what a card out of memory leaves there is unmeasured.
pub fn implausible(input: &Rgbf, hole: &[f32], out: &Rgbf) -> Option<String> {
    for (i, _) in hole.iter().enumerate().filter(|(_, m)| **m <= 0.0) {
        for c in 0..3 {
            let (a, b) = (input.data[i * 3 + c].clamp(0.0, 1.0), out.data[i * 3 + c]);
            if (a - b).abs() > OUTSIDE {
                return Some(format!(
                    "the picture outside the hole came back changed ({a:.3} as {b:.3})"
                ));
            }
        }
    }
    None
}

/// The index `i` of `0..len + pad` folded back into `0..len`, as a
/// mirror at the far edge.
pub(crate) fn mirror(i: usize, len: usize) -> usize {
    if i < len {
        i
    } else if len < 2 {
        0
    } else {
        let period = 2 * (len - 1);
        let k = i % period;
        if k < len { k } else { period - k }
    }
}

pub struct Fill {
    loaded: Loaded,
}

/// The providers LaMa is tried on, of those `given`: not Core ML,
/// whose static shapes LaMa's free size would send to the CPU.
pub fn offered(given: &[Provider]) -> Vec<Provider> {
    Provider::without_core_ml(given)
}

impl Fill {
    pub fn load(store: &Store, providers: &[Provider]) -> Result<Self> {
        if !store.have(&FILL) {
            return Err(Error::Missing(FILL.name));
        }
        let providers = &offered(providers);
        let path = store.path(&FILL, &FILL.files[0]);
        let remember = runtime::Remembered {
            store_root: store.root(),
            model: FILL.id,
            hash: FILL.files[0].sha256,
        };
        let loaded = runtime::open(
            &path,
            GraphOptimizationLevel::Level3,
            providers,
            Some(remember),
            |s| {
                let image =
                    Tensor::from_array(([1usize, 3, WARM, WARM], vec![0.5f32; 3 * WARM * WARM]))?;
                let mask =
                    Tensor::from_array(([1usize, 1, WARM, WARM], vec![0.0f32; WARM * WARM]))?;
                s.run(ort::inputs!["image" => image, "mask" => mask])?;
                Ok(())
            },
        )?;
        Ok(Self { loaded })
    }

    pub fn provider(&self) -> Provider {
        self.loaded.provider
    }

    /// `image`, in 0 to 1, with the pixels where `hole` is nonzero made
    /// up, in one run of the model: no side over [`MAX`]. A side that
    /// is not a multiple of 8 is mirrored out to one and cut back. The
    /// answer is in 0 to 1 too, and [`Error::Implausible`] when it
    /// cannot be right ([`implausible`]).
    pub fn fill(&mut self, image: &Rgbf, hole: &[f32]) -> Result<Rgbf> {
        let (w, h) = (image.width, image.height);
        let pw = w.next_multiple_of(STEP).max(SMALLEST);
        let ph = h.next_multiple_of(STEP).max(SMALLEST);
        if w == 0 || h == 0 || pw > MAX || ph > MAX || hole.len() != w * h {
            return Err(Error::Shape(format!(
                "fill wants a side of 1 to {MAX}, got {w}x{h} and a hole of {}",
                hole.len()
            )));
        }
        // Mirrored out to the model's sizes, the hole too, so nothing
        // under the hole is shown as known in the margin. The model's
        // own forward pass blanks the picture under the mask before the
        // network; the export does as well, and so does this, which
        // costs nothing.
        let n = pw * ph;
        let mut planes = vec![0.0f32; 3 * n];
        let mut mask = vec![0.0f32; n];
        for y in 0..ph {
            let sy = mirror(y, h);
            for x in 0..pw {
                let s = sy * w + mirror(x, w);
                let m = if hole[s] > 0.0 { 1.0 } else { 0.0 };
                let d = y * pw + x;
                mask[d] = m;
                for c in 0..3 {
                    planes[c * n + d] = image.data[s * 3 + c].clamp(0.0, 1.0) * (1.0 - m);
                }
            }
        }
        let image_in = Tensor::from_array(([1usize, 3, ph, pw], planes))?;
        let mask_in = Tensor::from_array(([1usize, 1, ph, pw], mask))?;
        let outputs = self
            .loaded
            .session
            .run(ort::inputs!["image" => image_in, "mask" => mask_in])?;
        let (shape, out) = outputs["output"].try_extract_tensor::<f32>()?;
        if out.len() != 3 * n {
            return Err(Error::Shape(shape.to_string()));
        }
        if out.iter().any(|v| !v.is_finite()) {
            return Err(Error::Implausible("the fill is not finite".into()));
        }
        // Some exports answer in 0 to 255.
        let peak = out.iter().copied().fold(0.0f32, f32::max);
        let scale = if peak > 2.0 { 1.0 / 255.0 } else { 1.0 };
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let d = y * pw + x;
                for c in 0..3 {
                    data.push((out[c * n + d] * scale).clamp(0.0, 1.0));
                }
            }
        }
        let out = Rgbf::new(w, h, data);
        if let Some(why) = implausible(image, hole, &out) {
            return Err(Error::Implausible(why));
        }
        Ok(out)
    }

    /// [`Fill::fill`] for a window of any size and a hole `depth` deep
    /// ([`filled`]), in tiles over [`MAX`].
    pub fn fill_window(&mut self, image: &Rgbf, hole: &[f32], depth: f32) -> Result<Filled> {
        filled(image, hole, depth, MAX, |tile, part| self.fill(tile, part))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lama_is_never_tried_on_core_ml() {
        use crate::Provider;
        let all = [Provider::CoreMl, Provider::WebGpu, Provider::Cpu];
        assert_eq!(super::offered(&all), [Provider::WebGpu, Provider::Cpu]);
    }

    use super::*;

    fn win(x: usize, y: usize, width: usize, height: usize) -> Window {
        Window {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn the_window_is_the_box_and_half_its_longer_side_round_it() {
        // A 100 × 60 box, 30 deep: 50 each side, 200 × 160, both
        // multiples of 8.
        let w = window(win(1000, 1000, 100, 60), 30.0, 8000, 6000);
        assert_eq!(w, win(950, 950, 200, 160));
        // A small one gets the least margin.
        let w = window(win(500, 500, 10, 10), 5.0, 8000, 6000);
        assert_eq!(w, win(468, 468, 80, 80));
        // A long thin stroke, 3000 by 20 and 10 deep: not 1500 each
        // side, the least margin.
        let w = window(win(2000, 2000, 3000, 20), 10.0, 8000, 6000);
        assert_eq!(w, win(1968, 1968, 3064, 88));
    }

    #[test]
    fn the_window_is_rounded_up_to_a_multiple_of_8() {
        // 101 × 37: margins 50, so 201 × 137, rounded to 208 × 144,
        // grown on the far side.
        let w = window(win(1000, 2000, 101, 37), 50.0, 8000, 6000);
        assert_eq!((w.x, w.y), (950, 1950));
        assert_eq!((w.width, w.height), (208, 144));
        assert_eq!(w.width % 8, 0);
        assert_eq!(w.height % 8, 0);
    }

    #[test]
    fn a_hole_at_the_frame_edge_keeps_its_window_inside_the_frame() {
        // At the bottom right corner: the window grows inward instead.
        let w = window(win(7950, 5970, 50, 30), 15.0, 8000, 6000);
        assert_eq!(w.x + w.width, 8000);
        assert_eq!(w.y + w.height, 6000);
        assert_eq!(w.width % 8, 0);
        assert_eq!(w.height % 8, 0);
        assert!(w.x <= 7950 - 32 && w.y <= 5970 - 32);
        // At the top left.
        let w = window(win(0, 0, 40, 40), 20.0, 8000, 6000);
        assert_eq!((w.x, w.y), (0, 0));
        assert_eq!((w.width, w.height), (72, 72));
        // A frame whose side is no multiple of 8 and narrower than the
        // window: the frame, left for the model's mirror to pad.
        let w = window(win(10, 10, 80, 80), 40.0, 101, 6000);
        assert_eq!((w.x, w.width), (0, 101));
    }

    #[test]
    fn a_window_past_the_cap_is_tiled() {
        // A window of 6000 × 4000, as a shallow stroke across a 45 MP
        // frame makes.
        let w = win(0, 0, 6000, 4000);
        let gpu = tiles(w.width, w.height, MAX);
        // Seven across and five down, each within the cap and a
        // multiple of 8, covering the window, neighbors overlapping
        // enough.
        assert_eq!(gpu.len(), 7 * 5);
        for t in &gpu {
            assert!(t.width <= MAX && t.height <= MAX);
            assert_eq!(t.width % 8, 0);
            assert_eq!(t.height % 8, 0);
            assert!(t.x + t.width <= w.width && t.y + t.height <= w.height);
        }
        let xs = spans(w.width, MAX);
        assert_eq!(xs.first().unwrap().0, 0);
        assert_eq!(xs.last().map(|s| s.0 + s.1), Some(w.width));
        for p in xs.windows(2) {
            assert!(p[0].0 + p[0].1 >= p[1].0 + OVERLAP);
        }
        // Within the cap, one tile.
        assert_eq!(tiles(MAX, 800, MAX), vec![win(0, 0, MAX, 800)]);
        assert_eq!(spans(MAX + 1, MAX).len(), 2);
    }

    #[test]
    fn the_ramps_sum_to_one_across_every_overlap() {
        for (len, max) in [
            (2049, MAX),
            (3000, MAX),
            (4100, MAX),
            (6000, MAX),
            (9999, MAX),
            (3001, 2048),
        ] {
            let s = spans(len, max);
            for p in 0..len {
                let sum: f32 = (0..s.len())
                    .filter(|&i| p >= s[i].0 && p < s[i].0 + s[i].1)
                    .map(|i| ramp(&s, i, p))
                    .sum();
                assert!((sum - 1.0).abs() < 1e-5, "{len} at {p}: {sum}");
            }
        }
    }

    /// A fill that answers each tile with a flat value of its own in
    /// the hole: the near half of the overlap is the first tile's,
    /// handed on to the second as known, and the far half ramps from
    /// the first to the second; outside the hole the picture comes
    /// back untouched.
    #[test]
    fn tiles_blend_across_their_overlap() {
        let (w, h) = (3000, 40);
        let image = Rgbf::new(w, h, vec![0.25; w * h * 3]);
        let hole: Vec<f32> = (0..w * h)
            .map(|i| {
                if (10..30).contains(&(i / w)) {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        let mut calls = 0;
        let (out, ran) = tiled(&image, &hole, 2048, |tile, part| {
            calls += 1;
            let v = calls as f32;
            let data = tile
                .data
                .as_chunks::<3>()
                .0
                .iter()
                .zip(part)
                .flat_map(|(p, &m)| if m > 0.0 { [v; 3] } else { [p[0], p[1], p[2]] })
                .collect();
            Ok(Rgbf::new(tile.width, tile.height, data))
        })
        .unwrap();
        assert_eq!(ran, 2);
        let s = spans(w, 2048);
        let (a, b) = (s[1].0, s[0].0 + s[0].1);
        assert!(b - a >= OVERLAP);
        let mid = (a + b) / 2;
        let row = 20;
        // Each tile's own part.
        assert_eq!(out.at(a / 2, row)[0], 1.0);
        assert_eq!(out.at((b + w) / 2, row)[0], 2.0);
        // The near half of the overlap carried, the far half a ramp.
        for x in a..mid {
            assert_eq!(out.at(x, row)[0], 1.0, "{x}");
        }
        let mut last = 1.0;
        for x in mid..b {
            let v = out.at(x, row)[0];
            assert!(v > last && v < 2.0, "{x}: {last} to {v}");
            last = v;
        }
        assert!(last > 1.95);
        // Outside the hole, the picture.
        assert_eq!(out.at(a, 2), [0.25; 3]);
        assert_eq!(out.at(10, 39), [0.25; 3]);
    }

    #[test]
    fn a_tile_without_a_hole_is_not_run() {
        let (w, h) = (5000, 64);
        let image = Rgbf::new(w, h, vec![0.5; w * h * 3]);
        let hole: Vec<f32> = (0..w * h)
            .map(|i| if (i % w) < 100 { 1.0 } else { 0.0 })
            .collect();
        let (out, ran) = tiled(&image, &hole, MAX, |tile, _| Ok(tile.clone())).unwrap();
        assert_eq!(ran, 1);
        assert_eq!(out.at(4000, 10), [0.5; 3]);
    }

    fn hole_of(w: usize, h: usize, inside: impl Fn(usize, usize) -> bool) -> Vec<f32> {
        (0..w * h)
            .map(|i| if inside(i % w, i / w) { 1.0 } else { 0.0 })
            .collect()
    }

    #[test]
    fn a_holes_depth_is_how_far_its_middle_is_from_the_known() {
        // A square of 100: 50 deep, to within the chamfer's error.
        let hole = hole_of(300, 300, |x, y| {
            (100..200).contains(&x) && (100..200).contains(&y)
        });
        let d = depth(&hole, 300, 300);
        assert!((d - 50.0).abs() < 2.0, "{d}");
        // A stroke 20 wide however long: 10.
        let hole = hole_of(3000, 100, |_, y| (40..60).contains(&y));
        let d = depth(&hole, 3000, 100);
        assert!((d - 10.0).abs() < 1.0, "{d}");
        // A diagonal stroke, about as deep.
        let hole = hole_of(400, 400, |x, y| (x as isize - y as isize).abs() < 10);
        let d = depth(&hole, 400, 400);
        assert!((6.0..=10.0).contains(&d), "{d}");
        // At the frame's edge the far side is not known: a band 30
        // wide along the top is 30 deep.
        let hole = hole_of(200, 200, |_, y| y < 30);
        let d = depth(&hole, 200, 200);
        assert!((d - 30.0).abs() < 1.0, "{d}");
        // No hole, no depth; all hole, as deep as the window is long.
        assert_eq!(depth(&vec![0.0; 100], 10, 10), 0.0);
        assert_eq!(depth(&vec![1.0; 300], 30, 10), 30.0);
    }

    #[test]
    fn a_deep_hole_is_filled_at_the_scale_that_brings_it_to_the_depth() {
        assert_eq!(scale_for(10.0), 1.0);
        assert_eq!(scale_for(DEPTH), 1.0);
        assert_eq!(scale_for(4.0 * DEPTH), 0.25);
        // A square of 512 in a 1024 window: 256 deep, so a quarter.
        let (w, h) = (1024, 1024);
        let image = Rgbf::new(w, h, vec![0.3; w * h * 3]);
        let hole = hole_of(w, h, |x, y| {
            (256..768).contains(&x) && (256..768).contains(&y)
        });
        let mut seen = (0, 0);
        let out = filled(&image, &hole, depth(&hole, w, h), MAX, |tile, part| {
            seen = (tile.width, tile.height);
            // The hole made 0.7, the rest handed back.
            let data = tile
                .data
                .as_chunks::<3>()
                .0
                .iter()
                .zip(part)
                .flat_map(|(p, &m)| {
                    if m > 0.0 {
                        [0.7; 3]
                    } else {
                        [p[0], p[1], p[2]]
                    }
                })
                .collect();
            Ok(Rgbf::new(tile.width, tile.height, data))
        })
        .unwrap();
        assert!((out.scale - 0.25).abs() < 0.01, "{}", out.scale);
        assert_eq!(seen, (256, 256));
        assert_eq!(out.size, (256, 256));
        assert_eq!((out.image.width, out.image.height), (w, h));
        // Under the hole the answer, taken back up; outside, the
        // window as it was.
        assert!((out.image.at(512, 512)[0] - 0.7).abs() < 1e-5);
        assert_eq!(out.image.at(100, 100), [0.3; 3]);
        assert_eq!(out.image.at(255, 512), [0.3; 3]);
        // A shallow one at the window's own pixels.
        let hole = hole_of(w, h, |x, y| {
            (500..540).contains(&x) && (100..900).contains(&y)
        });
        let out = filled(&image, &hole, depth(&hole, w, h), MAX, |tile, _| {
            Ok(tile.clone())
        })
        .unwrap();
        assert_eq!(out.scale, 1.0);
        assert_eq!(out.size, (w, h));
    }

    #[test]
    fn shrinking_averages_and_keeps_every_hole_pixel() {
        // Stripes a pixel wide average to their mean, not to one of
        // them.
        let (w, h) = (64, 64);
        let data = (0..w * h)
            .flat_map(|i| [if i % 2 == 0 { 1.0 } else { 0.0 }; 3])
            .collect();
        let small = shrink(&Rgbf::new(w, h, data), 16, 16);
        for v in &small.data {
            assert!((v - 0.5).abs() < 1e-6);
        }
        // One hole pixel marks the new pixel it falls in.
        let mut hole = vec![0.0; w * h];
        hole[37 * w + 21] = 1.0;
        let small = shrink_hole(&hole, w, h, 16, 16);
        assert_eq!(small.iter().filter(|&&v| v > 0.0).count(), 1);
        assert_eq!(small[(37 / 4) * 16 + 21 / 4], 1.0);
    }

    #[test]
    fn an_answer_that_changed_the_outside_is_refused() {
        let (w, h) = (64, 64);
        let input = Rgbf::new(w, h, vec![0.8; w * h * 3]);
        let hole = hole_of(w, h, |x, y| (16..48).contains(&x) && (16..48).contains(&y));
        let answer = |inside: f32, outside: f32| {
            let data = hole
                .iter()
                .flat_map(|&m| [if m > 0.0 { inside } else { outside }; 3])
                .collect();
            Rgbf::new(w, h, data)
        };
        // A fill a little off its surround is a fill.
        assert_eq!(implausible(&input, &hole, &answer(0.6, 0.8)), None);
        // The outside not given back.
        assert!(implausible(&input, &hole, &answer(0.8, 0.79)).is_some());
        // A dark hole in a light surround, or the reverse, may be
        // right: dust on a lamp.
        assert_eq!(implausible(&input, &hole, &answer(0.0, 0.8)), None);
        // Values past 1 go in clamped and come back so.
        let bright = Rgbf::new(w, h, vec![1.4; w * h * 3]);
        assert_eq!(implausible(&bright, &hole, &answer(0.9, 1.0)), None);
    }

    #[test]
    fn the_mirror_folds_back_from_the_far_edge() {
        let folded: Vec<usize> = (0..9).map(|i| mirror(i, 5)).collect();
        assert_eq!(folded, [0, 1, 2, 3, 4, 3, 2, 1, 0]);
        assert_eq!(mirror(7, 1), 0);
        assert_eq!(mirror(40, 5), mirror(40 % 8, 5));
    }
}
