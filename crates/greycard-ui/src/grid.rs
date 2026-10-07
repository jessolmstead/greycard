//! The grid view's arithmetic: how many cells fit across the window,
//! where an arrow leaves the selection, and which frames a scroll
//! offset leaves on screen.
//!
//! All of it is pure and lives here rather than in a `.slint`
//! expression, because a binding cannot be tested. The window asks
//! through pure callbacks and lays the cells out from what comes
//! back; the constants below are handed to the `.slint` file at
//! startup, so the layout there and the arithmetic here cannot drift
//! apart.

/// The gap between cells, logical pixels.
pub const GAP: f32 = 8.0;
/// The padding round the whole grid, logical pixels.
pub const PAD: f32 = 12.0;
/// The room under a cell's picture for the file's name, logical
/// pixels: the line, the space above it and the space below.
pub const LABEL: f32 = 22.0;
/// The cell sizes the zoom steps through, logical pixels. The first
/// is the smallest a name still reads under, the last as large as a
/// thumbnail made from a camera's own preview is worth showing.
pub const STEPS: [f32; 6] = [96.0, 128.0, 176.0, 256.0, 360.0, 512.0];
/// The cell the grid opens at when nothing is remembered.
pub const CELL: f32 = STEPS[2];
/// The largest a thumbnail is ever made, whatever the cell asks.
/// The last step shows a 360 px picture stretched by a factor of
/// 1.4, which is soft under a loupe and indistinguishable at arm's
/// length, and costs two fifths of what a 512 costs to hold: a
/// picture is kept twice over, once as the bytes the worker made
/// and once as the image the renderer uploaded.
pub const MAX_RENDER: u32 = STEPS[4] as u32;

/// The long edge to make a picture at for a cell of this size.
pub fn render_size(cell: f32) -> u32 {
    if !cell.is_finite() {
        return MAX_RENDER;
    }
    (cell.ceil().max(1.0) as u32).min(MAX_RENDER)
}

/// The long edges a picture is actually made at: the strip's 170 and
/// the grid's cells rounded up to one of these, so the thumbnail
/// cache, which keeps a picture a size, holds a few sizes of a frame
/// rather than one for every size a timing happened to ask for. The
/// grid's smallest cells get a 128 where they asked for 96, which is
/// the most this costs.
pub const MADE: [u32; 4] = [128, 176, 256, MAX_RENDER];

/// The size from [`MADE`] a picture asked for at `asked` is made at:
/// the least that is no smaller, or `asked` itself past the last.
pub fn made_size(asked: u32) -> u32 {
    MADE.iter().copied().find(|&m| m >= asked).unwrap_or(asked)
}

/// Whether a picture made at `made` is too small for a cell asking
/// for `want`. A quarter more is worth making it again; a few
/// percent is not, since the downscale is by a whole factor and a
/// few percent usually lands on the very same pixels.
pub fn wants_bigger(made: u32, want: u32) -> bool {
    want * 4 >= made * 5
}

/// A row's pitch: its picture, its name, and the gap below it.
pub fn row_pitch(cell: f32) -> f32 {
    cell + LABEL + GAP
}

/// A folder tile's height: half a cell's picture, whatever the cell.
/// Its width is the cell's.
pub fn tile_height(cell: f32) -> f32 {
    cell / 2.0
}

/// How far the folder tiles push the cells down: their rows, each a
/// tile's height and the gap below it, in the grid's columns. Nothing
/// when there are no tiles. Every offset below takes it as `top`, the
/// cells starting at the padding and this.
pub fn tiles_top(tiles: i32, cell: f32, columns: i32) -> f32 {
    rows(tiles, columns) as f32 * (tile_height(cell) + GAP)
}

/// How many cells fit across `width`; at least one, however narrow
/// the window or however large the cell.
pub fn columns(width: f32, cell: f32) -> i32 {
    let pitch = cell + GAP;
    if pitch <= 0.0 || !pitch.is_finite() {
        return 1;
    }
    // The gap is between cells, so the last one needs none: add one
    // back to the room before dividing.
    let room = width - 2.0 * PAD + GAP;
    if !room.is_finite() {
        return 1;
    }
    ((room / pitch).floor() as i32).max(1)
}

/// How many rows `count` frames take in `columns` columns.
pub fn rows(count: i32, columns: i32) -> i32 {
    if count <= 0 || columns <= 0 {
        return 0;
    }
    (count + columns - 1) / columns
}

/// The frame at a row and a column, clamped to the last frame: the
/// bottom row is usually short, and a column past its end is the
/// nearest frame there is.
pub fn at(row: i32, col: i32, columns: i32, count: i32) -> i32 {
    if count <= 0 || columns <= 0 {
        return -1;
    }
    (row.max(0) * columns + col.clamp(0, columns - 1)).clamp(0, count - 1)
}

/// Where an arrow leaves the selection: `dx` along the folder, which
/// runs off the end of a row into the next as the strip does, `dy` by
/// whole rows with the column kept. Both clamp at the folder's ends.
pub fn step(selected: i32, dx: i32, dy: i32, columns: i32, count: i32) -> i32 {
    if count <= 0 || columns <= 0 {
        return -1;
    }
    let from = selected.clamp(0, count - 1);
    if dy == 0 {
        return (from + dx).clamp(0, count - 1);
    }
    let row = from / columns + dy;
    at(
        row.clamp(0, rows(count, columns) - 1),
        from % columns,
        columns,
        count,
    )
}

/// Where Home (`end` false) or End leaves the selection: the first
/// frame or the last, in the grid and the strip alike. -1 when there
/// is none.
pub fn end(end: bool, count: i32) -> i32 {
    match (count > 0, end) {
        (false, _) => -1,
        (true, false) => 0,
        (true, true) => count - 1,
    }
}

/// The frames on screen at `offset`, first and last, the partly shown
/// rows at either edge included. `None` when there is nothing to
/// show. This is what the grid reports so the worker makes those
/// pictures before the folder's others; a shoot of hundreds must not
/// be rendered up front, the same property the strip has.
pub fn visible(
    offset: f32,
    height: f32,
    cell: f32,
    columns: i32,
    count: i32,
    top: f32,
) -> Option<(i32, i32)> {
    let rows = rows(count, columns);
    if rows == 0 || height <= 0.0 {
        return None;
    }
    let pitch = row_pitch(cell);
    if pitch <= 0.0 || !pitch.is_finite() {
        return None;
    }
    let row_at = |y: f32| {
        ((y - PAD - top) / pitch)
            .floor()
            .clamp(0.0, (rows - 1) as f32) as i32
    };
    let first = row_at(offset);
    let last = row_at(offset + height).max(first);
    Some((
        at(first, 0, columns, count),
        at(last, columns - 1, columns, count),
    ))
}

/// The rows the grid has cells for past those on screen, above and
/// below: the window moves with the scroll in the same event, so one
/// is enough for the rows that come into view in a frame.
pub const MARGIN_ROWS: i32 = 1;

/// The frames the grid has cells for at `offset`, as the first and
/// how many: the rows on screen, the partly shown ones at either edge
/// included, and [`MARGIN_ROWS`] either side. How many follows the
/// sheet's height and the cell alone, not the offset, so a scroll
/// moves the window without changing its size. What is past the last
/// frame is the caller's to hold it to.
pub fn window(
    offset: f32,
    height: f32,
    cell: f32,
    columns: i32,
    count: i32,
    top: f32,
) -> (usize, usize) {
    let pitch = row_pitch(cell);
    if count <= 0 || columns <= 0 || height <= 0.0 || pitch <= 0.0 || !pitch.is_finite() {
        return (0, 0);
    }
    let first_row = (((offset - PAD - top) / pitch).floor() as i32 - MARGIN_ROWS).max(0);
    let rows = (height / pitch).ceil() as i32 + 1 + 2 * MARGIN_ROWS;
    ((first_row * columns) as usize, (rows * columns) as usize)
}

/// The scroll that shows the selected cell with the least movement,
/// its padding beside it, or what we have when it is already on
/// screen. A cell of the first row brings the folder tiles above it
/// with it, when the two fit on the sheet together; when the tiles
/// are taller than that, the cell is shown as any other. Everything
/// past the ends is clamped away.
pub fn reveal(
    offset: f32,
    height: f32,
    cell: f32,
    columns: i32,
    count: i32,
    selected: i32,
    top: f32,
) -> f32 {
    let rows = rows(count, columns);
    if rows == 0 || selected < 0 || height <= 0.0 {
        return 0.0;
    }
    let pitch = row_pitch(cell);
    let row = selected.clamp(0, count - 1) / columns;
    let y = PAD + top + row as f32 * pitch;
    let bottom = y + cell + LABEL;
    let lead = if row == 0 && PAD + top + cell + LABEL + PAD <= height {
        0.0
    } else {
        y - PAD
    };
    let wanted = if lead < offset {
        lead
    } else if bottom + PAD > offset + height {
        bottom + PAD - height
    } else {
        offset
    };
    wanted.clamp(0.0, max_scroll(height, cell, columns, count, top))
}

/// The empty half-width beside a full row, so the cells sit in the
/// middle of the window rather than against its left edge: at 512
/// two columns leave a third of a 1500 px window over.
pub fn slack(width: f32, cell: f32, columns: i32) -> f32 {
    let row = 2.0 * PAD + columns.max(1) as f32 * (cell + GAP) - GAP;
    ((width - row) / 2.0).max(0.0)
}

/// How far the grid can be scrolled: the whole sheet, the folder
/// tiles' rows and the cells', less what is on screen, never below
/// nothing.
pub fn max_scroll(height: f32, cell: f32, columns: i32, count: i32, top: f32) -> f32 {
    let body = top + rows(count, columns) as f32 * row_pitch(cell);
    if body <= 0.0 {
        return 0.0;
    }
    (2.0 * PAD + body - GAP - height).max(0.0)
}

/// The scroll bar's thumb keeps this far from the ends of its band,
/// logical pixels. The strip's bar asks the same arithmetic along its
/// width, which stands for the sheet's height throughout.
pub const BAR_INSET: f32 = 4.0;
/// The shortest the thumb gets, however long the folder: still
/// something to take hold of.
pub const BAR_MIN: f32 = 24.0;

/// The room the scroll bar's thumb travels in, on a sheet `height`
/// tall.
fn bar_track(height: f32) -> f32 {
    (height - 2.0 * BAR_INSET).max(0.0)
}

/// The scroll bar's thumb length on a sheet `height` tall that
/// scrolls `max` further: the track in the share of the whole sheet
/// that is on screen, no shorter than [`BAR_MIN`] where the track
/// has room for it. Nothing when nothing scrolls.
pub fn bar_length(height: f32, max: f32) -> f32 {
    if max <= 0.0 || height <= 0.0 || !max.is_finite() {
        return 0.0;
    }
    let track = bar_track(height);
    (track * height / (height + max)).max(BAR_MIN.min(track))
}

/// The thumb's top, from the top of the sheet, at a scroll of
/// `scroll` out of `max`: at the inset at the top, the thumb's
/// length short of the far inset at the foot.
pub fn bar_offset(height: f32, max: f32, scroll: f32) -> f32 {
    if max <= 0.0 || !max.is_finite() {
        return BAR_INSET;
    }
    let room = bar_track(height) - bar_length(height, max);
    BAR_INSET + room.max(0.0) * (scroll / max).clamp(0.0, 1.0)
}

/// The scroll a thumb taken at a scroll of `from` and moved `by`
/// along the bar comes to: the thumb follows the pointer, so a move
/// down the whole of the thumb's room is the whole of `max`. Held to
/// the range.
pub fn bar_drag(height: f32, max: f32, from: f32, by: f32) -> f32 {
    if max <= 0.0 || !max.is_finite() {
        return 0.0;
    }
    let room = bar_track(height) - bar_length(height, max);
    if room <= 0.0 {
        return from.clamp(0.0, max);
    }
    (from + by * max / room).clamp(0.0, max)
}

/// The next cell size up (`by` positive) or down the steps, from
/// whichever step the cell is nearest.
pub fn zoom(cell: f32, by: i32) -> f32 {
    let mut nearest = 0;
    for (i, s) in STEPS.iter().enumerate() {
        if (s - cell).abs() < (STEPS[nearest] - cell).abs() {
            nearest = i;
        }
    }
    let to = (nearest as i32 + by).clamp(0, STEPS.len() as i32 - 1) as usize;
    STEPS[to]
}

/// A cell size from the command line or the settings file, held to
/// the range the steps cover.
pub fn clamp_cell(cell: f32) -> f32 {
    if cell.is_finite() {
        cell.clamp(STEPS[0], STEPS[STEPS.len() - 1])
    } else {
        CELL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fit_the_width_with_the_padding_and_the_gaps() {
        // 1500 wide, 176 cells: 1500 - 24 + 8 = 1484 over a pitch of
        // 184 is 8 columns, and 8 cells with 7 gaps and the padding
        // come to 1480, which fits.
        assert_eq!(columns(1500.0, 176.0), 8);
        let across = |n: f32| 2.0 * PAD + n * 176.0 + (n - 1.0) * GAP;
        assert!(across(8.0) <= 1500.0);
        assert!(across(9.0) > 1500.0);
        // The exact fit of one more cell is taken, not refused.
        assert_eq!(columns(across(9.0), 176.0), 9);
        // A window narrower than a cell still shows a column.
        assert_eq!(columns(80.0, 512.0), 1);
        assert_eq!(columns(0.0, 176.0), 1);
    }

    #[test]
    fn rows_take_the_short_last_one() {
        assert_eq!(rows(0, 5), 0);
        assert_eq!(rows(1, 5), 1);
        assert_eq!(rows(10, 5), 2);
        assert_eq!(rows(11, 5), 3);
        assert_eq!(rows(7, 0), 0);
    }

    #[test]
    fn a_row_and_a_column_give_a_frame_and_the_last_row_clamps() {
        // 11 frames in 4 columns: rows 0, 1 and a short row 2.
        assert_eq!(at(0, 0, 4, 11), 0);
        assert_eq!(at(1, 2, 4, 11), 6);
        assert_eq!(at(2, 2, 4, 11), 10);
        // Past the end of the short row: the last frame there is.
        assert_eq!(at(2, 3, 4, 11), 10);
        // Past the last row, and past the last column.
        assert_eq!(at(9, 0, 4, 11), 10);
        assert_eq!(at(0, 9, 4, 11), 3);
        assert_eq!(at(0, 0, 4, 0), -1);
    }

    #[test]
    fn arrows_move_along_the_row_and_by_whole_rows() {
        let (cols, count) = (4, 11);
        assert_eq!(step(0, 1, 0, cols, count), 1);
        // Along the row, over its end into the next, as the strip does.
        assert_eq!(step(3, 1, 0, cols, count), 4);
        assert_eq!(step(0, -1, 0, cols, count), 0);
        assert_eq!(step(10, 1, 0, cols, count), 10);
        // Down a row keeps the column.
        assert_eq!(step(1, 0, 1, cols, count), 5);
        assert_eq!(step(5, 0, -1, cols, count), 1);
        assert_eq!(step(1, 0, -1, cols, count), 1);
        // Down from the second row into the short third: column 2 is
        // there, column 3 is not and lands on the last frame.
        assert_eq!(step(6, 0, 1, cols, count), 10);
        assert_eq!(step(7, 0, 1, cols, count), 10);
        // Down from the last row stays put.
        assert_eq!(step(10, 0, 1, cols, count), 10);
        assert_eq!(step(0, 0, 1, cols, 0), -1);
    }

    #[test]
    fn the_visible_rows_follow_the_scroll() {
        // 4 columns, 11 frames, 176 cells: rows are 206 apart and the
        // first begins 12 down.
        let (cols, count, cell) = (4, 11, 176.0);
        assert_eq!(row_pitch(cell), 206.0);
        // At the top, a 500 tall sheet shows rows 0 to 2.
        assert_eq!(visible(0.0, 500.0, cell, cols, count, 0.0), Some((0, 10)));
        // Half a row down still shows the first row's top edge.
        assert_eq!(visible(100.0, 206.0, cell, cols, count, 0.0), Some((0, 7)));
        // Scrolled past the first row exactly: the second row alone,
        // with the third's top edge.
        assert_eq!(visible(218.0, 206.0, cell, cols, count, 0.0), Some((4, 10)));
        // Over-scrolled: the last row, not an index past the end.
        assert_eq!(
            visible(9000.0, 206.0, cell, cols, count, 0.0),
            Some((8, 10))
        );
        assert_eq!(visible(0.0, 0.0, cell, cols, count, 0.0), None);
        assert_eq!(visible(0.0, 500.0, cell, cols, 0, 0.0), None);
    }

    #[test]
    fn the_cells_cover_what_is_on_screen_and_a_row_either_side() {
        let (cols, count, cell, height) = (8, 20_000, 176.0, 773.0);
        // 773 over a pitch of 206 is four rows and a part, and a sixth
        // part-row as it scrolls: seven with a row either side, at any
        // offset.
        for offset in [0.0, 5.0, 100.0, 206.0, 1000.0, 123_456.0] {
            let (first, n) = window(offset, height, cell, cols, count, 0.0);
            assert_eq!(n, 7 * 8, "{offset}");
            let (a, b) = visible(offset, height, cell, cols, count, 0.0).unwrap();
            assert!(first <= a as usize && (b as usize) < first + n, "{offset}");
            assert_eq!(first % 8, 0);
        }
        // At the top there is no row above: the window starts at 0.
        assert_eq!(window(0.0, height, cell, cols, count, 0.0).0, 0);
        // Scrolled three rows down, the window starts a row above.
        assert_eq!(
            window(PAD + 3.0 * 206.0, height, cell, cols, count, 0.0).0,
            2 * 8
        );
        assert_eq!(window(0.0, 0.0, cell, cols, count, 0.0), (0, 0));
        assert_eq!(window(0.0, height, cell, cols, 0, 0.0), (0, 0));
    }

    #[test]
    fn the_reveal_is_the_least_scroll_that_shows_the_cell() {
        let (cols, count, cell, height) = (4, 40, 176.0, 500.0);
        // The first row needs no scroll.
        assert_eq!(reveal(0.0, height, cell, cols, count, 1, 0.0), 0.0);
        // A frame below the fold comes up against the bottom edge
        // with its padding: row 2 ends at 12 + 2*206 + 198 = 622.
        assert_eq!(
            reveal(0.0, height, cell, cols, count, 9, 0.0),
            622.0 + PAD - height
        );
        // One already on screen is left where it is.
        let shown = reveal(0.0, height, cell, cols, count, 9, 0.0);
        assert_eq!(reveal(shown, height, cell, cols, count, 5, 0.0), shown);
        // Going back up puts its top edge, and its padding, at the top.
        assert_eq!(reveal(shown, height, cell, cols, count, 0, 0.0), 0.0);
        // Never past the end of the sheet.
        assert!(
            reveal(0.0, height, cell, cols, count, 39, 0.0)
                <= max_scroll(height, cell, cols, count, 0.0)
        );
    }

    /// The folder tiles above the cells: a row of them per row of the
    /// grid's columns, half a cell tall and a gap below each, shift the
    /// cells down by as much and lengthen the sheet by as much.
    #[test]
    fn the_folder_tiles_push_the_cells_down_by_their_rows() {
        let (cols, count, cell, height) = (4, 40, 176.0, 500.0);
        assert_eq!(tile_height(cell), 88.0);
        assert_eq!(tiles_top(0, cell, cols), 0.0);
        // One tile to four: one row of 88 and its gap; a fifth wraps.
        assert_eq!(tiles_top(1, cell, cols), 96.0);
        assert_eq!(tiles_top(4, cell, cols), 96.0);
        assert_eq!(tiles_top(5, cell, cols), 192.0);
        // At the smallest cell the row is 48 and its gap.
        assert_eq!(tiles_top(3, STEPS[0], cols), 56.0);
        let top = tiles_top(2, cell, cols);
        // The scroll range grows by exactly the tiles' rows.
        assert_eq!(
            max_scroll(height, cell, cols, count, top),
            max_scroll(height, cell, cols, count, 0.0) + top
        );
        // Tiles alone, no frames: the sheet is the padding and a row.
        assert_eq!(
            max_scroll(50.0, cell, cols, 0, top),
            2.0 * PAD + 88.0 - 50.0
        );
        assert_eq!(max_scroll(height, cell, cols, 0, top), 0.0);
        // What is on screen, and the cells made, move down a tile row:
        // scrolled by the tiles' height, the cells are where they were
        // at the top without them.
        assert_eq!(
            visible(top, height, cell, cols, count, top),
            visible(0.0, height, cell, cols, count, 0.0)
        );
        assert_eq!(
            window(top + PAD + 3.0 * 206.0, height, cell, cols, count, top).0,
            2 * 4
        );
        // At the top, the tiles take room the cells had: 300 tall
        // shows the second row's top edge without them and not with.
        assert_eq!(visible(0.0, 300.0, cell, cols, count, 0.0), Some((0, 7)));
        assert_eq!(visible(0.0, 300.0, cell, cols, count, top), Some((0, 3)));
        // A frame of the first row brings the tiles back into view; one
        // further down comes up against the foot by the tiles' height
        // more.
        assert_eq!(reveal(300.0, height, cell, cols, count, 2, top), 0.0);
        assert_eq!(
            reveal(0.0, height, cell, cols, count, 9, top),
            622.0 + top + PAD - height
        );
        // And back up to the second row: its top at the sheet's top
        // with the padding, the tiles above it scrolled away.
        let down = reveal(0.0, height, cell, cols, count, 39, top);
        assert_eq!(reveal(down, height, cell, cols, count, 4, top), top + 206.0);

        // Tiles taller than the sheet: six columns, 837 px, and 54
        // folders make nine tile rows, 864 px. Scrolled past them, a
        // frame of the first row is shown where it is, not under the
        // tiles at the top.
        let (cols, height) = (6, 837.0);
        let top = tiles_top(54, cell, cols);
        assert_eq!(top, 864.0);
        let first_row_y = PAD + top;
        let at = reveal(first_row_y - PAD, height, cell, cols, count, 2, top);
        assert_eq!(at, first_row_y - PAD);
        assert!(at <= first_row_y && first_row_y + cell + LABEL <= at + height);
        // And from the top, it comes up against the foot.
        let up = reveal(0.0, height, cell, cols, count, 2, top);
        assert_eq!(up, first_row_y + cell + LABEL + PAD - height);
        assert!(up <= first_row_y && first_row_y + cell + LABEL <= up + height);
    }

    #[test]
    fn a_cell_asks_for_a_picture_its_own_size_up_to_the_cap() {
        assert_eq!(render_size(96.0), 96);
        assert_eq!(render_size(176.0), 176);
        assert_eq!(render_size(360.0), 360);
        // The largest cell takes the cap and stretches it.
        assert_eq!(render_size(512.0), MAX_RENDER);
        assert_eq!(MAX_RENDER, 360);
        // A quarter more is worth making again; a few percent is not.
        assert!(!wants_bigger(170, 176));
        assert!(!wants_bigger(170, 170));
        assert!(wants_bigger(170, 256));
        assert!(wants_bigger(256, 360));
        // And a cell that has shrunk asks for nothing.
        assert!(!wants_bigger(360, 96));
        assert!(!wants_bigger(256, 176));
    }

    #[test]
    fn the_slack_puts_the_cells_in_the_middle() {
        // Two 512 cells and their gaps come to 1056 of 1500.
        assert_eq!(slack(1500.0, 512.0, 2), (1500.0 - 1056.0) / 2.0);
        // Eight 176 cells and their padding fill 1488 of it.
        assert_eq!(slack(1500.0, 176.0, 8), 6.0);
        // A row wider than the window is not pulled left.
        assert_eq!(slack(200.0, 512.0, 1), 0.0);
    }

    #[test]
    fn the_zoom_steps_and_stops() {
        assert_eq!(zoom(176.0, 1), 256.0);
        assert_eq!(zoom(176.0, -1), 128.0);
        assert_eq!(zoom(512.0, 1), 512.0);
        assert_eq!(zoom(96.0, -1), 96.0);
        // From a size that is not a step, the nearest one moves.
        assert_eq!(zoom(180.0, 1), 256.0);
        assert_eq!(clamp_cell(4000.0), 512.0);
        assert_eq!(clamp_cell(1.0), 96.0);
        assert_eq!(clamp_cell(f32::NAN), CELL);
    }

    /// The strip's size and the grid's cells land on a few made
    /// sizes, each no smaller than was asked.
    #[test]
    fn a_picture_is_made_at_one_of_a_few_sizes() {
        assert_eq!(made_size(crate::worker::THUMB_WIDTH), 176);
        assert_eq!(made_size(render_size(STEPS[0])), 128);
        assert_eq!(made_size(render_size(CELL)), 176);
        for cell in STEPS {
            let asked = render_size(cell);
            let made = made_size(asked);
            assert!(made >= asked && MADE.contains(&made), "{cell}");
            assert!(!wants_bigger(made, asked));
        }
        assert_eq!(made_size(600), 600);
    }
    #[test]
    fn home_and_end_land_on_the_first_frame_and_the_last() {
        assert_eq!(end(false, 40), 0);
        assert_eq!(end(true, 40), 39);
        assert_eq!(end(false, 1), 0);
        assert_eq!(end(true, 1), 0);
        assert_eq!(end(false, 0), -1);
        assert_eq!(end(true, 0), -1);
    }

    #[test]
    fn the_strips_bar_is_the_same_arithmetic_along_its_width() {
        // A strip 1500 wide over forty frames at a 186 pitch and 12 of
        // padding at each end: the row is 7464, so 5964 to scroll.
        let (w, row) = (1500.0, 40.0 * 186.0 + 24.0);
        let max = row - w;
        let track = w - 2.0 * BAR_INSET;
        let len = bar_length(w, max);
        assert!((len - track * w / row).abs() < 1e-3, "{len}");
        // At either end of the folder the thumb is at either end of
        // the track.
        assert_eq!(bar_offset(w, max, 0.0), BAR_INSET);
        assert!((bar_offset(w, max, max) + len - (w - BAR_INSET)).abs() < 1e-3);
        // A page is the strip's width, and a press past the thumb
        // moves the strip one: two from the start is twice the width,
        // and the thumb has moved by the track's share of it.
        let paged = 2.0 * w;
        assert!((bar_offset(w, max, paged) - BAR_INSET - (track - len) * paged / max).abs() < 1e-3);
        // Dragged the whole room, the strip comes to its end.
        assert_eq!(bar_drag(w, max, 0.0, track - len), max);
        // One frame, or a folder that fits: no thumb, nothing to drag.
        assert_eq!(bar_length(w, 0.0), 0.0);
        assert_eq!(bar_drag(w, 0.0, 0.0, 100.0), 0.0);
        // A window narrowed to 600 over the same row: a shorter thumb
        // and more room, the end still at the far inset.
        let (w, max) = (600.0, row - 600.0);
        let len = bar_length(w, max);
        assert!(len < bar_length(1500.0, row - 1500.0));
        assert!((bar_offset(w, max, max) + len - (w - BAR_INSET)).abs() < 1e-3);
    }

    #[test]
    fn the_bar_thumb_is_the_share_of_the_sheet_on_screen() {
        // An 837 px sheet that scrolls as far again: the thumb is half
        // the track, at the top inset at 0 and at the foot at the end.
        let (h, max) = (837.0, 837.0);
        let track = h - 2.0 * BAR_INSET;
        assert_eq!(bar_length(h, max), track / 2.0);
        assert_eq!(bar_offset(h, max, 0.0), BAR_INSET);
        assert_eq!(bar_offset(h, max, max), BAR_INSET + track / 2.0);
        assert_eq!(bar_offset(h, max, max / 2.0), BAR_INSET + track / 4.0);
        // Out of range is held to the ends.
        assert_eq!(bar_offset(h, max, -50.0), BAR_INSET);
        assert_eq!(bar_offset(h, max, 2.0 * max), BAR_INSET + track / 2.0);
        // A folder of thousands keeps a thumb to hold.
        assert_eq!(bar_length(h, 1.0e6), BAR_MIN);
        let end = bar_offset(h, 1.0e6, 1.0e6) + BAR_MIN;
        assert_eq!(end, h - BAR_INSET);
        // Nothing to scroll, no thumb; a sheet too short for the
        // least thumb gets the track.
        assert_eq!(bar_length(h, 0.0), 0.0);
        assert_eq!(bar_length(20.0, 500.0), 12.0);
    }

    #[test]
    fn a_thumb_dragged_follows_the_pointer_and_stays_in_range() {
        let (h, max) = (837.0, 837.0);
        let room = h - 2.0 * BAR_INSET - bar_length(h, max);
        // Down the whole room is the whole range; halfway, half.
        assert_eq!(bar_drag(h, max, 0.0, room), max);
        assert_eq!(bar_drag(h, max, 0.0, room / 2.0), max / 2.0);
        assert_eq!(bar_drag(h, max, max, -room / 2.0), max / 2.0);
        // Past either end is the end.
        assert_eq!(bar_drag(h, max, 100.0, -5000.0), 0.0);
        assert_eq!(bar_drag(h, max, 100.0, 5000.0), max);
        // And the thumb lands under the pointer that moved it.
        let to = bar_drag(h, max, 0.0, 100.0);
        assert!((bar_offset(h, max, to) - (BAR_INSET + 100.0)).abs() < 1e-3);
        assert_eq!(bar_drag(h, 0.0, 10.0, 50.0), 0.0);
    }

    #[test]
    fn a_least_thumb_still_drags_the_whole_range_under_the_pointer() {
        // A million pixels of scroll: the thumb is held at the least,
        // and its room is the track less that.
        let (h, max) = (837.0, 1.0e6);
        assert_eq!(bar_length(h, max), BAR_MIN);
        let room = h - 2.0 * BAR_INSET - BAR_MIN;
        assert_eq!(bar_drag(h, max, 0.0, room), max);
        assert_eq!(bar_drag(h, max, max, -room), 0.0);
        // A drag partway puts the thumb under the pointer.
        for by in [1.0, 37.5, room / 3.0, room - 1.0] {
            let to = bar_drag(h, max, 0.0, by);
            assert!(
                (bar_offset(h, max, to) - (BAR_INSET + by)).abs() < 1e-2,
                "{by}: {to}"
            );
        }
    }
}
