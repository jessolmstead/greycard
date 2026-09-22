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

#[cfg(test)]
mod tests {
    use super::*;

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
}
