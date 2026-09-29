# 96. The grid (2026-09-19)

The strip shows a dozen frames of a folder at a time and nothing
else does. G now puts the whole folder on the window as a contact
sheet, and G, Escape or Return bring the loupe back on the frame the
selection landed on. It is a view over the folder that is already
open, not a library: there is no catalog, no database and no
thumbnail cache on disk, and closing the window forgets everything
but the cell size.

**The panel goes with the viewport.** The task left it open whether
the develop panel stays beside the grid, to be decided from what the
panel does when no picture is developing. Everything in it belongs to
the picture in the viewport: the navigator is that frame, the
histogram and the scope are that frame's pixels, the crop handles,
the mask outlines and the retouch pins are drawn on the viewport
itself, and Fit and Export act on the file open. More to the point,
the selection in the grid runs ahead of the develop. Moving the
selection opens that file as a click on the strip does, so the panel
beside it would be a frame behind for as long as the develop takes —
1.3 s on the R6 Mark II, 5 s on the GFX — and the sliders would be
the old file's while the outline is on the new one. Editing blind
against a lagging histogram is worse than not offering it. So the
grid takes the whole window, with a slim header carrying the selected
file's name, the count, the cell size and the way back.

It is drawn *over* the window rather than swapped in for it. The
viewport's `Image`, its `TouchArea` and the strip's `Flickable` are
named from root properties and functions all over `app.slint`, and
putting them inside an `if` takes those names out of scope; an
opaque overlay with a `TouchArea` under it, the way the export and
preset sheets already work, costs a hidden viewport's worth of
drawing and nothing else. The one thing that had to come out of the
overlay is the sheet's width and height, mirrored into two root
properties by `changed width`/`changed height` handlers so the reveal
and the range report can be worked out whether the grid is open or
not.

**The arithmetic is Rust, and the layout asks for it.** Columns from
a width, rows from a count, the frame at a row and a column, the
scroll that reveals the selection, how far the sheet can scroll, the
frames a scroll leaves on screen, and the steps the zoom takes: all
of it is `crates/greycard-ui/src/grid.rs`, pure, with a test apiece.
The `.slint` file reaches it through pure callbacks, so the tested
code is the code that runs rather than a second copy of the same sums
in a binding nobody can execute. The gap, the padding and the room
under a picture for its name are constants there too, handed to the
window at startup, which is §89's lesson about the strip's three
literals applied before it could bite. Two of the numbers fall out
nicely: the scrolling layer's height is the sheet's height plus how
far it can scroll, which is the content's height when there is more
than a screenful and the screen's when there is not, and the cells
ride on that one layer, so a wheel notch moves one property and not
five hundred bindings.

The reveal is the strip's, turned ninety degrees: the least scroll
that shows the selected cell with its padding beside it, from a
`changed selected` handler that covers the arrows and the clicks
alike. A re-flow — a new cell size, a new width, a new folder — goes
back to the top first and reveals from there, because §89 found that
a least-scroll from an intermediate layout leaves the frame in a
place that is a function of nothing but the accident of that pass.

**Keys and the wheel.** Left and right step along the row and over
its ends, as they do on the strip; up and down move a whole row and
keep the column, and a column that the short last row does not reach
lands on the last frame there is. Return and a double click open the
frame in the loupe, Escape and G leave it as well, Ctrl and the wheel
or the plus and minus keys take the cell a step along 96, 128, 176,
256, 360 and 512, and a bare wheel scrolls a third of a row a notch,
since 60 logical pixels against a 542 px row would be a crawl. Slint
gives a scroll
event to the innermost `TouchArea` under the pointer and then
outwards, so the wheel is handled once, on the layer the cells sit
on: a cell's own `TouchArea` has no scroll handler, rejects, and the
event rises to it whether the pointer is on a picture or between two.
That is the same rule §89 leaned on for the strip, used the other way
round, and the overlay's own blocker takes the wheel too, or one over
the header would reach the viewport behind it and zoom a picture
nobody can see. There is no `Flickable` here at all, and so no drag
to scroll and no scrollbar; the wheel, the keys, a Loupe button in
the header and the reveal are the whole of it.

Two things the keys had to be told. A sheet — the export, the preset,
the model license — is declared after the grid and draws over it, so
while one is up the keys under it are not the grid's, G included; and
a brush, a dropper, a guide or the level tool left in hand is put
down on the way in, since nothing can be placed on a picture that is
not on screen, and otherwise Escape would be spent clearing a mode
the user cannot see instead of leaving the grid. S and J, the soft
proof and the clipping warnings, belong to the picture and are inert
here for the same reason Space and Z are.

**Pictures the size of the cell.** Thumbnails were made at a 170 px
long edge for a strip of 178 px items, which is mush in a 512 px
cell. The worker now holds the size it makes them at and the grid
sets it to whatever the cell asks for, up or down, so a cell that has
shrunk stops paying for the one before it. Two rules keep that from
being expensive. The size is capped at 360 however large the cell, so
the largest cell shows a 360 px picture stretched by 1.4 — soft under
a loupe, indistinguishable at arm's length, and side by side with a
true 512 the bracelet and the hair still read. And making a picture
again is the only part that steps up and never down: only what is on
screen, only what is a quarter too small, and only once each. A few
percent is worth nothing, since the downscale is by a whole integer
factor and a 176 px cell asking a 1620 px camera preview for more
than 170 gets the identical ten-to-one box and the identical pixels;
a quarter more lets 256 and 360 through and stops 96, 128 and 176.
One being made when the cells grow is the awkward case: it comes back
stamped at the old size, nothing else would ask for it again until
the grid next moved, and a snapshot waiting on the grid would wait
for ever, so the delivery itself asks again when what arrived is too
small for the cells it is landing in.

A picture is held three times over — the bytes the worker made, the
image the renderer was handed, and the texture it uploaded — so the
cap is worth more than it looks: about 1 MB of host memory and 0.7 MB
of video memory a frame at 512, against 0.5 and 0.35 at 360, and
nothing evicts any of it. A folder of five hundred walked end to end
still ends up holding a few hundred megabytes. That is the next thing
to fix if this is ever pointed at a wedding, and the fix is an
eviction of what the grid has scrolled past, not a cache on disk.

The range mechanism is the strip's with a rectangle in place of a
row. The grid reports the frames it shows on every scroll, every
re-flow and every move of the selection; `order_thumbnails` sorts the
pending queue around that range exactly as before, and the queue
drops a repeat. A shoot of hundreds is still queued whole at open
and still made outward from whatever is on screen, so nothing is
rendered up front. The one wrinkle is order of operations: pushing a
job clears the queue's idea of what it is sorted for, so the
re-renders go in first and the range is reported after them, or they
would sit behind the rest of the folder.

**What was checked.** `--grid` and `--grid-cell` open the window on
the sheet, the way `--show-mask` and `--scope` already open it on a
state, and a snapshot of the grid waits for the pictures the grid
shows the way a snapshot of the loupe waits for the develop — without
that it catches a sheet of empty cells. Over the 33 frames in
`~/Pictures/Test` at 1500 by 950: 14 columns at 96, 8 at 176, 2 at
512, which is what `columns` makes of that width; the row centered in
the window rather than against its left edge, which at 512 is a third
of the window that would otherwise be empty on the right; the
remembered frame outlined in the accent and revealed; and at 512 the
sheet scrolled to the selected frame's row with the row above it
half shown. The pictures in the largest cell are the 360 px render
stretched, which is the re-render working end to end, since the
snapshot only fires once every visible cell is made at the size the
cell asks for. The editor was run with its settings under the
scratchpad rather than the live file.

Keys could not be pressed at a Wayland session from here — no
xdotool, no wtype — so the window's own key handling is tested
instead, on Slint's headless backend, which is a new dev-dependency
and the first test in this tree to build the window: G opens the grid
and Return, Escape and G leave it; the arrows are the strip's one
dimension in the loupe and the grid's two in the grid; plus and minus
size the cells only in the grid; an open sheet keeps G and Return
from reaching the grid under it, and another folder's frames re-flow
it and make it say afresh what it shows; and with the sheet's size
written in by hand, since a headless backend lays nothing out, 1500
by 902 gives eight columns and a report of frames 0 to 39, while
selecting the last of 120 scrolls to the foot of the sheet and
reports 80 to 119. What is still unverified by anything but reading
is the wheel, plain and with Ctrl, and the double click.

**Addendum, the same evening: one column down the left.** Pressed
at a window already open, G laid the whole folder out as one column
against the left edge. The sheet's width reached the layout only
through a `changed width` handler, and Slint's `changed` says
nothing about an element's first value: opened by `--grid` the
sheet is created before the window has a size, the width goes from
nothing to the window's, and the handler fires, which is how every
snapshot and every headless test had passed; opened by G the sheet
is born at its full size, the handler never runs, and `columns` of
a zero width is one. The sheet's `init` now reads its own size and
relays, and the relay on `changed grid-open` goes, since the sheet's
birth is that relay. The headless tests had set the sheet's size by
hand, which is exactly what hid this; they size the window instead
now, and a new one presses G at a sized window and asks for eight
columns without any help, failing with a width of zero on the old
code. The sheet under a 950 px window is 905 px, not the 902 the
tests had assumed.
