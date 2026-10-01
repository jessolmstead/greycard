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

---

Built the same day, from the design above.

**Now.** The grid starts at the left pane's edge. The pane is the
loupe's, one pane: it was taken out of the window's layout, which keeps
its room (`left-w`) as an empty box, and drawn over that room after the
grid, the height of the row it sat in, or with the grid up the whole
window's. So it is the same width, the same F7 and chevron, the same
`left-hidden` kept in the settings, and its scroll, its folds and the
name's hover text are one set, not a copy kept in step. With the grid
up it shows Open folder and the folder's name with Recently opened
behind it, Import with its running line under it, ROOTS and FOLDERS;
the navigator, presets, snapshots and history are left out by
`!grid-open`, as culling already left three of them out. Settings and
Report stay at its foot in both.

Over the grid the pane is the window's height, and its foot lies over
the strip's left end, which the grid leaves drawn under it (the grid
covered x = 0 before). A Rectangle takes no events and the ScrollView
lets a press or a wheel through where its content cannot scroll, so
the pane's first child is a TouchArea that takes both; the sections
after it have theirs first. Without it a press in the pane's margin
chose a frame in the unseen strip, a right-click opened its menu and
the wheel scrolled it.

ROOTS (`RootsSection`, roots.slint) is a section that folds, kept in
`collapsed` as "roots": All roots with its sum when there is a root,
then a row a root, its name eliding at the pane's width with its count
muted at the right, or "offline" there and the name dimmed; the one
open lit (and the list's selected item, for a screen reader). A root's
name cut short is its hover text after 600 ms, through the window's tip
as the folder's name has its path. A right-click opens the chip's menu,
the path, Rename... and Remove from library, each pick ending in
`menu-done` so the keys have the focus back; a click opens the root and
hands the keys back. At the foot, "+ Add this folder" while the open
folder is under no root and "+ Add a root..." beside it. With no roots
it says "No folders in the library yet" over the two. `RootPill` and
`RootsRow` are gone. The rows are one model for the session, replaced
only when they change, as the tree's and Recently opened's are: `show`
runs at every pass and recount, and a row rebuilt under the pointer
lost its hover text and a press in progress.

The header is three rows: the selection's line, the frame count and
size, Cull and Loupe; the filter's meta chips with its words, the
count and Clear at the row's end; the facets. The chips keep their
whole width while the field has width to give, the field going down to
200 px from 360 before a chip is clipped: at the default size with the
pane up, the label chips were otherwise the part cut off. The field is
held to a control's height in the header (the style's LineEdit is 4 px
taller, which ran the facets into the header's foot); its height is
still spelled out, `2 * control-height + (control-height - 4px) +
4 * space-2 + 1px`, and the rows now fill it exactly: field 44 to 72,
facets 80 to 104, the rule at 112. The header's Open folder, the name
and Import went; Ctrl+O and Ctrl+Shift+I are the window's keys, as
before. An import's
running line, which the header said in place of the selection since
the grid covers the status plate, is the pane's now, under Import, and
the header's again only while the pane is put away.

Over the grid F7 and Tab put the pane away and bring it back; F6 and
F8 still do nothing there. Tab is the pane's alone over the grid,
rather than the loupe's all-or-nothing, which from a pane put away by
F7 would have hidden the develop panel and the strip unseen and changed
nothing on screen. The pane's chevron is drawn after the grid, at the
grid's edge and the window's middle, and is not quiet over the grid.

**A disk folder under a root.** `tree::shown_for` says which root's tree
the pane shows and which folder it marks: a root's view and a folder of
its tree as before, and for a folder opened from the disk
(`View::Folder`) the root it lies under, found from `recent.open`, the
folder's canonical path, with the folder itself marked. `want`, `show`
and the switch all go through it. The root's row is not lit for such a
folder: it is a folder's view, not the root's. A click in the tree opens
a `Branch` as §209 has it. The switch turned on over a disk folder opens
it as a `Branch` with the folders under it. Over a folder from the disk
the switch shows off whatever it was left at, since the list is the
folder's own frames; the setting itself is kept, and the next folder
opened from the tree takes it. When the
marked folder changes, the folders between it and the root are unfolded
once (`reveal`), so a day opened from the disk or from Recently opened
is shown where it is rather than inside a folded year; a fold made
after is left alone. The launch's own folder has its tree once the
index opens (`want` in `told`).

**Measured.** At the default 1500 by 950 and the 176 px cell: the
header 177 px before, 113 now; the sheet 773 px tall before, 837 now,
four rows and a pixel of a fifth where it showed four. Cells a row,
before: 8 (the grid over the whole window). Now: 6 with the pane up
(1,260 px of sheet), 8 with it put away.

**Checked in the editor**, on a sandbox of hard links (root Archive:
2025/harbor 3, 2025/city 2, 2026/coast 1; Loose 2 and Many 16 under no
root), from this worktree's own target with every XDG directory in the
sandbox and `--no-sidecars`:
- `grid-loose.png`: a folder under no root, ROOTS with All roots and
  Archive, "+ Add this folder" and "+ Add a root...", no FOLDERS.
- `grid-loose-added.png`: a click on "+ Add this folder" added Loose to
  roots.json and the row; "+ Add a root..." stayed; the tree showed
  Loose, marked.
- `grid-harbor.png`: harbor from the disk, the tree unfolded to it,
  harbor lit, the root's row not lit.
- `loupe-harbor.png`, `loupe-harbor-hidden.png`: the loupe's pane with
  FOLDERS over the navigator, presets, snapshots, history; F7 puts it
  away.
- `grid-archive-clicked.png`: a click on Archive's row opened the root,
  its row and the tree's root lit.
- `grid-archive-menu.png`: a right-click on it, the path, Rename...,
  Remove from library.
- `grid-archive-removed-then-g.png`: Remove from library picked with
  the pointer, then G: the loupe, so the keys had the focus back;
  roots.json emptied, and restored after.
- `grid-unfold-then-click.png`: 2026 unfolded and coast clicked: coast
  open, one frame, lit.
- `grid-many-pane.png`, `grid-many-hidden.png`: sixteen frames, six a
  row with the pane and eight without, every meta chip in sight with
  the pane up.
- `grid-all-roots.png`: All roots lit, 6.
- `grid-harbor-review.png`: after the review's fixes, the header's rows
  inside it and the roots' names in line with Open folder and the
  switch.
An offline root's row could not be captured: a capture runs no launch
look at the roots, and a view whose first frame is offline ends a
capture run (as §209 found); the test covers it.

**Tests.** With the grid up, a press and a wheel in the pane's margin
over the strip's first cell choose nothing and scroll nothing, where in
the loupe the same point is the cell's (checked to fail without the
TouchArea). The pane's roots with the grid up: All roots, a root's row,
"+ Add a root..." and "+ Add this folder" each hand back their press,
found by their accessible labels in the pane; the header has none of
them (F7 takes every one away; the loupe's pane has none); an offline
root says "offline" where the count was; six long names each a row
inside the pane's width, the one open selected, the last reached, and
a cut-short name's hover text after a rest. A root is named from its
row's menu: a right-click opens the menu and picks nothing, the press
that closes it is not a pick, the next is. With a root's menu up, a
right-click on another opens that one's, All roots has none, and the
press that closes it does nothing on any row of the section. The pane's
counts from a real index, All roots lit for the view of every root and
a root's row for its own, "+ Add this folder" only while the open
folder is outside the roots. Import in the pane asks for the sheet,
Ctrl+O and Ctrl+Shift+I reach theirs over the grid, and an import's
line is the pane's, and the header's only with the pane away. The tree:
a disk folder under a root shows the root's tree with it marked, in the
loupe and the grid; a folder of it opens as a `Branch` with the row lit;
the switch on over the disk's folder opens it with those under it, and
left on it shows off over the next folder from the disk, the setting
kept, and on again opens the deep view; a folder deeper down opened
from the disk unfolds the folded day to it, and a fold made after stays;
a folder under no root has no tree. The grid's layout tests take the
new sheet (six columns, 837 px) and put the pane away by F7 for eight
and back by Tab.
The recent tests find the name once over the grid, and the hover text
goes when F7 takes the pane from under the pointer. A `testing::buttons`
finds buttons by label alone, since a text's own label is its words.

Two things on the testing backend, not in the editor: a row an unfold
has just added lies past the tree box's old foot and takes no click
there (in the editor it does; the test picks it by its callback), and a
control after an `if` that has just gone false in the same layout is
not found by label (in the editor it is there).

**Not done.**
- The keyboard in the roots list and the tree, the filter's count beside
  a root, and a width of the grid's own for the pane, as §211 left them.
- The row's menu is the chip's: the path, Rename..., Remove from
  library. §211 named a reveal in it too; the chip never had one.
- The chip's cross is gone, so a root is taken out only from its
  row's menu, by a right-click (or Control+click on a Mac): there is no
  way to remove a root from the keyboard or a screen reader, as there
  was none to rename one. A remove action of the row's own is the
  user's call.
- The root's row is not lit for a disk folder under it, nor the tree's
  root row: the folder is marked, the root's view is not open.
- Over the grid, F6 and F8 still do nothing; Tab is the pane's alone.
- The header's placeholder is cut at 200 px when the field gives way to
  the chips.

---

Built later the same day: the header's facets are on two fixed rows
in place of one, Camera and Lens on the first and Style, ISO, Focal
and Day (and any facet the index adds later) on the second, so the
lens chips are no longer lost down a sideways-scrolling line after
the bodies. The long chips are the reason: a body's or a lens's name
is most of a row's width, and the short facets are what scrolled the
lens off the screen. The window splits the rows, not Slint:
`library::split_facet_rows` sorts the index's rows by facet slot and
the window sets a long and a short model beside the one the culling
panel still takes, so the Slint stays a plain repeater, and the
stacked bar is unchanged, one column. Each row keeps its caption, its
chips and its sideways scroll only when it must. The first row carries
the note while the index has nothing, and the second takes no height
until it has a facet. The header is still a fixed height, spelled
out, one facet row and one gap taller: 145 px in place of 113, so at
the default 1500 by 950 window the sheet is 805 px tall in place of
837 and shows three rows and most of a fourth. The cost is part of a
row of thumbnails at the top of the grid, which the second facet row
buys back in what a tester can read at a glance.

