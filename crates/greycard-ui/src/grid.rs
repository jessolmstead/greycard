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
) -> Option<(i32, i32)> {
    let rows = rows(count, columns);
    if rows == 0 || height <= 0.0 {
        return None;
    }
    let pitch = row_pitch(cell);
    if pitch <= 0.0 || !pitch.is_finite() {
        return None;
    }
    let row_at = |y: f32| ((y - PAD) / pitch).floor().clamp(0.0, (rows - 1) as f32) as i32;
    let first = row_at(offset);
    let last = row_at(offset + height).max(first);
    Some((
        at(first, 0, columns, count),
        at(last, columns - 1, columns, count),
    ))
}

/// The scroll that shows the selected cell with the least movement,
/// its padding beside it, or what we have when it is already on
/// screen. Everything past the ends is clamped away.
pub fn reveal(offset: f32, height: f32, cell: f32, columns: i32, count: i32, selected: i32) -> f32 {
    let rows = rows(count, columns);
    if rows == 0 || selected < 0 || height <= 0.0 {
        return 0.0;
    }
    let pitch = row_pitch(cell);
    let row = selected.clamp(0, count - 1) / columns;
    let top = PAD + row as f32 * pitch;
    let bottom = top + cell + LABEL;
    let wanted = if top - PAD < offset {
        top - PAD
    } else if bottom + PAD > offset + height {
        bottom + PAD - height
    } else {
        offset
    };
    wanted.clamp(0.0, max_scroll(height, cell, columns, count))
}

/// The empty half-width beside a full row, so the cells sit in the
/// middle of the window rather than against its left edge: at 512
/// two columns leave a third of a 1500 px window over.
pub fn slack(width: f32, cell: f32, columns: i32) -> f32 {
    let row = 2.0 * PAD + columns.max(1) as f32 * (cell + GAP) - GAP;
    ((width - row) / 2.0).max(0.0)
}

/// How far the grid can be scrolled: the whole sheet less what is on
/// screen, never below nothing.
pub fn max_scroll(height: f32, cell: f32, columns: i32, count: i32) -> f32 {
    let rows = rows(count, columns);
    if rows == 0 {
        return 0.0;
    }
    (2.0 * PAD + rows as f32 * row_pitch(cell) - GAP - height).max(0.0)
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
        assert_eq!(visible(0.0, 500.0, cell, cols, count), Some((0, 10)));
        // Half a row down still shows the first row's top edge.
        assert_eq!(visible(100.0, 206.0, cell, cols, count), Some((0, 7)));
        // Scrolled past the first row exactly: the second row alone,
        // with the third's top edge.
        assert_eq!(visible(218.0, 206.0, cell, cols, count), Some((4, 10)));
        // Over-scrolled: the last row, not an index past the end.
        assert_eq!(visible(9000.0, 206.0, cell, cols, count), Some((8, 10)));
        assert_eq!(visible(0.0, 0.0, cell, cols, count), None);
        assert_eq!(visible(0.0, 500.0, cell, cols, 0), None);
    }

    #[test]
    fn the_reveal_is_the_least_scroll_that_shows_the_cell() {
        let (cols, count, cell, height) = (4, 40, 176.0, 500.0);
        // The first row needs no scroll.
        assert_eq!(reveal(0.0, height, cell, cols, count, 1), 0.0);
        // A frame below the fold comes up against the bottom edge
        // with its padding: row 2 ends at 12 + 2*206 + 198 = 622.
        assert_eq!(
            reveal(0.0, height, cell, cols, count, 9),
            622.0 + PAD - height
        );
        // One already on screen is left where it is.
        let shown = reveal(0.0, height, cell, cols, count, 9);
        assert_eq!(reveal(shown, height, cell, cols, count, 5), shown);
        // Going back up puts its top edge, and its padding, at the top.
        assert_eq!(reveal(shown, height, cell, cols, count, 0), 0.0);
        // Never past the end of the sheet.
        assert!(
            reveal(0.0, height, cell, cols, count, 39) <= max_scroll(height, cell, cols, count)
        );
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
}
