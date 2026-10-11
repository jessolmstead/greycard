# §307. The lightbox (2026-10-11)

The third step of the thumbnails-from-the-edit order, and the roadmap's
lightbox line: the grid over the view it shows, with everything that gets
in the way of judging a set's consistency taken out, each tile the
picture an export of the frame will be.

## What it is

A mode of the grid, not a view of its own. It lies over whatever the grid
lists: a collection in its own order (`Collection::refs`, so a set laid
out by hand is judged in that order; Capture order is the row's switch as
before), a smart collection, a folder, every root, each as the filter bar
leaves it, so the picks are the lightbox the roadmap asked for before
collections existed (the P chip, then L). It shares the grid's list,
selection, arrows, Home and End, scroll bar, frame menu, drag-to-reorder
in a collection's own order, and the arithmetic in `grid.rs`; it changes
what is drawn, the cell size, and where the pictures come from.

Taken out: the header (the selection's line, the filter bar and its
chips, the Rejects, Cull and Loupe buttons), the folder tiles, each
cell's name, its badges (rating, flag, label), its marks (edited,
exported, in a collection), the label tint on its plate and the
selection's and the hover's ground. The sheet is the canvas's dark
(`Theme.canvas`, the viewport's own surround) in place of the grid's
grey. Kept: a ring round the frame on screen and a thinner one round the
rest of the set, since the arrows and the set have to be followed; and
a tile's name as its accessible label, read to a screen reader and
never drawn. A picture that cannot be read still says Unreadable.

The picture fills the whole cell, the name's room under it included: the
grid's arithmetic keeps a label's height in every row's pitch, and
rather than carry a second pitch through `columns`, `window`, `visible`,
`reveal` and `max_scroll`, the tile's box is the cell and that room, and
a tall frame takes it. The gaps between pictures are set by their
shapes more than by the 22 pixels.

## Keys

- **L** in and out, from the loupe, the grid or culling (Lightroom's L is
  its lights-out, the nearest thing it has; nothing here bound L). G
  leaves to the grid as well. The header has a Lightbox button beside
  Loupe (the four-square grid icon), and the frame menu a Lightbox /
  Leave lightbox item. A held key is one press: L, G, Return and Escape
  ignore the repeat, or holding one would flip the lightbox and its loupe
  at the repeat's rate; plus and minus repeat.
- **Escape** leaves to the grid, as the line asks. A set of more than one
  frame goes back to the frame on screen first, as Escape does over the
  grid.
- **Return** is the loupe on the frame chosen (a double-click on a tile
  too). There Return, Escape or L come back to the lightbox, scrolled
  where it was, the least move from there that shows the frame the loupe
  left on (an arrow in the loupe steps the frame as it always does).
  G from the loupe leaves to the grid.
- **+ and -**, and Ctrl with the wheel, step the tiles across: two, a
  pair side by side, to six, a handful; four when it opens. The steps
  are across, not a cell size, so the tiles fill the sheet's width at
  every step; a window resized keeps the count and resizes the tiles.
- **Tab** is the grid's Tab: the left pane in and out (the grid covers
  the develop panel and the strip). Entering puts all three panes away
  and leaving puts each back as it was; the settings keep how they were
  before the lightbox if the editor quits inside it.
- The rating, flag and label keys work as everywhere, on the selection,
  and draw nothing.
- **Ctrl+Shift+S** opens the sync sheet from the frame on screen onto
  the rest of the set, as in the grid: choose the frame that is right,
  Ctrl+click the one that stands out, sync. The frame menu gained a
  "Sync settings of <the frame on screen> onto N other frames..." item
  with more than one chosen, in the grid and the strip too, since the key
  had no menu twin. It names the frame on screen because that is the
  source, which is not always the frame right-clicked (the Copy item above
  it names that one). The synced tiles keep their old picture, dimmed,
  until the reduced develop keeps the new edit's.

## Which picture each tile shows

The rule is the strip's, the grid's and the culling loupe's: a frame with
an edit shows its edit, one without shows the camera's JPEG, and "an edit"
is the strip's pencil (`Edited`, `is_default_edit`). An unedited raw
exports the default develop, not the camera's JPEG, but the rule is the
one the whole library already follows, and a lightbox that developed
every unedited frame would be a new develop path.

- **An edited frame**: the picture of its edit from the thumbnail cache,
  looked up as the loupe's is (the open frame's develop's recipe, then
  the reduced develop's): the 2048 shrunk to the tile when the tile asks
  for more than 360, otherwise the least made size that covers it, then
  anything larger, then anything smaller. With nothing of its edit kept
  (`Pick::Wait`) a tile that already had a picture keeps it, drawn at half
  opacity as pending, and a tile with none stays dark with a quiet
  "Developing" in it, so a dark tile is not read as a black picture. It
  never asks for the camera's JPEG then. The one exception is a frame
  that had no edit and gets one by a sync: its tile keeps the camera's
  picture it had, dimmed, until the edit's lands, which we prefer to
  blacking out the very frames the user is matching. The edit's pictures are made by the thumbnails' pool as
  for any cell: the lightbox's cells ask for their grid thumbnails like
  the grid's, each thumbnail delivered queues the frame's reduced develop
  when its edit's pictures are not kept (§285's queue), and the
  `EditedKept` that follows asks the tile again (through the same
  `ask_again` that asks the cells). A save, a sync or a paste asks again
  the same way; the old picture stays up until the new one lands.
- **A frame with no edit**: the camera's JPEG decoded at the tile's size,
  the culling loupe's decode (`cull::fetch`), or, for a frame under an
  offline root, its local preview, with the root's key. An offline frame
  with neither is left dark.

The pictures are read on decode threads of the lightbox's own, a second
`cull::Prefetcher` made the first time the lightbox opens and kept, its
list replaced whole as the sheet moves, the tiles on screen first and the
margin's rows after. A `Want` carries a `lightbox` flag, and the shared
`fetch` hands it to `lightbox::fetch`, which reads `Edited` and the cache
and falls to the loupe's own decode for the camera's. No develop.

Sizes: the tile's box (its cell and the name's room) in physical pixels,
rounded up to one of 256, 384, 512, 768, 1024, 1536 or 2048, so a window
resized a little does not remake every picture. A 1500 px window at a
scale of one, four across, asks for 384; two across on a 2560 px screen
at a scale of two asks for 2048. Only the rows the grid has cells for
(the screen and a row either side) hold a picture, on the row's new
`big` image; the grid's own thumbnail stays on `image` and is never drawn
in the lightbox. Leaving takes every tile's picture off.

The grid reports what it shows on every wheel notch, scroll step and
arrow, and the lightbox follows each report, so a report must cost
nothing for the tiles that stayed: only rows that came into the window
(or lost their picture to a new list) are written, and each picture keeps
the image made from it, turned, so a row is given the same image again
rather than a new copy, and a turn ([ or ]) is the only thing that makes
one anew. A step of the tiles' size (most plus and minus steps, a window
resized past a size) keeps every picture drawn, stretched, until the one
at the new size lands in its place; only the failures are forgotten.
A held L, G, Return or Escape is one press: the lightbox eats the
repeats, and the grid's own Escape and Return step out once and no
further, so holding Escape goes from the lightbox to the grid and stops
there.

The tile size is a property of its own, `tile-cell`: the lightbox's
tiles while it is on, the grid's cell otherwise, which the lightbox
leaves alone (the grid's plus and minus are still the grid's when it is
closed).

## Tests

In `lightbox.rs`: the arithmetic (the tiles go across as asked on widths
from 900 to 2560 with the grid's own `columns`, plus is fewer and larger,
the sizes asked, the order the kept sizes are tried in); L in, Escape and
G out to the grid with the panes back, Tab the left pane's; plus, minus
and Ctrl with the wheel reflow to whole rows, the grid's cell untouched;
the grid's badges, marks, name and header present and the lightbox's
none, the rating key still rating, the badges named in words for a
screen reader ("3 stars, pick, yellow"); the loupe round trip by Return, Escape
and L keeps the frame and the scroll, an arrow in the loupe steps, a
double-click's way out is the loupe; Ctrl+Shift+S opens the sync sheet
over the lightbox and Escape takes the sheet first; `fetch` over a real
`Edited` and cache: the edit's 2048 shrunk to the tile, a small tile the
least made size, an edited frame with nothing kept waits, an unedited one
is the file's picture; a tile's picture landing on its row, kept and
dimmed while its edit's new picture is made, a tile with nothing saying
Developing, the pictures kept drawn across a size step, and every one off
on the way out; the grid reopened under the loupe by another way than its
keys ending the loupe (plus then sizes the tiles, not the hidden grid);
Escape letting go of a set before it leaves; and the settings keeping the
panes as they were before the lightbox when the editor quits inside it. In
`panel/collections.rs`: the lightbox over a collection lays the tiles out
in its hand order.

## Left open

- A tile stays dark ("Developing") for good when its frame's edit's
  pictures can never be made or kept: a learned mask whose model is not
  downloaded, an X-Trans or monochrome frame the develop refuses, and a
  thumbnail cache set to 0 MB, where `Edited::lookup` finds nothing and
  nothing is kept, so every edited tile is dark. The strip shows the
  camera's JPEG in all three; the lightbox will not, by its rule.
- A tile dimmed as pending can stay dimmed for the session when its
  reduced develop fails (no model, a refused sensor, the cache at 0 MB):
  only `EditedKept`, or the frame leaving the window and coming back,
  clears its wait. Later: route the pool's refused outcome to the
  lightbox, so the tile can say so or fall back.
- Later: push the frames of the lightbox's window to the front of the
  reduced develop's queue. Today they are made in the thumbnails' order,
  the cells on screen first, which is the lightbox's own when it opened,
  but a scroll in the lightbox does not reorder what is already queued
  beyond what the grid's report does.

- The loupe from the lightbox is the window's loupe as it is: the culling
  loupe while culling, the editor (a develop, the edit's 2048 standing in
  until it lands) otherwise. A lightbox loupe of its own (the tile's
  picture large, no develop) would be faster to step through outside
  culling.
- An unedited frame's tile is the camera's JPEG decoded from the file each
  time it comes into the window; nothing caches it at the tile's size.
- Tiles across is not kept between sessions.
- An offline frame held as edited with no picture of its edit kept waits
  rather than show its local preview.
- No compare-two-tiles mode beyond two across, and no "dim the rest"
  (Lightroom's lights dim) around the chosen tile.
