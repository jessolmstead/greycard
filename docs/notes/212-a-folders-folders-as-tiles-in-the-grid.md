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

---

Built the same day, from the design above.

**Now.** While the grid shows a folder of a root's tree, the sheet
starts with a tile for each folder directly under it, then the cells.
A tile is the cell's width and half a cell's picture tall (88 px at the
default 176), on the pane's surface with its border, the folder icon at
its left, the name over the count in the pane's type, the count muted.
It is the tree's total for that folder: the frames under it in all.
The tiles wrap at the grid's columns, a row of them for each row of
columns, with the grid's gap below each row, and they scroll with the
sheet. A folder with nothing of its own opens to its tiles alone, with
§209's status line unchanged.

Which folders: `tree::tiles_for`, from `shown_for`, so the same three
cases as the tree. A `View::Branch` without the switch gets the
children of its folder. A folder opened from the disk under a root
(`View::Folder`) gets the children of that folder, since its list is
the folder's own frames. The root's own view (`View::Roots(Some)`) gets
the root's first-level folders while the switch is off. With the switch
on there are none: a deep `Branch`, and the root's view when the
switch is on. A view with no tree (all roots, a folder under no root),
or a tree for another root not yet landed, has none. The children come
from a private `children(&nodes, folder)` over the tree's nodes in
order; the node list stays private to `tree.rs`.

The tiles are one model for the session (`Tree::tiles`), replaced only
when they change, and each tile's folder is kept beside it on the Rust
side (`tile_folders`). A click comes back as the tile's number, as a
tree row's does, so a folder name that is not UTF-8 still opens.
`tree::show` refreshes them, so they follow every view's landing, the
tree's landing and every pass, as the rows do.

**A click** goes to `tile_picked`. That and `picked` (a row) now both
look up their folder and call one `open(folder)`: the root check, the
log line, `view_for` with the switch, and `roots::open_view`. So a tile
opens the same `View::Branch` the row does. The tree marks the folder
and unfolds the folders above it once (`reveal`), and Recently opened
records it through `open_loaded`. All of that happens after the view
lands, as it does for any view.

**A right-click** on a tile opens one menu for the sheet with one
item, Reveal in file manager (worded as the frame menu has it), and
only while the root is online. Over an offline root a right-click does
nothing. The pick calls `menu-done` as every menu does since §208, and
the keys take the focus before the menu opens, so `menu-up` reads
right. A press that closes a menu is not a click on the tile under it.
The reveal is `menu::reveal_folder`, beside the frame menu's
`reveal`, sharing its spawn. On a Mac and on Windows it is the same
command, the folder chosen in its parent. On Linux it opens the folder
itself through `xdg-open`, since its parent is usually the folder on
screen. The frame menu's reveal of a file is unchanged. Which folder
is shown is `tree::reveal_target`: the tile's own folder, or none
while the root is offline.

Tiles are not frames. They are not in `thumbs` or the cells, so
selection, ratings, the frame menu, the strip, culling and the filter's
counts never see them. The arrows (`grid::step`) work on frame numbers
alone, so Up from the first row stays on the first row and no tile is
ever selected.

**The layout.** `grid.rs` has `tile_height(cell) = cell / 2` and
`tiles_top(tiles, cell, columns)`, the tiles' rows times the tile's
height and the gap. `visible`, `window`, `reveal` and `max_scroll` all
take it as `top`: the cells start at the padding plus `top`, and the
scroll range grows by `top` (tiles with no frames make a sheet of the
padding and the tile rows). The window takes `grid-top` from a pure
callback over the tiles' count, the cell and the columns, as it takes
the columns. It hands that to `grid-max-scroll`, `grid-reveal-to` and
the `grid-range` report, and `menu-at` adds it too. A change of
`grid-top` re-flows the sheet as a change of `thumbs` does. `reveal`
scrolls to 0 for a frame of the first row, so the tiles come back into
view with it, but only when the tiles and that row fit on the sheet
together. When they don't (54 folders at six columns are nine tile
rows, 864 px, taller than the 837 px sheet), the frame gets the least
scroll as any other does. Without that check, a click on a first-row
frame below a tall block of tiles jumped the sheet to the top and left
the frame under the tiles (found in review). Otherwise it is the least
scroll, as before. The cells
for the screen alone (§202) are unchanged: still the rows on screen
and one either side, by frame number. The header's height is untouched.

The grid's existing layout tests take `top = 0` and their numbers
stand: six columns, the 837 px sheet, frames 0 to 29 at the top and 90
to 119 at the foot. The `grid-range` callback has a fourth argument,
the tiles' height, and the tests that invoke or hear it pass or ignore
0.

**Measured.** Nothing to time: a tile is a dozen elements, with at
most a few hundred for a year of days, and they are built only with
the grid up. Not virtualized (below).

**Checked in the editor**, on a sandbox root `studio` of hard links:
cedar holds three CR3s and cedar/export two JPEGs; birch holds nothing,
birch/birch two RAFs and birch/export one JPEG. The editor ran from
this worktree's own target, every XDG directory in the sandbox,
`--no-sidecars`, `--roots studio --grid`, driven by `--keys`
(`target/scratch/shots/`):
- `root.png`: studio's row clicked. Tiles birch 3 and cedar 5 above
  the root's eight frames, the tree's root lit.
- `birch-bare.png`: the birch tile clicked. birch 2 and export 1
  tiles and no cells, "0 frames"; the tree marks birch, the pane says
  "birch in studio".
- `birch-inner.png`: then the inner birch tile. Its two RAFs, no
  tiles; the tree unfolded birch and marks the inner birch, the same
  as a click on its row.
- `cedar.png`: the cedar tile from the root. One export 2 tile over
  cedar's three raws, the JPEGs not among them.
- `cedar-switch-on.png`: then With subfolders on. Five frames, raws
  and exports, no tiles.
- `cedar-tile-menu.png`: a right-click on the export tile. The one
  item, Reveal in file manager.
An offline root was not captured. A capture runs no launch check of
the roots (as §209 and §211 found), so the test covers it.

**Tests.** `grid.rs`: the tiles' rows push the cells down and lengthen
the range by exactly `top`; a fifth tile in four columns wraps; the
smallest cell's row is 48 and its gap; tiles alone make a sheet;
`visible` and `window` shift by `top`; a first-row frame reveals to 0
and a lower one comes up `top` further; with 54 tiles over a 837 px
sheet, a first-row frame scrolled to or revealed from the top stays on
screen (checked to fail with the unconditional 0). `menu.rs`: a folder's
reveal on Linux opens the folder itself. On the headless window
(`tree.rs`): the root's own view has its first-level folders. A folder
with a folder under it has that tile, one without has none, a fold in
the tree changes nothing, and in the two client layouts the root's view
has both clients and cedar has its raws and an export tile. A disk
folder under a root has its tiles, a folder under no root none. A bare
folder opens to tiles alone and the status line says "nothing directly
in birch". A tile clicked in the grid, found by its accessible label
"Folder birch", opens the same `View::Branch` as the tree's row, records
Recently opened and marks the inner birch. The switch on takes the tiles
away over a folder and over the root, and off again brings the root's
back. With the grid laid out at 1500 by 950: the report carries `top` =
96. The export tile sits at the sheet's first place, 176 by 88. A click
a tile row lower selects the third frame. Up and Left never leave the
frames or open anything. A click on the tile opens the export folder,
whose report carries 0. A right-click opens the tile's menu while the
root is online, and the left press that closes it opens no tile.
`tile_folder` gives each tile's folder by its number, and
`reveal_target` gives it online and none for a tile past the end. Once
the root is offline a right-click opens nothing and `reveal_target` is
none, while the tiles still come from the index, dimmed, and a click
still opens one.

**Decisions made in the build.**
- The root's own view shows tiles with the switch off, as the brief
  asked, though that view lists everything under the root. So on the
  root, the export folder's frames are in the list and its client
  appears as a tile too. The switch on removes them there.
- A tile's height follows the zoom (half the cell's picture: 48 to
  256 px). §212 can also be read as a fixed height with only the width
  following the cell.
- The count is the bare number, as the tree's is, not "5 frames".
- The tile's accessible label is "Folder <name>", so a screen reader
  and the tests tell it from the tree's row of the same name.
- An offline root's tiles are dimmed as its row is (0.55).

**Not done.**
- The tiles are not virtualized. A folder of a thousand subfolders
  makes a thousand tiles above its cells. Fine at an archive's scale,
  not at that one.
- No keyboard to a tile: the arrows walk the frames and a tile takes
  no focus. Enter on a tile, or Up from the first row into the tiles,
  is the user's call.
- No tiles in the loupe's strip, no parent tile, none for a folder
  opened with Open folder that lies under no root (§212's "not part of
  it").
