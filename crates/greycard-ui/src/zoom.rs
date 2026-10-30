//! What the viewport shows a picture at: the zoom asked for, or the
//! fit that brings the whole of it into the room it has, and how
//! much room that is when the compare view has laid several out.
//!
//! Pure, and here rather than in the viewport's callbacks, because
//! the fit is read in six places a frame — the pan, the wheel, the
//! crop's drag, the mask handles, the readout — and every one of
//! them has to come out with the same number.

use crate::cull;

/// The part of a `vw` by `vh` view one picture has: all of it, or
/// its tile of a compare view of `compare` frames. The tiles are all
/// one size, so the first one's is every one's.
pub fn cell(vw: u32, vh: u32, compare: usize) -> (u32, u32) {
    if compare > 1 {
        let (_, _, w, h) = cull::tile_rects(vw, vh, compare)[0];
        (w, h)
    } else {
        (vw, vh)
    }
}

/// The mat's band on each side of a fitted picture, as a share of
/// the view's shorter side.
pub const MAT: f32 = 0.06;

/// The room a fit has inside the mat: a `cell` less the band on each
/// side, the band a share of its shorter side. Only the fit reads
/// it; a zoom asked for is the zoom, and fills the view.
pub fn matted(cell: (u32, u32)) -> (u32, u32) {
    let band = (cell.0.min(cell.1) as f32 * MAT).round() as u32;
    (
        cell.0.saturating_sub(2 * band).max(1),
        cell.1.saturating_sub(2 * band).max(1),
    )
}

/// The zoom in display pixels per image pixel: `asked` when it is
/// one, else the fit of an `image` into a `cell`.
///
/// A fit never enlarges. A picture smaller than the cell is shown at
/// 1:1 with room around it rather than blown up, which is what "Fit"
/// means everywhere else and what keeps a thumbnail-sized frame from
/// arriving as a wall of soft pixels.
pub fn effective(asked: f32, cell: (u32, u32), image: (u32, u32)) -> f32 {
    if asked > 0.0 {
        return asked;
    }
    let (cw, ch) = (cell.0 as f32, cell.1 as f32);
    let (iw, ih) = (image.0 as f32, image.1 as f32);
    (cw / iw).min(ch / ih).min(1.0)
}

/// One axis of a touchpad's pan: the view's center along a `frame`
/// of image pixels, `shown` of them in view, moved `by` image pixels
/// the way the content goes (so the center goes the other).
///
/// The pan stops at the frame's edge rather than run the picture off
/// the view, and where the whole frame is in view it has nowhere to
/// go and the center stays. A center already past the edge, as a
/// drag can leave it, is never pulled back in a jump: the wheel only
/// stops it going further out.
pub fn wheel_pan(center: f32, by: f32, shown: f32, frame: f32) -> f32 {
    if shown >= frame {
        return center;
    }
    let (low, high) = (shown / 2.0, frame - shown / 2.0);
    (center - by).clamp(center.min(low), center.max(high))
}

/// Whether a point of a `view` (physical pixels from its top left)
/// lies on a picture of `image` pixels shown at `zoom` with its pixel
/// `center` in the middle of the view, rather than on the canvas
/// round it. A picture zoomed past the view covers all of it; a fit,
/// or a zoom out, leaves canvas on two sides or all four.
pub fn on_picture(
    point: (f32, f32),
    view: (u32, u32),
    zoom: f32,
    center: (f32, f32),
    image: (u32, u32),
) -> bool {
    let at = |p: f32, v: u32, c: f32| (p - v as f32 / 2.0) / zoom + c;
    let (u, v) = (at(point.0, view.0, center.0), at(point.1, view.1, center.1));
    (0.0..=image.0 as f32).contains(&u) && (0.0..=image.1 as f32).contains(&v)
}

/// Whether any of a picture placed as [`on_picture`] places it shows
/// in the `view`, or a drag has carried all of it off.
pub fn in_view(view: (u32, u32), zoom: f32, center: (f32, f32), image: (u32, u32)) -> bool {
    let shows = |v: u32, c: f32, i: u32| {
        let low = v as f32 / 2.0 - c * zoom;
        i > 0 && low < v as f32 && low + i as f32 * zoom > 0.0
    };
    shows(view.0, center.0, image.0) && shows(view.1, center.1, image.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wheel_pan_stops_at_the_edge_and_is_still_when_the_frame_fits() {
        // 6000 wide, 1000 in view: the center lives in 500..5500.
        assert_eq!(wheel_pan(3000.0, 100.0, 1000.0, 6000.0), 2900.0);
        assert_eq!(wheel_pan(3000.0, -100.0, 1000.0, 6000.0), 3100.0);
        assert_eq!(wheel_pan(600.0, 400.0, 1000.0, 6000.0), 500.0);
        assert_eq!(wheel_pan(5400.0, -400.0, 1000.0, 6000.0), 5500.0);
        // The whole frame in view: nowhere to go.
        assert_eq!(wheel_pan(3000.0, 100.0, 6000.0, 6000.0), 3000.0);
        assert_eq!(wheel_pan(3000.0, 100.0, 9000.0, 6000.0), 3000.0);
        // Dragged out past the edge: back in is free, further out is
        // held where it is, and nothing jumps.
        assert_eq!(wheel_pan(200.0, 100.0, 1000.0, 6000.0), 200.0);
        assert_eq!(wheel_pan(200.0, -100.0, 1000.0, 6000.0), 300.0);
    }

    #[test]
    fn a_point_is_on_the_picture_inside_its_placed_rectangle_only() {
        // 6000 by 4000 fitted into 1000 by 800 at a sixth: 1000 by
        // 667 in the middle, a band of canvas above and below.
        let fit = 1.0 / 6.0;
        let on = |x, y| on_picture((x, y), (1000, 800), fit, (3000.0, 2000.0), (6000, 4000));
        assert!(on(500.0, 400.0));
        assert!(on(5.0, 100.0) && on(995.0, 700.0));
        assert!(!on(500.0, 50.0), "the canvas above");
        assert!(!on(500.0, 760.0), "the canvas below");
        // Zoomed in, the picture covers the view: every point is on it.
        for (x, y) in [(0.0, 0.0), (999.0, 0.0), (0.0, 799.0), (999.0, 799.0)] {
            assert!(on_picture(
                (x, y),
                (1000, 800),
                1.0,
                (3000.0, 2000.0),
                (6000, 4000)
            ));
        }
        // Panned to the picture's right edge, the canvas shows past it.
        let edge = |x| on_picture((x, 400.0), (1000, 800), 1.0, (5800.0, 2000.0), (6000, 4000));
        assert!(edge(600.0) && !edge(800.0));
        // A small picture at 1:1 sits in the middle with canvas round it.
        let small = |x, y| on_picture((x, y), (1000, 800), 1.0, (200.0, 150.0), (400, 300));
        assert!(small(500.0, 400.0) && small(301.0, 251.0));
        assert!(!small(250.0, 400.0) && !small(500.0, 600.0));

        // Any of the picture in view, or none: a fit, a zoom, a pan to
        // the edge with canvas past it, and one dragged clean off.
        assert!(in_view((1000, 800), fit, (3000.0, 2000.0), (6000, 4000)));
        assert!(in_view((1000, 800), 1.0, (5800.0, 2000.0), (6000, 4000)));
        assert!(in_view((1000, 800), 1.0, (200.0, 150.0), (400, 300)));
        assert!(!in_view((1000, 800), 1.0, (-600.0, 150.0), (400, 300)));
        assert!(!in_view((1000, 800), 1.0, (200.0, 800.0), (400, 300)));
        assert!(!in_view((1000, 800), 1.0, (0.0, 0.0), (0, 0)));
    }

    #[test]
    fn a_fit_brings_the_whole_picture_in_and_never_enlarges() {
        // The long side decides: a wide picture in a square cell
        // fits by its width.
        assert!((effective(0.0, (1000, 1000), (6000, 4000)) - 1.0 / 6.0).abs() < 1e-6);
        assert!((effective(0.0, (1000, 1000), (4000, 6000)) - 1.0 / 6.0).abs() < 1e-6);
        // Smaller than the cell: 1:1 with room around it.
        assert_eq!(effective(0.0, (1000, 1000), (400, 300)), 1.0);
        // A zoom asked for is the zoom, whatever would have fitted.
        assert_eq!(effective(2.0, (1000, 1000), (6000, 4000)), 2.0);
        assert_eq!(effective(0.25, (10, 10), (6000, 4000)), 0.25);

        // One frame has the whole view; a compare view of four gives
        // it a quarter of it, less the gaps, and the fit follows.
        assert_eq!(cell(1000, 800, 1), (1000, 800));
        assert_eq!(cell(1000, 800, 0), (1000, 800));
        let (cw, ch) = cell(1000, 800, 4);
        assert!(cw < 1000 / 2 + 1 && ch < 800 / 2 + 1, "{cw} by {ch}");
        assert!(
            effective(0.0, (cw, ch), (6000, 4000)) < effective(0.0, (1000, 800), (6000, 4000)),
            "a tile fits smaller than the whole view"
        );
    }

    #[test]
    fn the_mat_takes_a_band_off_each_side_of_the_fit() {
        // 6% of the shorter side, 800: a band of 48 all round.
        assert_eq!(matted((1000, 800)), (904, 704));
        // A wide picture fits the narrower room by its width.
        let fit = effective(0.0, matted((1000, 800)), (6000, 4000));
        assert!((fit - 904.0 / 6000.0).abs() < 1e-6);
        assert!(fit < effective(0.0, (1000, 800), (6000, 4000)));
        // A zoom asked for fills the view whatever the mat.
        assert_eq!(effective(1.0, matted((1000, 800)), (6000, 4000)), 1.0);
        // A view too small for the band keeps a pixel of room.
        assert_eq!(matted((10, 1)), (10, 1));
        assert_eq!(matted((1, 1)), (1, 1));
    }
}
