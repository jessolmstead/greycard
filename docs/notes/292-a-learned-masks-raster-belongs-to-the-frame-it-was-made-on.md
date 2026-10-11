# §292. A learned mask's raster belongs to the frame it was made on (2026-10-10)

The user copied a Subject mask from one frame onto the next. Back on the
first frame, its mask then sat where the person stands in the second
frame. Going to a third frame and back made it look right again. Later
the second frame's mask was a poor fit.

The worker keeps nothing between frames. Every open clears what its
`Ai` held, the disk cache hashes the full path, and a mask's outcome
says which file it was made on. The panel was the problem. It holds
the rasters it has been given in `State::learned`, keyed by adjustment
id and component index, with the shape each was made for. It cleared
that map only on a turn. Opening another frame left it alone, and
`bake_locals` dropped only the keys the new edit no longer had.

A Subject (or Background) shape carries no data of its own, and
adjustment ids start at 1 on every frame. So two frames that each have
a Subject in their first adjustment share both key and shape, and a
paste produces exactly that. The second frame was drawn with the first
frame's raster and never asked the worker for its own. A frame in
between whose first adjustment had another shape (or none) emptied the
map, and the next frame got its own raster again; that is why it
seemed to fix itself. Two frames shot seconds apart put the person in
nearly the same place, so the borrowed raster was close but wrong: a
poor fit. The edit's pictures and the exports were never affected,
since both take their rasters from the worker.

Now `learned` and `asked` belong to one file, `State::learned_of`, the
one last sent to the worker to open. `panel::mask::learned_for` clears
both when an open names a different file. Every open the panel sends
is made by `panel::browser::open_job`, which calls it: opening a frame
from the browser, leaving culling, and opening the frame again once the
lens database arrives. That last one matters after a move that renamed
the open frame; otherwise the worker's path and `learned_of` would
diverge and every later raster would be dropped. A `Mask` outcome that
names a file other than `learned_of` is dropped, and its key stays in
`asked`, so the open frame's own ask stands and is not sent twice.
Opening the same file again keeps what the panel has.

The parts' asks had the same hazard. `part_asks` and `part_dismissed`
are keyed by adjustment and component too, and a Part pasted across is
the same shape on both frames. Opening a frame cleared them, along with
the picking and a part waiting on its people, but leaving culling did
not. Now leaving culling onto another frame than the one open before
the mode clears them through `panel::parts::another_picture`, as an
open does. Leaving it on that same frame keeps the asks, since the
frame's raster is kept too and nothing would ask again; only the
picking and a part waiting on its people are put down.

On the same path we found a second bug, in `take_current_settled`.
When the sidecar's new state develops the same picture as the panel's,
it only asked for a redraw and left `State::edit` as it was. Sliders do
not need it: `show_edit` sets them and `read_edit` reads them back from
the controls. Masks are another matter. `read_edit` takes the
adjustments from `State::edit` and only the chosen adjustment's
controls from the window. So any change to the sidecar that touched
only the masks was not drawn: a paste of the Adjustments section alone
onto the open frame, or an undo or redo of a mask-only step. The panel
also went on holding the old masks, and its next record
(`record_panel`, before a later step or an undo) would have written
them back over the change. The fix landed with the history section, not
with this change. Our test here covers only the paste. The undo case
and the next record follow from the same code, but no test covers
them yet.
