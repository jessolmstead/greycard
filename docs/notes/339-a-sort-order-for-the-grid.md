# §339. A sort order for the grid (2026-10-17)

The grid's order was fixed in the index's query, `ORDER BY
files.folder, files.name`. Within one folder that is the camera's
numbering, which is what a shoot wants. Over every root, or a
collection made from several, it is alphabetical by folder path, and
two bodies' frames of one day fall wherever their folders' names and
their letter prefixes put them (`IMG_` before `_DSC` before `scan`).
A user with a whole archive open called that arbitrary, and it is.
This gives the filter bar a Sort menu, a choice a view, kept across
sessions, and one rule for the order that every reader of the list
follows.

## The rule

`greycard_library::order` holds it: an `Order` is what by (`SortBy`:
capture time, name, rating, date edited) and which way up. It has two
spellings that agree, and a test holds them to it:

- `Order::sql`, the `ORDER BY` a query of the index lists by:
  `rows_under_canonical`, `rows_in_canonical` and `query_sorted` take
  an order;
- `Order::compare`, over a `SortKey` (a path, a capture time, a
  rating, a date edited), for a list made somewhere else: a folder
  read from the disk, a collection's references found one by one, the
  command line's stacks.

The SQL compares the `name` text and the `path` bytes; the comparison
compares the name as the index writes it (the file name, lossy) and
the path through `path_bytes`, the bytes the column holds. So a
folder read from the disk and the same folder opened as a root's read
in one order, ties and all; `each_order_lists_the_index_as_its_
comparison_sorts` sorts the index's rows, reversed first, by the
comparison and checks the query's order comes back, for all eight
orders.

**Ties** go by the file's name, then its path, whichever way the
order runs. A burst taken within one second then reads in the
camera's numbering, newest first or oldest, and every pass over the
same rows gives the same list, so a merge never shuffles equals under
the user. Reversing the ties with the order would put a burst
backwards in "newest first", which reads as a mistake. Two files of
one name in two folders (two cards that both started at `IMG_0001`)
go by their paths.

**A frame with no value goes last, both ways up**: no capture time in
its EXIF (a scan, a screenshot), or no develop when the order is date
edited. It is neither the oldest nor the newest, and putting it first
in one direction hides the frames that have a date behind the ones
that have none. In SQL this is `NULLS LAST` on both directions.

**Capture time** is the index's `taken`, the EXIF's original date
normalized to sort as text. It is read through one function,
`order::capture_time`, with `CAPTURE_TIME_SQL` its SQL twin, so the
capture-time shift on the roadmap (kept in the sidecar's meta, a
second body's clock brought into line with the first) can be laid
over it in one place, on both spellings, rather than in every list.

**Date edited** is the sidecar's last write as the index last saw it
(`files.sidecar_mtime`), counted only for a frame whose row has the
`edited` mark (its sidecar holds a develop). We took the mtime because
the index already has it: no schema step, no sidecar read, and a view
of a hundred thousand frames sorts on what it already reads. We gated
it on `edited` because a sidecar written for a rating alone is no
edit: without the gate every frame culled this week would sort as
"edited" this week. The cost of the mtime is what it measures. A
rating given to a developed frame moves its date edited, and so does a
copy or a sync tool that does not keep times. The sidecar's
revisions (§233) carry the time of each save and travel with the
file; mirroring the last develop's time into the index is the better
source, and wants a column and a pass that reads every sidecar again,
so it is left for when the mtime is found wanting.

**Rating** is the row's stars, none as nought; it ties by name.

## Where the order is read

The browser's list is put in its order where it is read, off the
window's thread (`roots::Look::run`), never on it:

- a view of the index (every root, a root, a folder of a root's tree,
  with its subfolders or without) by the query's own `ORDER BY`;
- a folder read from the disk, or files the desktop handed over, by
  `Order::compare` over the rows the index has for them, a file with
  no row yet sorting as one with no value (by name, last);
- a collection by `collections::resolve`, which sorts the references
  it found by the same comparison (a version after its file's own,
  the collection's order otherwise kept among equals), or leaves them
  in the collection's own order.

Everything that walks the list walks `State::files` and the cells made
from it, so it follows with no word of its own: the grid and the
strip, the arrows and keyboard navigation, culling's arrows and its
loupe, Home and End, compare, the lightbox, the rubber band, the
export queue. The filter's counts do not change: a count is no order.
A closed stack is its top's cell where the top sorts, and an open
stack's members follow the top in the stack's own order
(`stacks::arrange`, untouched). A camera's JPEG is the raw's companion
and no frame, so a pair sorts by the raw's values; with JPEGs listed
apart it is a frame and sorts by its own. A version's cell follows
its file's own, as `versions::cells_of` always laid it.

The command line's `library list` gains `--sort`, with the names the
settings file keeps (`capture`, `name`, `rating`, `edited`, each with
`-desc` for the other way up), capture time by default, as the editor
lists every root. Its stacks follow the same rule: a closed one's top
alone, where it sorts, and with `--stacks expanded` (or a stack kept
open) its members gathered behind the top in the stack's order
(`stacks::gather`, beside `collapsed`). The stacks are laid over the
whole list before the filter is asked, as the window makes its cells
and then filters them: an open stack whose top the filter drops keeps
its members that pass at the top's place, in the window and on the
command line alike. The first cut gathered after the filter and put
such members at the end. Before this the listing was by
folder then name, and the stack's members fell wherever their names
put them; `Library::query` keeps that order for anything that wants
the index's own listing.

## A choice a view, and the defaults

The choice is kept in `settings.json` beside the filter, as
`"sort": {"folder": "name", "roots": "capture-desc", "collections":
{"<id>": "hand"}}`, written as it is chosen (as culling's switch is,
so a session that never closes cleanly keeps it), and read back at
launch. A file from before the sort has no `sort` and reads as every
view at its default; a name this build does not know (a later build's
order) reads as the default and is left in the file as it was until a
choice here replaces it. One choice for each of three kinds of view,
and one for each collection:

- **One folder** (opened from the disk, or one folder of a root's tree
  on its own): **name**, so a folder's order stays the camera's. It is
  the order the browser always had there.
- **Several folders** (every root, one root, a folder with those under
  it): **capture time, the oldest first**, so the two bodies' frames
  of one day read as the day went. A smart collection opens as one of
  these views and so takes the same choice; we did not give it its own,
  since its frames are whatever its filter finds and its view is a
  root's.
- **A collection**: its own **hand order** until another is chosen,
  which lays the hand order aside without rewriting the file; "Hand
  order" is the first entry of a collection's menu, and brings it
  back. Each collection keeps its own choice, and a deleted
  collection's goes with it. A drag reorders only in hand order. The
  collection row's "Capture order" stays, as a quick way to capture
  time: it is checked whenever the collection is in any order chosen
  (newest first included) and takes it back to hand order, and the
  open row names its order ("capture time", "rating"). The first cut
  checked it for oldest first alone, so a collection newest first
  showed it off and a pick went to oldest first.

We kept one choice for all folders rather than one a folder: a folder
sorted by rating once would otherwise stay so, unseen, the next time it
is opened months later, and the list of folders would grow with no
bound in a file the user may read.

## A choice taken

A pick from the menu keeps the choice and reads the list again in
place (`refresh_view`, a merge read). The merge carries every frame's
sidecar, picture, row and place in the selection by its path, so the
frame on screen stays chosen; when the read lands, the strip and the
grid are brought to it (`reveal-selected`), since the row it is on may
be the same number while every row around it has moved. A list with
nothing to read it again from (files the desktop handed over from
several folders) keeps the choice, says so, and is in it the next time
it is read.

A view still being opened cannot be read again either (a read is out,
and `refresh_view` waits for it), and a choice made then is for the
view's kind, so a root's chip pressed from every root's view and Name
picked while it loads would land the root in capture time under a menu
saying Name. Every read now carries back the order it was made in, and
an open that lands in an order other than its view's choice now is
read again at once in it, the frame on screen brought into view. A
test holds the read out, picks, and lands it.

**The order in words.** The Sort button is the arrows alone (below), so
the header says the order on its top row, beside the frame count: "8
frames · by capture time · 176 px", lowercase, with no way up, which
the arrows carry. The review asked for it beside the filter's count,
"8 of 8 · capture time"; there it cost the last two label chips at the
default width, and the top row has room to spare. The CULLING
section's bar, which has no chips on its count's row, says it beside
the count.

The order is read when the list is. A rating given while the list is
by rating does not move the frame under the user's hand: the save is
indexed and the filter asked again, but the list is not read again,
so culling's arrows still go to the frame that was next. The frame
moves at the next read: a sort chosen again, the view opened again, a
pass over its folder that finds a change. A folder from the disk whose
rows were not in the index when it was opened (a new card) is read
again once its first pass has written them, when its order is
anything but name, which needs no row.

## Measured

`capture_time_over_a_hundred_thousand_rows_costs_about_what_name_does`
builds 100,000 rows in 200 folders under one root, two bodies'
prefixes interleaved, the capture times scattered over a year and one
in fifty undated, and times the all-roots query with each row's meta,
as a view of every root reads it: the best of three after a warm-up,
then the same with the index on `taken` dropped. Release build, the
index in memory, three runs, a load average of 11 to 15 from the
other builds on the machine:

| 100,000 rows, every root | by name | by capture time | capture time, no index on `taken` |
|---|---|---|---|
| the rows with their meta | 160 to 174 ms | 191 to 221 ms | 172 to 183 ms |
| the paths alone | 15 to 18 ms | 38 to 50 ms | 34 to 38 ms |

The old order, by folder then name (`paths_under_canonical`), was 101
to 120 ms for the paths. Capture time is within a third of name with
the meta, which is what the window reads, and the difference is under
50 ms on a hundred thousand frames: the same order of time. In the
debug build the test asserts it (627 ms by name, 683 ms by capture
time, 659 ms without the index).

SQLite's plans say why: by name it scans `files` and sorts in a
temporary B-tree; by capture time it walks `files_taken` (in the
schema since the first) and sorts only the ties' two terms, but every
step of the walk is a lookup into the table out of order, which costs
a little more than the scan and sort it saves. Without the index it
scans and sorts as by name, and is the faster of the two here. A root
that is a small part of a large library would take the folder's index
for its range and sort what it gives, as every order does. So we added
no index and no schema step, and there is no first pass to pay for: a
library opens in this build exactly as before. We kept `files_taken`,
which the `date` facet and filter terms use. Most of the time with the
meta is reading each row's versions as JSON, which every order pays
alike.

## Tests

Sixteen new, and four grown:

- The SQL for each order on a fixture index of two folders, two
  bodies, a burst in one second, one name in both folders, an undated
  frame, and developed and rated-only frames: the query, the rows and
  an entry's key against the comparison for all eight orders, and
  capture time both ways, name, rating and date edited spelled out;
  the key's names read back; the measurement (`order::tests`, six).
- `stacks::gather`: an open stack's members behind its top, and kept
  at the end when the list lacks the top.
- The memory: each view's default, a choice kept for its view alone
  and read back from the file, an old settings file with no sort, a
  later build's name and a hand order kept for a view that has none,
  the menu's entries and the way up kept across a pick (`sort::tests`,
  four), and the whole settings file's round trip with a sort in it.
- The window (`roots::tests::sort`, four), each by its callbacks:
  every root by capture time; Name from the menu, the list read again
  in place with the frame on screen kept and on its new row; the
  strip's rows, culling's arrows, Home and End and the strip's arrows
  walking the order; the other way up. A folder from the disk by name,
  then capture time from its rows. A collection's hand order, a sort
  over it that leaves its file alone and refuses a drag, Hand order
  back, and the row's Capture order from newest first back to hand
  order. A sort picked while a view's read is out, read again when it
  lands.
- The command line: every key parses and every order has one, capture
  time the default, a bad name refused, the listing in each order
  over two folders, a closed and an open stack in it, and an open
  stack whose top the filter drops (`the_listing_sorts_as_the_grid_does`).

The window's other tests were written when every view was by folder
then name, and thirty-six of them read a view of several folders and
check frames by their place. The window starts in the product's
defaults in every test; the eight helpers those tests build their
windows with (the stacks', the collections', the folder tree's, and
five of the roots' tests') set name order over several folders, which
over their fixtures is the order they were written for, and say why.
The first cut set it for every test window in `State::empty` under
`cfg(test)`, which ran the whole suite under a default the product
never ships. One test of the folder tree saw the root's own frame move
from first (folder then name) to last (name) and was changed to say
so. After the rebase onto master, 1,198 non-UI and 1,307 UI tests
pass; clippy and fmt are clean.

## Captures

Under a headless weston (`--renderer=gl --idle-time=0`), the debug
build, two folders of hard links to eight sample raws from three
bodies, `--roots` over them, `--no-sidecars`:

- `grid-capture-time.png`: the all-roots grid by capture time, the
  two folders' frames interleaved by the day (a Nikon from 2025, a
  Canon from February, two Canons in one second by name, then the
  Fujifilm frames of that evening).
- `sort-menu.png`: the Sort menu open (`--sheet sort`, added for it):
  the four orders, Capture time checked, and Oldest first checked
  under them.

**The header at the default width.** The first cut put the order's
name on the button ("Capture time"), and at the editor's default 1500
logical pixels it pushed the last four label chips off the header's
row. The button is now the arrows alone: they point the way the order
runs, its tip and accessible name say the order ("Sort: Capture time,
oldest first"), and the menu checks both. The text field gives up
width down to 136 px (it stopped at 200), and Save..., Clear and Sort
keep their width, so every chip, the count and the three buttons fit
at 1500. We chose the picture over wrapping the chips or scrolling
them: a wrapped row makes the header a row taller (its height is
written out, and the grid's layout test counts on it), and a scrolled
one hides chips that were on screen before; the order is a choice made
once a view and read from the grid itself, so it needs no words on the
bar.

## Left, and questions

- The command line's `library list` was by folder then name and is
  now by capture time unless `--sort` says otherwise. Scripts that
  read it in the old order want `--sort name` (or, for folder then
  name, the folder in their filter).

- The frame on screen followed out of a folder's view by a move is
  kept in the list where its name sorts, whatever the order
  (`kept_up::merge`): the merge has no row for it there.
- A view of every root by capture time walks `files_taken` in order
  and looks each row up out of order, which measured slower than a
  scan and a sort (above) on a warm index in memory. `ORDER BY
  +files.taken` keeps SQLite off the index for the order; it is worth
  measuring on a cold index on disk before it is changed.
- Date edited from the sidecar's revisions, which travel, rather than
  its mtime, which a copy may not keep (above).
- A capture-time shift: the reading is one function; the shift itself
  is the roadmap's.
- No order of its own for a smart collection, and none a folder.
