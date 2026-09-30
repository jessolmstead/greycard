# 211. The grid with the library on the left, and the filter on top (2026-09-30)

A design decision, written before the build. The user's morning
thought after §209 landed: the grid should move the file and root
browsing to a left sidebar and leave the filtering up top.

**What it is.** The grid's header (§202) holds five rows: Open folder,
the open folder's name with Recently opened behind it (§206), Import,
the selection's line, Cull and Loupe; then the size line and the
filter's words; then the roots as a row of chips (§174), the filter's
meta chips and the facets. The height is spelled out so the sheet does
not reflow twice on the way in. The grid covers the left pane, and the
left pane is where §209 put the root's folders as a tree, so the one
view where a folder is looked for cannot see the tree; §209 named that
as the decision to confirm. The roots' chips scroll sideways and lose
their names once there are a few.

**Why the split.** Where the frames are is a place; what is being
looked for is a query. The place belongs beside the sheet, with room
to read, and stays put while the query changes. The query belongs over
the sheet it narrows. Lightroom, Bridge and darktable all read that way
and people arrive with it in their hands. The header drops to what
belongs over a grid, and the pane holds what is browsed.

**The pane.** The grid keeps the loupe's left pane: the same width
(`left-w`), the same F7 and Tab to put it away (§178), the same chevron.
It shows a grid set of sections; the navigator, presets, snapshots and
history are the loupe's and stay with it. From the top:
- Open folder, and under it the folder's name with Recently opened
  behind it, as the loupe's pane has them today.
- Import, with its running line under it while an import runs.
- ROOTS: All roots first with its count, then a row a root with its
  name, its count muted at the right, dimmed when offline with §207's
  badge, the open one lit; the row's right-click menu is the chip's
  (rename, remove, reveal), and "Add this folder" is a row at the foot
  where the chip was, shown when the open folder is not under a root,
  with "Add a root..." beside it. The count is the index's, as the
  chips' was; the filter's count is the header's business.
- FOLDERS: §209's tree under the chosen root, unchanged, and since the
  pane is now in the view that opens folders, shown as well for a disk
  folder that lies under a root, marked at that folder, which §209 left
  undone.

**The header.** Three rows in place of five: the selection's line
with the frame count, the size, Cull and Loupe; the filter's meta
chips with its words; the facets. Still a fixed height, for the same
reason as before. `RootsRow` goes; `FolderName` is used once, in the
pane, and the grid's copy of the Open folder and Import buttons goes
with it. Ctrl+O and Ctrl+Shift+I still reach both.

**What it costs.** The sheet is the pane's width narrower while the
pane is up, one or two cells a row at the default size; the pane hides
with one key when the whole sheet is wanted, and that choice is kept
across the session as it is for the loupe. The pane's width is the
loupe's; a root's name longer than it elides with a tip.

**Not part of it.** The keyboard in the tree and in the roots list, the
filter's count beside a root, and a pane width of its own for the grid.
