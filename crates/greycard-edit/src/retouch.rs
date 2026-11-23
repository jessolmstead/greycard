//! Retouching: spots and strokes healed or cloned from elsewhere in
//! the picture, applied to the developed picture before its look.
//! Positions are in units of the picture's width, as the masks' are.

use serde::{Deserialize, Serialize};

use crate::mask::Pos;
use greycard_core::develop::retouch::{self, Region};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// The source's texture with the destination's color and light.
    #[default]
    Heal,
    /// The source as it is.
    Clone,
    /// Made up by a model from what is around it; no source.
    Fill,
}

impl Method {
    pub const ALL: [Method; 3] = [Method::Heal, Method::Clone, Method::Fill];

    pub fn name(self) -> &'static str {
        match self {
            Method::Heal => "Heal",
            Method::Clone => "Clone",
            Method::Fill => "Fill",
        }
    }

    /// Whether the method takes its pixels from elsewhere in the picture.
    pub fn sourced(self) -> bool {
        !matches!(self, Method::Fill)
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }

    /// The engine's method, for those that have one.
    pub fn engine(self) -> Option<retouch::Method> {
        match self {
            Method::Heal => Some(retouch::Method::Heal),
            Method::Clone => Some(retouch::Method::Clone),
            Method::Fill => None,
        }
    }
}

/// One repair: a spot (one point) or a stroke, `radius` wide, its
/// edge softened over the outer `feather` of the radius, taken from
/// `source` away, and blended at `opacity`. A Fill may be several
/// strokes painted as one area: the first is `points` and `radius`,
/// the rest are `more`, and the area is their union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Patch {
    pub id: u64,
    #[serde(deserialize_with = "crate::meta::newer_loose")]
    pub method: Method,
    pub points: Vec<Pos>,
    pub radius: f32,
    pub feather: f32,
    pub opacity: f32,
    /// The source's offset from the patch. None until the engine has
    /// chosen one.
    pub source: Option<Pos>,
    /// A Fill's variation: the generative remove makes another fill
    /// for each seed. Left out of the sidecar at 0.
    #[serde(skip_serializing_if = "is_zero")]
    pub seed: u32,
    /// A Fill made by the generative remove rather than LaMa, set when
    /// the patch is drawn where the remove can run. Where it cannot
    /// (no model, no GPU), the patch is left as it was rather than
    /// made by the other engine, which would be another picture.
    /// Left out of the sidecar when false.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub remove: bool,
    /// A remove's Fill made in four steps rather than 25: quicker,
    /// rougher on large areas. Only with `remove`; an older build
    /// reads it as the full remove and makes it so. Left out of the
    /// sidecar when false.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fast: bool,
    /// A Fill's strokes after its first, each with its own radius;
    /// the feather and the opacity are the repair's. Left out of the
    /// sidecar when empty, so a repair of one stroke writes what it
    /// wrote before version 9. Heal and Clone are never given any; one
    /// read with some is the union all the same.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub more: Vec<Stroke>,
}

/// A stroke of a repair after its first ([`Patch::more`]): its points
/// and its radius, in units of the picture's width.
/// A field missing from one read from a file takes a patch's default
/// (no points, the default radius) rather than failing the sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stroke {
    pub points: Vec<Pos>,
    pub radius: f32,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            radius: Patch::default().radius,
        }
    }
}

impl Stroke {
    /// The stroke carried round with a turned picture: its points and
    /// its radius, as a patch's are ([`Patch::turn`]).
    pub fn turn(&mut self, t: crate::mask::Turned) {
        if t.is_identity() {
            return;
        }
        for p in &mut self.points {
            *p = t.pos(*p);
        }
        self.radius *= t.scale();
    }

    /// Whether this stroke's cover and `other`'s overlap: some point is
    /// nearer each one's line than its radius, which is where
    /// [`Patch::region`] gives either a cover above nothing. Their
    /// rings meeting is not enough.
    pub fn touches(&self, other: &Stroke) -> bool {
        polyline_distance(&self.points, &other.points) < self.radius + other.radius
    }
}

/// The least distance between two polylines, a lone point being one of
/// no length; infinite when either has no points.
fn polyline_distance(a: &[Pos], b: &[Pos]) -> f32 {
    let seg = |pts: &[Pos], i: usize| (pts[i], *pts.get(i + 1).unwrap_or(&pts[i]));
    let mut best = f32::INFINITY;
    for i in 0..a.len() {
        for j in 0..b.len() {
            best = best.min(segment_distance(seg(a, i), seg(b, j)));
        }
    }
    best
}

/// The least distance between two segments.
fn segment_distance((a0, a1): (Pos, Pos), (b0, b1): (Pos, Pos)) -> f32 {
    let to = |p: Pos, s0: Pos, s1: Pos| {
        distance_to_polyline((p[0], p[1]), &[(s0[0], s0[1]), (s1[0], s1[1])])
    };
    // Two segments that cross are nothing apart.
    let side =
        |o: Pos, p: Pos, q: Pos| (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]);
    if side(a0, a1, b0) * side(a0, a1, b1) < 0.0 && side(b0, b1, a0) * side(b0, b1, a1) < 0.0 {
        return 0.0;
    }
    to(a0, b0, b1)
        .min(to(a1, b0, b1))
        .min(to(b0, a0, a1))
        .min(to(b1, a0, a1))
}

/// What makes a Fill: LaMa from the picture around it, or the
/// generative remove in four steps or in full ([`Patch::remove`],
/// [`Patch::fast`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FillChoice {
    Patch,
    Fast,
    Best,
}

impl FillChoice {
    pub const ALL: [FillChoice; 3] = [FillChoice::Patch, FillChoice::Fast, FillChoice::Best];

    pub fn name(self) -> &'static str {
        match self {
            FillChoice::Patch => "Patch",
            FillChoice::Fast => "Fast",
            FillChoice::Best => "Best",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Default for Patch {
    fn default() -> Self {
        Self {
            id: 0,
            method: Method::Heal,
            points: Vec::new(),
            radius: 0.02,
            feather: 0.3,
            opacity: 1.0,
            source: None,
            seed: 0,
            remove: false,
            fast: false,
            more: Vec::new(),
        }
    }
}

/// How far outside the patch its surroundings are looked at, in radii.
const RING: f32 = 1.7;

impl Patch {
    /// Its name on the panel and in the history: "Fill 3", and
    /// "Fill 3 (Fast)" for a four-step remove's.
    pub fn name(&self) -> String {
        if self.method == Method::Fill && self.fill_choice() == FillChoice::Fast {
            format!("{} {} (Fast)", self.method.name(), self.id)
        } else {
            format!("{} {}", self.method.name(), self.id)
        }
    }

    /// What makes it, if it is a Fill: `fast` counts only with `remove`.
    pub fn fill_choice(&self) -> FillChoice {
        match (self.remove, self.fast) {
            (false, _) => FillChoice::Patch,
            (true, true) => FillChoice::Fast,
            (true, false) => FillChoice::Best,
        }
    }

    /// Made by `choice` from now on.
    pub fn set_fill_choice(&mut self, choice: FillChoice) {
        self.remove = choice != FillChoice::Patch;
        self.fast = choice == FillChoice::Fast;
    }

    /// The patch carried round with a turned picture, so it heals
    /// the pixels it was drawn over: its points, its radius and the
    /// offset to its source, which is a vector and not a point.
    pub fn turn(&mut self, t: crate::mask::Turned) {
        if t.is_identity() {
            return;
        }
        for p in &mut self.points {
            *p = t.pos(*p);
        }
        self.radius *= t.scale();
        for s in &mut self.more {
            s.turn(t);
        }
        self.source = self.source.map(|d| t.offset(d));
    }

    /// Its strokes' points and radii: the first, then [`Self::more`].
    pub fn strokes(&self) -> impl Iterator<Item = (&[Pos], f32)> {
        std::iter::once((self.points.as_slice(), self.radius))
            .chain(self.more.iter().map(|s| (s.points.as_slice(), s.radius)))
    }

    /// Its strokes, owned: the first, then [`Self::more`].
    pub fn stroke_list(&self) -> Vec<Stroke> {
        self.strokes()
            .map(|(points, radius)| Stroke {
                points: points.to_vec(),
                radius,
            })
            .collect()
    }

    /// `strokes` made its strokes, the first as `points` and `radius`;
    /// none changes nothing.
    pub fn set_strokes(&mut self, mut strokes: Vec<Stroke>) {
        if strokes.is_empty() {
            return;
        }
        let first = strokes.remove(0);
        self.points = first.points;
        self.radius = first.radius;
        self.more = strokes;
    }

    /// The first stroke's radius made `radius` and every other's
    /// scaled with it: the Size slider on a repair of several strokes
    /// keeps them in proportion.
    ///
    /// The source stays, and a source chosen by the engine is then
    /// recorded with the patch as if placed by hand (§724): the person
    /// sees it stay with the spot, and what is written is what they
    /// saw.
    pub fn resize(&mut self, radius: f32) {
        if self.radius > 0.0 && radius != self.radius {
            let k = radius / self.radius;
            for s in &mut self.more {
                s.radius = (s.radius * k).clamp(0.002, 0.3);
            }
        }
        self.radius = radius;
    }

    /// The patch moved so its first point is at `to`. The source moves
    /// with it, and one the engine chose is recorded with it as if
    /// placed by hand (§724), as [`Self::resize`] has it.
    pub fn moved_to(&self, to: Pos) -> Self {
        let Some(first) = self.points.first().copied() else {
            return self.clone();
        };
        let (dx, dy) = (to[0] - first[0], to[1] - first[1]);
        let by = |points: &[Pos]| points.iter().map(|p| [p[0] + dx, p[1] + dy]).collect();
        Self {
            points: by(&self.points),
            more: self
                .more
                .iter()
                .map(|s| Stroke {
                    points: by(&s.points),
                    radius: s.radius,
                })
                .collect(),
            ..self.clone()
        }
    }

    /// The patch's center: the middle of its points' extent, every
    /// stroke's.
    pub fn center(&self) -> Pos {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in self.strokes().flat_map(|(points, _)| points) {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if self.points.is_empty() {
            return [0.0, 0.0];
        }
        [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]
    }

    /// The patch's region in a picture `w`×`h`: its window, each
    /// pixel's coverage, and the ring around it. None when the patch
    /// has no points or lies wholly outside. A patch of several strokes
    /// is their union: each pixel's cover the most any stroke gives it,
    /// and its ring outside every stroke and within some stroke's.
    pub fn region(&self, w: usize, h: usize) -> Option<Region> {
        if self.points.is_empty() || w == 0 || h == 0 {
            return None;
        }
        let scale = w as f32;
        let feather = self.feather.clamp(0.0, 1.0);
        // Each stroke in pixels: its line, its radius, where its
        // feather begins, and the window of the picture it reaches
        // (its ring's, as one stroke's region has always been), with
        // whether that window is on the picture at all.
        struct InPixels {
            pts: Vec<(f32, f32)>,
            r: f32,
            inner: f32,
            window: (usize, usize, usize, usize),
            on: bool,
        }
        let strokes: Vec<InPixels> = self
            .strokes()
            .filter(|(points, _)| !points.is_empty())
            .map(|(points, radius)| {
                let r = (radius * scale).max(0.5);
                let pts: Vec<(f32, f32)> = points
                    .iter()
                    .map(|p| (p[0] * scale, p[1] * scale))
                    .collect();
                let reach = r * RING + 1.0;
                let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
                for &(x, y) in &pts {
                    lo = (lo.0.min(x), lo.1.min(y));
                    hi = (hi.0.max(x), hi.1.max(y));
                }
                let window = (
                    (lo.0 - reach).floor().max(0.0) as usize,
                    (lo.1 - reach).floor().max(0.0) as usize,
                    ((hi.0 + reach).ceil() as usize + 1).min(w),
                    ((hi.1 + reach).ceil() as usize + 1).min(h),
                );
                let on = hi.0 + reach >= 0.0
                    && hi.1 + reach >= 0.0
                    && lo.0 - reach < w as f32
                    && lo.1 - reach < h as f32;
                InPixels {
                    pts,
                    r,
                    inner: r * (1.0 - feather).max(0.02),
                    window,
                    on,
                }
            })
            .collect();
        // A stroke off the picture reaches none of it and does not
        // widen the window; when none is on it, the first stroke's
        // window stands, as it always did.
        let mut kept: Vec<&InPixels> = strokes.iter().filter(|s| s.on).collect();
        if kept.is_empty() {
            kept.extend(strokes.first());
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for s in &kept {
            x0 = x0.min(s.window.0);
            y0 = y0.min(s.window.1);
            x1 = x1.max(s.window.2);
            y1 = y1.max(s.window.3);
        }
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        let (rw, rh) = (x1 - x0, y1 - y0);
        // Each stroke measured over its own window only, and merged:
        // past its ring a stroke gives a pixel no cover and no ring,
        // so this is the union over the whole window, at a cost of
        // each stroke's own area rather than every stroke's on every
        // pixel.
        let mut cover = vec![0.0f32; rw * rh];
        let mut inside = vec![false; rw * rh];
        let mut near = vec![false; rw * rh];
        for s in &kept {
            let (sx0, sy0, sx1, sy1) = s.window;
            #[cfg(test)]
            MEASURED.with(|m| {
                let span = |a: usize, b: usize| b.saturating_sub(a);
                m.set(m.get() + span(sx0.max(x0), sx1.min(x1)) * span(sy0.max(y0), sy1.min(y1)));
            });
            for y in sy0.max(y0)..sy1.min(y1) {
                for x in sx0.max(x0)..sx1.min(x1) {
                    let i = (y - y0) * rw + (x - x0);
                    let d = distance_to_polyline((x as f32 + 0.5, y as f32 + 0.5), &s.pts);
                    cover[i] = cover[i].max(1.0 - smoothstep(s.inner, s.r, d));
                    inside[i] |= d <= s.r;
                    near[i] |= d <= s.r * RING;
                }
            }
        }
        let ring = inside
            .iter()
            .zip(&near)
            .map(|(&inside, &near)| if !inside && near { 1.0 } else { 0.0 })
            .collect();
        Some(Region {
            x: x0,
            y: y0,
            width: rw,
            height: rh,
            cover,
            ring,
        })
    }
}

#[cfg(test)]
thread_local! {
    /// The pixels [`Patch::region`] has measured a stroke at, on this
    /// thread: the tests' count of its cost.
    static MEASURED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if b <= a {
        return if x < a { 0.0 } else { 1.0 };
    }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The distance from `p` to the nearest point of the polyline.
pub fn distance_to_polyline(p: (f32, f32), pts: &[(f32, f32)]) -> f32 {
    let mut best = f32::MAX;
    for (i, &a) in pts.iter().enumerate() {
        let b = pts.get(i + 1).copied().unwrap_or(a);
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let len2 = ex * ex + ey * ey;
        let t = if len2 > 0.0 {
            (((p.0 - a.0) * ex + (p.1 - a.1) * ey) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (qx, qy) = (a.0 + ex * t, a.1 + ey * t);
        best = best.min(((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt());
    }
    best
}

/// Every repair, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Retouch {
    /// The section's switch: off, the patches stay but do nothing.
    pub enabled: bool,
    pub patches: Vec<Patch>,
}

impl Retouch {
    /// Every patch carried round with a turned picture.
    pub fn turn(&mut self, t: crate::mask::Turned) {
        if t.is_identity() {
            return;
        }
        for patch in &mut self.patches {
            patch.turn(t);
        }
    }
}

impl Default for Retouch {
    fn default() -> Self {
        Self {
            enabled: true,
            patches: Vec::new(),
        }
    }
}

impl Retouch {
    /// Whether there is nothing to apply: no patches, or the switch off.
    pub fn is_empty(&self) -> bool {
        !self.enabled || self.patches.is_empty()
    }

    pub fn next_id(&self) -> u64 {
        self.patches.iter().map(|p| p.id).max().unwrap_or(0) + 1
    }

    /// Apply every patch to `image`, in order: the sourced ones from
    /// their sources (those without one are left as they are), the
    /// fills from `fill`, which is given the patch, its region and the
    /// picture as it is by then, and answers with the region's window
    /// in the picture's own values, or nothing to leave it.
    pub fn apply_with(
        &self,
        image: &mut greycard_core::image::WorkingImage,
        mut fill: impl FnMut(&Patch, &Region, &greycard_core::image::WorkingImage) -> Option<Vec<f32>>,
    ) {
        if !self.enabled {
            return;
        }
        for patch in &self.patches {
            let Some(region) = patch.region(image.width, image.height) else {
                continue;
            };
            match patch.method.engine() {
                Some(method) => {
                    let Some(source) = patch.source else {
                        continue;
                    };
                    let scale = image.width as f32;
                    let offset = (
                        (source[0] * scale).round() as i32,
                        (source[1] * scale).round() as i32,
                    );
                    retouch::apply(image, &region, offset, method, patch.opacity);
                }
                None => {
                    if let Some(replacement) = fill(patch, &region, image) {
                        retouch::apply_replacement(image, &region, &replacement, patch.opacity);
                    }
                }
            }
        }
    }

    /// Apply the sourced patches; fills are left as they are.
    pub fn apply(&self, image: &mut greycard_core::image::WorkingImage) {
        self.apply_with(image, |_, _, _| None);
    }

    /// Choose a source for every patch without one, from `image`;
    /// the ids and sources chosen.
    pub fn choose_sources(&self, image: &greycard_core::image::WorkingImage) -> Vec<(u64, Pos)> {
        let scale = image.width as f32;
        self.patches
            .iter()
            .filter(|_| self.enabled)
            .filter(|p| p.source.is_none() && p.method.sourced())
            .filter_map(|p| {
                let region = p.region(image.width, image.height)?;
                let (dx, dy) = retouch::find_source(image, &region);
                Some((p.id, [dx as f32 / scale, dy as f32 / scale]))
            })
            .collect()
    }

    /// With the sources `chosen` given to the patches that had none.
    pub fn with_sources(&self, chosen: &[(u64, Pos)]) -> Self {
        let mut out = self.clone();
        for patch in &mut out.patches {
            if patch.source.is_none()
                && let Some((_, s)) = chosen.iter().find(|(id, _)| *id == patch.id)
            {
                patch.source = Some(*s);
            }
        }
        out
    }

    /// The patches here the engine chose sources for, `chosen` being
    /// what [`Self::choose_sources`] answered and `self` the retouch
    /// with them put in: what a develop hands back, so the patches it
    /// chose for are named in full and not by id alone.
    pub fn chosen_patches(&self, chosen: &[(u64, Pos)]) -> Vec<Patch> {
        self.patches
            .iter()
            .filter(|p| p.source.is_some() && chosen.iter().any(|(id, _)| *id == p.id))
            .cloned()
            .collect()
    }

    /// Give each patch here still waiting on a source the one `done`
    /// has for it: the patch in `done` with the same id that is this
    /// one in every other field, so a later patch that took a
    /// removed one's id is not handed the old one's source. True
    /// when any patch took one.
    pub fn take_sources(&mut self, done: &[Patch]) -> bool {
        let mut took = false;
        for patch in &mut self.patches {
            if patch.source.is_some() || !patch.method.sourced() {
                continue;
            }
            let source = done.iter().filter(|q| q.id == patch.id).find_map(|q| {
                let s = q.source?;
                (*q == Patch {
                    source: Some(s),
                    ..patch.clone()
                })
                .then_some(s)
            });
            if let Some(s) = source {
                patch.source = Some(s);
                took = true;
            }
        }
        took
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fast` is kept in the sidecar beside `remove`, left out when
    /// false; a sidecar from before it reads as the full remove, and
    /// a Patch-made fill writes neither.
    #[test]
    fn the_fill_choice_round_trips() {
        let mut edit = crate::Edit::default();
        let mut p = Patch {
            id: 2,
            method: Method::Fill,
            points: vec![[0.5, 0.5]],
            ..Default::default()
        };
        for choice in FillChoice::ALL {
            p.set_fill_choice(choice);
            assert_eq!(p.fill_choice(), choice);
            assert_eq!(FillChoice::from_name(choice.name()), Some(choice));
        }
        p.set_fill_choice(FillChoice::Fast);
        edit.retouch.patches.push(p);
        let json = edit.to_json();
        assert!(
            json.contains("\"fast\"") && json.contains("\"remove\""),
            "{json}"
        );
        let back = crate::Edit::from_json(&json).unwrap();
        assert_eq!(back, edit);
        assert_eq!(back.retouch.patches[0].fill_choice(), FillChoice::Fast);
        assert_eq!(back.retouch.patches[0].name(), "Fill 2 (Fast)");
        // As an older build reads it, and writes it back: the full remove.
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["retouch"]["patches"][0]
            .as_object_mut()
            .unwrap()
            .remove("fast");
        let old = crate::Edit::from_json(&v.to_string()).unwrap();
        assert_eq!(old.retouch.patches[0].fill_choice(), FillChoice::Best);
        assert_eq!(old.retouch.patches[0].name(), "Fill 2");
        edit.retouch.patches[0].set_fill_choice(FillChoice::Patch);
        let json = edit.to_json();
        assert!(
            !json.contains("\"fast\"") && !json.contains("\"remove\""),
            "{json}"
        );
        // `fast` without `remove` is LaMa's.
        edit.retouch.patches[0].fast = true;
        assert_eq!(edit.retouch.patches[0].fill_choice(), FillChoice::Patch);
    }

    #[test]
    fn a_spot_covers_a_disc_with_a_soft_edge_and_a_ring_outside() {
        let patch = Patch {
            id: 1,
            points: vec![[0.5, 0.5]],
            radius: 0.1,
            feather: 0.5,
            ..Default::default()
        };
        let region = patch.region(100, 100).unwrap();
        let at = |x: usize, y: usize| {
            let i = (y - region.y) * region.width + (x - region.x);
            (region.cover[i], region.ring[i])
        };
        assert_eq!(at(50, 50), (1.0, 0.0));
        assert_eq!(at(53, 50).0, 1.0, "inside the unfeathered core");
        let (edge, _) = at(58, 50);
        assert!(edge > 0.0 && edge < 1.0, "in the feather: {edge}");
        assert_eq!(at(63, 50), (0.0, 1.0), "the ring");
        assert_eq!(at(68, 50), (0.0, 0.0), "beyond it");
        assert!(region.x < 40 && region.x + region.width > 60);
    }

    #[test]
    fn a_stroke_covers_along_its_line() {
        let patch = Patch {
            id: 1,
            points: vec![[0.2, 0.5], [0.8, 0.5]],
            radius: 0.05,
            feather: 0.0,
            ..Default::default()
        };
        let region = patch.region(100, 100).unwrap();
        let at = |x: usize, y: usize| region.cover[(y - region.y) * region.width + (x - region.x)];
        assert_eq!(at(50, 50), 1.0);
        assert_eq!(at(30, 52), 1.0);
        assert_eq!(at(50, 60), 0.0);
        assert!((patch.center()[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn retouch_applies_only_sourced_patches_and_keeps_its_ids() {
        let mut image = greycard_core::image::WorkingImage {
            width: 40,
            height: 40,
            // A pattern no shift reproduces.
            data: (0..40u32 * 40)
                .flat_map(|i| [(i.wrapping_mul(2654435761) >> 24) as f32 / 255.0; 3])
                .collect(),
        };
        let before = image.clone();
        let mut r = Retouch::default();
        r.patches.push(Patch {
            id: 1,
            method: Method::Clone,
            points: vec![[0.5, 0.5]],
            radius: 0.1,
            feather: 0.0,
            ..Default::default()
        });
        assert_eq!(r.next_id(), 2);
        r.apply(&mut image);
        assert_eq!(image.data, before.data, "no source, nothing done");
        let chosen = r.choose_sources(&image);
        assert_eq!(chosen.len(), 1);
        let r = r.with_sources(&chosen);
        assert_eq!(r.chosen_patches(&chosen), vec![r.patches[0].clone()]);
        assert!(r.chosen_patches(&[]).is_empty());
        assert!(r.patches[0].source.is_some());
        r.apply(&mut image);
        assert_ne!(image.data, before.data);
        let json = serde_json::to_string(&r).unwrap();
        let back: Retouch = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn a_fill_takes_its_window_from_the_caller_and_wants_no_source() {
        let mut image = greycard_core::image::WorkingImage {
            width: 40,
            height: 40,
            data: vec![0.5; 40 * 40 * 3],
        };
        let r = Retouch {
            enabled: true,
            patches: vec![Patch {
                id: 3,
                method: Method::Fill,
                points: vec![[0.5, 0.5]],
                radius: 0.1,
                feather: 0.0,
                ..Default::default()
            }],
        };
        assert!(r.choose_sources(&image).is_empty());
        let mut asked = 0;
        r.apply_with(&mut image, |p, region, _| {
            asked += 1;
            assert_eq!(p.id, 3);
            Some(vec![0.9; region.width * region.height * 3])
        });
        assert_eq!(asked, 1);
        let center = (20 * 40 + 20) * 3;
        assert!((image.data[center] - 0.9).abs() < 1e-6);
        assert!((image.data[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn moving_a_patch_carries_its_points() {
        let patch = Patch {
            id: 1,
            points: vec![[0.2, 0.5], [0.3, 0.6]],
            ..Default::default()
        };
        let moved = patch.moved_to([0.5, 0.5]);
        assert_eq!(moved.points, vec![[0.5, 0.5], [0.6, 0.6]]);
    }

    /// `Patch::region` as it was before a patch could have more than
    /// one stroke, kept to hold the one-stroke case to it.
    fn old_region(p: &Patch, w: usize, h: usize) -> Region {
        let scale = w as f32;
        let r = (p.radius * scale).max(0.5);
        let reach = r * RING + 1.0;
        let pts: Vec<(f32, f32)> = p
            .points
            .iter()
            .map(|p| (p[0] * scale, p[1] * scale))
            .collect();
        let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for &(x, y) in &pts {
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x), hi.1.max(y));
        }
        let x0 = (lo.0 - reach).floor().max(0.0) as usize;
        let y0 = (lo.1 - reach).floor().max(0.0) as usize;
        let x1 = ((hi.0 + reach).ceil() as usize + 1).min(w);
        let y1 = ((hi.1 + reach).ceil() as usize + 1).min(h);
        let inner = r * (1.0 - p.feather.clamp(0.0, 1.0)).max(0.02);
        let (mut cover, mut ring) = (Vec::new(), Vec::new());
        for y in y0..y1 {
            for x in x0..x1 {
                let d = distance_to_polyline((x as f32 + 0.5, y as f32 + 0.5), &pts);
                cover.push(1.0 - smoothstep(inner, r, d));
                ring.push(if d > r && d <= r * RING { 1.0 } else { 0.0 });
            }
        }
        Region {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
            cover,
            ring,
        }
    }

    fn two_strokes() -> Patch {
        Patch {
            id: 1,
            method: Method::Fill,
            points: vec![[0.2, 0.5], [0.4, 0.5]],
            radius: 0.05,
            feather: 0.5,
            more: vec![Stroke {
                points: vec![[0.7, 0.5]],
                radius: 0.1,
            }],
            ..Default::default()
        }
    }

    /// A repair of several strokes covers their union: each pixel the
    /// most any stroke gives it, and the ring only outside them all; a
    /// stroke alone is what a patch of that one stroke covers.
    #[test]
    fn several_strokes_cover_their_union() {
        let both = two_strokes();
        let first = Patch {
            more: Vec::new(),
            ..both.clone()
        };
        let second = Patch {
            points: vec![[0.7, 0.5]],
            radius: 0.1,
            more: Vec::new(),
            ..both.clone()
        };
        let (w, h) = (200, 100);
        let union = both.region(w, h).unwrap();
        let parts = [first.region(w, h).unwrap(), second.region(w, h).unwrap()];
        let at = |r: &Region, x: usize, y: usize| {
            if x < r.x || y < r.y || x >= r.x + r.width || y >= r.y + r.height {
                return (0.0, 0.0);
            }
            let i = (y - r.y) * r.width + (x - r.x);
            (r.cover[i], r.ring[i])
        };
        for y in 0..h {
            for x in 0..w {
                let (c, ring) = at(&union, x, y);
                let (a, b) = (at(&parts[0], x, y), at(&parts[1], x, y));
                assert_eq!(c, a.0.max(b.0), "{x},{y}");
                let inside = a.0 > 0.0 || b.0 > 0.0;
                let near = a.1 > 0.0 || b.1 > 0.0;
                if inside {
                    assert_eq!(ring, 0.0, "{x},{y}: inside a stroke is no ring");
                } else if near {
                    assert_eq!(ring, 1.0, "{x},{y}");
                }
            }
        }
        // The window holds both.
        assert!(union.x <= parts[0].x && union.x + union.width >= parts[1].x + parts[1].width);
        // One stroke: the same region to the bit as before `more`.
        let r = &parts[0];
        let old = old_region(&first, w, h);
        assert_eq!(
            (r.x, r.y, r.width, r.height),
            (old.x, old.y, old.width, old.height)
        );
        let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&r.cover), bits(&old.cover));
        assert_eq!(bits(&r.ring), bits(&old.ring));
        // Its center spans every stroke; moving and turning carry them.
        assert!((both.center()[0] - 0.45).abs() < 1e-6);
        let moved = both.moved_to([0.3, 0.5]);
        assert_eq!(moved.more[0].points, vec![[0.8, 0.5]]);
        assert_eq!(moved.more[0].radius, 0.1);
        let mut resized = both.clone();
        resized.resize(0.1);
        assert_eq!((resized.radius, resized.more[0].radius), (0.1, 0.2));
    }

    /// A Fill of many strokes: `count` strokes of twenty points, each
    /// beside the last, along a picture's width.
    fn long_fill(count: usize) -> Patch {
        let stroke = |k: usize| Stroke {
            points: (0..20)
                .map(|j| {
                    [
                        0.1 + 0.025 * k as f32 + 0.001 * j as f32,
                        0.4 + 0.002 * j as f32,
                    ]
                })
                .collect(),
            radius: 0.012,
        };
        let mut patch = Patch {
            id: 1,
            method: Method::Fill,
            ..Default::default()
        };
        patch.set_strokes((0..count).map(stroke).collect());
        patch
    }

    /// A Fill grown from many strokes measures each stroke over its own
    /// window, as many Fills of one stroke would, not every stroke on
    /// every pixel of the union: counted, the pixels measured are the
    /// thirty strokes' alone, and far fewer than thirty unions.
    #[test]
    fn many_strokes_measure_only_their_own_areas() {
        let measured = |p: &Patch| {
            super::MEASURED.with(|m| m.set(0));
            let r = p.region(1500, 1000).unwrap();
            (
                super::MEASURED.with(std::cell::Cell::get),
                r.width * r.height,
            )
        };
        let patch = long_fill(30);
        let alone: usize = patch
            .stroke_list()
            .into_iter()
            .map(|s| {
                let mut p = patch.clone();
                p.set_strokes(vec![s]);
                measured(&p).0
            })
            .sum();
        let (union, area) = measured(&patch);
        assert_eq!(union, alone);
        assert!(union < area * 30 / 4, "{union} measured, the window {area}");
    }

    /// [`many_strokes_measure_only_their_own_areas`] in time: thirty
    /// strokes at 1500x1000 against the thirty alone, both measured
    /// here, so the profile is the same.
    #[test]
    #[ignore = "a measurement, run with --release --ignored --nocapture"]
    fn many_strokes_cost_about_their_own_areas() {
        let patch = long_fill(30);
        let alone: Vec<Patch> = patch
            .stroke_list()
            .into_iter()
            .map(|s| {
                let mut p = patch.clone();
                p.set_strokes(vec![s]);
                p
            })
            .collect();
        let best = |f: &dyn Fn()| {
            (0..3)
                .map(|_| {
                    let t = std::time::Instant::now();
                    f();
                    t.elapsed()
                })
                .min()
                .unwrap()
        };
        let union = best(&|| {
            patch.region(1500, 1000).unwrap();
        });
        let separate = best(&|| {
            for p in &alone {
                p.region(1500, 1000).unwrap();
            }
        });
        // Measured every pixel against every stroke, the union took
        // thirty times as long and more.
        assert!(
            union < separate * 4,
            "{union:?} for the union, {separate:?} for the strokes alone"
        );
    }

    /// The timings the review asked for, in the build they run in.
    #[test]
    #[ignore = "a measurement, run with --release --ignored --nocapture"]
    fn many_strokes_timed() {
        for w in [1500, 6000] {
            let patch = long_fill(30);
            let t = std::time::Instant::now();
            let r = patch.region(w, w * 2 / 3).unwrap();
            eprintln!(
                "30 strokes of 20 points at {w} wide: {:?}, a window {}x{}",
                t.elapsed(),
                r.width,
                r.height
            );
        }
    }

    /// A stroke off the picture leaves the window as the others make it.
    #[test]
    fn a_stroke_off_the_picture_does_not_widen_the_window() {
        let on = Patch {
            id: 1,
            method: Method::Fill,
            points: vec![[0.5, 0.5]],
            radius: 0.05,
            ..Default::default()
        };
        let mut off = on.clone();
        off.more.push(Stroke {
            points: vec![[-0.5, 0.5]],
            radius: 0.05,
        });
        let (a, b) = (on.region(200, 200).unwrap(), off.region(200, 200).unwrap());
        assert_eq!((a.x, a.y, a.width, a.height), (b.x, b.y, b.width, b.height));
        assert_eq!(a.cover, b.cover);
        // Off it first, and the stroke on it after: the same again.
        let mut first_off = off.clone();
        first_off.set_strokes(vec![off.more[0].clone(), on.stroke_list().remove(0)]);
        let c = first_off.region(200, 200).unwrap();
        assert_eq!((a.x, a.width), (c.x, c.width));
    }

    /// A Heal read with more strokes, as a hand-edited sidecar might
    /// have, covers their union; a further stroke missing its radius
    /// reads with the default rather than failing the edit.
    #[test]
    fn more_strokes_read_from_a_file_on_a_heal_and_without_a_radius() {
        let json = r#"{"version":9,"retouch":{"patches":[{"id":1,"method":"heal","points":[[0.2,0.5]],"radius":0.03,"more":[{"points":[[0.7,0.5]]}]}]}}"#;
        let edit = crate::Edit::from_json(json).unwrap();
        let p = &edit.retouch.patches[0];
        assert_eq!(p.method, Method::Heal);
        assert_eq!(p.more[0].radius, Patch::default().radius);
        let r = p.region(100, 100).unwrap();
        let at = |x: usize, y: usize| r.cover[(y - r.y) * r.width + (x - r.x)];
        assert_eq!(at(20, 50), 1.0);
        assert_eq!(at(70, 50), 1.0, "the further stroke's");
        assert_eq!(at(45, 50), 0.0, "and nothing between");
    }

    /// Two strokes touch where their covers overlap, not where only
    /// their rings do; a lone point and a crossing count.
    #[test]
    fn strokes_touch_where_their_covers_overlap() {
        let s = |points: Vec<Pos>, radius| Stroke { points, radius };
        let a = s(vec![[0.2, 0.5], [0.4, 0.5]], 0.05);
        assert!(
            a.touches(&s(vec![[0.48, 0.5]], 0.05)),
            "0.08 apart, 0.1 of radii"
        );
        assert!(!a.touches(&s(vec![[0.52, 0.5]], 0.05)), "rings alone");
        assert!(
            a.touches(&s(vec![[0.3, 0.3], [0.3, 0.7]], 0.001)),
            "crossing"
        );
        assert!(!a.touches(&s(vec![[0.3, 0.6]], 0.04)), "beside it, apart");
        let cover_meets = |p: &Patch, q: &Patch| {
            let (r, t) = (p.region(400, 400).unwrap(), q.region(400, 400).unwrap());
            (0..r.height).any(|y| {
                (0..r.width).any(|x| {
                    let (gx, gy) = (r.x + x, r.y + y);
                    r.cover[y * r.width + x] > 0.0
                        && gx >= t.x
                        && gy >= t.y
                        && gx < t.x + t.width
                        && gy < t.y + t.height
                        && t.cover[(gy - t.y) * t.width + gx - t.x] > 0.0
                })
            })
        };
        let patch = |st: &Stroke| Patch {
            points: st.points.clone(),
            radius: st.radius,
            ..Default::default()
        };
        for b in [
            s(vec![[0.47, 0.5]], 0.05),
            s(vec![[0.53, 0.5]], 0.05),
            s(vec![[0.3, 0.565]], 0.02),
            s(vec![[0.3, 0.58]], 0.02),
        ] {
            assert_eq!(a.touches(&b), cover_meets(&patch(&a), &patch(&b)), "{b:?}");
        }
    }

    /// A repair of one stroke writes what it wrote at version 8, to
    /// the byte; one of several keeps them through the sidecar.
    #[test]
    fn one_stroke_writes_as_before_and_several_round_trip() {
        // As version 8 wrote a Fill of the remove's and a Heal.
        let retouch = r#"{"enabled":true,"patches":[{"id":1,"method":"fill","points":[[0.25,0.5],[0.3,0.5]],"radius":0.02,"feather":0.3,"opacity":1.0,"source":null,"seed":2,"remove":true},{"id":2,"method":"heal","points":[[0.5,0.5]],"radius":0.03,"feather":0.3,"opacity":0.8,"source":[0.1,0.0]}]}"#;
        let v8 = format!(r#"{{"version":8,"retouch":{retouch}}}"#);
        let edit = crate::Edit::from_json(&v8).unwrap();
        assert_eq!(edit.version, crate::VERSION);
        assert!(edit.retouch.patches.iter().all(|p| p.more.is_empty()));
        assert_eq!(serde_json::to_string(&edit.retouch).unwrap(), retouch);
        // And through the whole edit as it is written and read again.
        let again = crate::Edit::from_json(&edit.to_json()).unwrap();
        assert_eq!(serde_json::to_string(&again.retouch).unwrap(), retouch);

        let mut edit = crate::Edit::default();
        edit.retouch.patches.push(two_strokes());
        let json = edit.to_json();
        assert!(json.contains("\"more\""), "{json}");
        let back = crate::Edit::from_json(&json).unwrap();
        assert_eq!(back, edit);
        assert_eq!(back.retouch.patches[0].more[0].radius, 0.1);
    }
}
