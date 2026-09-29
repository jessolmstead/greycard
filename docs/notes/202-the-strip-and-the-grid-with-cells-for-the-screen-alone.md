# 202. The strip and the grid with cells for the screen alone (2026-09-28)

Roadmap line: a virtualized grid and strip, only the rows on
screen given a cell, where every row of a 20,000-frame folder had one
and a frame cost 45 to 400 ms (§191).

**What it was.** The strip was a `HorizontalLayout` with a `for` over
`thumbs`, the grid a `for` over `thumbs` placed by hand, so a folder of
twenty thousand was forty thousand cells of a dozen elements each with
the grid open, and twenty thousand in the loupe. Slint lays out and
visits every one of them for every frame, clipped or not. §191 made the
pictures come in without holding the window, and left this as the
longest wait: a frame over the folder, whether the pictures were coming
or the grid was being scrolled. It also left rows far from the screen
without their picture until a scroll came near them, with three ranges
deciding what "near" was (256 rows round the current frame, and the
grid's and the strip's last ranges widened by as much again), because a
picture put on a row cost the next frame more than it cost going on.

**Now.** The list keeps every row, and the views keep a window of it
(`cells.rs`).
- `thumbs` is a model of our own, `Rows`, holding every row as before:
  the header reads the current frame's name there, and Rust sets a
  row's picture, badges and `chosen` there through `set_row_data`, as
  it did. The strip and the grid repeat over `strip-cells` and
  `grid-cells` instead, each a `Cells` model over the same rows: a
  window of the rows on screen and a margin, each cell a `ThumbCell`
  of the row's number and the row. A cell is placed by its row's
  number, so the strip lost its `HorizontalLayout` for cells placed at
  the pad and a pitch a row, which is where the layout put them.
- A window's cells are slots, and a row always takes the slot its
  number falls on modulo the slots. A scroll of one row makes cells
  afresh in the slots of the rows that left, for the rows that came
  in, and every other cell keeps its row, its bindings and the texture
  femtovg made of its picture (kept per image element). A window that
  renumbered every cell on every scroll would hand every cell on
  screen a new picture at every notch of the wheel. Whether a binding
  that is re-evaluated to the same picture also costs an upload was
  not measured; the slots were chosen so the question does not arise.
- A slot whose row changes is a new cell: the model says the slot was
  removed and added (`row_removed`, `row_added`), and Slint's repeater
  makes a new instance there and hands the others their own data
  again, which changes nothing (a property set to its value is not
  marked). A move past the whole window resets it. The first cut
  handed the kept cell the new row (`row_changed`), and the cell
  carried over what the old row had left in it: `animate background`
  faded from the old row's ground, so after a jump the slot that had
  held the current frame wore the selection's ground on another row
  for `Theme.quick` (found in the review). Hover and a press waiting
  for its release live in the cell too, and go with it now. Fling and
  notch timings were the same after the change (below).
- A change to a row reaches the view with a cell for it as a change
  to that one cell (`row_changed` on the slot), and nothing else: a
  row without a cell costs a `set_row_data` and no more.
- The windows are moved from the reports the views already sent. The
  grid's report (`grid-range`: scroll, height, columns) gives its
  window through `grid::window`, the rows partly or wholly on screen
  and one either side, as many at any scroll for a given height and
  cell, so a scroll moves the window without resizing it. The strip's
  (`strip-range`) gives the rows it reports and three either side
  (`cells::STRIP_MARGIN`): its report runs after its move, from a
  change handler, so the margin is what a fast drag's next frame is
  drawn from. A window's slot count changes only when the view wants
  more, or fewer than half: the strip's range is one frame longer or
  shorter depending on the scroll's phase, and that would otherwise
  make every cell again.
- The strip now reports what it shows with nothing selected too.
  Before, its reveal returned early without a selection, which cost
  nothing while every row had a cell; now a strip that has not
  reported has no cells. The first test to click a strip cell with
  nothing selected (the sync sheet's) found it.

**The pictures follow the cells.** §191's "rows far from the screen
left for a scroll" is gone, and so is its notion of near. A row
carries its picture exactly while a view has a cell for it
(`settle_pictures`): when a window moves, the rows it came to take
their pictures from `thumb_base`, turned as the frame is turned now,
and the rows no view has a cell for any more have theirs taken off.
`show_thumb` puts a picture on a row with a cell and otherwise leaves
it in `thumb_base`. So a picture is held once, as bytes, for every
frame of the folder, and a second time, as an image, for a few screens'
worth; §191 held the second copy for every row that had ever been near
the screen, and master before it for every row. `rebuild_browser`
carries nothing across any more: it makes the rows without pictures and
puts on the pictures of the rows with cells from the bytes, a few
hundred at most however long the list. That drops `rows_shown` and the
carry by path, which a review's bug (the rejects moved out, and a
picture carried to its neighbor's row) was about; nothing is carried now, so nothing can land
on a row renumbered under it. `ROWS_AROUND`, `near_screen`,
`row_near_screen` and `fill_near_screen` are gone, and so are the fills
in `open_row`: a jump moves the strip or the grid, which reports.

**Why not Slint's `ListView`.** Slint virtualizes a `for` inside a
`ListView`, and the grid could be rows of cells, a `ListView` over the
rows with a horizontal `for` in each. Rejected:
- `ListView` scrolls vertically only, so the strip would still need a
  window of its own: two mechanisms for one job.
- The grid's model would become rows of rows, rebuilt at every change
  of the column count (a resize, a zoom), and every change to a frame
  would have to find its row's inner model.
- `ListView` owns its scroll. The grid's wheel (a third of a row a
  notch, Ctrl for the zoom), its reveal and its reports are arithmetic
  in `grid.rs` with tests; they would become bindings on the view's
  `viewport-y`, and a `ListView` instantiates by estimated heights.

A window over the flat list costs two small models and a callback
already there, and the grid's arithmetic stays where it is tested.

Also rejected, the window in Slint alone: `for k in N` with each cell
reading `root.thumbs[first + k]`. Less Rust, but indexing a model in
Slint tracks one "some row changed" property for the whole model, so
any `set_row_data` on a row ever indexed (a badge, a `chosen` flag on
every arrow in the grid) dirties every cell's binding, and every scroll
dirties every cell's index. What that costs the renderer in textures
was reasoned from the source, not measured.

**Measured.** A folder of 20,000 hard links to the 33 sample raws in
`~/Pictures/Test`, and one of 200, under the worktree's `target/scratch`,
every run with its own `XDG_CONFIG_HOME`, `XDG_DATA_HOME`,
`XDG_CACHE_HOME` and `XDG_STATE_HOME` there, the thumbnail cache warmed
by a run first (every run below: 20,000 hits). Release builds, headless
mutter at 1920x1200 through its Xwayland, `SLINT_SCALE_FACTOR=1`, the
window at its own 1500x950, the RTX 5070 Ti, 8 pool threads, opened on
the first frame. "Before" is master with the new timing flag only
(first commit on the branch). A hidden `--time-scroll N` moves the grid,
or the strip in the loupe, 1,000 px a frame once the pictures are in (a
fling; back at either end), and logs the time from each move to the end
of the frame that draws it; the folder's log has a new line for the
first frame that draws the rows on screen with their pictures. Other
builds ran on the machine throughout: runs were interleaved, each
started once the load average was under 12 (it was 9.4 to 15.6 at the
starts, 22 and 27 for two runs noted below). Three or four runs each:

The 20,000, grid (`--grid`):
- Cells made: 40,000 before (the grid's and the strip's), 56 after
  (seven rows of eight; the strip has none while the grid was opened
  over it at launch, since it reports only out of the grid).
- The first frame (the rendering setup's `gpu:` line, uptime): 2.12 to
  2.50 s before, 0.46 to 0.65 s after.
- The rows on screen drawn with their pictures: 1.79 to 2.14 s from the
  folder's open before, 0.23 to 0.34 s after. Before, that was the
  first frame: the pictures on screen were in long before the window
  could draw them.
- Scrolling a fling's 1,000 px a frame, move to the end of its frame:
  mean 90.6 to 96.3 ms (95th percentile 96 to 128, the most 148 to
  164) before; 12.6 to 14.2 ms (95th 14.3 to 17.7, the most 19.6 to
  22.6) after. Of that, the frame's own drawing: 63.6 to 67.9 ms
  against 11.6 to 12.9.
- A wheel's notch, 69 px a frame (after only; the before binary has no
  step flag): 16.1 ms twice, the frame's drawing 8.6 and 10.0 ms; a
  third run, the first of its round at load 17, had 38.8 and 36.4.
- The longest wait between two frames while the pictures came: 298 to
  351 ms before, with 2 to 5 frames drawn; 106 to 195 ms after, with
  119 to 133 frames drawn. The window's longest turn was 1 to 2 ms, so
  the 100-odd ms is not the pictures.
- Resident at the end of the scroll: 2,809 to 2,824 MB before, 2,327 to
  2,356 MB after.

The 20,000, loupe (the strip):
- Cells made: 20,000 before, 17 after.
- The first frame at 1.97 to 2.21 s before, 0.54 to 0.58 s after; the
  strip's rows drawn with their pictures at 1.88 to 2.09 s against 0.30
  to 0.32 s.
- A fling: 39.7 to 42.9 ms (drawing 28.6 to 30.6) before, 16.4 to 16.5
  ms (drawing 7.9 to 9.2) after, which is the display's 60 Hz: the
  95th percentile 17.9 to 19.8. A notch (62 px): 16.3 to 16.5 ms.
- Resident: 2,583 to 2,619 MB before, 2,319 to 2,363 MB after.

After the review's change to making a re-pointed slot's cell afresh,
two runs each at load 11 to 15: the grid's fling 12.2 and 12.4 ms
(drawing 11.2 and 11.5), a notch 16.1 and 16.2 ms, the strip's fling
16.5 and 16.6 ms; the rows on screen with their pictures at 0.23 to
0.30 s; resident 2,325 to 2,349 MB. The same as before the change.
The snapshots were taken again and still match.

The first frame that draws the rows on screen with their pictures is
asked about at each frame until it comes, and for no longer than 30 s
(`GRID_WAIT`) after the folder opens: a view that never says what it
shows is not asked about at every frame for the session.

One before run of the grid at load 27 never drew a frame until the
last picture was in: 9.0 s, all 20,000 taken in 2 turns. The after run
beside it drew its first frame at 0.64 s.

The last picture taken in came later after than before: 2.72 to 2.98 s
from the folder's open against 2.19 to 2.89 s, the pool's summed time
21.6 to 23.6 s against 17.3 to 22.3 s. The window's own share fell
(0.01 s in all, in 3,000 to 5,400 turns, against 0.04 s in about 20)
and it drew 120 to 140 frames meanwhile instead of 2 to 10. Nothing on
screen waits for it: the rows on screen are in by 0.34 s. What makes
the pool's pictures slower was not pinned down; the frames the window
now has time to draw share the machine with it, and the machine was
loaded throughout.

The 200 (nothing should change, and nothing did):
- Cells made: 400 before (grid and strip) and 56 after in the grid; 200
  against 17 in the loupe.
- First frame 0.44 to 1.24 s before and 0.41 to 1.15 s after in the
  grid (the slow one of each at load 16 and 22), 0.41 to 0.52 against
  0.42 to 0.65 in the loupe; rows on screen with their pictures 0.25 to
  0.59 s against 0.24 to 0.82 s, and 0.32 to 0.39 against 0.23 to 0.37.
- A fling in the grid: 18.4 to 21.6 ms before, 17.5 to 30.8 after (the
  30.8 at load 22; 17.5 and 18.5 otherwise). At 1,000 px on a sheet of
  25 rows the grid bounces off an end every few moves, and most cells
  on screen take a new row at each. The strip: 18.5 to 19.1 against 17.6
  to 18.9.

Grid snapshots and loupe snapshots (`--snapshot`) against the before
binary, opened at the top and on the middle (frame 15,007 of the
20,000, frame 100 of the 200), are pixel for pixel the same in the
strip and the grid (ImageMagick `compare`). What differs is the header's
facet counts, which follow the library's indexing of the folder as it
runs, and the develop's time in the status line.

**Tests.**
- `cells`: a row takes the slot its number falls on, and a one-row
  scroll changes only the slots of the rows that came in; the window
  is held to the list; it keeps its slots through a range that wobbles
  by one and follows a real change; a change to a row reaches its cell;
  a new, shorter list is held to.
- `grid`: the window covers every row on screen at any offset, with a
  row either side, and is the same size at every offset.
- browser, headless, 5,000 frames at 1500x950: the strip makes at most
  the frames on screen and the margin (15 cells, 9 on screen), the grid
  7 rows of 8 (32 on screen at the top), and only those 56 rows carry a
  picture; scrolled to the end and back, in the grid and the strip, the
  last frame has a cell at the end, the cell count is the same, the
  current frame is still current and selected, and a picture that came
  for it while it was away is the one its cell shows on return; a far
  frame made current (4,321 in the grid, 3,210 in the strip) is scrolled
  to and has a cell and its picture.
- §191's tests rewritten for the new rule: a row carries its picture
  while a view has a cell for it (on, off when the grid scrolls away,
  on again, a larger picture that came meanwhile the one put on); a
  picture that came while the filter hid its frame waits for a cell
  when the filter clears.
- The layout tests are unchanged and pass. Tests that put pictures on
  rows now give the rows cells first, as a laid-out window would (the
  rejects-moved-out test in `roots.rs` gives the strip eight).

**Not done.**
- `thumb_base` still holds every frame's bytes: 20,000 at 176 px is
  about 1.2 GB, and at the grid's 360 px about 5 GB. The cells bound
  the images, not the bytes; bounding those means taking them from the
  thumbnail cache again when a row comes to a cell.
- Hover and a press: a cell made afresh under a still pointer after a
  wheel fling has no hover until the pointer moves, and a press whose
  cell is re-pointed before the release goes with the old cell, so the
  release clicks nothing. On the first cut, which handed a kept cell
  the new row, the old hover stayed on the new row and the release
  clicked it. Both reasoned from Slint's repeater, neither observed;
  no test presses and scrolls at once.
- The last of the 20,000 pictures lands later than before (2.72 to
  2.98 s against 2.19 to 2.89 s). The review measured the window
  taking them in over 3,900 to 6,400 turns and drawing about 120
  frames during the load, and the pool's summed lookup time up about
  25%. The cause was not isolated. Nothing on screen waits on it.
- `take_thumbnail`'s check that a picture that came too small is asked
  for again compares the file's number with the grid's rows
  (`grid_shown`), which are not the same numbers under a filter. Found
  here, older than this, not fixed.
- The grid's window stays where it was when the grid closes, so up to
  a screenful of rows keeps its picture while nobody can see them. It
  is what lets the grid come back filled.
- Not measured on the all-roots view or on a network root: the list
  and the windows do not depend on where the rows come from.
