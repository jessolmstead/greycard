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
/// `source` away, and blended at `opacity`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Patch {
    pub id: u64,
    pub method: Method,
    pub points: Vec<Pos>,
    pub radius: f32,
    pub feather: f32,
    pub opacity: f32,
    /// The source's offset from the patch. None until the engine has
    /// chosen one.
    pub source: Option<Pos>,
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
        }
    }
}

/// How far outside the patch its surroundings are looked at, in radii.
const RING: f32 = 1.7;

impl Patch {
    pub fn name(&self) -> String {
        format!("{} {}", self.method.name(), self.id)
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
        self.source = self.source.map(|d| t.offset(d));
    }

    /// The patch moved so its first point is at `to`.
    pub fn moved_to(&self, to: Pos) -> Self {
        let Some(first) = self.points.first().copied() else {
            return self.clone();
        };
        let (dx, dy) = (to[0] - first[0], to[1] - first[1]);
        Self {
            points: self.points.iter().map(|p| [p[0] + dx, p[1] + dy]).collect(),
            ..self.clone()
        }
    }

    /// The patch's center: the middle of its points' extent.
    pub fn center(&self) -> Pos {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in &self.points {
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
    /// has no points or lies wholly outside.
    pub fn region(&self, w: usize, h: usize) -> Option<Region> {
        if self.points.is_empty() || w == 0 || h == 0 {
            return None;
        }
        let scale = w as f32;
        let r = (self.radius * scale).max(0.5);
        let reach = r * RING + 1.0;
        let pts: Vec<(f32, f32)> = self
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
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        let (rw, rh) = (x1 - x0, y1 - y0);
        let inner = r * (1.0 - self.feather.clamp(0.0, 1.0)).max(0.02);
        let mut cover = Vec::with_capacity(rw * rh);
        let mut ring = Vec::with_capacity(rw * rh);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = distance_to_polyline((x as f32 + 0.5, y as f32 + 0.5), &pts);
                cover.push(1.0 - smoothstep(inner, r, d));
                ring.push(if d > r && d <= r * RING { 1.0 } else { 0.0 });
            }
        }
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
}
