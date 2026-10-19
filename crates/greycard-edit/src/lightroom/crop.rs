//! Lightroom's crop onto ours.
//!
//! Lightroom keeps a crop as two opposite corners of the cropped
//! rectangle, shares of the file's width and height in the file's own
//! way up (before its orientation tag or a turn in Lightroom), and the
//! angle the rectangle is turned by: its sides run along (cos, sin) and
//! (-sin, cos) from the corner at `CropLeft`, `CropTop`, down being
//! positive. The export is that rectangle sampled out, then turned by
//! Lightroom's orientation of the image.
//!
//! Ours is the other way about ([`crate::geometry`]): the picture is
//! taken the way up its tag and the frame's turn say, turned by the
//! edit's quarter turns, then by the fine angle about its center, and
//! the crop is an upright rectangle of that leveled plane. So the
//! mapping is a change of frame, done on points: the rectangle's
//! corners taken from the file's way up to the way the picture is
//! shown, then into the leveled plane through
//! [`Geometry::to_plane`], where they stand upright.
//!
//! **The angle.** A side along (cos, sin) in the file is level once the
//! picture is turned the other way by the same angle, which is our
//! counter-clockwise turn, `angle = CropAngle`. The tag's quarter turns
//! commute with it; a tag that mirrors the picture reverses it, so a
//! mirrored frame takes `-CropAngle`.
//!
//! **The quarter turns.** Lightroom's orientation of an image starts at
//! the file's tag and moves with its Rotate Left and Right. Where it
//! differs from the tag by quarter turns, the edit takes those turns,
//! so the plane stands the way Lightroom's export does. An orientation
//! mirrored where the tag is not (Lightroom's Flip) has no quarter
//! turn, and the crop is not carried.

use crate::geometry::{self, Aspect, Geometry};
use greycard_core::raw::Orientation;

/// Lightroom's crop: two opposite corners of the cropped rectangle, as
/// shares of the file's width and height before any turn, and the angle
/// it is turned by, in degrees. The corner at `left`, `top` is the
/// rectangle's own top left, and its sides run along the angle: the
/// width along (cos, sin), the height along (-sin, cos), down being
/// positive. Checked against the cropped sizes the catalog keeps beside
/// the settings, to the pixel, at -0.24 and 1.1 degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crop {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub angle: f64,
}

/// The keys of Lightroom's crop that hold numbers.
pub const KEYS: [&str; 5] = [
    "CropLeft",
    "CropTop",
    "CropRight",
    "CropBottom",
    "CropAngle",
];

/// A crop key's number, none when it is not a finite one.
fn finite(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Whether the settings say there is no crop (`HasCrop` false).
fn says_none(words: &std::collections::BTreeMap<String, String>) -> bool {
    words
        .get("HasCrop")
        .is_some_and(|v| v.eq_ignore_ascii_case("false"))
}

impl Crop {
    /// Whether the settings hold a crop that does not read: one of its
    /// keys there and no finite number (`NaN`, `inf`, or no number at
    /// all), with `HasCrop` not saying there is none. That is a file
    /// written wrong, not an edge left at the file's own, and no
    /// rectangle can be told from it.
    pub fn unreadable(words: &std::collections::BTreeMap<String, String>) -> bool {
        !says_none(words)
            && KEYS
                .iter()
                .any(|k| words.get(*k).is_some_and(|v| finite(v).is_none()))
    }

    /// The crop the settings hold, none when they hold none: the
    /// catalog writes the edges only when the frame is cropped, an
    /// absent edge at the file's own. None too for a crop that does
    /// not read ([`Self::unreadable`]), which a caller with a report
    /// says.
    pub fn of(words: &std::collections::BTreeMap<String, String>) -> Option<Crop> {
        if Self::unreadable(words) {
            return None;
        }
        let num = |k: &str| words.get(k).and_then(|v| finite(v));
        let crop = Crop {
            left: num("CropLeft").unwrap_or(0.0),
            top: num("CropTop").unwrap_or(0.0),
            right: num("CropRight").unwrap_or(1.0),
            bottom: num("CropBottom").unwrap_or(1.0),
            angle: num("CropAngle").unwrap_or(0.0),
        };
        let none = Crop {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
            angle: 0.0,
        };
        (!says_none(words) && crop != none && crop.right > crop.left && crop.bottom > crop.top)
            .then_some(crop)
    }

    /// The angle in radians.
    pub fn radians(&self) -> f64 {
        self.angle.to_radians()
    }

    /// The cropped rectangle's width and height in pixels, for a file of
    /// `w` by `h` before any turn: the diagonal between the corners taken
    /// along the turned sides.
    pub fn size(&self, w: f64, h: f64) -> (f64, f64) {
        let (dx, dy) = ((self.right - self.left) * w, (self.bottom - self.top) * h);
        let (s, c) = self.radians().sin_cos();
        ((dx * c + dy * s).abs(), (-dx * s + dy * c).abs())
    }

    /// The rectangle's four corners in pixels of a file `w` by `h` in its
    /// own way up, clockwise from its own top left.
    pub fn corners(&self, w: f64, h: f64) -> [(f64, f64); 4] {
        let (cw, ch) = self.size(w, h);
        let (s, c) = self.radians().sin_cos();
        let p0 = (self.left * w, self.top * h);
        let p1 = (p0.0 + cw * c, p0.1 + cw * s);
        let p3 = (p0.0 - ch * s, p0.1 + ch * c);
        let p2 = (p1.0 - ch * s, p1.1 + ch * c);
        [p0, p1, p2, p3]
    }
}

/// What the crop is laid over: the frame as the catalog has it and as
/// this build develops it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// The develop's size before its tag turns it (the decoder's
    /// stance).
    pub size: (f64, f64),
    /// The catalog's size of the file before any turn
    /// (`fileWidth`, `fileHeight`), in which its shares were taken;
    /// none to take ours. The two differ by the few pixels of border
    /// one raw converter keeps and the other does not, and the shares
    /// are carried as shares of the frame.
    pub catalog: Option<(f64, f64)>,
    /// The file's orientation tag.
    pub tag: Orientation,
    /// The frame's own quarter turns clockwise over the tag (the
    /// sidecar's `turn`).
    pub turn: u8,
    /// Lightroom's orientation of the image (`orientation`), none when
    /// it does not say.
    pub lightroom: Option<Orientation>,
}

/// Why a crop could not be carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotPlaced {
    /// Lightroom's orientation mirrors the picture where the frame's
    /// does not, or the other way: no quarter turn takes one to the
    /// other.
    Mirrored,
    /// The frame has no size.
    NoSize,
    /// The crop lands nowhere a number can say: an edge so far out,
    /// on a turned frame, that the arithmetic runs past the floats.
    Unreadable,
}

/// A crop carried: the geometry's fields it sets, and whether it had to
/// be drawn in to stay on the picture (its corners past the turned
/// picture's edge by more than the geometry's own tolerance, as a crop
/// Lightroom let run onto the blank corners of a turned frame is).
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub turns: u8,
    pub angle: f32,
    pub crop: geometry::Crop,
    pub aspect: Aspect,
    pub portrait: bool,
    pub drawn_in: bool,
}

impl Placed {
    /// `g` with the crop laid on: its quarter turns, its angle, its crop
    /// and the shape it keeps; the mirror and the keystone as they were.
    pub fn applied(&self, g: &Geometry) -> Geometry {
        Geometry {
            turns: self.turns,
            angle: self.angle,
            crop: Some(self.crop),
            aspect: self.aspect,
            portrait: self.portrait,
            ..g.clone()
        }
    }
}

/// Lightroom's orientation code, as the catalog keeps it for each
/// image: the shown picture's corners named A to D clockwise from its
/// top left, and the two letters where the file's own top left and top
/// right land in it. `AB` is the file as it is; `DA`, its top edge
/// standing up the left side, is a quarter turn counter-clockwise
/// (`Rotate270`), which every frame tagged 8 in the catalogs we hold
/// has. The four with the letters the other way round are mirrored:
/// `BA` across, `DC` up and down, `AD` and `CB` about a diagonal. None
/// for anything else.
pub fn orientation_of(code: &str) -> Option<Orientation> {
    Some(match code.trim() {
        "AB" => Orientation::Normal,
        "BC" => Orientation::Rotate90,
        "CD" => Orientation::Rotate180,
        "DA" => Orientation::Rotate270,
        "BA" => Orientation::FlipHorizontal,
        "AD" => Orientation::Transpose,
        "DC" => Orientation::FlipVertical,
        "CB" => Orientation::Transverse,
        _ => return None,
    })
}

/// The edit's quarter turns, counter-clockwise, that stand a frame
/// shown as `shown` (its tag and its own turn) the way Lightroom's
/// orientation `lightroom` does; none when Lightroom's mirrors it where
/// the frame's does not, or the other way, which no quarter turn
/// undoes.
pub fn turns_for(shown: Orientation, lightroom: Orientation) -> Result<u8, NotPlaced> {
    let quarters = lightroom.turns_from(shown).ok_or(NotPlaced::Mirrored)?;
    Ok((4 - quarters) % 4)
}

/// Whether an orientation lays the picture on its side.
fn swaps(o: Orientation) -> bool {
    matches!(
        o,
        Orientation::Transpose
            | Orientation::Rotate90
            | Orientation::Transverse
            | Orientation::Rotate270
    )
}

/// Whether an orientation mirrors the picture.
pub fn mirrors(o: Orientation) -> bool {
    matches!(
        o,
        Orientation::FlipHorizontal
            | Orientation::FlipVertical
            | Orientation::Transpose
            | Orientation::Transverse
    )
}

/// A point of a frame `w` by `h` in its own way up, where it stands once
/// the frame is turned by `o`: the inverse of the develop's `orient`,
/// on points rather than pixels.
pub fn shown_point(o: Orientation, (x, y): (f64, f64), w: f64, h: f64) -> (f64, f64) {
    match o {
        Orientation::Normal => (x, y),
        Orientation::FlipHorizontal => (w - x, y),
        Orientation::Rotate180 => (w - x, h - y),
        Orientation::FlipVertical => (x, h - y),
        Orientation::Transpose => (y, x),
        Orientation::Rotate90 => (h - y, x),
        Orientation::Transverse => (h - y, w - x),
        Orientation::Rotate270 => (y, w - x),
    }
}

/// Lightroom's crop laid onto a frame as our geometry has it, with
/// `constrained` for Lightroom's lock on the crop's shape
/// (`CropConstrainAspectRatio`).
pub fn place(crop: &Crop, constrained: bool, frame: &Frame) -> Result<Placed, NotPlaced> {
    let (w, h) = frame.size;
    if w < 1.0 || h < 1.0 {
        return Err(NotPlaced::NoSize);
    }
    // The shares are of the catalog's frame, its angle in its pixels;
    // the corners scaled onto ours, which differ by a border at most.
    let (cw, ch) = frame
        .catalog
        .filter(|s| s.0 >= 1.0 && s.1 >= 1.0)
        .unwrap_or((w, h));
    let (kx, ky) = (w / cw, h / ch);
    let corners = crop.corners(cw, ch).map(|(x, y)| (x * kx, y * ky));
    let shown = frame.tag.turned(i32::from(frame.turn % 4));
    let (sw, sh) = if swaps(shown) { (h, w) } else { (w, h) };
    let wanted = frame.lightroom.unwrap_or(shown);
    let angle = if mirrors(shown) {
        -crop.angle
    } else {
        crop.angle
    };
    let mut g = Geometry {
        turns: turns_for(shown, wanted)?,
        angle: angle.clamp(
            -f64::from(geometry::MAX_ANGLE),
            f64::from(geometry::MAX_ANGLE),
        ) as f32,
        ..Geometry::default()
    };
    let (pw, ph) = g.plane_size(sw as f32, sh as f32);
    let plane = corners.map(|p| {
        let s = shown_point(shown, p, w, h);
        g.to_plane((s.0 as f32, s.1 as f32), sw as f32, sh as f32)
    });
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in plane {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let mut c = geometry::Crop {
        x: x0 / pw,
        y: y0 / ph,
        w: (x1 - x0) / pw,
        h: (y1 - y0) / ph,
    };
    if g.angle == 0.0 {
        // Unturned, an edge at the frame's own is exactly there.
        let (r, b) = ((c.x + c.w).min(1.0), (c.y + c.h).min(1.0));
        c.x = c.x.max(0.0);
        c.y = c.y.max(0.0);
        c.w = r - c.x;
        c.h = b - c.y;
    }
    let ratio = (c.w * pw) / (c.h * ph).max(1e-3);
    let drawn_in = !g.fits(&c, sw as f32, sh as f32);
    if drawn_in {
        // Lightroom lets a turned crop's corners run onto the blank
        // past the picture only when told not to hold it on the
        // picture; ours never does, so it is drawn in about its center
        // to the largest of its shape that stays on.
        c = g.largest_fit(c.center(), ratio, sw as f32, sh as f32);
    }
    g.crop = Some(c);
    // The shape it keeps: the frame's own when Lightroom held that one,
    // the crop's otherwise; free when Lightroom held none.
    let own = pw / ph;
    let (aspect, portrait) = if !constrained {
        (Aspect::Free, false)
    } else if ((ratio / own) - 1.0).abs() < 0.005 {
        (Aspect::Original, false)
    } else {
        // Landscape, with `portrait` to turn it: one of the panel's
        // when it is within half a percent of one, else its own to
        // three places.
        let wide = if ratio >= 1.0 { ratio } else { 1.0 / ratio };
        let named = geometry::PRESETS.iter().find_map(|(_, a)| match *a {
            Aspect::Ratio { w, h } if ((w / h) / wide - 1.0).abs() < 0.005 => Some(*a),
            _ => None,
        });
        (
            named.unwrap_or(Aspect::Ratio {
                w: (wide * 1000.0).round() / 1000.0,
                h: 1.0,
            }),
            ratio < 1.0,
        )
    };
    // Every number the geometry takes, finite: one that is not writes
    // a sidecar that does not read back.
    let ratio_finite = match aspect {
        Aspect::Ratio { w, h } => w.is_finite() && h.is_finite(),
        _ => true,
    };
    if ![g.angle, c.x, c.y, c.w, c.h].iter().all(|v| v.is_finite()) || !ratio_finite {
        return Err(NotPlaced::Unreadable);
    }
    Ok(Placed {
        turns: g.turns,
        angle: g.angle,
        crop: c,
        aspect,
        portrait,
        drawn_in,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plane's crop in pixels: origin and size.
    fn pixels(p: &Placed, shown: (f32, f32)) -> (f32, f32, f32, f32) {
        let g = p.applied(&Geometry::default());
        let (pw, ph) = g.plane_size(shown.0, shown.1);
        (p.crop.x * pw, p.crop.y * ph, p.crop.w * pw, p.crop.h * ph)
    }

    fn frame(size: (f64, f64), tag: Orientation) -> Frame {
        Frame {
            size,
            catalog: None,
            tag,
            turn: 0,
            lightroom: None,
        }
    }

    fn near(a: f32, b: f32, by: f32) -> bool {
        (a - b).abs() <= by
    }

    #[test]
    fn an_unturned_crop_on_an_untagged_frame_is_its_rectangle() {
        let crop = Crop {
            left: 0.1,
            top: 0.2,
            right: 0.7,
            bottom: 0.9,
            angle: 0.0,
        };
        let p = place(&crop, false, &frame((6000.0, 4000.0), Orientation::Normal)).unwrap();
        assert_eq!((p.turns, p.angle), (0, 0.0));
        let (x, y, w, h) = pixels(&p, (6000.0, 4000.0));
        assert!(near(x, 600.0, 0.5) && near(y, 800.0, 0.5), "{x} {y}");
        assert!(near(w, 3600.0, 0.5) && near(h, 2800.0, 0.5), "{w} {h}");
        assert_eq!(p.aspect, Aspect::Free);
        assert!(!p.drawn_in);
    }

    #[test]
    fn a_tagged_frame_takes_the_rectangle_the_way_it_is_shown() {
        // A frame tagged 8 (the camera turned left) is shown on its
        // side: its own right edge on top. The file's rectangle from
        // (600, 800) to (4200, 3600) of 6000 by 4000 is shown at
        // (800, 6000 - 4200) to (3600, 6000 - 600).
        let crop = Crop {
            left: 0.1,
            top: 0.2,
            right: 0.7,
            bottom: 0.9,
            angle: 0.0,
        };
        let p = place(
            &crop,
            false,
            &frame((6000.0, 4000.0), Orientation::Rotate270),
        )
        .unwrap();
        assert_eq!(p.turns, 0);
        let (x, y, w, h) = pixels(&p, (4000.0, 6000.0));
        assert!(near(x, 800.0, 0.5) && near(y, 1800.0, 0.5), "{x} {y}");
        assert!(near(w, 2800.0, 0.5) && near(h, 3600.0, 0.5), "{w} {h}");
        // The other way round, tagged 6, the left edge on top.
        let p = place(
            &crop,
            false,
            &frame((6000.0, 4000.0), Orientation::Rotate90),
        )
        .unwrap();
        let (x, y, w, h) = pixels(&p, (4000.0, 6000.0));
        assert!(
            near(x, 4000.0 - 3600.0, 0.5) && near(y, 600.0, 0.5),
            "{x} {y}"
        );
        assert!(near(w, 2800.0, 0.5) && near(h, 3600.0, 0.5), "{w} {h}");
    }

    #[test]
    fn a_turned_crop_is_the_turned_rectangle_level_in_the_plane() {
        // The catalog's own: a 6000 by 4000 frame at 1.1 degrees, whose
        // cropped size Lightroom keeps as 5201 by 3901.
        let crop = Crop {
            left: 0.066054,
            top: 1e-6,
            right: 0.920273,
            bottom: 1.0,
            angle: 1.1,
        };
        let p = place(&crop, true, &frame((6000.0, 4000.0), Orientation::Normal)).unwrap();
        assert_eq!(p.angle, 1.1);
        let (_, _, w, h) = pixels(&p, (6000.0, 4000.0));
        assert!(near(w, 5201.0, 1.0) && near(h, 3901.0, 1.0), "{w} {h}");
        // Each corner of Lightroom's rectangle lands on a corner of
        // ours: the plane's corners taken back to the file are the
        // rectangle's.
        let g = p.applied(&Geometry::default());
        let (x, y, w, h) = pixels(&p, (6000.0, 4000.0));
        let ours = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
            .map(|q| g.to_source(q, 6000.0, 4000.0));
        for (a, b) in ours.iter().zip(crop.corners(6000.0, 4000.0)) {
            assert!(
                near(a.0, b.0 as f32, 1.0) && near(a.1, b.1 as f32, 1.0),
                "{a:?} {b:?}"
            );
        }
        // Held to its shape in Lightroom, which is not the frame's:
        // 4:3 within its rounding, one of the panel's.
        assert_eq!(p.aspect, Aspect::Ratio { w: 4.0, h: 3.0 });
        assert!(!p.portrait);
        // A tall shape of none of the panel's is its own, turned.
        let tall = Crop {
            left: 0.3,
            top: 0.0,
            right: 0.7,
            bottom: 1.0,
            angle: 0.0,
        };
        let p = place(&tall, true, &frame((6000.0, 4000.0), Orientation::Normal)).unwrap();
        assert_eq!(p.aspect, Aspect::Ratio { w: 1.667, h: 1.0 });
        assert!(p.portrait);
    }

    #[test]
    fn a_turned_crop_on_a_frame_on_its_side_keeps_its_corners() {
        // Tagged 8, 2 degrees: every corner of the file's rectangle,
        // shown, is a corner of the plane's crop taken back.
        let crop = Crop {
            left: 0.08,
            top: 0.05,
            right: 0.9,
            bottom: 0.93,
            angle: 2.0,
        };
        for tag in [
            Orientation::Rotate270,
            Orientation::Rotate90,
            Orientation::Rotate180,
            Orientation::FlipHorizontal,
            Orientation::Transpose,
        ] {
            let f = frame((6000.0, 4000.0), tag);
            let p = place(&crop, false, &f).unwrap();
            let shown = if swaps(tag) {
                (4000.0, 6000.0)
            } else {
                (6000.0, 4000.0)
            };
            let g = p.applied(&Geometry::default());
            let (x, y, w, h) = pixels(&p, shown);
            let ours: Vec<(f32, f32)> = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
                .iter()
                .map(|&q| g.to_source(q, shown.0, shown.1))
                .collect();
            for c in crop.corners(6000.0, 4000.0) {
                let s = shown_point(tag, c, 6000.0, 4000.0);
                assert!(
                    ours.iter()
                        .any(|o| near(o.0, s.0 as f32, 1.0) && near(o.1, s.1 as f32, 1.0)),
                    "{tag:?}: {s:?} not among {ours:?}"
                );
            }
            let expected = if mirrors(tag) { -2.0 } else { 2.0 };
            assert_eq!(p.angle, expected, "{tag:?}");
        }
    }

    #[test]
    fn a_turn_in_lightroom_becomes_the_edit_s_quarter_turns() {
        // The file untagged, turned right in Lightroom (`BC`): the
        // export stands on its side, the file's left edge on top. A
        // point of the file lands in our plane where Lightroom's
        // orientation puts it.
        let crop = Crop {
            left: 0.1,
            top: 0.2,
            right: 0.7,
            bottom: 0.9,
            angle: 0.0,
        };
        let f = Frame {
            lightroom: orientation_of("BC"),
            ..frame((6000.0, 4000.0), Orientation::Normal)
        };
        let p = place(&crop, false, &f).unwrap();
        assert_eq!(p.turns, 3, "a quarter clockwise is three of ours");
        let g = p.applied(&Geometry::default());
        let (x, y, w, h) = pixels(&p, (6000.0, 4000.0));
        assert!(near(w, 2800.0, 0.5) && near(h, 3600.0, 0.5), "{w} {h}");
        let point = (1000.0, 1000.0);
        let want = shown_point(Orientation::Rotate90, point, 6000.0, 4000.0);
        let got = g.to_plane((point.0 as f32, point.1 as f32), 6000.0, 4000.0);
        assert!(
            near(got.0, want.0 as f32, 0.5) && near(got.1, want.1 as f32, 0.5),
            "{got:?} {want:?}"
        );
        // And the crop's corner is the shown rectangle's.
        let corner = shown_point(Orientation::Rotate90, (600.0, 3600.0), 6000.0, 4000.0);
        assert!(near(x, corner.0 as f32, 0.5) && near(y, corner.1 as f32, 0.5));
        // A Lightroom turn agreeing with the tag adds none.
        let f = Frame {
            lightroom: orientation_of("DA"),
            ..frame((6000.0, 4000.0), Orientation::Rotate270)
        };
        assert_eq!(place(&crop, false, &f).unwrap().turns, 0);
        // A mirror Lightroom has and the frame has not is refused.
        let f = Frame {
            lightroom: Some(Orientation::FlipHorizontal),
            ..frame((6000.0, 4000.0), Orientation::Normal)
        };
        assert_eq!(place(&crop, false, &f), Err(NotPlaced::Mirrored));
    }

    #[test]
    fn the_shares_are_of_the_catalog_s_frame_and_land_on_ours() {
        // Lightroom's 6000 by 4000 against our 6024 by 4016 of the same
        // sensor: the crop's middle is the frame's middle either way.
        let crop = Crop {
            left: 0.25,
            top: 0.25,
            right: 0.75,
            bottom: 0.75,
            angle: 0.0,
        };
        let f = Frame {
            catalog: Some((6000.0, 4000.0)),
            ..frame((6024.0, 4016.0), Orientation::Normal)
        };
        let p = place(&crop, true, &f).unwrap();
        let (x, y, w, h) = pixels(&p, (6024.0, 4016.0));
        assert!(near(x + w / 2.0, 3012.0, 0.5) && near(y + h / 2.0, 2008.0, 0.5));
        assert!(near(w, 3012.0, 0.5) && near(h, 2008.0, 0.5));
        // Its shape the frame's own: held as Original.
        assert_eq!(p.aspect, Aspect::Original);
    }

    #[test]
    fn a_crop_past_the_turned_picture_is_drawn_in_to_stay_on_it() {
        // The whole frame at 5 degrees: its corners are off the picture,
        // which ours never shows, so it is drawn in about its center.
        let crop = Crop {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
            angle: 5.0,
        };
        let f = frame((6000.0, 4000.0), Orientation::Normal);
        let p = place(&crop, false, &f).unwrap();
        assert!(p.drawn_in);
        let g = p.applied(&Geometry::default());
        assert!(g.fits(&p.crop, 6000.0, 4000.0));
        assert_eq!(g.effective_crop(6000.0, 4000.0), p.crop);
    }

    /// An edge finite but far past the frame, on a turned crop, runs
    /// the arithmetic past the floats: refused, not placed as `NaN`.
    #[test]
    fn a_crop_past_the_floats_is_not_placed() {
        let crop = Crop {
            left: 0.0,
            top: 0.0,
            right: 1e300,
            bottom: 0.9,
            angle: 5.0,
        };
        let f = frame((6000.0, 4000.0), Orientation::Normal);
        for constrained in [false, true] {
            assert_eq!(
                place(&crop, constrained, &f),
                Err(NotPlaced::Unreadable),
                "{constrained}"
            );
        }
    }

    /// A crop key there and no finite number is a crop that does not
    /// read, not an edge at the file's own; `HasCrop` false is still no
    /// crop.
    #[test]
    fn a_crop_key_that_is_no_number_is_no_crop_and_says_so() {
        let words = |pairs: &[(&str, &str)]| -> std::collections::BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        for v in ["nan", "NaN", "inf", "-inf", "infinity", "wide"] {
            for key in KEYS {
                let w = words(&[("CropLeft", "0.1"), ("CropTop", "0.1"), (key, v)]);
                assert!(Crop::unreadable(&w), "{key} {v}");
                assert_eq!(Crop::of(&w), None, "{key} {v}");
            }
            let off = words(&[("HasCrop", "False"), ("CropAngle", v)]);
            assert!(!Crop::unreadable(&off), "{v}");
            assert_eq!(Crop::of(&off), None, "{v}");
        }
        let fine = words(&[("CropLeft", "0.1"), ("CropAngle", "2")]);
        assert!(!Crop::unreadable(&fine));
        assert!(Crop::of(&fine).is_some());
    }

    #[test]
    fn no_size_is_no_crop() {
        let crop = Crop {
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
            angle: 0.0,
        };
        assert_eq!(
            place(&crop, false, &frame((0.0, 0.0), Orientation::Normal)),
            Err(NotPlaced::NoSize)
        );
    }

    #[test]
    fn the_four_codes_read_and_others_do_not() {
        assert_eq!(orientation_of("AB"), Some(Orientation::Normal));
        assert_eq!(orientation_of("BC"), Some(Orientation::Rotate90));
        assert_eq!(orientation_of("CD"), Some(Orientation::Rotate180));
        assert_eq!(orientation_of("DA"), Some(Orientation::Rotate270));
        assert_eq!(orientation_of("BA"), Some(Orientation::FlipHorizontal));
        assert_eq!(orientation_of("AD"), Some(Orientation::Transpose));
        assert_eq!(orientation_of("DC"), Some(Orientation::FlipVertical));
        assert_eq!(orientation_of("CB"), Some(Orientation::Transverse));
        assert_eq!(orientation_of("XY"), None);
        assert_eq!(orientation_of(""), None);
        // Each code read as the corners it names: the file's top left
        // and top right land where its letters are in the shown frame.
        let corners = |o: Orientation| {
            let (w, h) = (6.0, 4.0);
            let (sw, sh) = if swaps(o) { (h, w) } else { (w, h) };
            let named = |p: (f64, f64)| match (p.0 == 0.0, p.1 == 0.0) {
                (true, true) => 'A',
                (false, true) => 'B',
                (false, false) => 'C',
                (true, false) => 'D',
            };
            let at = |p: (f64, f64)| {
                let s = shown_point(o, p, w, h);
                named(((s.0 / sw).round(), (s.1 / sh).round()))
            };
            format!("{}{}", at((0.0, 0.0)), at((w, 0.0)))
        };
        for code in ["AB", "BC", "CD", "DA", "BA", "AD", "DC", "CB"] {
            assert_eq!(corners(orientation_of(code).unwrap()), code);
        }
    }
}
