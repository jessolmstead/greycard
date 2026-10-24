# §416. The export queue in the grid

*2026-10-24*

§409 listed the queue in the export sheet, a row a set with a count
and a bin. The user found that tells you something is queued, not
what, and gives no way to take one frame out. The queue goes into
the grid, where every tool for looking at frames already is.

## One queue, grouped

The queue is stored as sets, each with the sheet it was queued under,
and that stays. What the user sees is one listing: an "Export queue"
row in the roots pane, and every queued frame as a tile, grouped
under a heading a destination (the sheet's description and the
folder), so two batches queued the same way merge under one heading
and two destinations read as two. A frame queued twice shows twice,
since each is an export. The grid had no headings; they are added as
a general mechanism, not rows of the list but breaks in its layout
(`grid_heads`: the first row under each, its text, and its count),
so selection, files, and every index stay as they were and the
lightbox and the strip never see them. Date groups can use the same.

Delete, and "Remove from queue..." in the frame menu, take the chosen
frames out of their sets after a confirm that names the count; an
emptied set is dropped; the running set's frames are kept out of a
removal, as the sheet's bin keeps them. Sort offers only the queue's
order. The status line while the queue runs counts across the whole
run, "exporting 3 of 12", where before it counted within the set, so
a queue built one frame at a time read "1 of 1" for every file.

## Frozen

A queued frame carries the edit as it was at queue time, and that is
what exports (§213). The Loupe from the queue therefore shows the
queued edit, with its turn and seed, and nothing that would write an
edit: the Develop, Crop, Masks, and Retouch tabs are hidden, the
presets, snapshots, and history with them, and Paste, Sync, and the
version items are disabled. Zoom, Fit, the scopes, soft proof, and
Cull's rating, flags, and labels work, since those are the sidecar's
own and not the edit's; Copy gives the queued edit, so a copy is of
what will export.

The first version set a flag at each open and listed the keys to
swallow, and the reviewer walked through it by the frame menu, undo
from the grid, a cull round trip, and the window between a change to
the queue and its relisting. The second answers the question once:
a frame is frozen when the view is the queue, and the one place
every edit write already asks whether the panel owns the frame says
no there, with the history line unreachable in that view. A runtime
id on each queued frame lets the listing update in place while the
queue runs, so the selection, the thumbnails, and the status line
survive a set finishing, and a duplicate keeps its own copy.

## Not done

- Changing a queued set's sheet, and adding frames to a set.
- Backspace as a removal key is bound only on a Mac, as elsewhere.
- An index pass over a folder while the queue view is open relists by
  file, so a duplicate's selection can land on the other copy.
