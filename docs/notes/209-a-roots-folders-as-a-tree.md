# 209. A root's folders as a tree (2026-09-30)

Roadmap line (Next 0.4.0): "A folder tree under a chosen root: the
root's folders as a tree in the left pane, from the index rather than
the disk so an offline root still shows its shape, each folder with its
frame count, a click opening that folder's frames (its subfolders' too,
as a switch) and the view's chip staying on; a folder's open then reads
only that folder's sidecars rather than the whole root's, which is the
cheap way into a large archive."

**What it was.** A root's chip opened everything under the root (§174),
every frame standing in from its row since §207, and the filter's
`folder:` term narrowed it by typing. To get at one day of a 12,000-frame
archive the choices were that, or Open folder and the chooser walked to
it, which lists the folder on disk and knows nothing of the root. The
index had every folder's rows and nothing drew them.

**Now.** While a root's view is open, the left pane shows a FOLDERS
section under the name of what is open (§206's name under the Open
folder button): a switch, "With subfolders", and the root's folders as
a tree in a box that scrolls past 320 px, as the history's does. The
root is the first row, by its chip's name, unfolded the first time its
tree is shown; each folder after the one it is in, indented 12 px a
level, a triangle where it has folders of its own, its name, and its
count muted at the right. The folder open is lit. A click on a row
opens that folder's frames; a click on its triangle folds or unfolds it
and opens nothing. The section folds away like the others and is kept
folded in `collapsed` as "folders". It is not in the grid's header,
which keeps the chips; so it is seen with the loupe, not over the grid.

The root's chip, in the grid's header, is the one place a root is
chosen; there is no chip in the left pane. The tree goes under the
pane's name of the open folder rather than under a chip, since that name
already says "harbor in Archive" and is where the eye goes for what is
open.

**The counts.** A folder's count is the frames directly in it while it
is unfolded, and the frames under it in all while it is folded, as the
line asked. An unfolded folder with nothing of its own (the year above
the days) shows no count rather than a 0 beside every year.

**From the index.** The tree is built from one query,
`Library::folder_counts_under_canonical`: the folders under the root
that hold files, with how many each, the missing left out, a `GROUP BY`
over the folder column within §174's range. A folder with files only
under it (the year) is not a row of the index and is made by its
children's paths. Nothing is listed on disk, so an offline root's tree
is its shape as the index last saw it, and its folders open from their
rows, dimmed, as §207 opens the root itself. The query and the tree are
built on a thread of their own with a read-only connection of their
own, and handed to the window whole (`tree::Build::run`, `tree::land`),
as a view's rows are; a build asked for later drops an earlier one by a
token. A test window with a reader and no index path reads the counts
on its own thread, as `open_view` does its rows.

The tree is asked for when a root's view lands and whenever the
indexer's word lands (`background_done`), and built again only when it
is behind: the root changed, the root's count moved, or a pass added,
moved, lost or found files under it (`tree::passed`), which catches a
move that leaves the count as it was. A pass that lands while a root's
first build is still out marks that build stale too, so its older counts
are not kept. A pass that only refreshed a sidecar (a rating) moves
nothing in the tree and builds nothing. The tree already on screen stays
until its replacement lands, unless it is another root's.

A row comes back from the pane as its number in the list shown, and its
folder is looked up on the Rust side (`Tree::shown`): a path through
the window's strings is lossy, and a folder whose name is not UTF-8
would come back as another path, open nothing and fold nothing.

**A folder opened.** `View` has a third case, `Branch { root, folder,
deep }`: a view of the index narrowed to one folder, the root's view
still. It goes through `roots::open_view` exactly as a root's view does:
the rows read off the window's thread, each frame standing in from its
row, its sidecar read when something needs it (§207), the folder looked
at once for whether it can be read, a root offline taken as §207 takes
it. Without the switch the rows are the folder's own
(`Library::rows_in_canonical`, one indexed equality on the folder
column, by name); with it, the rows under it (`rows_under_canonical`
with the folder as the root). The root's chip stays lit (`show` takes a
`Branch`'s root), the merge after a pass reads the same rows again
(`ask`), a pass is worth a read when it is over the folder or above
it, and with the switch on also under it (without the switch, a pass
under the folder cannot change its own rows), the index's folder pass
is skipped as for a root's view, and a root
removed under a `Branch` goes to all roots as it does from the root's
view. The root with the switch on is the root's own view,
`View::Roots(Some(root))`, the same list its chip gives; with the switch
off it is the root's own frames alone.

This is what the line meant by the cheap way in: a folder of twelve
opens as twelve rows and one folder looked at, whatever the root holds.

A folder with nothing directly in it, opened without the switch, opens
empty rather than leaving the list as it was: the tree marks it, and the
status line says "nothing directly in 2025: turn on With subfolders to
see the frames under it". The first cut sent it to the root view's
empty path, which said "nothing under the library's folders". With the
switch on, a folder that comes back empty says "nothing under 2025"
rather than offering the switch it already has.

**The switch** is `settings.folder_tree_subfolders`, off by default: a
folder's own frames are the cheap open, and the root's chip is already
the everything-under-it one. It is written when flipped
(`prefs::keep`), kept from the file at the close as culling's move-on
is, and flipping it opens the folder the tree marks again with it,
unless that is the view already open (the root's own view, turned on).

**Recently opened** records a `Branch` as the folder, with or without
the switch: it is a folder opened. `recent::opened` takes the view's
folder as it stands, the index's canonical spelling, rather than
`folder_of`'s realpath of the files' folder, so an offline root's folder
is recorded without a look at the disk. It goes through `open_loaded`
as every list does; nothing new records. Chosen again from the list, it
opens as a folder from the disk (§206), its own frames, and the chip
goes off; a folder under an offline root says it is not there, as any
other gone folder does.

**Measured.** Debug build, local disk, a root of 12,000 hard links in
1,000 day folders under ten years (1,011 folders in all), indexed:
- the tree built in 20.0 ms off the window's thread, the connection's
  open included;
- the root's view in the browser 503 ms after the chip, on screen at
  530 ms (for comparison, not changed here);
- a day of 12 from the tree in the browser at 58 ms, on screen at
  140 ms, 0 sidecars read, one folder looked at.

**Checked in the editor**, on a sandbox library of hard links (root
Archive: a frame of its own, 2025/harbor 3, 2025/city 2, 2026/coast 1,
2026/coast/select 1): the chip opens the root and the tree shows Archive
1, 2025 5, 2026 2, the root lit; unfolding 2025 shows city 2 and harbor
3 with 2025's count gone; city opens its two frames, lit, named "city in
Archive" under the button; 2026 without the switch opens empty and says
so; the switch on reopens 2026 with its two frames under it;
settings.json then holds `folder_tree_subfolders: true` and 2026 at the
front of `recent_folders`. With the root renamed away, the chip's view
listed its 8 frames and the tree its 7 folders from the index; a capture
could not be driven further, since the view's first frame is offline
and a failed open ends a capture run.

**A harness fix on the way.** `--keys` with `--snapshot` sent every key
twice when a click opened a list whose frame developed: the flag that
guards a second sending was set only on the no-snapshot path. A fold
clicked that way came back folded. `schedule_snapshot` sets it with the
snapshot too.

**Tests.** The library: a root's folders counted, its own among them, a
parent with nothing of its own and a sibling whose name starts like it
left out, and a folder's own rows without those under it. The tree: built
from counts (nesting, counts direct and total, ordering with the case
folded, a sibling root left out), a root with one folder, with none, and
with its frames all in itself; folding hides what is under and shows the
total, an unfolded folder with nothing of its own no count; the root with
the switch is the root's view. On the headless window: the root's view
shows its tree with the root lit; a folder chosen lists its own frames,
standing in and unread, the chip on and the row lit; the switch lists
those under it too; the root with the switch is `Roots(Some)`, without it
the root's own frame; a folder with nothing of its own opens empty and
says why. A click on a row found by its accessible label opens the
folder and records it in Recently opened, named with its root; a click
on its triangle, by its own label, unfolds it and opens nothing. A pass
that adds a folder has it in the tree; a sidecar's refresh builds
nothing; a frame moved to a new folder with the root's count unchanged
has the tree built again by the move alone, with the tree built and
with its first build still out (checked to fail without `passed`). An
empty folder offers the switch only when it is off. An offline root's
tree comes from the index and its folder opens from the rows, dimmed.
`roots::land_sent` lands the tree builds with the reads.

**Not done.**
- No keyboard in the tree (none was asked for): no arrows through it,
  and a row takes no focus.
- The unfolded folders are the window's, not kept across launches, and
  a folder opened by other means (Recently opened, Open folder) does not
  unfold the tree to it: that is a folder's view, not the root's, and
  shows no tree.
- A folder opened with Open folder under a root is still a folder from
  disk (§206), with no tree; only the chip's view has one.
- A folder with nothing of its own opens empty and leaves the last
  frame's picture on the loupe, as an empty view of the roots does.
- The count shown beside a folder is the index's, as the chips' are;
  frames filtered out are counted.
- A rejects folder under a day is in the tree as a folder like any
  other, since the index holds its frames.
- The build reads every folder under the root at each rebuild; a
  rebuild that touched only the folders a pass went over would be less,
  and at 20 ms for 1,000 folders off the window's thread was not worth
  it.

---

Changed the same day, at the desk. An archive's client folders hold
their frames one level down, in a folder of the same name, and each
opened as a blank grid: the status line that said "nothing directly in
it: turn on With subfolders" is not shown over the grid, and
a tester reads a blank grid as a folder that will not open. A folder
with nothing directly in it now opens with the frames under it whatever
the switch says, decided in `open_view` from the index's two counts, so
a tree built before the folder emptied cannot leave the list blank. The
switch itself is left as it was. Only a folder with nothing under it at
all lands empty, and says "nothing under" it.

