# 212. A folder's folders as tiles in the grid (2026-09-30)

A design decision, written before the build, from the first morning
with §209's tree over a real archive.

**What went wrong.** An archive's client folders are laid out two
ways: a folder with its raws in it and an export folder beside them,
or a folder with nothing of its own, a same-named folder of raws under
it and sometimes an export folder beside that. §209 opens a folder's
own frames with the switch off, and a folder with nothing of its own
came up as an empty grid. The status line that said why is not shown
over the grid, so the tester read it as a folder that would not open.
The first fix, opening such a folder with everything under it, was
taken back the same hour: it put the export folder's files in the
list with the raws, which is the one thing the shallow open exists to
prevent.

**The decision.** The grid shows what is under a folder as well as
what is in it. When the folder open from the tree has folders of its
own, the sheet starts with a row of folder tiles, one a folder, each
with its name and the frames under it as the tree counts them, and the
thumbnails follow. A click on a tile opens that folder, exactly as its
row in the tree does: the same `View::Branch`, the tree marked at it,
Recently opened told. A folder with nothing of its own opens to its
tiles alone, so the same-name case is one click from its raws and the
two-folder case shows both and mixes neither. A folder with frames and
an export folder shows its frames under one small tile. The grid is
never blank for a folder that holds anything, anywhere under it.

**What stays.** The switch, and its meaning: with it on a folder opens
with everything under it, and the tiles are not shown, since there is
nothing left to descend into. The tree, as the map; the tiles are the
map's next step shown where the eye already is. §209's status line for
a folder with nothing of its own stays as the words beside the tiles.

**The tiles.** A row of their own above the cells, half a cell's
height, the folder's name and its count muted, in the pane's type,
with the folder icon; they take the grid's cell size only in their
width. They are not frames: no selection, no rating, no menu of a
frame's, not in the strip and not in culling. A right-click on a tile
offers Reveal in the file manager, when the root is online. They scroll
with the sheet. Over an offline root they come from the index like
everything else.

**Not part of it.** Tiles in the loupe's strip; a tile for the parent
folder (the tree has it); folder tiles for a folder opened from the
disk with Open folder, which lists the disk and has no tree, unless it
lies under a root, where it has the root's tree and takes the tiles
with it.
