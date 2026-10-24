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

/// The gap between cells, logical pixels: the theme's smallest
/// spacing step. The rings are drawn inside a cell, so any gap keeps
/// two neighbors' rings apart; this one is enough to see the two.
pub const GAP: f32 = 4.0;
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

/// A row's pitch: its picture room (the cell, for a row with a
/// square picture in it; [`picture_room`] for a shaped one), its name,
/// and the gap below it.
pub fn row_pitch(room: f32) -> f32 {
    room + LABEL + GAP
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

/// The frames on screen at `offset`, first and last, over cells all
/// square: [`Layout::visible`] over [`Layout::square`], the grid as it
/// was before its rows took the pictures' shape, which the tests keep
/// as the reference the shaped rows reduce to.
#[cfg(test)]
pub fn visible(
    offset: f32,
    height: f32,
    cell: f32,
    columns: i32,
    count: i32,
    top: f32,
) -> Option<(i32, i32)> {
    Layout::square(cell, columns, count, &[]).visible(offset, height, top)
}

/// The rows the grid has cells for past those on screen, above and
/// below: the window moves with the scroll in the same event, so one
/// is enough for the rows that come into view in a frame.
pub const MARGIN_ROWS: i32 = 1;

/// [`Layout::window`] over square cells.
#[cfg(test)]
pub fn window(
    offset: f32,
    height: f32,
    cell: f32,
    columns: i32,
    count: i32,
    top: f32,
) -> (usize, usize) {
    Layout::square(cell, columns, count, &[]).window(offset, height, top)
}

/// [`Layout::reveal`] over square cells.
#[cfg(test)]
pub fn reveal(
    offset: f32,
    height: f32,
    cell: f32,
    columns: i32,
    count: i32,
    selected: i32,
    top: f32,
) -> f32 {
    Layout::square(cell, columns, count, &[]).reveal(offset, height, selected, top)
}

/// The empty half-width beside a full row, so the cells sit in the
/// middle of the window rather than against its left edge: at 512
/// two columns leave a third of a 1500 px window over.
pub fn slack(width: f32, cell: f32, columns: i32) -> f32 {
    let row = 2.0 * PAD + columns.max(1) as f32 * (cell + GAP) - GAP;
    ((width - row) / 2.0).max(0.0)
}

/// [`Layout::max_scroll`] over square cells.
#[cfg(test)]
pub fn max_scroll(height: f32, cell: f32, columns: i32, count: i32, top: f32) -> f32 {
    Layout::square(cell, columns, count, &[]).max_scroll(height, top)
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

/// The cell size `by` steps up (positive) or down: each the next of
/// [`STEPS`] past the cell in that direction, so a size typed between
/// two steps goes to the nearer one in the direction asked, never
/// over it. Held at the ends.
pub fn zoom(cell: f32, by: i32) -> f32 {
    let mut cell = clamp_cell(cell);
    for _ in 0..by.unsigned_abs() {
        cell = if by > 0 {
            STEPS.iter().copied().find(|&s| s > cell)
        } else {
            STEPS.iter().rev().copied().find(|&s| s < cell)
        }
        .unwrap_or(cell);
    }
    cell
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

/// A heading row's height, logical pixels: a line of small text and
/// room round it, full width above the frames it heads.
pub const HEAD: f32 = 28.0;

/// A run of the list under one heading, or before the first: where it
/// starts, how many frames, whether a heading stands above it, the
/// grid rows before it, and the headings above it, its own included.
#[derive(Debug, Clone, Copy)]
struct Run {
    start: i32,
    len: i32,
    head: bool,
    line: i32,
    heads: i32,
}

/// The runs `heads` cut `count` frames into: each heading starts a row
/// of its own, its frames on the rows under it. A heading at a frame
/// past the list, or twice at one, is none.
fn runs(count: i32, columns: i32, heads: &[i32]) -> Vec<Run> {
    let mut starts: Vec<i32> = heads
        .iter()
        .copied()
        .filter(|&h| h >= 0 && h < count)
        .collect();
    starts.sort_unstable();
    starts.dedup();
    let mut out = Vec::with_capacity(starts.len() + 1);
    let (mut line, mut seen) = (0, 0);
    let first = starts.first().copied().unwrap_or(count);
    if first > 0 {
        out.push(Run {
            start: 0,
            len: first,
            head: false,
            line,
            heads: 0,
        });
        line += rows(first, columns);
    }
    for (k, &s) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(count);
        seen += 1;
        out.push(Run {
            start: s,
            len: end - s,
            head: true,
            line,
            heads: seen,
        });
        line += rows(end - s, columns);
    }
    out
}

/// The run frame `i` is in.
fn run_of(runs: &[Run], i: i32) -> Option<&Run> {
    let k = runs.partition_point(|r| r.start <= i);
    runs.get(k.checked_sub(1)?)
}

/// Where frame `i` sits under `heads`: its column, its grid row
/// counted over the whole sheet, and how many headings are above it.
/// The cell's top is the padding, the tiles, the rows above at a row's
/// pitch, and the headings above at [`HEAD`]. Without headings, the
/// plain grid's place.
pub fn place(i: i32, columns: i32, heads: &[i32]) -> (i32, i32, i32) {
    let columns = columns.max(1);
    if heads.is_empty() || i < 0 {
        return (i.max(0) % columns, i.max(0) / columns, 0);
    }
    let count = i + 1 + heads.iter().copied().max().unwrap_or(0).max(0);
    let runs = runs(count, columns, heads);
    match run_of(&runs, i) {
        Some(r) => {
            let local = i - r.start;
            (local % columns, r.line + local / columns, r.heads)
        }
        None => (i % columns, i / columns, 0),
    }
}

/// The grid rows `count` frames take under `heads`.
fn lines(runs: &[Run], columns: i32) -> i32 {
    runs.last().map_or(0, |r| r.line + rows(r.len, columns))
}

/// The run grid row `line` is in.
fn run_of_line(runs: &[Run], columns: i32, line: i32) -> Option<&Run> {
    runs.iter()
        .find(|r| line < r.line + rows(r.len, columns))
        .or(runs.last())
}

/// The frame on grid row `line` at column `col`, held to the row's
/// last frame: a run's last row is usually short.
fn frame_on(runs: &[Run], columns: i32, line: i32, col: i32) -> i32 {
    let Some(r) = run_of_line(runs, columns, line) else {
        return -1;
    };
    let row = (line - r.line).clamp(0, rows(r.len, columns) - 1);
    r.start + (row * columns + col.clamp(0, columns - 1)).min(r.len - 1)
}

/// [`step`] under `heads`: along the list as ever, or by grid rows with
/// the column kept, over the headings, which no arrow lands on.
pub fn step_in(selected: i32, dx: i32, dy: i32, columns: i32, count: i32, heads: &[i32]) -> i32 {
    if heads.is_empty() || dy == 0 {
        return step(selected, dx, dy, columns, count);
    }
    if count <= 0 || columns <= 0 {
        return -1;
    }
    let runs = runs(count, columns, heads);
    let (col, line, _) = place(selected.clamp(0, count - 1), columns, heads);
    let to = (line + dy).clamp(0, lines(&runs, columns) - 1);
    frame_on(&runs, columns, to, col)
}

/// [`visible`] under `heads`, over square cells.
#[cfg(test)]
pub fn visible_in(
    offset: f32,
    height: f32,
    cell: f32,
    columns: i32,
    count: i32,
    top: f32,
    heads: &[i32],
) -> Option<(i32, i32)> {
    Layout::square(cell, columns, count, heads).visible(offset, height, top)
}

/// [`window`] under `heads`, over square cells.
#[cfg(test)]
pub fn window_in(
    offset: f32,
    height: f32,
    cell: f32,
    columns: i32,
    count: i32,
    top: f32,
    heads: &[i32],
) -> (usize, usize) {
    Layout::square(cell, columns, count, heads).window(offset, height, top)
}

/// [`max_scroll`] under `heads`, over square cells.
#[cfg(test)]
pub fn max_scroll_in(
    height: f32,
    cell: f32,
    columns: i32,
    count: i32,
    top: f32,
    heads: &[i32],
) -> f32 {
    Layout::square(cell, columns, count, heads).max_scroll(height, top)
}

/// [`reveal`] under `heads`, over square cells.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub fn reveal_in(
    offset: f32,
    height: f32,
    cell: f32,
    columns: i32,
    count: i32,
    selected: i32,
    top: f32,
    heads: &[i32],
) -> f32 {
    Layout::square(cell, columns, count, heads).reveal(offset, height, selected, top)
}

/// How far a cell's picture is inset from the cell's edges, logical
/// pixels: the theme's `space-1`, which the `.slint` file insets it by.
pub const INSET: f32 = 4.0;

/// The shape, height over width, a frame is laid out at before its
/// picture says otherwise: most cameras' 3:2, on its side.
pub const UNKNOWN_ASPECT: f32 = 2.0 / 3.0;

/// The room a picture of `aspect` (height over width; nothing known
/// for zero or less) takes in a cell `cell` wide: its height at the
/// cell's width, inside the inset, and never more than the square cell
/// it used to have. A portrait is held to that square, as before, since
/// its thumbnail is made at the cell's long edge and no larger.
pub fn picture_room(cell: f32, aspect: f32) -> f32 {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        UNKNOWN_ASPECT
    };
    let inner = (cell - 2.0 * INSET).max(0.0);
    (2.0 * INSET + inner * aspect).round().min(cell).max(0.0)
}

/// The pitch of a row of 3:2 frames, the common row, which a wheel's
/// notch is a third of.
pub fn common_pitch(cell: f32) -> f32 {
    row_pitch(picture_room(cell, UNKNOWN_ASPECT))
}

/// Where a frame sits on the shaped grid: its column, its grid row,
/// the headings above it with its own, and its row's top from the
/// padding and the folder tiles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub col: i32,
    pub line: i32,
    pub heads: i32,
    pub y: f32,
}

/// The grid's rows laid out once: each row's first frame, its top, and
/// its picture room, the tallest picture in it at the cell's width; a
/// row's pitch is that room, the name's, and the gap. The tops are a
/// prefix sum, so where a frame sits and which rows a scroll shows are
/// a lookup and a binary search, not a walk over the folder.
///
/// Headings (`heads`, each at the first frame it heads) break the rows
/// as [`place`] has them, each a [`HEAD`] above its first row.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    columns: i32,
    count: i32,
    /// Each row's first frame.
    starts: Vec<i32>,
    /// Each row's top, from the padding and the tiles.
    tops: Vec<f32>,
    /// Where each row's band begins: its top, or its heading's.
    bands: Vec<f32>,
    /// Each row's picture room.
    rooms: Vec<f32>,
    /// Whether a heading stands right above each row.
    headed: Vec<bool>,
    /// The headings above each row, its own included.
    heads: Vec<i32>,
    /// The rows' and headings' whole height, the last row's gap in.
    body: f32,
    /// The shortest row's pitch.
    least: f32,
}

impl Layout {
    /// `count` frames in `columns` columns of `cell`, frame `i` of
    /// shape `aspect(i)` (height over width; zero or less for not
    /// known, laid out as [`UNKNOWN_ASPECT`]).
    pub fn new(
        cell: f32,
        columns: i32,
        count: i32,
        heads: &[i32],
        aspect: impl Fn(i32) -> f32,
    ) -> Self {
        let mut out = Self {
            columns: columns.max(1),
            count: 0,
            starts: Vec::new(),
            tops: Vec::new(),
            bands: Vec::new(),
            rooms: Vec::new(),
            headed: Vec::new(),
            heads: Vec::new(),
            body: 0.0,
            least: 0.0,
        };
        if count <= 0 || columns <= 0 || !cell.is_finite() || cell <= 0.0 {
            return out;
        }
        out.count = count;
        let lines = (count / columns + 1) as usize;
        out.starts.reserve(lines);
        out.tops.reserve(lines);
        out.rooms.reserve(lines);
        let (mut y, mut seen) = (0.0f32, 0);
        let mut least = f32::INFINITY;
        for run in runs(count, columns, heads) {
            if run.head {
                seen += 1;
            }
            for row in 0..rows(run.len, columns) {
                let first = run.start + row * columns;
                let last = (first + columns).min(run.start + run.len);
                let headed = run.head && row == 0;
                let band = y;
                if headed {
                    y += HEAD;
                }
                let room = (first..last)
                    .map(|i| picture_room(cell, aspect(i)))
                    .fold(0.0, f32::max);
                let pitch = row_pitch(room);
                least = least.min(pitch);
                out.starts.push(first);
                out.tops.push(y);
                out.bands.push(band);
                out.rooms.push(room);
                out.headed.push(headed);
                out.heads.push(seen);
                y += pitch;
            }
        }
        out.body = y;
        out.least = least;
        out
    }

    /// Every frame a square, as the grid was before its rows took the
    /// pictures' shape, and as the lightbox, which fills its tiles,
    /// still is.
    pub fn square(cell: f32, columns: i32, count: i32, heads: &[i32]) -> Self {
        Self::new(cell, columns, count, heads, |_| 1.0)
    }

    /// How many rows.
    pub fn lines(&self) -> usize {
        self.starts.len()
    }

    /// Row `line`'s top and its picture room.
    #[cfg(test)]
    pub fn line(&self, line: usize) -> Option<(f32, f32)> {
        Some((*self.tops.get(line)?, *self.rooms.get(line)?))
    }

    /// The row frame `i` is on, held to the list.
    fn line_of(&self, i: i32) -> usize {
        self.starts.partition_point(|&s| s <= i).saturating_sub(1)
    }

    /// Where frame `i` sits, held to the list; `None` for an empty one.
    pub fn place(&self, i: i32) -> Option<Place> {
        if self.starts.is_empty() {
            return None;
        }
        let i = i.clamp(0, self.count - 1);
        let line = self.line_of(i);
        Some(Place {
            col: i - self.starts[line],
            line: line as i32,
            heads: self.heads[line],
            y: self.tops[line],
        })
    }

    /// The row whose band takes `y`, from the padding and the tiles, a
    /// heading counting as part of the row under it, held to the sheet.
    fn line_at(&self, y: f32) -> usize {
        self.bands
            .partition_point(|&b| b <= y)
            .saturating_sub(1)
            .min(self.lines().saturating_sub(1))
    }

    /// The first frame of the row whose band takes `y`; -1 for none.
    pub fn start_at(&self, y: f32) -> i32 {
        if self.starts.is_empty() {
            return -1;
        }
        self.starts[self.line_at(y)]
    }

    /// The last frame of row `line`.
    fn end_of(&self, line: usize) -> i32 {
        self.starts.get(line + 1).copied().unwrap_or(self.count) - 1
    }

    /// The frames on screen at `offset` on a sheet `height` tall, first
    /// and last, the partly shown rows at either edge included; the
    /// cells start `top` below the padding. `None` when there is nothing
    /// to show. This is what the grid reports so the worker makes those
    /// pictures before the folder's others; a shoot of hundreds must not
    /// be rendered up front, the same property the strip has.
    pub fn visible(&self, offset: f32, height: f32, top: f32) -> Option<(i32, i32)> {
        if self.starts.is_empty() || height <= 0.0 {
            return None;
        }
        let first = self.line_at(offset - PAD - top);
        let last = self.line_at(offset + height - PAD - top).max(first);
        Some((self.starts[first], self.end_of(last)))
    }

    /// The frames the grid has cells for at `offset`, as the first and
    /// how many: the rows on screen, the partly shown ones at either
    /// edge included, and [`MARGIN_ROWS`] either side. The rows counted
    /// are as many as the shortest row's pitch fits on the sheet, so a
    /// scroll moves the window without changing its size but where the
    /// list runs out.
    pub fn window(&self, offset: f32, height: f32, top: f32) -> (usize, usize) {
        if self.starts.is_empty() || height <= 0.0 || self.least <= 0.0 {
            return (0, 0);
        }
        let first = self
            .line_at(offset - PAD - top)
            .saturating_sub(MARGIN_ROWS as usize);
        let rows = (height / self.least).ceil() as usize + 1 + 2 * MARGIN_ROWS as usize;
        let end = self.starts.get(first + rows).copied().unwrap_or(self.count);
        let at = self.starts[first];
        (at as usize, (end - at) as usize)
    }

    /// How far the sheet can be scrolled: the padding, the folder tiles'
    /// `top`, and the rows, less what is on screen, never below nothing.
    pub fn max_scroll(&self, height: f32, top: f32) -> f32 {
        let body = top + self.body;
        if body <= 0.0 {
            return 0.0;
        }
        (2.0 * PAD + body - GAP - height).max(0.0)
    }

    /// The scroll that shows the selected frame's row with the least
    /// movement, its padding beside it, or `offset` when it is already
    /// on screen. A frame of the first row brings the folder tiles
    /// above it with it, and one of a heading's first row the heading,
    /// when the two fit on the sheet together; otherwise it is shown as
    /// any other. Everything past the ends is clamped away.
    pub fn reveal(&self, offset: f32, height: f32, selected: i32, top: f32) -> f32 {
        if selected < 0 || height <= 0.0 {
            return 0.0;
        }
        let Some(p) = self.place(selected) else {
            return 0.0;
        };
        let line = p.line as usize;
        let y = PAD + top + p.y;
        let bottom = y + self.rooms[line] + LABEL;
        let lead = if line == 0 && bottom + PAD <= height {
            0.0
        } else if self.headed[line] {
            y - HEAD - PAD
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
        wanted.clamp(0.0, self.max_scroll(height, top))
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
        // 4 columns, 11 frames, 176 cells: rows are 202 apart and the
        // first begins 12 down.
        let (cols, count, cell) = (4, 11, 176.0);
        assert_eq!(row_pitch(cell), 202.0);
        // At the top, a 500 tall sheet shows rows 0 to 2.
        assert_eq!(visible(0.0, 500.0, cell, cols, count, 0.0), Some((0, 10)));
        // Half a row down still shows the first row's top edge.
        assert_eq!(visible(100.0, 202.0, cell, cols, count, 0.0), Some((0, 7)));
        // Scrolled past the first row exactly: the second row alone,
        // with the third's top edge.
        assert_eq!(visible(214.0, 202.0, cell, cols, count, 0.0), Some((4, 10)));
        // Over-scrolled: the last row, not an index past the end.
        assert_eq!(
            visible(9000.0, 202.0, cell, cols, count, 0.0),
            Some((8, 10))
        );
        assert_eq!(visible(0.0, 0.0, cell, cols, count, 0.0), None);
        assert_eq!(visible(0.0, 500.0, cell, cols, 0, 0.0), None);
    }

    #[test]
    fn the_cells_cover_what_is_on_screen_and_a_row_either_side() {
        let (cols, count, cell, height) = (8, 20_000, 176.0, 773.0);
        // 773 over a pitch of 202 is four rows and a part, and a sixth
        // part-row as it scrolls: seven with a row either side, at any
        // offset.
        for offset in [0.0, 5.0, 100.0, 202.0, 1000.0, 123_456.0] {
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
            window(PAD + 3.0 * 202.0, height, cell, cols, count, 0.0).0,
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
        // with its padding: row 2 ends at 12 + 2*202 + 198 = 614.
        assert_eq!(
            reveal(0.0, height, cell, cols, count, 9, 0.0),
            614.0 + PAD - height
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
        assert_eq!(tiles_top(1, cell, cols), 92.0);
        assert_eq!(tiles_top(4, cell, cols), 92.0);
        assert_eq!(tiles_top(5, cell, cols), 184.0);
        // At the smallest cell the row is 48 and its gap.
        assert_eq!(tiles_top(3, STEPS[0], cols), 52.0);
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
            window(top + PAD + 3.0 * 202.0, height, cell, cols, count, top).0,
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
            614.0 + top + PAD - height
        );
        // And back up to the second row: its top at the sheet's top
        // with the padding, the tiles above it scrolled away.
        let down = reveal(0.0, height, cell, cols, count, 39, top);
        assert_eq!(reveal(down, height, cell, cols, count, 4, top), top + 202.0);

        // Tiles taller than the sheet: six columns, 820 px, and 54
        // folders make nine tile rows, 828 px. Scrolled past them, a
        // frame of the first row is shown where it is, not under the
        // tiles at the top.
        let (cols, height) = (6, 820.0);
        let top = tiles_top(54, cell, cols);
        assert_eq!(top, 828.0);
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
        // Two 512 cells and their gaps come to 1052 of 1500.
        assert_eq!(slack(1500.0, 512.0, 2), (1500.0 - 1052.0) / 2.0);
        // Eight 176 cells and their padding fill 1460 of it.
        assert_eq!(slack(1500.0, 176.0, 8), 20.0);
        // A row wider than the window is not pulled left.
        assert_eq!(slack(200.0, 512.0, 1), 0.0);
    }

    #[test]
    fn the_zoom_steps_and_stops() {
        assert_eq!(zoom(176.0, 1), 256.0);
        assert_eq!(zoom(176.0, -1), 128.0);
        assert_eq!(zoom(512.0, 1), 512.0);
        assert_eq!(zoom(96.0, -1), 96.0);
        // From a size that is not a step, the next step in the
        // direction asked, never past it.
        assert_eq!(zoom(180.0, 1), 256.0);
        assert_eq!(zoom(174.0, 1), 176.0);
        assert_eq!(zoom(178.0, -1), 176.0);
        assert_eq!(zoom(174.0, -1), 128.0);
        assert_eq!(zoom(500.0, 1), 512.0);
        assert_eq!(zoom(100.0, -1), 96.0);
        assert_eq!(zoom(174.0, 2), 256.0);
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
    /// Two runs under headings, 5 frames and 3, in 4 columns: the
    /// second heading starts a row of its own, the frames under it on
    /// the rows below, and the arrows go over it.
    #[test]
    fn headings_break_the_rows_and_the_arrows_go_over_them() {
        let (cols, count, heads) = (4, 8, [0, 5]);
        assert_eq!(place(0, cols, &heads), (0, 0, 1));
        assert_eq!(place(4, cols, &heads), (0, 1, 1));
        // The second heading's first frame: column 0, the third row,
        // two headings above it.
        assert_eq!(place(5, cols, &heads), (0, 2, 2));
        assert_eq!(place(7, cols, &heads), (2, 2, 2));
        // Without headings, the plain grid.
        assert_eq!(place(5, cols, &[]), (1, 1, 0));
        // Down from the last frame above a heading lands on the first
        // below it; up comes back.
        assert_eq!(step_in(4, 0, 1, cols, count, &heads), 5);
        assert_eq!(step_in(5, 0, -1, cols, count, &heads), 4);
        // Down from column 3 of the first row: column 0 of the second,
        // the short row's one frame.
        assert_eq!(step_in(3, 0, 1, cols, count, &heads), 4);
        assert_eq!(step_in(1, 0, 1, cols, count, &heads), 4);
        // Along the list, as ever, and held at the ends.
        assert_eq!(step_in(4, 1, 0, cols, count, &heads), 5);
        assert_eq!(step_in(7, 0, 1, cols, count, &heads), 7);
        assert_eq!(step_in(0, 0, -1, cols, count, &heads), 0);
        // The sheet is two headings and three rows long.
        let cell = 176.0;
        let pitch = row_pitch(cell);
        assert_eq!(
            max_scroll_in(100.0, cell, cols, count, 0.0, &heads),
            2.0 * PAD + 3.0 * pitch + 2.0 * HEAD - GAP - 100.0
        );
        assert_eq!(
            max_scroll_in(100.0, cell, cols, count, 0.0, &[]),
            max_scroll(100.0, cell, cols, count, 0.0)
        );
        // At the top a tall sheet shows every frame; scrolled to the
        // third row, the second run alone.
        assert_eq!(
            visible_in(0.0, 2000.0, cell, cols, count, 0.0, &heads),
            Some((0, 7))
        );
        let third = PAD + 2.0 * pitch + 2.0 * HEAD;
        assert_eq!(
            visible_in(third, 100.0, cell, cols, count, 0.0, &heads),
            Some((5, 7))
        );
        // The cells cover the screen's rows and one either side.
        assert_eq!(
            window_in(third, 100.0, cell, cols, count, 0.0, &heads),
            (4, 4)
        );
        // Revealing the second run's first frame from the foot brings
        // its heading in above it.
        let max = max_scroll_in(250.0, cell, cols, count, 0.0, &heads);
        let to = reveal_in(max, 250.0, cell, cols, count, 5, 0.0, &heads);
        assert_eq!(to, third - HEAD - PAD);
        // A frame below the fold comes up against the foot.
        let down = reveal_in(0.0, 250.0, cell, cols, count, 7, 0.0, &heads);
        assert_eq!(down, (third + cell + LABEL + PAD - 250.0).min(max));
    }

    /// A row of 3:2 frames: the square cell less the plate above and
    /// below the picture, 56 px at the 176 cell, and the same for a
    /// frame whose shape is not known yet, laid out as 3:2.
    #[test]
    fn a_row_of_landscapes_loses_the_plate() {
        let cell = 176.0;
        assert_eq!(picture_room(cell, 1.0), cell);
        assert_eq!(picture_room(cell, 2.0 / 3.0), 120.0);
        assert_eq!(picture_room(cell, 0.0), 120.0);
        // The wheel's notch is a third of that row.
        assert_eq!(common_pitch(cell), 146.0);
        // A camera's thumbnail, 176 by 117, rounds to the same room.
        assert_eq!(picture_room(cell, 117.0 / 176.0), 120.0);
        // A portrait is held to the square it had.
        assert_eq!(picture_room(cell, 1.5), cell);
        for aspect in [2.0 / 3.0, 0.0] {
            let l = Layout::new(cell, 4, 8, &[], |_| aspect);
            assert_eq!(l.line(0), Some((0.0, 120.0)));
            assert_eq!(l.line(1), Some((146.0, 120.0)));
            assert_eq!(row_pitch(120.0), row_pitch(cell) - 56.0);
        }
        // The sheet is two of those rows long.
        let l = Layout::new(cell, 4, 8, &[], |_| 0.0);
        assert_eq!(
            l.max_scroll(100.0, 0.0),
            2.0 * PAD + 2.0 * 146.0 - GAP - 100.0
        );
    }

    /// A portrait among landscapes makes its row as tall as the square
    /// it had; the rows either side keep theirs, and the ones under it
    /// move down by the difference.
    #[test]
    fn a_portrait_raises_its_row_only() {
        let cell = 176.0;
        let aspect = |i: i32| if i == 5 { 1.5 } else { 2.0 / 3.0 };
        let l = Layout::new(cell, 4, 12, &[], aspect);
        assert_eq!(l.line(0), Some((0.0, 120.0)));
        assert_eq!(l.line(1), Some((146.0, cell)));
        assert_eq!(l.line(2), Some((146.0 + 202.0, 120.0)));
        // Every frame of the row is at its top.
        for i in 4..8 {
            assert_eq!(l.place(i).unwrap().y, 146.0, "{i}");
        }
        assert_eq!(l.place(8).unwrap().y, 348.0);
        assert_eq!(
            l.place(9),
            Some(Place {
                col: 1,
                line: 2,
                heads: 0,
                y: 348.0
            })
        );
        // The frame two rows under it, revealed from the top, comes up
        // against the foot by the tall row's height.
        let r = l.reveal(0.0, 300.0, 9, 0.0);
        assert_eq!(r, PAD + 348.0 + 120.0 + LABEL + PAD - 300.0);
    }

    /// A list of shapes that repeats on no short period: landscapes,
    /// portraits, squares, panoramas, and frames with none yet.
    fn shapes(seed: u32) -> impl Fn(i32) -> f32 {
        move |i: i32| {
            let h = (i as u32 ^ seed).wrapping_mul(2_654_435_761) >> 27;
            [2.0 / 3.0, 1.5, 1.0, 0.25, 0.0, 0.75, 2.0 / 3.0, 0.5][(h % 8) as usize]
        }
    }

    /// The rows by a walk over every frame: each frame's row, the rows'
    /// first frames, bands, tops, and rooms.
    struct Walk {
        line_of: Vec<usize>,
        firsts: Vec<i32>,
        bands: Vec<f32>,
        tops: Vec<f32>,
        rooms: Vec<f32>,
        headed: Vec<bool>,
        body: f32,
    }

    fn walk(
        cell: f32,
        columns: i32,
        count: i32,
        heads: &[i32],
        aspect: &dyn Fn(i32) -> f32,
    ) -> Walk {
        let mut w = Walk {
            line_of: Vec::new(),
            firsts: Vec::new(),
            bands: Vec::new(),
            tops: Vec::new(),
            rooms: Vec::new(),
            headed: Vec::new(),
            body: 0.0,
        };
        let (mut y, mut col) = (0.0, columns);
        for i in 0..count {
            let head = heads.contains(&i);
            if head || col == columns {
                if let Some(room) = w.rooms.last() {
                    y += room + LABEL + GAP;
                }
                w.bands.push(y);
                if head {
                    y += HEAD;
                }
                w.firsts.push(i);
                w.tops.push(y);
                w.rooms.push(0.0);
                w.headed.push(head);
                col = 0;
            }
            let room = w.rooms.last_mut().unwrap();
            *room = room.max(picture_room(cell, aspect(i)));
            w.line_of.push(w.firsts.len() - 1);
            col += 1;
        }
        w.body = w.rooms.last().map_or(0.0, |r| y + r + LABEL + GAP);
        w
    }

    /// Every question of a layout of mixed shapes, with headings and
    /// without, against a walk over every frame and a scan over its
    /// rows.
    #[test]
    fn the_layout_agrees_with_a_walk_over_every_frame() {
        let cell = 176.0;
        let top = 92.0;
        for (seed, columns, count, heads) in [
            (1, 4, 37, vec![]),
            (2, 1, 9, vec![]),
            (3, 6, 100, vec![0, 13, 14, 50]),
            (4, 5, 41, vec![7, 30]),
            (5, 8, 8, vec![]),
        ] {
            let aspect = shapes(seed);
            let l = Layout::new(cell, columns, count, &heads, &aspect);
            let w = walk(cell, columns, count, &heads, &aspect);
            assert_eq!(l.lines(), w.firsts.len());
            assert_eq!(l.starts, w.firsts);
            assert_eq!(l.tops, w.tops);
            assert_eq!(l.bands, w.bands);
            assert_eq!(l.rooms, w.rooms);
            for i in 0..count {
                let line = w.line_of[i as usize];
                let p = l.place(i).unwrap();
                assert_eq!(p.line as usize, line);
                assert_eq!(p.col, i - w.firsts[line]);
                assert_eq!(p.y, w.tops[line]);
                // The headings' arithmetic and the layout's agree.
                assert_eq!(place(i, columns, &heads), (p.col, p.line, p.heads));
            }
            let height = 400.0;
            let max = 2.0 * PAD + top + w.body - GAP - height;
            assert_eq!(l.max_scroll(height, top), max.max(0.0));
            // The row a height is in, by a scan.
            let scan = |y: f32| {
                (0..w.bands.len())
                    .rev()
                    .find(|&k| w.bands[k] <= y)
                    .unwrap_or(0)
            };
            let mut offset = -30.0;
            while offset < max + 300.0 {
                let a = scan(offset - PAD - top);
                let b = scan(offset + height - PAD - top).max(a);
                let end = w.firsts.get(b + 1).copied().unwrap_or(count) - 1;
                assert_eq!(
                    l.visible(offset, height, top),
                    Some((w.firsts[a], end)),
                    "{seed} {offset}"
                );
                let (at, n) = l.window(offset, height, top);
                assert!(
                    at as i32 <= w.firsts[a] && end < (at + n) as i32,
                    "{seed} {offset}"
                );
                offset += 37.0;
            }
            // Every frame revealed from either end is on screen with its
            // padding, or as far as the sheet goes, and one on screen
            // stays put.
            for i in 0..count {
                let line = w.line_of[i as usize];
                let y = PAD + top + w.tops[line];
                let bottom = y + w.rooms[line] + LABEL;
                for from in [0.0, max.max(0.0)] {
                    let r = l.reveal(from, height, i, top);
                    assert!((0.0..=max.max(0.0)).contains(&r), "{seed} {i}");
                    assert!(
                        (y - PAD >= r || r == 0.0) && (bottom + PAD <= r + height || r == max),
                        "{seed} {i} from {from}: {r}"
                    );
                    if w.headed[line] && line > 0 && r < from {
                        assert_eq!(r, y - HEAD - PAD, "{seed} {i}");
                    }
                    assert_eq!(l.reveal(r, height, i, top), r, "{seed} {i}");
                }
            }
        }
    }

    /// Over twenty thousand frames the layout is a prefix sum: made
    /// well inside a frame's time.
    #[test]
    fn a_layout_of_twenty_thousand_is_quick() {
        let aspect = shapes(7);
        let started = std::time::Instant::now();
        let l = Layout::new(176.0, 8, 20_000, &[], &aspect);
        let took = started.elapsed();
        assert_eq!(l.lines(), 2500);
        println!("layout of 20000 frames: {:.3} ms", took.as_secs_f64() * 1e3);
        assert!(took.as_secs_f64() < 0.05, "{took:?}");
    }
}
