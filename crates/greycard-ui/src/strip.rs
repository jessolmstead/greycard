//! The filmstrip's arithmetic: each frame's cell its picture's shape
//! at the row's height, never narrower than its name, the cells side
//! by side with a gap between and the pad at either end; where a cell
//! is, what a scroll shows, the scroll that shows a frame, and the
//! scroll that keeps the view still when shapes come in after the
//! layout.
//!
//! The window draws the cells where this puts them, asked through its
//! `strip-place` and the rest, so the numbers are tested here rather
//! than left in a binding.

use crate::grid::UNKNOWN_ASPECT;

/// The padding at the strip's ends and above its cells, logical
/// pixels: the theme's `space-3`.
pub const PAD: f32 = 12.0;
/// The gap between two cells: the theme's `space-2`, room to breathe
/// between pictures that have no padding of their own.
pub const GAP: f32 = 8.0;
/// How far a picture is inset from its cell's edges: the selection
/// ring's width, so the ring and the hover's ground hug the picture.
pub const RING: f32 = 2.0;
/// The pictures' height, the row's, until a drag on the strip's top
/// edge sets another: the default the window starts at and a
/// double-click on the edge goes back to.
pub const PHOTO: f32 = 91.0;
/// The least the pictures' height is dragged to, so the names under
/// them stay readable.
pub const LEAST_PHOTO: f32 = 60.0;
/// The narrowest and the widest a picture is drawn, width over height:
/// a frame past either is fitted inside a cell of that shape, so a
/// sliver keeps a cell that can be clicked and a panorama does not
/// take the strip.
pub const NARROWEST: f32 = 0.5;
pub const WIDEST: f32 = 3.0;
/// The narrowest a cell is, so a camera's twelve-character name shows
/// whole under a portrait: "4Z4A2978.CR3" and "DSCF0438.RAF" are 74 px
/// wide in the theme's 11 px Noto Sans, "_DSC0001.ARW" 78; 80 leaves
/// room for a wider system face, and the floor leaves the theme's
/// `space-2` of slack either side of that (the name has no padding of
/// its own). A picture narrower sits centered in its cell.
pub const FLOOR: f32 = 96.0;

/// A picture's width for `aspect`, height over width as the grid's
/// shapes are (zero or less for not known, which is laid out as most
/// cameras' 3:2 on its side): its width at the row's height `photo`,
/// held to [`NARROWEST`] and [`WIDEST`], in whole pixels.
pub fn photo_width(aspect: f32, photo: f32) -> f32 {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        UNKNOWN_ASPECT
    };
    let wide = (1.0 / aspect).clamp(NARROWEST, WIDEST);
    (photo * wide).round()
}

/// The long edge a strip whose pictures are `photo` high asks the
/// thumbnail pool for, as the grid asks for its cells': a landscape's
/// width at that height, most frames' longest, and never less than the
/// default strip's [`THUMB_WIDTH`](crate::worker::THUMB_WIDTH), so a
/// shorter strip keeps the pictures it has.
pub fn render_size(photo: f32) -> u32 {
    crate::grid::render_size(photo / UNKNOWN_ASPECT).max(crate::worker::THUMB_WIDTH)
}

/// A cell's width for a picture of `aspect`: the picture's
/// [`photo_width`] and the ring either side, or [`FLOOR`] when that is
/// wider.
pub fn cell_width(aspect: f32, photo: f32) -> f32 {
    (photo_width(aspect, photo) + 2.0 * RING).max(FLOOR)
}

/// The cells of a strip: each one's left edge along the row, from the
/// row's start, its width, and its picture's.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Layout {
    lefts: Vec<f32>,
    widths: Vec<f32>,
    photos: Vec<f32>,
}

impl Layout {
    /// `count` cells, row `i`'s picture of shape `aspect(i)`, the
    /// pictures `photo` high.
    pub fn new(count: usize, photo: f32, aspect: impl Fn(usize) -> f32) -> Self {
        let mut lefts = Vec::with_capacity(count);
        let mut widths = Vec::with_capacity(count);
        let mut photos = Vec::with_capacity(count);
        let mut x = PAD;
        for i in 0..count {
            let a = aspect(i);
            let w = cell_width(a, photo);
            lefts.push(x);
            widths.push(w);
            photos.push(photo_width(a, photo));
            x += w + GAP;
        }
        Layout {
            lefts,
            widths,
            photos,
        }
    }

    /// How many cells.
    // Public for the binary; nobody asks whether a strip is empty.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.lefts.len()
    }

    /// Row `row`'s cell: its left edge and its width.
    pub fn place(&self, row: i32) -> Option<(f32, f32)> {
        let r = usize::try_from(row).ok()?;
        Some((*self.lefts.get(r)?, self.widths[r]))
    }

    /// Row `row`'s picture in its cell: how far its ring sits in from
    /// the cell's left edge, none when the picture fills the cell, else
    /// half what the floor leaves, in whole pixels so the ring stays
    /// sharp; and the picture's width inside the ring.
    pub fn photo(&self, row: i32) -> Option<(f32, f32)> {
        let r = usize::try_from(row).ok()?;
        let (w, p) = (*self.widths.get(r)?, self.photos[r]);
        Some((((w - p - 2.0 * RING) / 2.0).floor(), p))
    }

    /// The row's length: the last cell's right edge and the pad.
    pub fn length(&self) -> f32 {
        match (self.lefts.last(), self.widths.last()) {
            (Some(l), Some(w)) => l + w + PAD,
            _ => 2.0 * PAD,
        }
    }

    /// How far a view `view` wide scrolls along the row.
    pub fn max_scroll(&self, view: f32) -> f32 {
        (self.length() - view).max(0.0)
    }

    /// The rows a view `view` wide scrolled `offset` along shows any of,
    /// first and last; none for an empty strip.
    pub fn shown(&self, offset: f32, view: f32) -> Option<(i32, i32)> {
        let n = self.len();
        if n == 0 {
            return None;
        }
        // The last cell starting at or before the view's left edge,
        // or the next when the edge is in the gap after it.
        let k = self.lefts.partition_point(|&l| l <= offset).max(1) - 1;
        let first = if self.lefts[k] + self.widths[k] <= offset {
            (k + 1).min(n - 1)
        } else {
            k
        };
        // The last cell starting short of the right edge.
        let last = self
            .lefts
            .partition_point(|&l| l < offset + view)
            .max(1)
            .saturating_sub(1)
            .max(first);
        Some((first as i32, last as i32))
    }

    /// The least scroll from `offset` that shows row `row` whole in a
    /// view `view` wide, the pad beside it, held to the row's range.
    pub fn reveal(&self, offset: f32, view: f32, row: i32) -> f32 {
        let Some((left, width)) = self.place(row) else {
            return offset.clamp(0.0, self.max_scroll(view));
        };
        let right = left + width;
        let wanted = if left - PAD < offset {
            left - PAD
        } else if right + PAD > offset + view {
            right + PAD - view
        } else {
            offset
        };
        wanted.clamp(0.0, self.max_scroll(view))
    }
}

/// The scroll that keeps a view `view` wide still, scrolled `scroll`
/// along `before`, now the shapes have made it `after`. The selected
/// frame on screen holds its place, else the first frame on screen,
/// so a cell off the left edge that grows or shrinks moves nothing in
/// view; a selected frame that was whole is shown whole again if its
/// own cell grew past an edge. A strip at its start stays there.
pub fn held(before: &Layout, after: &Layout, scroll: f32, view: f32, selected: i32) -> f32 {
    let Some((first, last)) = before.shown(scroll, view) else {
        return 0.0;
    };
    let on_screen = (first..=last).contains(&selected);
    let whole = on_screen && before.reveal(scroll, view, selected) == scroll;
    let anchor = if on_screen { selected } else { first };
    let moved = match (before.place(anchor), after.place(anchor)) {
        (Some((was, _)), Some((now, _))) if scroll > 0.0 => scroll + now - was,
        _ => scroll,
    };
    if whole {
        after.reveal(moved, view, selected)
    } else {
        moved.clamp(0.0, after.max_scroll(view))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANDSCAPE: f32 = 2.0 / 3.0;
    const PORTRAIT: f32 = 1.5;

    #[test]
    fn a_cell_takes_its_pictures_shape_at_the_rows_height() {
        // 3:2 on its side: 91 high is 136.5 wide, 137 in whole pixels,
        // and the ring either side, wider than the floor.
        assert_eq!(photo_width(LANDSCAPE, PHOTO), 137.0);
        assert_eq!(cell_width(LANDSCAPE, PHOTO), 137.0 + 4.0);
        // On its end, 61, and a square, 91: the floor.
        assert_eq!(photo_width(PORTRAIT, PHOTO), 61.0);
        assert_eq!(cell_width(PORTRAIT, PHOTO), FLOOR);
        assert_eq!(photo_width(1.0, PHOTO), 91.0);
        assert_eq!(cell_width(1.0, PHOTO), FLOOR);
        // Not known yet: laid out as the landscape.
        assert_eq!(cell_width(0.0, PHOTO), cell_width(LANDSCAPE, PHOTO));
        assert_eq!(cell_width(f32::NAN, PHOTO), cell_width(LANDSCAPE, PHOTO));
        // A panorama and a sliver are held, the sliver in the floor.
        assert_eq!(cell_width(0.1, PHOTO), 273.0 + 4.0);
        assert_eq!(photo_width(10.0, PHOTO), 46.0);
        assert_eq!(cell_width(10.0, PHOTO), FLOOR);
        // A taller row, a wider picture at its height.
        assert_eq!(photo_width(LANDSCAPE, 180.0), 270.0);
        assert_eq!(cell_width(PORTRAIT, 180.0), 120.0 + 4.0);
    }

    /// The pool's long edge follows the row's height up past the
    /// default's, and never below it.
    #[test]
    fn a_taller_strip_asks_for_larger_pictures() {
        assert_eq!(render_size(PHOTO), crate::worker::THUMB_WIDTH);
        assert_eq!(render_size(LEAST_PHOTO), crate::worker::THUMB_WIDTH);
        assert_eq!(render_size(160.0), 240);
        assert_eq!(render_size(400.0), crate::grid::MAX_RENDER);
    }

    #[test]
    fn a_picture_narrower_than_the_floor_sits_centered_in_its_cell() {
        let shapes = [PORTRAIT, LANDSCAPE, 1.0, 10.0];
        let l = Layout::new(shapes.len(), PHOTO, |i| shapes[i]);
        // 61 and the ring in 96: 15.5 a side, 15 in whole pixels.
        assert_eq!(l.photo(0), Some((15.0, 61.0)));
        // A landscape fills its cell.
        assert_eq!(l.photo(1), Some((0.0, 137.0)));
        // A square's ring is 95 of the 96.
        assert_eq!(l.photo(2), Some((0.0, 91.0)));
        assert_eq!(l.photo(3), Some((23.0, 46.0)));
        assert_eq!(l.photo(4), None);
        assert_eq!(l.photo(-1), None);
    }

    #[test]
    fn the_cells_run_from_the_pad_with_a_gap_between() {
        let shapes = [LANDSCAPE, PORTRAIT, 1.0, 0.0];
        let l = Layout::new(shapes.len(), PHOTO, |i| shapes[i]);
        assert_eq!(l.place(0), Some((12.0, 141.0)));
        assert_eq!(l.place(1), Some((12.0 + 141.0 + 8.0, 96.0)));
        assert_eq!(l.place(2), Some((161.0 + 96.0 + 8.0, 96.0)));
        assert_eq!(l.place(3), Some((265.0 + 96.0 + 8.0, 141.0)));
        assert_eq!(l.place(4), None);
        assert_eq!(l.place(-1), None);
        assert_eq!(l.length(), 369.0 + 141.0 + 12.0);
        // Empty: the pads alone.
        let empty = Layout::new(0, PHOTO, |_| 0.0);
        assert_eq!(empty.length(), 24.0);
        assert_eq!(empty.shown(0.0, 1500.0), None);
        assert_eq!(empty.reveal(40.0, 1500.0, 0), 0.0);
    }

    #[test]
    fn a_scroll_shows_the_cells_it_touches() {
        // Portraits, the floor's 96 wide at a pitch of 104 from 12.
        let l = Layout::new(100, PHOTO, |_| PORTRAIT);
        assert_eq!(l.shown(0.0, 1500.0), Some((0, 14)));
        // The left edge in the gap after row 1: row 2 is the first.
        assert_eq!(l.shown(12.0 + 104.0 + 97.0, 300.0).unwrap().0, 2);
        // On row 1's last pixel: still row 1.
        assert_eq!(l.shown(12.0 + 104.0 + 95.5, 300.0).unwrap().0, 1);
        // A right edge on a cell's left edge does not show that cell.
        let left_of_3 = l.place(3).unwrap().0;
        assert_eq!(l.shown(0.0, left_of_3).unwrap().1, 2);
        assert_eq!(l.shown(0.0, left_of_3 + 1.0).unwrap().1, 3);
        // Past the end: the last.
        assert_eq!(l.shown(1e6, 1500.0), Some((99, 99)));
    }

    #[test]
    fn a_frame_at_the_right_edge_is_revealed_with_the_pad_beside_it() {
        let shapes = |i: usize| {
            if i.is_multiple_of(3) {
                PORTRAIT
            } else {
                LANDSCAPE
            }
        };
        let l = Layout::new(40, PHOTO, shapes);
        let view = 1000.0;
        // Row 7 runs past the right edge of the view at the start.
        let (left, width) = l.place(7).unwrap();
        assert!(left < view && left + width > view);
        let to = l.reveal(0.0, view, 7);
        assert_eq!(to, left + width + PAD - view);
        // Shown, it stays where it is.
        assert_eq!(l.reveal(to, view, 7), to);
        // Off the left: brought in with the pad before it.
        assert!(l.place(0).unwrap().0 < to);
        assert_eq!(l.reveal(to, view, 0), l.place(0).unwrap().0 - PAD);
        // The last frame: the strip's end.
        assert_eq!(l.reveal(0.0, view, 39), l.max_scroll(view));
        assert_eq!(l.reveal(0.0, view, 0), 0.0);
    }

    #[test]
    fn a_late_shape_left_of_the_view_keeps_the_view_steady() {
        // Nothing known: every cell the 3:2 landscape's.
        let before = Layout::new(60, PHOTO, |_| 0.0);
        let view = 1200.0;
        let scroll = before.place(30).unwrap().0 - 20.0;
        let (first, last) = before.shown(scroll, view).unwrap();
        // Rows 3 and 10, far left of the view, come in as portraits.
        let after = Layout::new(
            60,
            PHOTO,
            |i| if i == 3 || i == 10 { PORTRAIT } else { 0.0 },
        );
        // Selected off screen: the first frame on screen holds.
        let to = held(&before, &after, scroll, view, 0);
        let gone = 2.0 * (cell_width(0.0, PHOTO) - cell_width(PORTRAIT, PHOTO));
        assert_eq!(to, scroll - gone);
        for row in first..=last {
            let x = |l: &Layout, s: f32| l.place(row).unwrap().0 - s;
            assert_eq!(x(&after, to), x(&before, scroll), "row {row} moved");
        }
        // Selected on screen: it holds, and so does the rest.
        let to = held(&before, &after, scroll, view, first + 2);
        assert_eq!(to, scroll - gone);
        // A strip at its start stays there.
        assert_eq!(held(&before, &after, 0.0, view, 0), 0.0);
    }

    #[test]
    fn a_whole_selected_frame_that_grows_past_the_edge_is_shown_whole() {
        // Portraits all; the selected frame is the last whole one on
        // screen and comes in as a panorama.
        let before = Layout::new(60, PHOTO, |_| PORTRAIT);
        let view = 700.0;
        let scroll = 0.0;
        let (_, last) = before.shown(scroll, view).unwrap();
        let sel = last - 1;
        assert_eq!(before.reveal(scroll, view, sel), scroll, "whole");
        let after = Layout::new(60, PHOTO, |i| if i as i32 == sel { 0.1 } else { PORTRAIT });
        let to = held(&before, &after, scroll, view, sel);
        assert_eq!(to, after.reveal(scroll, view, sel));
        assert!(to > 0.0);
        // Cut at the edge before, it is left where it was.
        let cut = last;
        assert_ne!(before.reveal(scroll, view, cut), scroll);
        let after = Layout::new(60, PHOTO, |i| if i as i32 == cut { 0.1 } else { PORTRAIT });
        assert_eq!(held(&before, &after, scroll, view, cut), scroll);
    }

    #[test]
    fn a_strip_of_twenty_thousand_is_laid_out_quickly() {
        let started = std::time::Instant::now();
        let l = Layout::new(20_000, PHOTO, |i| {
            if i.is_multiple_of(4) {
                PORTRAIT
            } else {
                LANDSCAPE
            }
        });
        assert_eq!(l.len(), 20_000);
        assert!(l.shown(l.max_scroll(1500.0), 1500.0).unwrap().1 == 19_999);
        assert!(started.elapsed().as_millis() < 50);
    }
}
