# §304. Collections and smart collections, built (2026-10-11)

§273 settled the three questions; this is what we built on it, and the
details §273 left to the building.

## The files

`collections/` beside the library's database (so `~/.local/share/
greycard/collections/` for the user's library, and a `--library`
elsewhere carries its own, as `roots.json` does). One file a collection,
named by an id of 32 hex digits made at random when it is created:

- `<id>.json`: `{"version": 1, "name": "Cover picks", "set":
  "Clients/2026", "made": <seconds>, "refs": [{"hash": "...", "hint":
  "/photos/shoot/IMG_0001.CR3"}, ...]}`. `refs` is the collection's
  order. The hint is the path as the index keys it, as text.
- `<id>.smart.json`: `{"version": 1, "name", "set", "made", "filter":
  {...}, "root": "/photos" | "/photos/2026" | null}`. `root` is a root,
  or a folder of a root's tree, or null for every root. A folder is
  reopened as that folder with the folders in it (the tree's switch on):
  we keep no `deep` flag, so a smart collection saved from a folder
  shown without its subfolders comes back wider, and its row stays
  unlit over the narrower view until the row is pressed. A folder no
  longer under any root is taken as every root.

Each is written pretty, through a part file named with the process id
and renamed over it, as `roots.json` is. At load, a file that will not
parse, or one of a later version, is said in the log and left where it
is. A file read fine at load and then broken by a hand edit is written
over at our next save, with a log line; a file that turned a later
version after load is never written, and the changes made to it in the
session are lost at exit, said only in the log (both left open below).
Deleting a collection deletes its file.

**The smart collection keeps the bar, not a line of text.** §273 asked
for the filter in the filter bar's text form, read back by
`Filter::parse`. The bar's chips cannot all be spelled in the language:
the flag and label rows are any-of (picks or unflagged), and a facet's
chips are a `Term::Facet`, which the language deliberately has no text
for. So the file keeps the bar's state in the form the settings already
keep it between sessions (`filter::Saved`: stars and whether exactly,
flags, labels, the text field, the facets by field). The text field is
in it verbatim, in the language, and is what `Filter::parse` reads; the
chips beside it come back as chips. A Lightroom import (§79) writes the
same shape. This makes `filter::Saved`'s serde shape a file format: a
change to it bumps the collections' `VERSION`, so an older build leaves
the newer file alone rather than misread it.

**Sets are typed.** A name with slashes, `Clients/2026/Cover picks`, is
the collection `Cover picks` in the set `Clients/2026`, as keywords take
their hierarchy from slash paths. The list sorts by set, then name, and
shows the set muted before the name. There is no set as a thing of its
own to create or delete.

## Membership, and what an open shows

Membership is by hash, as §273 says, and every row of a member's hash
wears the mark, the selects' copy of a frame as much as the shoot's.
The window keeps the ids of those rows (`ids_by_hashes`) and asks again
when a collection changes and after each pass the index tells it of.

A collection opens as a view of the index, as a root does
(`View::Collection`). The read off the window's thread finds each
reference (`collections::resolve`): the file at the hint when the index
has one of that hash there and its root is not offline; else the first
of the hash's files under a root that is not an archive; else any online
copy, the archive's included; else the hinted one, dimmed as an offline
root's frames are. Each hash is listed once. From there the list goes
through the same path as a root's view (folders that cannot be read left
out, offline roots dimmed, stand-ins from the rows), kept in the
collection's order.

**The hint follows the file.** When nothing of the hash is at the hint
any more (a shoot moved, the selects copy deleted), the open moves the
hint to the file it showed and writes it. Without that, a frame moved
and later rewritten by another tool could not be healed, since the heal
goes by the hint.

**A missing reference is counted, not drawn.** A reference with nothing
at its hash or its hint is kept in the file and said in the status line
when the collection opens ("Cover picks: 12 frames shown, 2 missing").
We do not give it a tile: a tile is a frame the browser can open, rate,
export and delete, and every one of those would need a case for a file
that is not there. When the shoot comes back under a root, the next
open shows it again.

## Healing in the index's pass

The index's report gained `rehashed`: each file at a path it already
had a row for whose content hash changed with it, with the old hash and
the new (`Plan::Changed`, and a row another writer added between the
phases). The row now carries its hash for this. The editor reads the
reports of every pass where they land on the window's thread (the
launch and watcher passes, the open folder's pass, and the rows written
after a save, whose `index_file` can see a rewrite too) and calls
`Store::heal`: for a background pass after its line in the log, for the
open folder's pass before the window checks the pass is still the one
it wants (a pass over a folder since left heals all the same), and in
both before the window's own reaction to the report. Only rewrites of
a member's hash are looked at, and only those hashes are asked of the
index; when the index cannot be asked (no reader, or a read that
fails) the copies are unknown, nothing is taken for gone and no hint
is moved.

The heal goes in two rounds, decided before anything changes. First by
the hint: a reference to the old hash whose hint is the rewritten
file's path follows the file to its new hash. Then, for a reference no
rewrite's path matched, only when the old hash is now nowhere in the
index: the picture was rewritten wherever it was, and the reference
follows the first rewrite of it. So when a pass rewrites the shoot's and
the selects' copies of one picture to two different hashes, each
reference follows the copy at its own hint. A reference whose old hash
is still on another copy, and whose hint is elsewhere, stays with that
copy: it is the same picture there.

A heal never drops a reference. When the new hash is already another
reference's (a file overwritten with another member's content), taking
it would list that other picture twice, and the overwrite says nothing
about where this reference's own picture went. So the reference stays
on its old hash: with its hint moved to a copy of its picture that
survives, or, with none, as it was, missing until the picture is
found, by the same rule an open keeps. Each collection changed is
written at once. If the collection healed is the one open, its list is
read again.

The open heals too: a reference whose hash the index has nowhere, with
a file at its hint, takes that file's hash, unless that hash is one the
collection already holds (the file at the hint is then another member,
and the reference stays, missing). That covers a rewrite while
the editor was closed whose launch pass reported it to nobody (a run of
the CLI's `library index`, say).

A read of a collection carries the collection's id, and what it finds
is put into that collection, whichever view is open when it lands: a
merge read of one collection can land after another has been opened.

**Two editors on one folder.** Each file's mtime and length are kept
from the read and from our last write, and with them the set of hashes
the file held then. A save that finds the file changed since (a second
editor, a hand edit, or only its time moved by a sync or backup tool)
reads it and joins against that set: a reference on disk that was not
in it is the other side's addition and goes at our end; one in it that
is gone from disk is the other side's removal and is taken out of ours;
what we added or took out ourselves stands, so a touched file never
brings back our own removals or a healed reference's old hash. Both
are said in the log. The trade is that the join is by hash and by
membership alone: the other side's reordering and hint moves are lost
under ours, and a reference both sides changed (healed here, removed
there) follows whichever rule above reaches it. A smart collection is
ours and the other is logged as overwritten. A new id is drawn again
while a file of that id exists.

The marks are asked again only when a pass added, moved, brought back,
lost or rewrote rows, never for a sidecar save alone, and when rows
go outside a pass (a root taken out of the library, the window's own
deletes); the rows on screen are gone over only when the member rows
actually changed.

## The window

- **The pane.** A COLLECTIONS section under ROOTS and FOLDERS in the
  grid's left pane, folding like them (its fold kept with theirs). A
  row a collection: the set muted, the name, the count; the smart ones
  after them with the sparkles mark and no count (counting one would be
  running its filter over every root). A click opens it. The row's
  right-click menu: Rename..., Capture order (checked while on), and
  Delete..., which asks first on the same sheet; the frames are not
  touched. "+ New collection..." at the foot.
- **The frame menu**, in the grid and the strip alike: an "Add to
  collection" submenu of the collections by name and "New
  collection...", and while a collection is open, "Remove from <name>"
  (or "Remove 3 frames from ..."). Each acts on the selection, as every
  item there does.
- **B**, as Lightroom has it for its quick collection: the selection
  into the collection open, else the last one added to or made this
  session (the target is not kept across sessions); when every frame
  of the selection is in it already, B takes them out. With no
  collection to go to, it asks for a new one with the selection.
- **A drag in the grid** reorders a collection open in its own order: a
  press moved past six pixels drags, a bar marks the gap it would land
  in, and the frame (or the set, when it is in the set) goes before the
  frame after that gap. The press is then no click. With a filter on,
  the drag is among the frames shown, and they move in the full list.
- **Capture order** is the row's menu item, a view of the list by the
  time taken (the undated last), and rewrites nothing. A drag then does
  nothing. The lightbox, when it comes, reads the collection's own
  order from `Collection::refs`.
- **The filter bar's Save...**, beside Clear in the grid's header (not
  the CULLING section's narrower bar), keeps the bar as a smart
  collection over the view's own scope: the root, a folder of a root's
  tree (kept as that folder with the folders in it), or every root for a
  view of every root or a folder opened from the disk, which the sheet
  says in words before it is saved. Over a collection there is no
  Save...: its list is no place a filter can be kept over. Its row puts
  the filter back on the bar and opens that scope; it is lit while the
  bar says its filter there, and goes dark when a chip moves.
- **The mark.** A bookmark in the corner beside the edited and exported
  marks (§276), on the grid's tiles and the strip's cells.

Where §273 left a detail open, we took the plainest thing the editor
already does: the root sheet's look for the name sheet, the roots'
rows for the collections' rows, the frame menu's existing pattern for
the submenu, Lightroom's B because it is the key people have in their
hands.

## Left open

- On Windows a rename onto a file another program holds open fails;
  the write is said in the log and the file on disk is the older one
  until the next save of that collection.
- Frames in a folder that cannot be read (not an offline root, whose
  frames are dimmed) leave a collection's view without being counted
  among the missing.
- A "Forget missing" for a collection's references that will not be
  found again is a follow-up; until then a missing reference stays, and
  the status line counts it, for good.
- A file that turned a later build's after load is never written over,
  and the session's changes to that collection are lost at exit while
  the status line still says "added to"; the status should say so, or
  the collection go read-only.
- A file broken by a hand edit after load is written over; a copy to
  `<id>.json.bad` first would keep the edit.
- The change stamp is mtime and length, so an outside edit inside one
  mtime tick that keeps the length (a hint moved to a path of the same
  length, on FAT or SMB) is missed and written over.
- Under `--no-sidecars` a view of the index holds no row ids for its
  frames (an existing gap: the stand-in from a row is skipped with
  sidecars off), so the marks do not show there; adding and opening
  still work, by hashing the files.
- Adding a root while a collection is open does not read the
  collection again until the next pass under it.
- The CLI's `library index` reports rewritten files and heals nothing;
  the editor's open heals them by the hint afterwards.
- Virtual copies: a reference will carry the copy's version once they
  exist; the file has no field for it yet.
- A smart collection's count, and moving a collection between machines,
  stay with the carry item on the roadmap.

**Added the same day.** Windows CI failed seven of the eight panel
tests at teardown: the window's read-only index reader was still open
when the test removed its folder, and SQLite on Windows opens its files
without `FILE_SHARE_DELETE`, so the removal gets "being used by another
process" (the cull test's reader and the pairing test met the same
wall). A test-only `teardown` now drops the writer, clears the reader
and drops the app, the worker and the state before the folder goes.

