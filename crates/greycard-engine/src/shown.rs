//! What the crop shows of the picture the retouch works on: a Fill
//! sees what the user sees. The retouch runs before the geometry, on
//! the whole frame, so a Fill's window can take in pixels the crop
//! hides, which the user never saw and could not paint: an object
//! running on past the crop's edge is there to the model, outside the
//! hole, and it carries the object back into the hole. So the pixels
//! of a Fill's window the crop hides are given to its engine as part
//! of the hole, unknown, made up with it and never read as its
//! surround; only the patch's own coverage is put back in the picture.
//!
//! The crop is the frame's outline on the picture before the
//! geometry: four corners, the frame's, taken back through the turn,
//! the angle, and the keystone, so it is a quadrilateral, convex, and
//! a rectangle only when nothing turns it. A pixel is hidden when its
//! center is outside it.

use greycard_ai::fill::Window;
use greycard_edit::Geometry;
use greycard_edit::geometry::Crop;
use greycard_edit::retouch::{Method, Retouch};

/// The frame's outline on the picture the retouch works on, its
/// corners in fractions of that picture's width and height, round
/// the frame in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    corners: [[f32; 2]; 4],
}

impl Shown {
    /// What `geometry` shows of a picture `w` by `h` before it, the
    /// camera's frame `camera` as the finish takes it: none when it
    /// shows all of it (no crop, a quarter turn, a mirror), or when a
    /// corner of the frame lies past the keystone's horizon, where the
    /// outline is no quadrilateral; then nothing is hidden, as before.
    /// That last is rare: two strong keystones, or one with an angle,
    /// tilting a corner of the frame past the horizon; a Fill there
    /// sees what the crop hides, as every Fill did before.
    pub fn of(geometry: &Geometry, w: f32, h: f32, camera: Option<Crop>) -> Option<Self> {
        if w <= 0.0 || h <= 0.0 || geometry.is_identity_on(w, h, camera) {
            return None;
        }
        let g = geometry.with_camera_frame(w, h, camera);
        let f = g.frame(w, h, None);
        let (x0, y0) = f.origin;
        let (x1, y1) = (x0 + f.size.0, y0 + f.size.1);
        let plane = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        if plane.iter().any(|&p| !g.in_front(p, w, h)) {
            return None;
        }
        let shown = Self {
            corners: plane.map(|p| {
                let s = g.to_source(p, w, h);
                [s.0 / w, s.1 / h]
            }),
        };
        let whole = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        (!whole.iter().all(|&[x, y]| shown.inside(x, y))).then_some(shown)
    }

    /// Whether the point (`x`, `y`), in fractions, is within the
    /// outline, its edge included: on the same side of each edge,
    /// whichever way round a mirror took the corners.
    pub fn inside(&self, x: f32, y: f32) -> bool {
        let (mut neg, mut pos) = (false, false);
        for i in 0..4 {
            let [ax, ay] = self.corners[i];
            let [bx, by] = self.corners[(i + 1) % 4];
            let cross = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
            neg |= cross < -1e-7;
            pos |= cross > 1e-7;
        }
        !(neg && pos)
    }

    /// The pixels of `window`, in a picture of `size`, the crop hides,
    /// row by row: none when it hides none of them.
    pub fn hidden(&self, window: Window, size: (usize, usize)) -> Option<Vec<bool>> {
        let (w, h) = (size.0 as f32, size.1 as f32);
        let mut any = false;
        let hidden: Vec<bool> = (0..window.width * window.height)
            .map(|i| {
                let (x, y) = (window.x + i % window.width, window.y + i / window.width);
                let out = !self.inside((x as f32 + 0.5) / w, (y as f32 + 0.5) / h);
                any |= out;
                out
            })
            .collect();
        any.then_some(hidden)
    }

    /// Whether the crop's outline may cross the window of a Fill of
    /// `retouch`, in a picture `w` by `h`: the box of each Fill's
    /// strokes with twice their radius round it, grown by half its
    /// longer side and the window's least margin and step, which holds
    /// its window ([`greycard_ai::fill::window`]), held to the picture,
    /// is not wholly within the outline. A crop that crosses no Fill's reach hides nothing
    /// from any, and its moves make no fill again.
    pub fn reaches(&self, retouch: &Retouch, w: f32, h: f32) -> bool {
        retouch
            .patches
            .iter()
            .filter(|p| p.method == Method::Fill && !p.points.is_empty())
            .any(|p| {
                let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
                for (points, radius) in p.strokes() {
                    // Points and radii are in the picture's widths.
                    let r = 2.0 * radius * w;
                    for q in points {
                        lo = [lo[0].min(q[0] * w - r), lo[1].min(q[1] * w - r)];
                        hi = [hi[0].max(q[0] * w + r), hi[1].max(q[1] * w + r)];
                    }
                }
                let grow = (hi[0] - lo[0]).max(hi[1] - lo[1]) / 2.0 + 48.0;
                // Held to the picture, as the window is: a Fill near its
                // edge is not reached by every crop for a box past it.
                let (x0, y0) = (((lo[0] - grow) / w).max(0.0), ((lo[1] - grow) / h).max(0.0));
                let (x1, y1) = (((hi[0] + grow) / w).min(1.0), ((hi[1] + grow) / h).min(1.0));
                ![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
                    .iter()
                    .all(|&(x, y)| self.inside(x, y))
            })
    }
}

/// A fingerprint of what the crop hides of a window, for a fill's
/// keys: none when it hides nothing, so a fill no crop reaches keeps
/// the keys it had.
pub fn fingerprint(hidden: Option<&[bool]>) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let hidden = hidden?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    hidden.len().hash(&mut h);
    for chunk in hidden.chunks(64) {
        chunk
            .iter()
            .enumerate()
            .fold(0u64, |a, (i, &b)| a | (u64::from(b) << i))
            .hash(&mut h);
    }
    Some(h.finish())
}

#[cfg(test)]
mod tests {
    use greycard_edit::retouch::Patch;

    use super::*;

    fn cropped(x: f32, y: f32, w: f32, h: f32) -> Geometry {
        Geometry {
            crop: Some(Crop { x, y, w, h }),
            ..Geometry::default()
        }
    }

    fn whole(w: usize, h: usize) -> Window {
        Window {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    /// No crop, a quarter turn, or a mirror shows the whole picture:
    /// nothing is hidden, and nothing about a fill changes.
    #[test]
    fn the_whole_picture_hides_nothing() {
        for g in [
            Geometry::default(),
            Geometry {
                turns: 1,
                ..Geometry::default()
            },
            Geometry {
                flip: true,
                ..Geometry::default()
            },
            cropped(0.0, 0.0, 1.0, 1.0),
        ] {
            assert_eq!(Shown::of(&g, 400.0, 300.0, None), None, "{g:?}");
        }
        assert_eq!(fingerprint(None), None);
    }

    /// A crop hides the pixels outside it, and only those.
    #[test]
    fn a_crop_hides_what_is_outside_it() {
        let shown = Shown::of(&cropped(0.1, 0.2, 0.8, 0.7), 400.0, 300.0, None).expect("a crop");
        let hidden = shown.hidden(whole(400, 300), (400, 300)).expect("some");
        for (i, &out) in hidden.iter().enumerate() {
            let (x, y) = (i % 400, i / 400);
            let want = !(40..360).contains(&x) || !(60..270).contains(&y);
            assert_eq!(out, want, "({x}, {y})");
        }
        // A window wholly inside it hides nothing.
        let inside = Window {
            x: 100,
            y: 100,
            width: 50,
            height: 50,
        };
        assert_eq!(shown.hidden(inside, (400, 300)), None);
        // At another size of the same picture, the same fractions.
        let half = shown.hidden(whole(200, 150), (200, 150)).expect("some");
        assert!(half[75 * 200 + 19] && !half[75 * 200 + 20]);
    }

    /// Under an angle, a keystone, a mirror, and a turn, a pixel is
    /// hidden exactly where the frame does not take it in: its point on
    /// the plane is outside the frame, but along the frame's edge,
    /// where the two may round apart.
    #[test]
    fn a_turned_frame_hides_what_the_frame_leaves_out() {
        let (w, h) = (240usize, 160usize);
        let geometries = [
            Geometry {
                angle: 4.0,
                ..Geometry::default()
            },
            Geometry {
                angle: -7.5,
                crop: Some(Crop {
                    x: 0.2,
                    y: 0.1,
                    w: 0.6,
                    h: 0.7,
                }),
                ..Geometry::default()
            },
            Geometry {
                vertical: 12.0,
                horizontal: -6.0,
                ..Geometry::default()
            },
            Geometry {
                angle: 3.0,
                turns: 1,
                flip: true,
                ..Geometry::default()
            },
        ];
        for g in geometries {
            let shown = Shown::of(&g, w as f32, h as f32, None).expect("hides something");
            let hidden = shown.hidden(whole(w, h), (w, h)).expect("some");
            let f = g.frame(w as f32, h as f32, None);
            let (x0, y0) = f.origin;
            let (x1, y1) = (x0 + f.size.0, y0 + f.size.1);
            let (mut out_n, mut in_n) = (0, 0);
            for (i, &out) in hidden.iter().enumerate() {
                let s = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let p = g.to_plane(s, w as f32, h as f32);
                let inside = p.0 >= x0 && p.0 <= x1 && p.1 >= y0 && p.1 <= y1;
                let edge = [p.0 - x0, x1 - p.0, p.1 - y0, y1 - p.1]
                    .iter()
                    .any(|d| d.abs() < 1.5);
                if !edge {
                    assert_eq!(out, !inside, "{g:?} at {s:?}");
                }
                out_n += usize::from(out);
                in_n += usize::from(!out);
            }
            assert!(out_n > 0 && in_n > 0, "{g:?}");
            // The picture's middle is always shown.
            assert!(shown.inside(0.5, 0.5));
        }
    }

    /// The camera's frame frames an edit with no crop, and hides what
    /// it leaves out, as the finish cuts it.
    #[test]
    fn the_cameras_frame_hides_what_it_leaves_out() {
        let camera = Crop {
            x: 0.0,
            y: 0.125,
            w: 1.0,
            h: 0.75,
        };
        let shown = Shown::of(&Geometry::default(), 400.0, 300.0, Some(camera)).expect("framed");
        assert!(!shown.inside(0.5, 0.1) && shown.inside(0.5, 0.2));
    }

    fn fill_at(x: f32, y: f32, method: Method) -> Retouch {
        Retouch {
            enabled: true,
            patches: vec![Patch {
                method,
                points: vec![[x, y]],
                radius: 0.02,
                ..Patch::default()
            }],
        }
    }

    /// A crop reaches a Fill only near its edge, and never a heal.
    #[test]
    fn a_crop_reaches_a_fill_near_its_edge_alone() {
        let shown = Shown::of(&cropped(0.0, 0.0, 0.9, 1.0), 1000.0, 600.0, None).expect("a crop");
        assert!(!shown.reaches(&fill_at(0.3, 0.3, Method::Fill), 1000.0, 600.0));
        assert!(shown.reaches(&fill_at(0.85, 0.3, Method::Fill), 1000.0, 600.0));
        assert!(!shown.reaches(&fill_at(0.85, 0.3, Method::Heal), 1000.0, 600.0));
    }

    /// SplitMix64's uniforms in 0 to 1, for the property below.
    struct Rng(u64);
    impl Rng {
        fn f(&mut self) -> f32 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            (z >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    /// The reach is conservative: over random Fills (one to three
    /// strokes, radii from a pixel to a fifth of the width, anywhere,
    /// edges included), geometries (crops, angles, keystones, turns,
    /// mirrors, camera frames), and sizes, a crop that hides any of a Fill's window as
    /// the engine finds it (`ai::window_for`) reaches it. The growth
    /// cut to a quarter of the box with no least margin fails it.
    #[test]
    fn a_crop_that_hides_part_of_a_window_reaches_its_fill() {
        let mut r = Rng(11);
        let sizes = [(640usize, 427usize), (1500, 1000), (400, 600), (900, 300)];
        let (mut hid, mut checked, mut framed) = (0, 0, 0);
        for k in 0..800 {
            let (w, h) = sizes[k % sizes.len()];
            let (wf, hf) = (w as f32, h as f32);
            let crop = (r.f() < 0.8).then(|| {
                let (cw, ch) = (0.3 + 0.6 * r.f(), 0.3 + 0.6 * r.f());
                Crop {
                    x: (1.0 - cw) * r.f(),
                    y: (1.0 - ch) * r.f(),
                    w: cw,
                    h: ch,
                }
            });
            let g = Geometry {
                turns: (r.f() * 4.0) as u8 % 4,
                flip: r.f() < 0.5,
                angle: if r.f() < 0.6 {
                    (r.f() - 0.5) * 20.0
                } else {
                    0.0
                },
                vertical: if r.f() < 0.4 {
                    (r.f() - 0.5) * 30.0
                } else {
                    0.0
                },
                horizontal: if r.f() < 0.4 {
                    (r.f() - 0.5) * 30.0
                } else {
                    0.0
                },
                crop,
                ..Geometry::default()
            };
            // A camera's frame, as a phone's DNG or a 4:3 crop records.
            let camera = (r.f() < 0.25).then(|| {
                let (cw, ch) = (0.7 + 0.3 * r.f(), 0.7 + 0.3 * r.f());
                Crop {
                    x: (1.0 - cw) * r.f(),
                    y: (1.0 - ch) * r.f(),
                    w: cw,
                    h: ch,
                }
            });
            let Some(shown) = Shown::of(&g, wf, hf, camera) else {
                continue;
            };
            framed += usize::from(camera.is_some());
            let radius = [0.0003f32, 0.002, 0.01, 0.05, 0.2][(r.f() * 5.0) as usize % 5];
            let n = 1 + (r.f() * 3.0) as usize;
            let points: Vec<[f32; 2]> = (0..n).map(|_| [r.f(), r.f() * hf / wf]).collect();
            let patch = Patch {
                id: 1,
                method: Method::Fill,
                points,
                radius,
                feather: r.f(),
                ..Patch::default()
            };
            let Some(region) = patch.region(w, h) else {
                continue;
            };
            if !region.cover.iter().any(|&c| c > 0.0) {
                continue;
            }
            let (window, _) = crate::ai::window_for(&region, w, h);
            checked += 1;
            if shown.hidden(window, (w, h)).is_some() {
                hid += 1;
                let retouch = Retouch {
                    enabled: true,
                    patches: vec![patch],
                };
                assert!(
                    shown.reaches(&retouch, wf, hf),
                    "{w}x{h}, radius {radius}, window {window:?}, {g:?}, camera {camera:?}"
                );
            }
        }
        // Enough of both to mean something.
        assert!(
            checked > 500 && hid > 100 && framed > 100,
            "{checked} checked, {hid} hidden, {framed} framed"
        );
    }

    /// Two crops that hide different pixels of a window key a fill
    /// apart; two that hide the same do not.
    #[test]
    fn the_fingerprint_follows_what_is_hidden() {
        let window = Window {
            x: 300,
            y: 100,
            width: 100,
            height: 100,
        };
        let key = |c: Geometry| {
            fingerprint(
                Shown::of(&c, 400.0, 300.0, None)
                    .and_then(|s| s.hidden(window, (400, 300)))
                    .as_deref(),
            )
        };
        let a = key(cropped(0.0, 0.0, 0.9, 1.0));
        assert!(a.is_some());
        assert_ne!(a, key(cropped(0.0, 0.0, 0.92, 1.0)));
        assert_eq!(a, key(cropped(0.0, 0.0, 0.9, 1.0)));
        // A crop whose edge is outside the window hides none of it.
        assert_eq!(key(cropped(0.0, 0.1, 1.0, 0.9)), None);
    }
}
