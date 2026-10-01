# 214. A scroll bar on the grid (2026-09-30)

Roadmap line: a scroll bar on the grid: a thin bar at the sheet's
right edge sized and placed from the scroll, its maximum and the
sheet's height, shown while scrolling or hovered and faded at rest,
draggable, and a click in its track paging; the same offset the wheel
and the keys drive, so §202's cells stay the screen's alone.

**What it was.** The grid had no bar. §202 made the sheet one
TouchArea slid by `grid-scroll`, with no ScrollView to draw one, so a
folder of a few hundred frames gave no sign of how long it was or
where the screen stood in it, and the only ways down were the wheel
and the arrows.

**Now.** A bar in the sheet's right edge, over the cells, in the band
the pane's lists get from the fluent style: 14 px
(`Theme.scrollbar-band`), a 2 px thumb 4 px in from the edge that
widens to 6 px on a rounded track under the pointer. It is not the
widget: the offset stays the window's own. The thumb is solid grey,
`#8a8a8a` (`scrollbar-thumb`, about 5.6:1 on the dark ground and 3.5:1
on white), `#b4b4b4` once it widens (`scrollbar-thumb-hover`), on
fluent-dark's alternate background `#2c2c2c` (`scrollbar-track`). The
first cut took fluent-dark's border, `#ffffff14`, for the thumb as
well. It barely read: 2 px of 8% white vanished over a bright picture,
and the hovered thumb on the track was about 1.2:1. Fluent's bar gets
away with it because it has arrow buttons and a track border, and
this bar has neither (found in review). The thumb's place, length,
width and x are rounded to whole device pixels, so it is crisp at 1.25
and 1.5.
- Sized and placed by `grid.rs`, through three pure callbacks as the
  rest of the layout is: `bar_length(height, max)`, the track (the
  sheet less a 4 px inset at each end) in the share of the whole sheet
  on screen, never under 24 px where the track has room;
  `bar_offset(height, max, scroll)`; and `bar_drag(height, max, from,
  by)`, the scroll a thumb taken at `from` and moved `by` comes to, so
  the thumb stays under the pointer. `max` is `grid-max`, which already
  holds the tiles' rows (§212); nothing else in `grid.rs` changed.
- Hidden when `grid-max` is 0: the band is not visible and takes no
  press, and the cell under it gets the click as before.
- A drag of the thumb and a press in the track both go through a new
  `grid-scroll-to(length)` in `app.slint`, the wheel's path: the offset
  held to [0, max], then `grid-report`, so the §202 range and the cells
  follow. A press above the thumb pages up a sheet's height, below it
  down; one page a press, no repeat while held. A screen reader's
  increment and decrement on the bar page the same way.
- A drag steps the sheet at most once a frame. Each report on a long
  folder whose step lands more than a screen away takes the window's
  full reset (§202) and puts every cell's picture on again. Measured on
  2,000 hard links in a release build with `GREYCARD_UI_TIMING=1` and a
  temporary timer round the `grid-range` handler: a drag in 40 px moves
  (about 3,400 px of sheet, 16 rows, each) cost 5.9 to 7.5 ms a report,
  20 runs. One move a frame is fine. But a mouse at 125 Hz sends two
  moves a frame, which would be 14 ms of reports, and one at 500 to
  1,000 Hz sends eight to sixteen, which would be 50 to 110 ms. So a
  move now only notes the scroll it wants. A 16 ms timer, running only
  while a step waits, applies the last one and reports. The release
  applies whatever is still waiting. The thumb follows the pointer at
  once, from the wanted scroll, and the sheet catches up within a
  frame. `--keys` sends a move every 200 ms, so a fast mouse was not
  replayed in the editor. The headless test drags 16 moves at once and
  gets one report, then three moves 20 ms apart and gets three.
- The band is the bar's: a press in it, left or right, never reaches
  the cell under it (no selection, no frame menu). A press that closes
  a menu is nothing more, as on a cell, and leaves the focus to the
  menu's closing. The wheel over the band is `grid-wheel`, Ctrl and
  all. Any other press, of any button, gives the keys the focus
  (`focus-keys`). After a left press the release gives it again, since
  the drag may have moved it, but only when the press was the bar's.
- A tap in the band at rest pages, though the bar is faded out there.
  That is mainly a touch or pen tap at the sheet's right edge.
- Shown while the sheet scrolls and while the pointer is over the band
  or holds the thumb; faded otherwise, the opacity animated over
  200 ms. Every change of `grid-scroll` stamps a counter and wakes the
  bar; a 600 ms Timer, running only while the bar is awake, puts it to
  sleep on the first tick that finds no new stamp and no thumb held.
  So it fades 0.6 to 1.2 s after the last scroll. The pointer leaving
  the band wakes it the same way, so it lingers rather than vanishing.
  The Timer is stopped at rest and the opacity has reached 0, so
  nothing asks for a frame (§208). A sheet re-flowed or opened at a
  scroll that changes shows the bar briefly too, which reads as a hint.

The Timer has no restart in the `.slint` language, which is why it
compares stamps rather than being restarted at each scroll.

**Checked in the editor**, on 330 hard links of the sample raws under
the worktree's `target/scratch/many`, every XDG directory in the
sandbox, `--no-sidecars --grid`, a debug build, driven by `--keys`
(`target/scratch/shots/`):
- `hover.png` (`hover-edge.png`, the edge): the pointer moved onto the
  band. The track and the 6 px light thumb at the top, about 61 px
  long over a sheet of 55 rows. Retaken with the new colors.
- `drag.png` (`drag-edge.png`): a press on the thumb and a move of
  360 px, held. The sheet is at F0139 to F0168, the thumb is in the
  middle of the track, the header is still on F0001, and the press
  selected nothing. Retaken with the new colors. `drag-125.png`
  (`drag-125-edge.png`) is the same at `SLINT_SCALE_FACTOR=1.25`, the
  thumb on whole pixels.
- `paged.png`: two presses near the track's foot, the pointer left in
  the band: two sheets down, the thumb a sixth of the way.
- `rest.png` with `GREYCARD_UI_TIMING=1` (log under
  `target/scratch/state/greycard/`): a track press, a move along the
  band and one off it, then 30 empty steps. Frames came for the press
  and the hover's widening (23.4 to 24.4 s), then the fade (25.10 to
  25.33 s), then none at all until the snapshot at 31.96 s. The edge
  in `rest.png` has no bar.
`edges.png` sets the hover's and the drag's edges side by side.
`paged.png` and `rest.png` predate the color change.

**Tests.** `grid.rs`: the thumb is the track's share of the sheet on
screen, at the inset at 0 and the far inset at the end, held at the
ends out of range, 24 px for a million pixels of scroll, none when
nothing scrolls, the track when the track is shorter than the least
thumb. A drag down the whole room is the whole range, halfway is half,
past either end is the end, and the thumb lands under the pointer. The
same holds with the least thumb engaged: over a million pixels, the
room is the whole range and a drag partway puts the thumb under the
pointer.
Headless browser at 1500 by 950: the band is 14 px at the sheet's
right edge, 837 tall under the header. A thumb dragged 150 px in 15
moves with no time between them comes to `bar_drag`'s scroll in one
report. Three moves 20 ms apart make three reports. The cells hold
the rows then on screen, the selection is unchanged, and G still
closes the grid (the keys kept the focus). A thumb dragged 2,000 px
past the foot holds at the end, and brought back to 50 px under the
grab it comes to `bar_drag(h, max, 0, 50)`, with no jump. With the
frame menu up, a press in the track closes the menu and pages nothing,
and the next press pages. Presses below the thumb
page 837 and 1,674, above it back to 837 and held at 0, at the foot
held at the end; the wheel over the band scrolls. On a sheet exactly a
row of eight wide, the last column reaching under the band: with 8
frames (no bar) a left and a right press there click frame 7 and ask
its menu; with 400 they do neither. The bar sleeps when opened, wakes
with the wheel, stays awake through scrolls 400 ms apart, sleeps two
quiet ticks later and stays asleep with nothing reported for 5 s more.
A folder that fits has no bar (not found by its accessible label), a
press and a wheel down the band move nothing, and a longer folder
brings it.

**Decisions made in the build.**
- The drag's step timer is 16 ms, a 60 Hz frame. It is a timer, not
  the frame itself, so on a faster display the sheet may trail the
  thumb by a frame.
- The band is there, invisible, whenever the sheet scrolls, so a hover
  can find the bar at rest. Its 14 px take the presses at the sheet's
  right edge; the last column's cell ends 12 px plus the slack in, so
  at most 2 px of a cell lose their click, and only on a sheet whose
  slack is 0.
- The band is a slider to a screen reader ("Grid scroll", 0 to the
  maximum, the scroll as its value). Increment and decrement page; it
  has no set-value action.
- The track runs the sheet's full height with a 4 px inset, with no
  arrow buttons, so the fluent bar's 16 px end offsets are not taken.

**Not done.**
- No press-and-hold repeat in the track.
- No keyboard to the bar itself; Page Up and Page Down on the grid are
  not part of this.
- A snapshot of the bar awake from a scroll alone: `--snapshot` waits
  1.5 s after the last key, past the fade, so the snapshots show the
  hovered or held bar.
- Not checked on a Mac or Windows, or on a touchpad's fine scroll.
