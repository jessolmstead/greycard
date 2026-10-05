# 246. The rejects folders a view shows, and deletes (2026-10-04)

Three things kept a rejects folder out of sight or out of reach. The
tree missed the one Move rejects had just made. A folder opened from
outside the roots showed no folders at all. And Delete rejects folder
answered every view but a folder's own with "a rejects folder is a
folder's: open the folder first", which read as nonsense on a folder
picked in the tree, since that already looks like an open folder.

**The tree after a move.** Move rejects and Move back tell the
indexer each move (`Indexer::moved`), then ask for a pass over both
folders. The tree is built again when a pass reports a file added,
moved, missing or returned (`tree::passed`), or when the root's count
changes. A move inside a root does neither: the count stays the same,
and by the time the folder passes run, the rows are already where the
files are, so they report nothing. The tree kept its old nodes until
a later pass or a restart. The indexer now says `Told::Moved` with
the folders a move left and reached, once the rows are written, and
the window marks the tree stale for each. It comes from the indexer
rather than from the move itself so the rebuild reads the rows after
the move, not before.

**Tiles for a folder under no root.** §212 gave a folder opened from
outside the roots no tiles, because the tiles came from the tree and
the tree comes from the index. We reversed that: a rejects folder
made there was out of reach from the window. Such a folder now reads
its own folders from the disk, on a thread of its own as the tree is
built (`tree::loose_nodes`). A folder has a tile when it holds frames
directly; not when it is hidden or a link, and not when its frames
are only further down, since the tile opens its folder as Open folder
does and Open folder refuses a folder with nothing directly in it.
The count is those frames, since walking deeper on a share could take
minutes; the tree's tiles count everything under them. Nothing
watches a folder outside the roots, so it is read again whenever it
is opened and whenever frames leave the list (`drop_files`: a move, a
Move back, a delete). A read still out when another is asked for is
let finish and the folder read once more when it lands, so a burst of
moves on a slow share does not stack readers. A folder that cannot
be read shows no tiles, rather than the ones it had. A tile opens its
folder as Open folder does, not as a `View::Branch`, since there is
no root to be a branch of. The pane still shows no tree there.

**Delete rejects folder over many folders.** The rule was §190's: the
all-roots view has no single folder to have a rejects folder beside
it. That was about what to delete, not about safety. Every guard
works per folder (each path checked against the allowed folders,
links refused, a sidecar never without its raw), and the sheet
already counts frames across folders. So a view over many folders,
which is all the roots, one root, or a folder of the tree with the
folders under it, now deletes every rejects folder it lists frames
in. The frames come from the list, not the disk, so nothing goes
that the view has not counted, and only the rejects folders are
canonicalized, never every folder in the list. A frame under an
offline root is left out, said only when nothing else is left. A
rejects folder that is a link is left alone with its frames, and the
sheet says how many. The title reads
"Delete 3 frames from 2 rejects folders?". Each rejects folder left
empty is removed, as the single one was.

**An archive's rejects stay with the archive's view.** Remove rejects
(§218) moves an archive's copies of the rejects into the archive's
own rejects folders. Over every root, one delete would list both a
reject here and its copy there, take both, and the sheet's line on
the archives would count the copy as the frame being safe. So a frame
on an archive is left out unless the view is of that archive (its
root's view, or a folder of its tree), and the sheet says how many:
"1 frame on an archive left alone: delete it from the archive's own
view." That is where §218 already sends an archive's rejects folder.

A rejects folder goes whole, whatever the filter shows, as it did
from the folder's own view: the sheet counts every frame that goes,
so nothing goes unseen.

A folder of the tree shown alone (`View::Branch` without the folders
under it) is one folder, and takes the rejects folder beside it from
the disk, as the folder's own view does. That holds even when nothing
of its own is left in the list, which is the case once every frame in
it has been moved out. The folder's own view had the same gap: it
found its folder from the first frame in the list, and with every
frame moved out it said "nothing is open". It, and Remove rejects
with it, now falls back to the folder Recently opened records.

**Remove rejects stays one folder.** Remove rejects weighs each
rejects folder against the archive before anything moves. Over many
folders that would be a batch with its own queue, and we have not
designed one. It now takes a folder of the tree shown alone, and over
a view of many it says "Remove rejects works on one folder: pick one
in the tree with With subfolders off".

Tests: a move in a folder of the tree, through a real indexer, shows
the rejects folder in the tree and the tiles (with the window's
handling of the indexer's word stubbed out, it fails); a view of the
roots deletes two rejects folders, leaves a linked one and the
shoots' frames alone, and with only the link left says so; over every
root a reject on an archive is left out and said, with only the
archive's left the status line says where to go, and the archive's
own view takes it; a folder of the tree shown alone deletes its
rejects folder; a view with no rejects says so; Remove rejects from a
folder of the tree with an empty list finds its rejects, and over the
roots says to pick one; a folder under no root shows its folders as
tiles (not one with frames only further down, nor empty, hidden or
linked ones), gains the rejects tile on Move rejects, keeps its
rejects folder to delete once every frame is moved out, opens a tile
as a folder, and shows no tiles once its folder is gone.

**Not done.** The menu item still says "Delete rejects folder..." in
the views where it deletes several. A folder under no root is not
watched, so a folder added there from outside shows on the next open.
The delete is planned on the window's thread, at the sheet and again
at the confirm, a few looks at the disk a frame; that was so for the
selection already, but every root's rejects at once makes thousands
of frames ordinary, and on a share that is a pause. A folder emptied
by Move rejects and then opened again is refused as a folder with no
pictures, though its rejects folder is there.
