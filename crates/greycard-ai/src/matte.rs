//! The sky's edge as a matte: a color-line matte at the frame's own
//! resolution, in its linear working-space values, where the preview
//! may clip a bright sky to white and has a quarter of the pixels.
//!
//! What a pixel between the sky and what is in front of it is: a mix
//! of the two, so its color lies on the line from the local not-sky
//! color to the local sky color, and how far along is its share of
//! sky. The two colors are normalized Gaussian blurs of the regions
//! known to be each; nothing is solved, so the cost is linear in the
//! pixels. The trial behind it (notes, the sky trial and its matting
//! follow-up) rejected closed-form and KNN matting (minutes a frame,
//! and a haze of a third to a half over trees far from clear sky) and
//! SAM on full-size tiles (a binary outline, not a matte).
//!
//! Where it departs from the trial's `edges_matte.py`:
//!
//! - The trimap comes from the prior at the preview: known sky is the
//!   outline shrunk by half a percent of the long side; known not-sky
//!   is everything outside the outline grown by two percent, less the
//!   prior's tree, flower and unlabeled pixels within a fifth of the
//!   long side of the known sky, which stay in question since sky
//!   shows through them; the rest is in question. What SAM, or a
//!   person's negative pick, left out of the outline is not brought
//!   back because the prior calls it sky.
//! - The prior's things (people and objects: COCO's first 80 classes),
//!   shrunk by half a percent, are known not-sky, so a lens or a shirt
//!   the color of the sky takes no alpha (the trial's sunglasses took
//!   a quarter).
//! - The known sky grows in three passes before the full-size matte,
//!   pixels scored clearly sky joining it after each, so the local sky
//!   color is sampled close to each gap in a canopy rather than from
//!   the open sky beside it (`tree_gaps.py`'s growth). "Clearly sky"
//!   also asks that the pixel sit near the line and not far past the
//!   sky's end of it, which a white wall brighter than the sky would.
//!
//! - A pixel's share is not the projection alone. Far off the line, or
//!   far past the sky's end of it, it takes none (a leaf of another
//!   hue, a white wall against a blue sky, a street lamp); as bright as
//!   the sky, it must be of the sky's hue too (the unmix stage's gate:
//!   a white sign against a pale overcast). And where
//!   the prior names a class sky does not show through (a mountain, a
//!   building, the sea, a person), its sky probability bounds the
//!   share: none past a low probability outside the outline, since a
//!   snowy ridge under a grey sky lies on the line between the sky and
//!   the dark water below; all of it past a high one, since a dark
//!   cloud SAM left out is sky, but only for a pixel darker than the
//!   local sky and not near a person's negative pick. Tree, flower and
//!   unlabeled pixels, the ones sky shows through, are the
//!   projection's alone.
//! - The soft map over every query is diluted by the model's leftover
//!   queries, on some frames to a quarter over a sky the model is sure
//!   of (a sea of cloud under a peak). The floor above reads it as it
//!   is, since there a low value is the prior's doubt and the floor
//!   overrides the pixel's color. Two places read the map over the
//!   kept queries instead, as the gate does, within the labeled sky:
//!   which pixels may join the known sky in the growth (still on the
//!   sky's color, near the line and not past it), and the sure sky's
//!   own floor. That floor holds a pixel up to all sky where the kept
//!   map is sure and the pixel is the sky's own color a little darker:
//!   in the last part of the line toward the sky (from `LINE_FLOOR`),
//!   and, read off the preview's pixel where the frame's noise is
//!   averaged out, near the line and of the sky's hue. That is the
//!   frame's own noise on a flat sky the projection clamps at the sky's
//!   end, which left a pale sky blotched where nothing held it up; a
//!   pixel of another color is the projection's, as before. It applies
//!   to the labeled sky inside the outline, and to the unlabeled
//!   pockets the labels leave at the logits' grid inside it
//!   ([`pockets`]); a pole the labels' coarse step calls sky lies near
//!   the not-sky end of the line and is not held.
//!
//! The known regions and their colors are made at the preview (the
//! growth) and at about 512 pixels (the blurs); only the projection
//! and the final guided filter (radius 8, ε 1e-4, the frame's own
//! luminance as the guide) run at full size, a block of the output
//! at a time, and a block with nothing in question costs a lookup.

use rayon::prelude::*;

use greycard_core::guided;
use greycard_core::image::WorkingImage;

use crate::image::Mask;
use crate::sky::{Prior, SKY_CLASS, UNLABELED, dilate, erode};

/// `tree-merged` and `flower` among the model's classes: sky shows
/// through them.
pub const TREE_CLASS: u8 = 116;
pub const FLOWER_CLASS: u8 = 88;
/// COCO's things (people, animals, vehicles, objects) are its first 80
/// classes; the rest are stuff.
pub const THINGS: u8 = 80;

/// The trimap's distances, as shares of the preview's long side.
const SHRINK: f32 = 0.005;
const GROW: f32 = 0.02;
const REACH: f32 = 0.20;
const THING_SHRINK: f32 = 0.005;
/// The blurs' reach, as shares of the long side: near, and far where
/// the near one has too little weight (into a canopy).
const NEAR: f32 = 0.02;
const FAR: f32 = 0.15;
/// The long side the local colors are made at.
const COLOR_SIDE: usize = 512;
/// Growth passes, and what counts as clearly sky in them.
const PASSES: usize = 3;
const CLEARLY: f32 = 0.95;
/// How far off the line a pixel may be, in the line's lengths, before
/// its share fades, and where it is gone.
const OFF_LINE: (f32, f32) = (0.25, 0.5);
/// How far past the sky's end of the line, likewise.
const BEYOND: (f32, f32) = (1.3, 1.8);
/// How far a pixel as bright as the sky may be from the sky's own hue
/// (the distance between their chromaticities, r, g and b over their
/// sum, summed) before its share fades, and where it is gone: the
/// unmix stage's gate, which keeps a white sign or a pale wall against
/// a pale blue overcast out.
const HUE: (f32, f32) = (0.04, 0.10);
/// For a pixel of a class sky does not show through: the prior's sky
/// probability over which it is held up to all sky, and under which it
/// is held down to none outside the outline.
const FLOOR: (f32, f32) = (0.7, 0.95);
const CAP: (f32, f32) = (0.05, 0.35);
/// For a pixel of the sure sky inside the outline: how far along the
/// line toward the sky it must lie before the kept map holds it up,
/// and where it is held up to all of it.
const LINE_FLOOR: (f32, f32) = (0.4, 0.7);
/// An unlabeled pocket of the sky keeps at least this share of the
/// soft map of the sky around it, and covers no more than this share
/// of the frame: a few dozen of the logits' cells (the largest of a sea
/// of cloud's pockets was 0.40%), not a foreground the labels left.
const POCKET: f32 = 0.8;
const POCKET_MOST: f32 = 0.005;
/// How far a person's negative pick reaches over the region it was
/// made on, as a share of the long side.
const NEGATIVE_REACH: f32 = 0.2;
/// The final guided filter.
const RADIUS: usize = 8;
const EPS: f32 = 1e-4;
/// Output pixels a block, each way.
const BLOCK: usize = 128;

/// What the matte knows of a pixel before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Known {
    Sky,
    NotSky,
    Unknown,
}

/// The trimap at the prior's size, from the outline (one or nothing)
/// and the prior.
pub fn trimap(outline: &Mask, prior: &Prior) -> Vec<Known> {
    let (w, h) = (prior.width, prior.height);
    assert_eq!((outline.width, outline.height), (w, h));
    let long = w.max(h) as f32;
    let px = |share: f32| ((share * long).round() as usize).max(1);
    let m: Vec<bool> = outline.data.iter().map(|&v| v > 0.5).collect();
    let things = erode(
        &prior.labels.iter().map(|&l| l < THINGS).collect::<Vec<_>>(),
        w,
        h,
        px(THING_SHRINK),
    );
    let sky: Vec<bool> = erode(&m, w, h, px(SHRINK))
        .into_iter()
        .zip(&things)
        .map(|(s, t)| s && !t)
        .collect();
    // In question near the outline, not near the prior's sky: what SAM
    // (or a person's negative pick) left out of the outline stays out.
    let grown = dilate(&m, w, h, px(GROW));
    let near_sky = dilate(&sky, w, h, px(REACH));
    (0..w * h)
        .map(|i| {
            let l = prior.labels[i];
            let treeish = l == TREE_CLASS || l == FLOWER_CLASS || l == UNLABELED;
            if things[i] {
                Known::NotSky
            } else if sky[i] {
                Known::Sky
            } else if !grown[i] && !(treeish && near_sky[i]) {
                Known::NotSky
            } else {
                Known::Unknown
            }
        })
        .collect()
}

/// The unlabeled pockets of the sky, at the prior's size: each region
/// (four ways) of pixels no kept query claims that borders only
/// labeled sky (and the frame's edge), all of that border inside the
/// outline, no larger than `POCKET_MOST` of the frame, and whose soft
/// map over all the queries keeps at least `POCKET` of its border's on
/// average. The labels leave such pockets at the logits' grid where
/// the winning mask dips under a half in open sky. What borders a tree
/// or any other label is not one. A thing in the sky the labels did
/// not name is often kept out by the soft map, which the sky's masks
/// let fall over it (a lantern keeps a third of the sky's, a railing a
/// ninth), but not always: a thin dark post the labels' coarse step
/// surrounds keeps nearly all of it, and is a pocket where the outline
/// is the labels. What keeps such a thing out is the sure sky's floor
/// itself, which holds only the sky's own color.
pub fn pockets(outline: &[f32], prior: &Prior) -> Vec<bool> {
    let mut out = vec![false; prior.width * prior.height];
    mark_pockets(outline, prior, &mut out);
    out
}

/// [`pockets`] set in `out`, which is left as it is elsewhere.
fn mark_pockets(outline: &[f32], prior: &Prior, out: &mut [bool]) {
    let (w, h) = (prior.width, prior.height);
    let unlabeled = |i: usize| prior.labels[i] == UNLABELED;
    let mut seen: Vec<bool> = Vec::new();
    let mut stack = Vec::new();
    let mut region = Vec::new();
    for start in 0..w * h {
        if !unlabeled(start) || seen.get(start).copied().unwrap_or(false) {
            continue;
        }
        if seen.is_empty() {
            seen = vec![false; w * h];
        }
        seen[start] = true;
        stack.push(start);
        region.clear();
        let mut enclosed = true;
        let (mut border, mut n) = (0.0f64, 0usize);
        while let Some(i) = stack.pop() {
            region.push(i);
            let (x, y) = (i % w, i / w);
            let steps = [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then(|| i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then(|| i + w),
            ];
            for j in steps.into_iter().flatten() {
                if unlabeled(j) {
                    if !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                } else if prior.labels[j] == SKY_CLASS && outline[j] > 0.5 {
                    border += prior.sky[j] as f64;
                    n += 1;
                } else {
                    enclosed = false;
                }
            }
        }
        if !enclosed || n == 0 || region.len() as f32 > POCKET_MOST * (w * h) as f32 {
            continue;
        }
        let inside = region.iter().map(|&i| prior.sky[i] as f64).sum::<f64>() / region.len() as f64;
        if inside >= POCKET as f64 * border / n as f64 {
            for &i in &region {
                out[i] = true;
            }
        }
    }
}

/// What a person's negative picks say is not sky, at the prior's size:
/// from each pick, the pixels outside the outline of the prior's class
/// at the pick, connected to it, within `NEGATIVE_REACH` of the long
/// side. So a click on a wall the prior calls sky covers the wall, and
/// does not spread into the sky inside the outline.
pub fn negated(outline: &[f32], prior: &Prior, negatives: &[[f32; 2]]) -> Vec<bool> {
    let (w, h) = (prior.width, prior.height);
    let mut out = vec![false; w * h];
    let reach = (NEGATIVE_REACH * w.max(h) as f32).max(1.0);
    for n in negatives {
        let (x0, y0) = (
            ((n[0] * w as f32) as usize).min(w - 1),
            ((n[1] * h as f32) as usize).min(h - 1),
        );
        let start = y0 * w + x0;
        if outline[start] > 0.5 {
            continue;
        }
        let label = prior.labels[start];
        let mut stack = vec![start];
        out[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            let steps = [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then(|| i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then(|| i + w),
            ];
            for j in steps.into_iter().flatten() {
                let (jx, jy) = (j % w, j / w);
                let far = (jx as f32 - x0 as f32).hypot(jy as f32 - y0 as f32) > reach;
                if !out[j] && !far && outline[j] <= 0.5 && prior.labels[j] == label {
                    out[j] = true;
                    stack.push(j);
                }
            }
        }
    }
    out
}

/// A linear RGB plane, three values a pixel.
struct Plane {
    w: usize,
    h: usize,
    data: Vec<f32>,
}

/// `image` averaged down to `w` × `h` (each output pixel the mean of
/// the input pixels whose centers fall in it), or up bilinearly if it
/// is smaller.
fn area_rgb(src: &[f32], sw: usize, sh: usize, w: usize, h: usize) -> Plane {
    let mut data = vec![0.0f32; w * h * 3];
    data.par_chunks_mut(w * 3).enumerate().for_each(|(y, row)| {
        let (y0, y1) = span(y, h, sh);
        for x in 0..w {
            let (x0, x1) = span(x, w, sw);
            let mut acc = [0.0f32; 3];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = (yy * sw + xx) * 3;
                    acc[0] += src[i];
                    acc[1] += src[i + 1];
                    acc[2] += src[i + 2];
                }
            }
            let n = ((y1 - y0) * (x1 - x0)).max(1) as f32;
            row[x * 3..x * 3 + 3].copy_from_slice(&acc.map(|v| v / n));
        }
    });
    Plane { w, h, data }
}

/// The input rows (or columns) output `o` of `n` averages over, of
/// `size`: never empty.
fn span(o: usize, n: usize, size: usize) -> (usize, usize) {
    let a = o * size / n;
    let b = ((o + 1) * size / n).max(a + 1).min(size);
    (a.min(size - 1), b)
}

fn area_plane(src: &[f32], sw: usize, sh: usize, w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (y0, y1) = span(y, h, sh);
        for (x, o) in row.iter_mut().enumerate() {
            let (x0, x1) = span(x, w, sw);
            let mut acc = 0.0f32;
            for yy in y0..y1 {
                acc += src[yy * sw + x0..yy * sw + x1].iter().sum::<f32>();
            }
            *o = acc / ((y1 - y0) * (x1 - x0)).max(1) as f32;
        }
    });
    out
}

/// A Gaussian blur of `sigma` pixels, as three box blurs.
fn gauss(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let r = (((12.0 * sigma * sigma / 3.0 + 1.0).sqrt() - 1.0) / 2.0)
        .round()
        .max(1.0) as usize;
    let mut out = guided::box_mean(plane, w, h, r);
    for _ in 0..2 {
        out = guided::box_mean(&out, w, h, r);
    }
    out
}

/// The local sky and not-sky colors at about `COLOR_SIDE`, and where
/// each is known at all.
struct Colors {
    w: usize,
    h: usize,
    sky: Vec<[f32; 3]>,
    not: Vec<[f32; 3]>,
    sky_ok: Vec<f32>,
    not_ok: Vec<f32>,
}

impl Colors {
    /// From the linear picture `lin` and the known regions (one or
    /// nothing, the same size), all at `COLOR_SIDE`.
    fn of(lin: &Plane, sky: &[f32], not: &[f32]) -> Self {
        let (w, h) = (lin.w, lin.h);
        let long = w.max(h) as f32;
        let side = |region: &[f32]| -> (Vec<[f32; 3]>, Vec<f32>) {
            let blurred = |sigma: f32| -> ([Vec<f32>; 3], Vec<f32>) {
                let num = std::array::from_fn(|c| {
                    let p: Vec<f32> = (0..w * h)
                        .map(|i| lin.data[i * 3 + c] * region[i])
                        .collect();
                    gauss(&p, w, h, sigma)
                });
                (num, gauss(region, w, h, sigma))
            };
            let (near, dn) = blurred(NEAR * long);
            let (far, df) = blurred(FAR * long);
            let mut color = vec![[0.0f32; 3]; w * h];
            let mut ok = vec![0.0f32; w * h];
            for i in 0..w * h {
                let (num, den) = if dn[i] > 0.05 {
                    (&near, dn[i])
                } else {
                    (&far, df[i])
                };
                if den > 1e-4 {
                    color[i] = [num[0][i] / den, num[1][i] / den, num[2][i] / den];
                    ok[i] = 1.0;
                }
            }
            (color, ok)
        };
        let (sky_c, sky_ok) = side(sky);
        let (not_c, not_ok) = side(not);
        Self {
            w,
            h,
            sky: sky_c,
            not: not_c,
            sky_ok,
            not_ok,
        }
    }

    /// Both colors at (u, v) in fractions of the frame, bilinearly,
    /// and whether both are known there.
    fn at(&self, u: f32, v: f32) -> Option<([f32; 3], [f32; 3])> {
        let fx = (u * self.w as f32 - 0.5).max(0.0);
        let fy = (v * self.h as f32 - 0.5).max(0.0);
        let x0 = (fx as usize).min(self.w - 1);
        let y0 = (fy as usize).min(self.h - 1);
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let idx = [
            y0 * self.w + x0,
            y0 * self.w + x1,
            y1 * self.w + x0,
            y1 * self.w + x1,
        ];
        let wts = [
            (1.0 - tx) * (1.0 - ty),
            tx * (1.0 - ty),
            (1.0 - tx) * ty,
            tx * ty,
        ];
        let mut s = [0.0f32; 3];
        let mut n = [0.0f32; 3];
        let (mut so, mut no) = (0.0f32, 0.0f32);
        for (&i, &wt) in idx.iter().zip(&wts) {
            so += self.sky_ok[i] * wt;
            no += self.not_ok[i] * wt;
            for c in 0..3 {
                s[c] += self.sky[i][c] * self.sky_ok[i] * wt;
                n[c] += self.not[i][c] * self.not_ok[i] * wt;
            }
        }
        if so < 0.5 || no < 0.5 {
            return None;
        }
        Some((s.map(|v| v / so), n.map(|v| v / no)))
    }
}

/// Whether the prior's class for a pixel is one sky shows through:
/// tree, flower, or nothing it could name.
fn treeish(label: u8) -> bool {
    label == TREE_CLASS || label == FLOWER_CLASS || label == UNLABELED
}

/// A pixel's share of sky from its place on the line (`a`, unclamped)
/// and its distance off it (`off`), both in the line's lengths. Near
/// the line the place is the share; far off it (a leaf, a white wall
/// against a blue sky) the pixel is no mix of the two and takes none;
/// far past the sky's end (a street lamp) none either.
fn share(a: f32, off: f32) -> f32 {
    a.clamp(0.0, 1.0)
        * (1.0 - ramp(off, OFF_LINE.0, OFF_LINE.1))
        * (1.0 - ramp(a, BEYOND.0, BEYOND.1))
}

/// The hue gate on a share: a pixel as bright as the sky (`a` near 1
/// or over) must be of the sky's hue; a mix further down the line is
/// let off, since its hue is partly what is in front of the sky.
fn hue(p: [f32; 3], sky: [f32; 3], a: f32) -> f32 {
    1.0 - ramp(a, 0.7, 1.0) * ramp(chroma_distance(p, sky), HUE.0, HUE.1)
}

/// The distance between two colors' chromaticities: each channel over
/// the three's sum, the differences summed.
fn chroma_distance(p: [f32; 3], q: [f32; 3]) -> f32 {
    let sp = (p[0] + p[1] + p[2]).max(1e-6);
    let sq = (q[0] + q[1] + q[2]).max(1e-6);
    (0..3).map(|c| (p[c] / sp - q[c] / sq).abs()).sum()
}

/// A share held to what the prior says, for a pixel of a class sky
/// does not show through: no more than its sky probability allows
/// outside the outline (a snowy ridge on the line between a grey sky
/// and dark water is not sky, however bright), and no less than its
/// sky probability demands (a dark cloud SAM left out of the outline
/// is sky) where `floored` (the pixel is darker than the local sky and
/// not near a negative pick). Outside the outline near a person's
/// negative pick (`negated`), none at all.
fn bounded(a: f32, p: f32, inside: bool, floored: bool, negated: bool) -> f32 {
    let floor = if floored {
        ramp(p, FLOOR.0, FLOOR.1)
    } else {
        0.0
    };
    let cap = if inside {
        1.0
    } else if negated {
        0.0
    } else {
        ramp(p, CAP.0, CAP.1)
    };
    a.max(floor).min(cap.max(floor))
}

fn ramp(x: f32, a: f32, b: f32) -> f32 {
    ((x - a) / (b - a)).clamp(0.0, 1.0)
}

/// `plane` (`w` × `h`) at (u, v) in fractions, bilinearly.
fn sample(plane: &[f32], w: usize, h: usize, u: f32, v: f32) -> f32 {
    let fx = (u * w as f32 - 0.5).max(0.0);
    let fy = (v * h as f32 - 0.5).max(0.0);
    let x0 = (fx as usize).min(w - 1);
    let y0 = (fy as usize).min(h - 1);
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let top = plane[y0 * w + x0] + (plane[y0 * w + x1] - plane[y0 * w + x0]) * tx;
    let bottom = plane[y1 * w + x0] + (plane[y1 * w + x1] - plane[y1 * w + x0]) * tx;
    top + (bottom - top) * ty
}

/// A pixel's place on the line from the not-sky color to the sky
/// color: the projection (unclamped) and how far off the line it is,
/// both in units of the line's length; `None` where the two colors
/// are too close to tell apart (`floor` is the least squared length).
fn project(p: [f32; 3], sky: [f32; 3], not: [f32; 3], floor: f32) -> Option<(f32, f32)> {
    let d = [sky[0] - not[0], sky[1] - not[1], sky[2] - not[2]];
    let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if dd < floor {
        return None;
    }
    let q = [p[0] - not[0], p[1] - not[1], p[2] - not[2]];
    let a = (q[0] * d[0] + q[1] * d[1] + q[2] * d[2]) / dd;
    let off = [q[0] - a * d[0], q[1] - a * d[1], q[2] - a * d[2]];
    let dist = ((off[0] * off[0] + off[1] * off[1] + off[2] * off[2]) / dd).sqrt();
    Some((a, dist))
}

fn luminance(p: [f32; 3]) -> f32 {
    // Rec.2020's weights: the working space's.
    0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2]
}

/// Whether the matte holds the sure sky up near the sky's end of the
/// line (the module's header says why).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// The Sky's rule: the labeled sky inside the outline and its
    /// unlabeled pockets, where the map over the kept queries is sure.
    Sure,
    /// No such floor: the landscape's groups, whose soft map is over
    /// the kept queries already and whose edges were measured without
    /// it.
    None,
}

/// The matte of the sky, `out_w` × `out_h`, from the outline (one or
/// nothing at the prior's size), the prior, and the frame's linear
/// picture at its own size, holding the sure sky up as `hold` says.
/// `None` when the frame is empty or not the prior's shape, for the
/// caller to fall back on.
pub fn color_line(
    outline: &Mask,
    prior: &Prior,
    linear: &WorkingImage,
    negatives: &[[f32; 2]],
    hold: Hold,
    out_w: usize,
    out_h: usize,
) -> Option<Mask> {
    let (w, h) = (prior.width, prior.height);
    let (fw, fh) = (linear.width, linear.height);
    if fw == 0 || fh == 0 || linear.data.len() != fw * fh * 3 {
        return None;
    }
    // The same picture: the aspects agree to a pixel of the preview.
    if ((fw as f32 / fh as f32) - (w as f32 / h as f32)).abs() > 2.0 / h.min(w) as f32 {
        return None;
    }
    let mut map = trimap(outline, prior);
    let lin_p = area_rgb(&linear.data, fw, fh, w, h);
    let long = w.max(h);
    let (cw, ch) = if long > COLOR_SIDE {
        (
            ((w * COLOR_SIDE) as f32 / long as f32).round().max(1.0) as usize,
            ((h * COLOR_SIDE) as f32 / long as f32).round().max(1.0) as usize,
        )
    } else {
        (w, h)
    };
    let lin_c = area_rgb(&lin_p.data, w, h, cw, ch);
    let as_plane = |map: &[Known], k: Known| -> Vec<f32> {
        let p: Vec<f32> = map
            .iter()
            .map(|&m| if m == k { 1.0 } else { 0.0 })
            .collect();
        area_plane(&p, w, h, cw, ch)
    };
    // The sky's own brightness sets the scale of what "too close to
    // tell apart" and the guide mean.
    let (mut ysum, mut yn) = (0.0f64, 0usize);
    for (i, m) in map.iter().enumerate() {
        if *m == Known::Sky {
            ysum += luminance([
                lin_p.data[i * 3],
                lin_p.data[i * 3 + 1],
                lin_p.data[i * 3 + 2],
            ]) as f64;
            yn += 1;
        }
    }
    let y_sky = if yn > 0 {
        (ysum / yn as f64) as f32
    } else {
        0.0
    };
    if y_sky <= 1e-6 {
        return None;
    }
    let floor = 1e-4 * y_sky * y_sky;
    // What the growth asks of the prior: the map over the kept queries
    // within the labeled sky, as the gate reads it, since over all the
    // queries a sky the model is sure of can sit at a quarter; the map
    // over all of them elsewhere, where a low value is the prior's
    // doubt.
    let grow_p = |i: usize| {
        if prior.labels[i] == SKY_CLASS {
            prior.sky_kept[i]
        } else {
            prior.sky[i]
        }
    };
    // The known sky grows toward each gap: clearly-sky pixels join it
    // after each pass but the last, whose colors the full-size matte
    // uses.
    let mut colors = Colors::of(
        &lin_c,
        &as_plane(&map, Known::Sky),
        &as_plane(&map, Known::NotSky),
    );
    for _ in 1..PASSES {
        let joined: Vec<usize> = (0..w * h)
            .into_par_iter()
            .filter(|&i| {
                if map[i] != Known::Unknown {
                    return false;
                }
                let (x, y) = (i % w, i / w);
                let Some((sky, not)) = colors.at((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32)
                else {
                    return false;
                };
                let p = [lin_p.data[i * 3], lin_p.data[i * 3 + 1], lin_p.data[i * 3 + 2]];
                (treeish(prior.labels[i]) || grow_p(i) >= CAP.1)
                    && chroma_distance(p, sky) < HUE.0
                    && matches!(project(p, sky, not, floor), Some((a, off)) if (CLEARLY..=BEYOND.0).contains(&a) && off < OFF_LINE.0)
            })
            .collect();
        if joined.is_empty() {
            break;
        }
        for i in joined {
            map[i] = Known::Sky;
        }
        colors = Colors::of(
            &lin_c,
            &as_plane(&map, Known::Sky),
            &as_plane(&map, Known::NotSky),
        );
    }
    let m: Vec<f32> = outline.data.clone();
    let negated_map = negated(&m, prior, negatives);
    let near_negative = |u: f32, v: f32| {
        negated_map[((v * h as f32) as usize).min(h - 1) * w + ((u * w as f32) as usize).min(w - 1)]
    };
    // The sure sky the kept map holds up near the sky's end of the
    // line: the labeled sky inside the outline, and its pockets.
    let sure: Vec<bool> = match hold {
        Hold::Sure => {
            let mut sure: Vec<bool> = prior
                .labels
                .par_iter()
                .zip(&m)
                .map(|(&l, &o)| l == SKY_CLASS && o > 0.5)
                .collect();
            mark_pockets(&m, prior, &mut sure);
            sure
        }
        Hold::None => vec![false; w * h],
    };

    // Full size, a block of the output at a time.
    let (ow, oh) = (out_w.max(1), out_h.max(1));
    let small = fw * fh <= ow * oh;
    let alpha_at = |x: usize, y: usize| -> f32 {
        let (px, py) = ((x * w / fw).min(w - 1), (y * h / fh).min(h - 1));
        let i = py * w + px;
        match map[i] {
            Known::Sky => 1.0,
            Known::NotSky => 0.0,
            Known::Unknown => {
                let o = (y * fw + x) * 3;
                let p = [linear.data[o], linear.data[o + 1], linear.data[o + 2]];
                let (u, v) = ((x as f32 + 0.5) / fw as f32, (y as f32 + 0.5) / fh as f32);
                let local = colors.at(u, v);
                let projected =
                    local.and_then(|(sky, not)| project(p, sky, not, floor).map(|t| (t, sky)));
                let a = projected.map_or(m[i], |((a, off), sky)| share(a, off) * hue(p, sky, a));
                // Both floors are for a pixel darker than the local sky
                // and nowhere near a person's word that it is not sky.
                let dark = || local.is_some_and(|(sky, _)| luminance(p) < luminance(sky));
                let (a, dark, negated) = if treeish(prior.labels[i]) {
                    // Sky shows through: the projection's alone, but for
                    // a pocket of the sure sky below.
                    if !sure[i] {
                        return a;
                    }
                    (a, dark(), near_negative(u, v))
                } else {
                    let (dark, negated) = (dark(), near_negative(u, v));
                    // The floor is for a dark cloud the outline missed.
                    let a = bounded(
                        a,
                        sample(&prior.sky, w, h, u, v),
                        m[i] > 0.5,
                        dark && !negated,
                        negated,
                    );
                    (a, dark, negated)
                };
                match projected {
                    Some(((t, _), sky)) if sure[i] && dark && !negated => {
                        // The place on the line from the frame's own
                        // pixel; whether it is the sky's color at all
                        // from the preview's, whose noise is averaged
                        // out: near the line, and of the sky's hue at
                        // any place on it.
                        let q = [
                            lin_p.data[i * 3],
                            lin_p.data[i * 3 + 1],
                            lin_p.data[i * 3 + 2],
                        ];
                        let off = local
                            .and_then(|(sky, not)| project(q, sky, not, floor))
                            .map_or(1.0, |(_, off)| off);
                        a.max(
                            ramp(sample(&prior.sky_kept, w, h, u, v), FLOOR.0, FLOOR.1)
                                * ramp(t, LINE_FLOOR.0, LINE_FLOOR.1)
                                * hue(q, sky, 1.0)
                                * (1.0 - ramp(off, OFF_LINE.0, OFF_LINE.1)),
                        )
                    }
                    _ => a,
                }
            }
        }
    };
    let guide_at = |x: usize, y: usize| -> f32 {
        let o = (y * fw + x) * 3;
        let yv = luminance([linear.data[o], linear.data[o + 1], linear.data[o + 2]]);
        (yv.max(0.0) / y_sky).powf(1.0 / 2.2)
    };
    let unknown_near = |x0: usize, y0: usize, x1: usize, y1: usize| -> bool {
        let (px0, py0) = (x0 * w / fw, y0 * h / fh);
        let (px1, py1) = (
            ((x1 * w).div_ceil(fw)).min(w),
            ((y1 * h).div_ceil(fh)).min(h),
        );
        (py0..py1).any(|py| (px0..px1).any(|px| map[py * w + px] == Known::Unknown))
    };
    // The matte over a region of the frame, filtered, with the filter's
    // margin taken from beyond it where there is any.
    let region = |x0: usize, y0: usize, x1: usize, y1: usize| -> Vec<f32> {
        let m = 2 * RADIUS + 2;
        let (ex0, ey0) = (x0.saturating_sub(m), y0.saturating_sub(m));
        let (ex1, ey1) = ((x1 + m).min(fw), (y1 + m).min(fh));
        let (rw, rh) = (ex1 - ex0, ey1 - ey0);
        let mut a = vec![0.0f32; rw * rh];
        let mut g = vec![0.0f32; rw * rh];
        a.par_chunks_mut(rw)
            .zip(g.par_chunks_mut(rw))
            .enumerate()
            .for_each(|(ry, (ar, gr))| {
                for rx in 0..rw {
                    ar[rx] = alpha_at(ex0 + rx, ey0 + ry);
                    gr[rx] = guide_at(ex0 + rx, ey0 + ry);
                }
            });
        let f = guided::filter(&g, Some(&a), rw, rh, RADIUS, EPS);
        let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0));
        for y in y0..y1 {
            let row = (y - ey0) * rw;
            out.extend(
                f[row + x0 - ex0..row + x1 - ex0]
                    .iter()
                    .map(|v| v.clamp(0.0, 1.0)),
            );
        }
        out
    };
    if small {
        let full = if unknown_near(0, 0, fw, fh) {
            region(0, 0, fw, fh)
        } else {
            (0..fw * fh).map(|i| alpha_at(i % fw, i / fw)).collect()
        };
        return Some(Mask::new(fw, fh, full).resampled(ow, oh));
    }
    log::debug!(
        "sky matte at {ow}x{oh} from {fw}x{fh}: {:?}",
        footprint((w, h), (fw, fh), (ow, oh), rayon::current_num_threads())
    );
    // A block of the output, from the frame's pixels under it.
    let block = |bx: usize, by: usize| -> Vec<f32> {
        let (ox0, oy0) = (bx * BLOCK, by * BLOCK);
        let (ox1, oy1) = ((ox0 + BLOCK).min(ow), (oy0 + BLOCK).min(oh));
        let (x0, y0) = (span(ox0, ow, fw).0, span(oy0, oh, fh).0);
        let (x1, y1) = (span(ox1 - 1, ow, fw).1, span(oy1 - 1, oh, fh).1);
        let rw = x1 - x0;
        let full = if unknown_near(
            x0.saturating_sub(2 * RADIUS + 2),
            y0.saturating_sub(2 * RADIUS + 2),
            (x1 + 2 * RADIUS + 2).min(fw),
            (y1 + 2 * RADIUS + 2).min(fh),
        ) {
            region(x0, y0, x1, y1)
        } else {
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .map(|(x, y)| alpha_at(x, y))
                .collect()
        };
        let mut out = Vec::with_capacity((ox1 - ox0) * (oy1 - oy0));
        for oy in oy0..oy1 {
            let (ya, yb) = span(oy, oh, fh);
            for ox in ox0..ox1 {
                let (xa, xb) = span(ox, ow, fw);
                let mut acc = 0.0f32;
                for y in ya..yb {
                    let row = (y - y0) * rw;
                    acc += full[row + xa - x0..row + xb - x0].iter().sum::<f32>();
                }
                out.push(acc / ((yb - ya) * (xb - xa)) as f32);
            }
        }
        out
    };
    // A row of blocks at a time, written into the output where it
    // goes: the output is held once, and beside it only the rows of
    // blocks under way.
    let mut data = vec![0.0f32; ow * oh];
    data.par_chunks_mut(BLOCK * ow)
        .enumerate()
        .for_each(|(by, rows)| {
            let done: Vec<Vec<f32>> = (0..ow.div_ceil(BLOCK))
                .into_par_iter()
                .map(|bx| block(bx, by))
                .collect();
            for (bx, b) in done.iter().enumerate() {
                let ox0 = bx * BLOCK;
                let bw = (ox0 + BLOCK).min(ow) - ox0;
                for (k, row) in b.chunks(bw).enumerate() {
                    rows[k * ow + ox0..k * ow + ox0 + bw].copy_from_slice(row);
                }
            }
        });
    Some(Mask::new(ow, oh, data))
}

/// What [`color_line`] is expected to hold at once, in bytes: for a
/// prior (and outline) of `preview` (width, height), a frame of
/// `frame` and an output of `out`, with `threads` at work, from the
/// buffers' sizes rather than a measure of the allocator. The blocks
/// under way are counted as one a thread, which is rayon's usual
/// schedule and not a hard bound: the rows, the blocks in a row, a
/// region's fill and the guided filter all run in parallel within one
/// another, and a thread waiting in an inner join may steal another
/// block and start its region. The preview's part and the output are
/// held once whatever the schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footprint {
    /// At the preview, whatever the output: the trimap, the preview's
    /// linear picture, the outline, the sure sky, the negated picks,
    /// the growth's joined pixels and plane and the pockets' fill, and
    /// the local colors at `COLOR_SIDE`. Linear in the preview's
    /// pixels, so the growth and the pockets cost the same at any
    /// raster width.
    pub preview: usize,
    /// The output, once, and the rows of blocks under way beside it.
    pub output: usize,
    /// The blocks under way, one a thread: each its region of the
    /// frame and its filter's margin, the guided filter's planes over
    /// it.
    pub blocks: usize,
}

impl Footprint {
    pub fn total(&self) -> usize {
        self.preview + self.output + self.blocks
    }
}

/// Bytes a preview pixel holds at most: the trimap (1), the linear
/// picture (12), the outline (4), the sure sky and the negated picks
/// (1 each), and the larger of the growth's joined pixels and plane
/// (8 + 4) and the pockets' seen, stack and region (1 + 8 + 8).
const PER_PREVIEW_PIXEL: usize = 1 + 12 + 4 + 1 + 1 + 17;
/// Floats a pixel of the local colors' planes holds at most.
const COLOR_FLOATS: usize = 40;
/// The planes over a block's region at once: the alpha and the guide,
/// and the guided filter's eleven and its box mean's two.
const PLANES_A_REGION: usize = 2 + 11 + 2;

/// See [`Footprint`].
pub fn footprint(
    preview: (usize, usize),
    frame: (usize, usize),
    out: (usize, usize),
    threads: usize,
) -> Footprint {
    let f = std::mem::size_of::<f32>();
    let (pw, ph) = preview;
    let long = pw.max(ph).max(1);
    let colors = ((pw * COLOR_SIDE).div_ceil(long)) * ((ph * COLOR_SIDE).div_ceil(long));
    // The color planes at `COLOR_SIDE`: the linear picture and the
    // two known planes, and for each side its blurs near and far, its
    // colors and the blur's temporaries.
    let preview = pw * ph * PER_PREVIEW_PIXEL + colors * COLOR_FLOATS * f;
    let (ow, oh) = (out.0.max(1), out.1.max(1));
    let threads = threads.max(1);
    let rows = threads.min(oh.div_ceil(BLOCK));
    let output = ow * oh * f + rows * BLOCK * ow * f;
    // A block's region: its share of the frame, a pixel more for the
    // spans' rounding, and the filter's margin either side.
    let margin = 2 * (2 * RADIUS + 2);
    let side = |o: usize, f: usize| (BLOCK * f).div_ceil(o).min(f) + 1 + margin;
    let region = side(ow, frame.0) * side(oh, frame.1);
    let blocks = threads * region * PLANES_A_REGION * f;
    Footprint {
        preview,
        output,
        blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: u8 = TREE_CLASS;
    const PERSON: u8 = 0;

    /// A frame `s` times the prior's `w` × `h`: a gradient sky, bluer
    /// and darker up, over a dark ground from `ground` of the height;
    /// thin dark vertical lines (a canopy) from `canopy` to the ground
    /// every 24 full-size pixels, 4 wide; and a yellow thing, the color
    /// of nothing in the sky, at `thing` if given.
    struct Scene {
        prior: Prior,
        outline: Mask,
        linear: WorkingImage,
    }

    fn scene(w: usize, h: usize, s: usize, thing: Option<(usize, usize, usize)>) -> Scene {
        let (fw, fh) = (w * s, h * s);
        let (canopy, ground) = (0.35, 0.7);
        let mut linear = WorkingImage::new(fw, fh);
        for y in 0..fh {
            for x in 0..fw {
                let v = y as f32 / fh as f32;
                let mut p = if v < ground {
                    [0.3 + 0.25 * v, 0.4 + 0.3 * v, 0.65 + 0.35 * v]
                } else {
                    [0.03, 0.025, 0.02]
                };
                if (canopy..ground).contains(&v) && x % 24 < 4 {
                    p = [0.03, 0.025, 0.02];
                }
                if let Some((tx, ty, r)) = thing {
                    let (dx, dy) = (
                        x as f32 / s as f32 - tx as f32,
                        y as f32 / s as f32 - ty as f32,
                    );
                    if dx.abs() < r as f32 && dy.abs() < r as f32 {
                        p = [0.5, 0.45, 0.05];
                    }
                }
                let i = (y * fw + x) * 3;
                linear.data[i..i + 3].copy_from_slice(&p);
            }
        }
        // The prior: sky above the canopy, tree in it, ground below;
        // the outline SAM gave, the open sky alone.
        let mut labels = vec![0u8; w * h];
        let mut sky = vec![0.0f32; w * h];
        let mut outline = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let v = y as f32 / h as f32;
                let i = y * w + x;
                (labels[i], sky[i]) = if v < canopy {
                    (SKY_CLASS, 0.97)
                } else if v < ground {
                    (TREE, 0.2)
                } else {
                    (130, 0.01)
                };
                if v < canopy {
                    outline[i] = 1.0;
                }
                if let Some((tx, ty, r)) = thing
                    && (x as isize - tx as isize).unsigned_abs() < r
                    && (y as isize - ty as isize).unsigned_abs() < r
                {
                    labels[i] = PERSON;
                    sky[i] = 0.02;
                }
            }
        }
        Scene {
            prior: Prior {
                width: w,
                height: h,
                sky_kept: sky.clone(),
                sky,
                labels,
                sure: 0.999,
            },
            outline: Mask::new(w, h, outline),
            linear,
        }
    }

    /// Thin dark lines over a gradient sky: the sky between them is
    /// found though the outline stopped above them, the lines stay
    /// dark, the ground stays out.
    #[test]
    fn sky_between_thin_lines_is_found_and_the_lines_are_not() {
        let (w, h, s) = (200usize, 150usize, 4usize);
        let sc = scene(w, h, s, None);
        let map = trimap(&sc.outline, &sc.prior);
        // In question: the canopy near the sky, and the band.
        assert_eq!(map[(0.5 * h as f32) as usize * w + 100], Known::Unknown);
        assert_eq!(map[10 * w + 100], Known::Sky);
        assert_eq!(map[(h - 5) * w + 100], Known::NotSky);
        // At full size, so each line and gap can be read.
        let (fw, fh) = (w * s, h * s);
        let matte = color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::Sure, fw, fh)
            .expect("a matte");
        let row = (0.55 * fh as f32) as usize;
        let (mut gaps, mut lines) = (Vec::new(), Vec::new());
        for x in 20..fw - 20 {
            match x % 24 {
                12 => gaps.push(matte.at(x, row)),
                1 => lines.push(matte.at(x, row)),
                _ => {}
            }
        }
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!(mean(&gaps) > 0.8, "gaps {}", mean(&gaps));
        assert!(
            mean(&lines) < mean(&gaps) - 0.3,
            "lines {} gaps {}",
            mean(&lines),
            mean(&gaps)
        );
        // The open sky full, the ground empty.
        assert!(matte.at(fw / 2, fh / 10) > 0.98);
        assert!(matte.at(fw / 2, fh - 10) < 0.02);
    }

    /// A thing in the band the prior calls a person takes no alpha,
    /// though its color is nearer the sky's than the ground's.
    #[test]
    fn a_thing_takes_no_alpha() {
        let (w, h, s) = (200usize, 150usize, 2usize);
        let sc = scene(w, h, s, Some((100, 60, 12)));
        let matte = color_line(
            &sc.outline,
            &sc.prior,
            &sc.linear,
            &[],
            Hold::Sure,
            w * s,
            h * s,
        )
        .unwrap();
        assert!(
            matte.at(100 * s, 60 * s) < 0.02,
            "{}",
            matte.at(100 * s, 60 * s)
        );
    }

    /// Made at the raster's size from a frame four times larger, the
    /// matte is the full-size one averaged: the same means over rows.
    #[test]
    fn the_matte_at_a_smaller_size_is_the_full_one_averaged() {
        let (w, h, s) = (120usize, 90usize, 4usize);
        let sc = scene(w, h, s, None);
        let full = color_line(
            &sc.outline,
            &sc.prior,
            &sc.linear,
            &[],
            Hold::Sure,
            w * s,
            h * s,
        )
        .unwrap();
        let small = color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::Sure, w, h).unwrap();
        assert_eq!((small.width, small.height), (w, h));
        for y in [5usize, 40, 45, 80] {
            let a: f32 = (0..w).map(|x| small.at(x, y)).sum::<f32>() / w as f32;
            let b: f32 = (y * s..(y + 1) * s)
                .flat_map(|yy| (0..w * s).map(move |x| (x, yy)))
                .map(|(x, yy)| full.at(x, yy))
                .sum::<f32>()
                / (w * s * s) as f32;
            assert!((a - b).abs() < 1e-3, "row {y}: {a} against {b}");
        }
    }

    /// A matte twice as wide, a Sky's wide raster, brought down two by
    /// two is the narrow one: within two levels where the frame's
    /// pixels split evenly (four a narrow texel, two a wide one; the
    /// blocks cover other parts of the frame, so a block's filter may
    /// run where the other's is skipped), and within 0.15 at a thin
    /// line, a hundredth on average, where a narrow texel's five
    /// pixels split three and two. What the wider one adds is
    /// resolution, not another matte.
    #[test]
    fn a_matte_twice_as_wide_averages_to_the_narrow_one() {
        let (w, h) = (120usize, 90usize);
        for (s, most, mean_most) in [(4usize, 2.0 / 255.0, 1e-4f32), (5, 0.15, 0.01)] {
            let sc = scene(w, h, s, None);
            let matte =
                |ow, oh| color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::Sure, ow, oh);
            let narrow = matte(w, h).unwrap();
            let wide = matte(2 * w, 2 * h).unwrap();
            assert_eq!((wide.width, wide.height), (2 * w, 2 * h));
            let (mut worst, mut sum) = (0.0f32, 0.0f32);
            for y in 0..h {
                for x in 0..w {
                    let down = (wide.at(2 * x, 2 * y)
                        + wide.at(2 * x + 1, 2 * y)
                        + wide.at(2 * x, 2 * y + 1)
                        + wide.at(2 * x + 1, 2 * y + 1))
                        / 4.0;
                    let d = (down - narrow.at(x, y)).abs();
                    worst = worst.max(d);
                    sum += d;
                }
            }
            let mean = sum / (w * h) as f32;
            eprintln!("{s} pixels a texel: worst {worst}, mean {mean}");
            assert!(worst <= most, "{s} a texel: worst {worst}");
            assert!(mean <= mean_most, "{s} a texel: mean {mean}");
        }
    }

    /// The matte's working set at a 100 MP frame's wide raster: the
    /// preview's part is the same at either width and linear in the
    /// preview's pixels, so the growth and the pockets do not follow
    /// the raster; the output is held once; and a wider output has
    /// smaller blocks of the frame under way, not larger.
    #[test]
    fn the_matte_s_expected_working_set() {
        let (preview, frame) = ((1536, 2048), (8736, 11648));
        let narrow = footprint(preview, frame, (2048, 2731), 4);
        let wide = footprint(preview, frame, (4096, 5461), 4);
        eprintln!(
            "narrow {narrow:?} {}, wide {wide:?} {}",
            narrow.total(),
            wide.total()
        );
        assert_eq!(narrow.preview, wide.preview);
        let twice = footprint((2 * 1536, 2048), frame, (4096, 5461), 4);
        assert!(twice.preview < 2 * wide.preview && twice.preview > wide.preview * 3 / 2);
        // The output: 4096 by 5461 floats, and four rows of blocks.
        assert_eq!(wide.output, 4096 * 5461 * 4 + 4 * 128 * 4096 * 4);
        assert!(wide.blocks < narrow.blocks);
        // As said: under 280 MB on four threads, and the most a
        // thread adds a block's region, under 6 MB.
        assert!(wide.total() < 280_000_000, "{}", wide.total());
        let one = footprint(preview, frame, (4096, 5461), 1);
        assert!(wide.blocks / 4 == one.blocks && one.blocks < 6_000_000);
        // The rule's largest wide raster, 4096 by 8192, a frame twice
        // as tall as wide: still a fixed share past its output.
        let tallest = footprint((1024, 2048), (6200, 12400), (4096, 8192), 4);
        assert!(tallest.total() < 300_000_000, "{}", tallest.total());
    }

    /// No known sky, or an empty frame: nothing to go on, and the
    /// caller falls back; the matte never makes sky from nothing.
    #[test]
    fn with_no_known_sky_there_is_no_matte() {
        let (w, h, s) = (100usize, 80usize, 2usize);
        let mut sc = scene(w, h, s, None);
        sc.outline = Mask::new(w, h, vec![0.0; w * h]);
        assert!(color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::Sure, w, h).is_none());
        let sc = scene(w, h, s, None);
        assert!(
            color_line(
                &sc.outline,
                &sc.prior,
                &WorkingImage::new(0, 0),
                &[],
                Hold::Sure,
                w,
                h
            )
            .is_none()
        );
        // A frame of another shape.
        assert!(
            color_line(
                &sc.outline,
                &sc.prior,
                &WorkingImage::new(300, 50),
                &[],
                Hold::Sure,
                w,
                h
            )
            .is_none()
        );
    }

    /// Off the line or far past the sky, no share; a class sky does not
    /// show through is held to its sky probability.
    #[test]
    fn a_share_is_held_off_the_line_and_by_the_prior() {
        assert_eq!(share(0.6, 0.0), 0.6);
        assert_eq!(share(0.6, 0.6), 0.0);
        assert_eq!(share(3.0, 0.0), 0.0);
        assert_eq!(share(1.2, 0.1), 1.0);
        // A white sign against a pale blue overcast: as bright, not
        // of its hue.
        let sky = [0.55, 0.6, 0.7];
        assert!(hue([0.9, 0.9, 0.9], sky, 1.2) < 0.2);
        assert_eq!(hue([0.56, 0.61, 0.7], sky, 1.0), 1.0);
        // A mix half down the line keeps its share whatever its hue.
        assert_eq!(hue([0.3, 0.3, 0.2], sky, 0.5), 1.0);
        // A snowy ridge outside the outline, the prior sure it is not sky.
        assert_eq!(bounded(0.9, 0.02, false, true, false), 0.0);
        // The same inside the outline keeps what the line says.
        assert_eq!(bounded(0.9, 0.02, true, true, false), 0.9);
        // A dark cloud SAM left out, the prior sure it is sky.
        assert_eq!(bounded(0.3, 0.97, false, true, false), 1.0);
        // Not floored (bright, or by a negative pick): the line's word.
        assert_eq!(bounded(0.3, 0.97, false, false, false), 0.3);
        // Outside the outline by a person's negative pick: none.
        assert_eq!(bounded(0.6, 0.97, false, false, true), 0.0);
    }

    /// A bright neutral ridge beside the sky, which the prior calls a
    /// mountain, stays out though it lies on the line; a street lamp in
    /// the canopy stays out though it is brighter than the sky.
    #[test]
    fn a_bright_ridge_and_a_lamp_take_no_sky() {
        let (w, h, s) = (200usize, 150usize, 2usize);
        let mut sc = scene(w, h, s, None);
        let (fw, fh) = (w * s, h * s);
        // The ridge: a band under the sky, grey-white, labeled
        // mountain (124) with no sky probability.
        let canopy = (0.35 * h as f32) as usize;
        for y in canopy..canopy + 16 {
            for x in 0..w {
                sc.prior.labels[y * w + x] = 124;
                sc.prior.sky[y * w + x] = 0.01;
            }
        }
        for y in canopy * s..(canopy + 16) * s {
            for x in 0..fw {
                sc.linear.data[(y * fw + x) * 3..][..3].copy_from_slice(&[0.45, 0.5, 0.62]);
            }
        }
        // The lamp: a small very bright spot in a dark patch of the
        // canopy, as a street lamp among trees at dusk.
        let (lx, ly) = (fw / 3, (0.55 * fh as f32) as usize);
        for y in ly - 20..ly + 20 {
            for x in lx - 20..lx + 20 {
                let lamp = x.abs_diff(lx) < 3 && y.abs_diff(ly) < 3;
                let p = if lamp {
                    [8.0, 7.0, 5.0]
                } else {
                    [0.03, 0.025, 0.02]
                };
                sc.linear.data[(y * fw + x) * 3..][..3].copy_from_slice(&p);
            }
        }
        let matte =
            color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::Sure, fw, fh).unwrap();
        assert!(
            matte.at(fw / 2, (canopy + 8) * s) < 0.1,
            "{}",
            matte.at(fw / 2, (canopy + 8) * s)
        );
        assert!(matte.at(lx, ly) < 0.1, "{}", matte.at(lx, ly));
    }

    #[test]
    fn a_pixel_on_the_line_projects_to_its_share() {
        let (sky, not) = ([0.4, 0.5, 0.8], [0.05, 0.05, 0.02]);
        let mix = |t: f32| [0, 1, 2].map(|c| not[c] + t * (sky[c] - not[c]));
        let (a, off) = project(mix(0.3), sky, not, 1e-6).unwrap();
        assert!((a - 0.3).abs() < 1e-5 && off < 1e-5);
        let (_, off) = project([0.9, 0.9, 0.05], sky, not, 1e-6).unwrap();
        assert!(off > 0.3);
        assert!(project(sky, sky, sky, 1e-6).is_none());
    }

    /// A pale overcast over a dark ground, as a sea of cloud under a
    /// peak: the labels sky above `GROUND` of the height and ground
    /// below, the outline the labels themselves (no seed for SAM), and
    /// the soft map over all the queries diluted to a quarter over the
    /// sky while the map over the kept ones is sure of it. A square of
    /// the prior's pixels, `side` wide about (cx, cy), is cloud a fifth
    /// of the way down the line toward the ground, and the labels leave
    /// it unlabeled, as they leave pockets at the logits' grid.
    const GROUND: f32 = 0.6;
    const OVERCAST: [f32; 3] = [0.12, 0.135, 0.17];
    const EARTH: [f32; 3] = [0.03, 0.035, 0.045];

    fn mix(t: f32) -> [f32; 3] {
        [0, 1, 2].map(|c| EARTH[c] + t * (OVERCAST[c] - EARTH[c]))
    }

    fn overcast(w: usize, h: usize, s: usize, square: (usize, usize, usize)) -> Scene {
        let (cx, cy, side) = square;
        let in_square = |x: usize, y: usize| x.abs_diff(cx) * 2 < side && y.abs_diff(cy) * 2 < side;
        let (fw, fh) = (w * s, h * s);
        let mut linear = WorkingImage::new(fw, fh);
        for y in 0..fh {
            for x in 0..fw {
                let p = if in_square(x / s, y / s) {
                    mix(0.8)
                } else if (y as f32) < GROUND * fh as f32 {
                    OVERCAST
                } else {
                    EARTH
                };
                linear.data[(y * fw + x) * 3..][..3].copy_from_slice(&p);
            }
        }
        let mut labels = vec![0u8; w * h];
        let mut sky = vec![0.0f32; w * h];
        let mut kept = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                (labels[i], sky[i], kept[i]) = if in_square(x, y) {
                    (UNLABELED, 0.24, 0.99)
                } else if (y as f32) < GROUND * h as f32 {
                    (SKY_CLASS, 0.25, 1.0)
                } else {
                    (126, 0.01, 0.02)
                };
            }
        }
        let outline = labels
            .iter()
            .map(|&l| if l == SKY_CLASS { 1.0 } else { 0.0 })
            .collect();
        Scene {
            prior: Prior {
                width: w,
                height: h,
                sky,
                sky_kept: kept,
                labels,
                sure: 0.997,
            },
            outline: Mask::new(w, h, outline),
            linear,
        }
    }

    /// The least of the matte over the square's middle, at full size.
    fn least(matte: &Mask, s: usize, square: (usize, usize, usize)) -> f32 {
        let (cx, cy, side) = square;
        let r = side / 4;
        let mut least = f32::INFINITY;
        for y in (cy - r) * s..(cy + r) * s {
            for x in (cx - r) * s..(cx + r) * s {
                least = least.min(matte.at(x, y));
            }
        }
        least
    }

    /// An unlabeled pocket of a sure sky, of the sky's own color a
    /// fifth of the way down the line, fills: before, the projection
    /// alone held it at about 0.8, a square hole in flat cloud, since
    /// the diluted map gave the sky around it no floor either.
    #[test]
    fn an_unlabeled_pocket_of_a_sure_sky_fills() {
        let (w, h, s) = (200usize, 150usize, 2usize);
        let square = (100, 45, 12);
        let sc = overcast(w, h, s, square);
        let (fw, fh) = (w * s, h * s);
        let pockets = pockets(&sc.outline.data, &sc.prior);
        assert!(pockets[45 * w + 100] && !pockets[10 * w + 10]);
        let held = color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::Sure, fw, fh).unwrap();
        assert!(
            least(&held, s, square) > 0.95,
            "{}",
            least(&held, s, square)
        );
        // Master's rule, for the record of what the hole was.
        let open = color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::None, fw, fh).unwrap();
        assert!(least(&open, s, square) < 0.9, "{}", least(&open, s, square));
        // The ground stays out either way.
        assert!(held.at(fw / 2, fh - 4) < 0.02);
    }

    /// The same square touching a tree is no pocket of the sky: the
    /// tree's region, which sky shows through, is the projection's,
    /// and so is the square with it.
    #[test]
    fn the_same_square_touching_a_tree_does_not_fill() {
        let (w, h, s) = (200usize, 150usize, 2usize);
        let square = (100, 45, 12);
        let mut sc = overcast(w, h, s, square);
        // A trunk from the ground up to the square's foot, labeled tree
        // and dark, a column four of the prior's pixels wide.
        let (fw, fh) = (w * s, h * s);
        let foot = 45 + 6;
        let ground = (GROUND * h as f32) as usize;
        for y in foot..ground {
            for x in 98..102 {
                sc.prior.labels[y * w + x] = TREE;
                sc.prior.sky[y * w + x] = 0.05;
                sc.prior.sky_kept[y * w + x] = 0.2;
                sc.outline.data[y * w + x] = 0.0;
            }
        }
        for y in foot * s..ground * s {
            for x in 98 * s..102 * s {
                sc.linear.data[(y * fw + x) * 3..][..3].copy_from_slice(&EARTH);
            }
        }
        assert!(!pockets(&sc.outline.data, &sc.prior)[45 * w + 100]);
        let matte =
            color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::Sure, fw, fh).unwrap();
        assert!(
            least(&matte, s, square) < 0.9,
            "{}",
            least(&matte, s, square)
        );
        // The trunk itself stays dark.
        assert!(matte.at(100 * s, (ground - 8) * s) < 0.2);
    }

    /// What is no pocket: a thing in the sky the sky's masks pass over
    /// (its soft map a fifth of the sky's), an unlabeled region whose
    /// sky is outside the outline (what SAM left out stays out), and a
    /// foreground the labels left unlabeled from the bottom of the
    /// frame to both its sides, larger than any pocket of the logits'
    /// grid.
    #[test]
    fn a_lantern_and_what_the_outline_left_out_are_no_pockets() {
        let (w, h, s) = (200usize, 150usize, 1usize);
        let square = (100, 45, 12);
        let mut lantern = overcast(w, h, s, square);
        for v in lantern.prior.sky.iter_mut().zip(&lantern.prior.labels) {
            if *v.1 == UNLABELED {
                *v.0 = 0.05;
            }
        }
        assert!(!pockets(&lantern.outline.data, &lantern.prior)[45 * w + 100]);
        let mut left_out = overcast(w, h, s, square);
        for y in 30..60 {
            for x in 85..115 {
                left_out.outline.data[y * w + x] = 0.0;
            }
        }
        assert!(!pockets(&left_out.outline.data, &left_out.prior)[45 * w + 100]);
        let mut foreground = overcast(w, h, s, square);
        for (l, v) in foreground
            .prior
            .labels
            .iter_mut()
            .zip(foreground.prior.sky.iter_mut())
        {
            if *l == 126 {
                (*l, *v) = (UNLABELED, 0.25);
            }
        }
        let found = pockets(&foreground.outline.data, &foreground.prior);
        assert!(!found[(h - 2) * w + 100]);
        assert!(found[45 * w + 100], "the cloud's pocket is still one");
    }

    /// A pocket of the sure sky of another color, a rose a little darker
    /// than the overcast that sits near the sky's end of the line in
    /// brightness, is no sky the floor holds up: it keeps the
    /// projection's share, as under master's rule, whatever its place.
    #[test]
    fn an_off_hue_pocket_stays_out() {
        let (w, h, s) = (200usize, 150usize, 2usize);
        let square = (100, 45, 12);
        let (fw, fh) = (w * s, h * s);
        for rose in [[0.16, 0.08, 0.11], [0.15, 0.06, 0.16]] {
            let mut sc = overcast(w, h, s, square);
            for y in (45 - 6) * s..(45 + 6) * s {
                for x in (100 - 6) * s..(100 + 6) * s {
                    sc.linear.data[(y * fw + x) * 3..][..3].copy_from_slice(&rose);
                }
            }
            assert!(pockets(&sc.outline.data, &sc.prior)[45 * w + 100]);
            let held =
                color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::Sure, fw, fh).unwrap();
            let open =
                color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::None, fw, fh).unwrap();
            let (x, y) = (100 * s, 45 * s);
            assert!(held.at(x, y) < 0.5, "{rose:?}: {}", held.at(x, y));
            assert!(
                (held.at(x, y) - open.at(x, y)).abs() < 0.02,
                "{rose:?}: {} against {}",
                held.at(x, y),
                open.at(x, y)
            );
        }
    }

    /// A dark pole the labels' coarse step calls sky, on a sure sky,
    /// lies near the not-sky end of the line and is not held up.
    #[test]
    fn a_dark_pole_the_labels_call_sky_is_not_held_up() {
        let (w, h, s) = (800usize, 600usize, 1usize);
        let mut sc = overcast(w, h, s, (400, 180, 0));
        let (fw, fh) = (w * s, h * s);
        let ground = (GROUND * h as f32) as usize;
        // The pole, twelve pixels wide, inside the labeled
        // sky's shrink band near the ground, where the matte decides.
        for y in (ground - 20) * s..ground * s {
            for x in 240..252 {
                sc.linear.data[(y * fw + x) * 3..][..3].copy_from_slice(&mix(0.1));
            }
        }
        let held = color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::Sure, fw, fh).unwrap();
        let open = color_line(&sc.outline, &sc.prior, &sc.linear, &[], Hold::None, fw, fh).unwrap();
        let (x, y) = (246, ground - 2);
        assert!(
            held.at(x, y) < 0.5,
            "{} against {}",
            held.at(x, y),
            open.at(x, y)
        );
        assert!(
            (held.at(x, y) - open.at(x, y)).abs() < 0.02,
            "{} against {}",
            held.at(x, y),
            open.at(x, y)
        );
        assert!(held.at(500, ground - 2) > 0.98);
    }
}
