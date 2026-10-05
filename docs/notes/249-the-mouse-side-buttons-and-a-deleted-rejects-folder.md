# 249. The mouse's side buttons in the grid, and a deleted rejects folder in the tree (2026-10-04)

**Back and forward step the grid.** A mouse's back and forward
buttons step to the frame before and after, as the left and right
arrows do in the grid. Slint gives a press to the topmost TouchArea
under the pointer, whatever the button, and nothing above it sees
the press, so there is no one place in the window to catch these
two. The grid's own TouchAreas each ask `side-press` first: a cell,
a folder tile, the scroll bar, the sheet between the cells, and the
grid's background. While a menu is up the press steps nothing. The
grid's header (the filter, its fields and chips) does not take them,
and neither does the loupe: the viewport's press already decides
between the menu, a dropper, a shape being placed and a pan, and a
side button there is a design of its own.

**A deleted rejects folder leaves the tree.** Delete rejects folder
took the folder off the disk, and the index forgot its rows, but the
tree and the grid's tiles kept the folder until a later pass or a
restart. §246 found the same gap after a move: the tree is built
again when a pass reports a change or the root's count moves, and a
delete is neither. The indexer's `Told::Forgotten` now carries the
folders the forgotten rows were in, and the window marks the tree
stale for each, as for `Told::Moved`. It comes from the indexer for
the same reason as there: the rebuild reads the rows after they are
gone.

Tests: the side buttons over a cell and over the sheet below the
cells step the grid one each way, and a left press there steps
nothing; §246's move test goes on to delete the rejects folder
through a real indexer and finds it gone from the tree and the tiles
(with the window's handling of the folders stubbed out, it fails).
