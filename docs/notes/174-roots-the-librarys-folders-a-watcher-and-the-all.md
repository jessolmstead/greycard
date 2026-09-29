# 174. Roots: the library's folders, a watcher, and the all-roots view (2026-09-25)

This is the roadmap's v0.7.0 roots line, plus the parts of the
filter-bar line that roots unblocked: the all-roots view and a filter
remembered between sessions. §72 named roots as the library's second
step after the index. §160 left `index_tree` and `folders` as their
start, and §168 left the `Indexer` thread taking a list of folders.
Wave C's largest item, an opus author and an opus reviewer, two
reads and three rounds.

**Where the roots live.** A root is a folder the user has added. The
list is truth, not cache: the index can be deleted and rebuilt from
the files, but only if something says which files, and that is this
list. So the list is not a table in `library.sqlite`, which a rebuild
would drop, and it is not in settings.json either. It is `roots.json`,
in the same folder as the index (`greycard_library::Roots`).

It sits beside the index rather than in the editor's settings because
it describes the library, not the window:
- a `--library` elsewhere carries its own roots beside it;
- a test's library has its own roots in its own directory, so no test
  can reach the user's;
- §72's collections file, "one small file of its own under the data
  directory", will sit beside it for the same reason.

The file's format and how it is guarded:
- It is `{"version": 1, "roots": [...]}`, written through a part file
  named with the process's id and renamed over it.
- Adding or removing a root reads the file as it is on disk at that
  moment, changes it, and saves it (`Roots::edit`). So two editors
  open on one library each keep the root the other added. The first
  cut saved each editor's whole list, and one editor's root was lost;
  both also wrote the same part file.
- A file that will not parse is renamed to `roots.json.unreadable`
  rather than written over by the next save.
- A file that cannot be read, or one a later build wrote (a version
  above 1), is an error. The editor then keeps no file to save to for
  the session, rather than write an empty or older list over it. The
  first cut read a permission error as "no roots" and wrote over the
  file on the next add.
- A path that is not valid Unicode cannot be held in JSON. It is
  refused when added, by name.

A root is canonical (`dunce::canonicalize`, as the index stores
folders), and no root is under another:
- adding a folder already under a root does nothing, and says which
  root covers it;
- adding a folder above existing roots takes their place, and says how
  many it absorbed.

So a file is under one root at most, and a pass over every root never
walks a folder twice. Removing a root forgets it and touches nothing:
not the folder, not its files, not their rows in the index. The rows
cost nothing to keep and are what a re-added root finds unchanged.

The grid's header has a row for the roots, under its first row. The
ways to add a root come first, so they are never the part pushed past
the edge:
- "+ Add a folder...", through the same chooser as Open folder (the
  portal on Linux, rfd elsewhere);
- "+ Add this folder", while the open folder is under no root;
- an "All roots" chip with the total;
- a chip per root with its name, its file count from the index (the
  missing left out), and a cross that removes it.

A root's chip opens the all-roots view narrowed to that root. The row
keeps each chip whole and scrolls sideways, with a plain wheel as the
facet row does. The first cut elided every chip to "…" at six roots
in 560 logical px. The header's height is still written out, one chip
row taller: the grid's layout test moved from 805 to 773 px of sheet,
still four rows and the first 32 frames.

**Offline roots.** A root that is not there, or cannot be read, is
offline. The common case is a drive unplugged, since udisks removes
the mount point when it unmounts. An offline root gets:
- no launch pass;
- no watch;
- a chip dimmed, saying "offline" in place of its count;
- its files left out of the view, with their rows in the index kept as
  they were.

The first cut passed over an unplugged root and marked every row under
it missing: Lightroom's exclamation mark, which §72 set out not to
build. The watcher says nothing about an offline root either, so an
unmount seen as the root going away is not a tree pass over it.
Frames in a folder under a root that cannot be read are also left out
of the view. The index keeps their rows, shielded.

**The pass on launch.** When the indexer starts, each online root gets
a tree pass on the indexer's thread, one after another. The root the
open folder is under goes first, and only after the open folder's own
pass has been asked for.
- It is an mtime pass: a file whose size and mtime the index already
  holds is not read.
- It never holds up the first frame, since it runs on the indexer's
  thread.
- It runs behind everything the window asks. A save's `index_file`,
  or a folder the window opens, stops it at its next batch, as §168's
  folder pass stops for a save.

A tree pass is now taken in steps. A `TreeWalk` holds the folders
still to walk, the folders visited and the folders shielded, and
`walk_until` takes up where the last step stopped. It stops after a
batch and also between folders, since a tree of small folders never
fills a batch. The first cut restarted from the root at every stop,
re-walking the part it had done at a stat and a sidecar read per file.
While someone culled at two or three keys a second during a large
root's first walk, it could make no headway at all. A test stops the
walk before every folder and checks each file is looked at once.

A walk marks nothing missing until it has finished. A folder the walk
stopped inside counts its files once, as the first look found them,
not again as unchanged to the second.

A capture of an empty view ends, failed, with a status saying why:
every root offline, or nothing under the roots once the launch pass is
done. The second read found `--all-roots --snapshot` with its only
root offline, and `--roots` on an empty folder, waiting forever; both
now exit 1 at once. A batch run (an
export, a screenshot) starts neither the launch pass nor the watcher.
A capture of the all-roots view does start the launch pass, and waits
for it when the index had nothing under the roots yet. The first cut
failed that capture with "no frames pass the filter": it pointed a
frame index into the list being replaced.

While a pass runs, it says every two seconds that it has written rows
(`Told::BackgroundProgress`), and only when it has. The window then
reads the counts again and merges what is new into a view of the
roots, so a large root walked for the first time fills in, rather
than appearing all at once at the end.

A folder under a root that cannot be read (permission denied, a mount
gone bad) used to end the whole tree pass. `index_tree` returned the
error, and a root with one locked folder in it was never indexed past
that folder. Now the folder is an error in the report, its rows are
left as they were (shielded, as an empty mount point is), and the rest
of the tree is walked.

**The watcher.** `notify` 8.0.0, pinned exactly, CC0-1.0: inotify on
Linux, FSEvents on macOS, ReadDirectoryChangesW on Windows. 8.0.0
rather than 8.2 because it takes the `windows-sys` 0.59 the tree
already has; 8.2 adds a fifth `windows-sys` (0.60.2) and a
`windows-targets` 0.53 with its eight platform crates. What it brings
in:
- `notify-types` (MIT or Apache-2.0);
- `inotify` and `inotify-sys` (ISC, which is MIT's terms in fewer
  words);
- `mio`, `kqueue`, `kqueue-sys` and `fsevent-sys` (MIT);
- `walkdir`, already in the tree.

The watcher lives in `greycard-library` as `roots::Watcher`, with no
UI in it. It hands batches of `roots::Change` to a closure:
- `Folder(dir)` for a raw or a picture added, changed, renamed or
  removed there;
- `Tree(dir)` for a folder that appeared or went.

The editor's closure sends these to the indexer through an `Asker`, a
clone of its channel's sender.

What is not a change:
- anything under a hidden folder under the root, the sidecars' own
  `.greycard` among them (§153; the editor's saves index their frame
  themselves);
- a hidden file, such as a copier's temporary;
- any file the index does not hold that is still there: a `.gcd`
  beside its frame, an XMP, a `.part`;
- an event that is only an open or a read. notify's inotify backend
  watches `IN_OPEN`, and the index's own pass and the thumbnails open
  every file under a root, so taking opens for changes would have the
  watcher chase its own tail.

A path that is gone and is not a file the index holds is taken for a
folder. The first cut took a gone path with an "extension" for a file,
so a folder named `2026.09.24` renamed or deleted was not seen until
relaunch. A pass over a gone path the index has no rows under is
refused at once, so a gone sidecar or temporary costs nothing. A
folder gone is walked from the highest gone folder under the root,
since a tree pass over a folder whose parent is gone too is refused.
An overflowed queue (`need_rescan`) walks every root again.

Changes are gathered until the disk has been quiet for 400 ms, or 5 s
have passed since the first. They are then handed on with the repeats
out, and every folder folded into a tree over it in the same batch. A
copy of a shoot is a stream of events for seconds: the quiet lets it
land, and the cap keeps a slow copy from hiding its first files for as
long as it runs. Only a change counts towards the quiet. In the first
cut every event reset it, reads included, so a batch under a root
being read waited out the 5 s cap. On the indexer's thread, the
watcher's changes go ahead of the launch pass and behind the window.

A root is watched recursively when the platform will. When it will
not (a folder under the root that cannot be read, a drive's
`lost+found` at 0700 being the common case), the root is watched
folder by folder around it. Each folder that can be read gets a watch
of its own, and each folder that appears later is added as it
appears. The folders that could not be watched are logged by path.
The first cut took the whole root back and watched none of it. A test
watches a root with a locked `lost+found`, sees a raw written into a
sibling folder, and sees a file in a folder made afterwards.

A watcher that cannot start at all, or a root it cannot watch at all,
is logged with the reason and left. The launch pass is the fallback,
and the editor never fails to start over it. Running out of inotify
watches was not reproduced, since it wants a sysctl this user cannot
set; it comes back through the same path.

On the way out, the watcher is dropped before the indexer is stopped,
and the indexer is woken with `Ask::Leave`. Otherwise the watcher's
end of the channel would keep it waiting on the next ask.

**The all-roots view.** `Library::paths_under(roots)` lists every file
under the roots that the index last found where it is, by folder then
name.
- "Under" is asked as a range over the folder column's bytes: from the
  root and a separator, up to the same with its last byte one higher.
  The folder's index answers that without reading every row, since
  BLOBs compare as `memcmp` and a separator's last byte is never 0xFF.
- The first cut used `substr(folder, 1, n) = prefix`, a scan: 49.5 ms
  for a 20,000-file root in a 40,000-row library, against 14.0 ms for
  the range.
- A root that already ends in a separator (`/`, or a drive's own `D:\`
  on Windows) keeps the one it has. `under_prefix` doubled it before,
  which meant nothing was ever under a drive's root, and a tree pass
  over one marked nothing gone.

The filter bar and its facet chips work over the view exactly as over
a folder. The frames' row ids come from `ids_of` as before, the facets
are the same `GROUP BY`s over those ids, and `folder:` typed in the
text field narrows to one folder, as the index's term. Checked on the
samples and a second root: `folder:shoot001` showed that folder's 5 of
43, with its own cameras and lenses on the chips.

Opening a frame from the view is opening a frame. `open_loaded` is
`open_files` with its sidecars read elsewhere, so the sidecar, the
thumbnail and the develop take the same path.

What the view could not do is what `open_files` did: read every
file's sidecar on the UI thread before showing anything. A view of the
whole library is the whole library's sidecars. So:
- the view's sidecars are read on the rayon pool, on a thread of their
  own;
- the window goes on with the list it has meanwhile, and says "reading
  N frames...";
- the list is replaced when the sidecars are in;
- a view asked for again meanwhile drops the older read, by a
  generation count.

The frame on screen stays selected when it is in the new list, else
the last file open.

A list that arrives before the rendering setup has run opens its frame
from that setup, as the launch's own file does, so the first develop
runs on the GPU. The first cut's first develop ran on the CPU and took
4.9 s. The check is now a flag set when the setup runs, not whether
there is a renderer yet. A frame the setup was to open belongs to the
list that is being replaced, and goes with it.

`--all-roots` or `--roots` with no path given opens the view and
nothing else. The first cut opened the last file's folder and
developed its frame (1.2 to 1.5 s on the GPU) before the view's frame.

The view skips the window's folder pass over its folders: its rows
came from the index, and the launch pass and the watcher keep them. A
folder pass still running for the list before is dropped by the
generation, rather than report over the view.

**Keeping a list up with the disk.** When a pass in the background
changes something the browser shows (a folder view's folder, or
anything under the view's roots), the list is read again, from the
folder or from the index. It is merged into what the window holds
rather than opened again:
- Each frame keeps its sidecar as edited in memory, its picture, its
  row id and its place in the selection, all by its path.
- A frame new to the list has its sidecar read. Past 200 new, the
  sidecars are read on the pool first and merged when they are in, the
  selection and the frame on screen kept as in any merge. The first
  cut reopened the view for more than 500 new, which dropped the
  selection and developed the frame on screen again.
- Every frame still without a picture is asked for one again under its
  new number, in place of whatever was queued. The first cut asked
  only for the new files. The jobs already queued came back under
  numbers that now meant other files, were dropped on arrival, and the
  grid stayed blank for good: in the review, one thumbnail was
  delivered in 15 s after a raw was copied into the 20,000-frame view
  0.74 s after it came up, against 20,502 without the copy.
- A file the pass found changed (a copy that finished) has its picture
  asked for again, by the path the report now names (`changed_files`).
  The first cut asked again for every file under the pass's path, which
  for the launch pass is the root: one changed file on a 20,000-file
  root re-delivered 20,000 pictures, a 4 to 7 s stall. Now one file
  replaced under that root brings one delivery.
- The owed pictures go back to the pool in one push after it forgets
  the old queue, skipping `push`'s check for a duplicate (a scan of the
  whole queue per push, quadratic at 20,000), with the rows on screen
  first.
- The culling pictures and the camera picture standing in, which are
  keyed by number, are put down.

The frame on screen is followed, and never to a copy of itself. When
its file is not where the window has it, `Library::found_at` says
where the row the window knew is now:
- at its new path, if the index found it moved (§160's rule);
- for a row marked missing, by its hash, but in one case only: a
  folder renamed. A row whose folder is gone is never a move's other
  end (§160), so a renamed folder's files become new rows. The answer
  is a present row with the same hash and the same name, added after
  the missing one, and only when the missing row's own folder is gone
  from the disk.

The first cut answered a missing row with any present row that had its
hash. So a frame deleted after being backed up to `backup/` was
"followed" to the backup, the list held that path twice, and the
deleted frame's edit would have been saved over the backup's sidecar.
The window also never takes a path it already lists, and a merge keeps
a path only once. One case the rule still gets wrong: a drive
unplugged, its folder gone, and a copy of the frame under the same
name indexed later on the laptop. That copy passes every test the
rule has, and the frame on screen would be followed to it. Telling
the two apart needs the row to know when it was added, which the
index does not record yet.

The window takes the new path and keeps the edit in memory. In a
folder's view it keeps the frame in the list, where its name sorts,
though the folder no longer has it, since it is still what is being
edited. A frame on screen that is gone (deleted, or its only answer a
copy) hands on to the nearest row. Nothing is kept or saved for it,
so no `.gcd` is written beside a file that is not there. The window
checks this after every pass, whatever the pass reports. The last
file of a folder deleted leaves the folder empty, which §160 takes
for a drive not mounted, so the pass marks nothing and no count moves;
the second read found the frame left on screen that way, where a save
would have written a sidecar beside nothing. Checked live:
the frame on screen renamed into another folder under the root was
"moved to ...; followed" 403 to 405 ms after the rename.

The sidecar in the hidden folder does not follow a move (§153). It is
found by the frame's name in the frame's own folder, and a frame moved
elsewhere has none there. For the frame on screen, the next save
repairs this by writing the edit held in memory beside the frame's new
path; the old copy stays in the old folder's `.greycard`. A frame
moved while not open loses its hidden-folder sidecar until it is moved
back. A `.gcd` beside the frame moves when the person moves it with
the file, and is found. Not solved here: finding a sidecar by the
frame's hash is the index's job, and wants a column for it.

**What the merge found in the browser.** `rebuild_browser` made every
thumbnail on the list again from its pixels, turned, each time the
list was rebuilt: a filter change, a merge. Over 20,000 frames that is
20,000 pixel copies and 20,000 model rows set one by one, and the
first merge of one new file into the 20,000-frame view took 3.3 to 5.4
s.

The pictures already on the window's rows are now carried over by the
path each row was made for (`State::rows_shown`), to the file's new
row when its turn has not changed. The rows go into one new model
rather than being set one at a time. The first cut of this carried
them by the row's number, and a list renumbered under the rows handed
frames their neighbors' pictures: after Move rejects over eight raws
with two rejects, every frame after the first reject showed the next
one's picture. A test moves the rejects out of eight frames and checks
each row's pixel. The merge alone takes 75 to 118 ms over 20,000
frames. That excludes reading the rows and facets again, which the
merge does once (11 to 17 ms for the ids and 44 to 58 ms for six
facets, in the review's measure); the first cut did it a second time
after each merge. This helps a filter change over any large folder
too, not only the roots.

**The filter remembered.** The filter bar's state is saved in
settings.json as `filter::Saved`: the stars and their reading, the
flags, the labels, the text, and the facets' chips. The chips are kept
by the facet's name in the filter language (`camera`, `lens`, ...)
rather than by slot, so a facet added to the bar later does not shift
the ones kept.

When it is saved and put back:
- It is written on the way out, with the rest of the panel's choices.
  A snapshot, a screenshot or a batch export writes nothing, and
  leaves the user's settings alone.
- It is put back when a session opens a folder, or the last file's
  folder, unless `--filter` names one.
- It is not put back for a batch run, nor over a file named on the
  command line (a double-click from the desktop). The first cut put
  it back for both. A kept "picks only" made `greycard-ui X.CR3
  --export out.jpg` say "no frames pass the filter" and exit 1 with no
  file, and hid a file double-clicked from the desktop.
- A file the desktop hands over mid-session clears a filter left on,
  for the same reason.
- Clear empties it, and an empty filter is what is kept then.

A kept facet whose value no frame in the folder holds shows as its
chip at zero, on, as §168 does for any chip on, so it can be turned
off. Checked by writing a settings file with `iso>=400` and the R6
Mark II chip, and opening a snapshot of the all-roots view: the text
in the field, the chip on, 2 of 43.

**Move rejects over the view.** `cull::move_rejects` put every reject
in the rejects folder of the first rejected frame's folder. That was
right when the browser only ever held one folder. Now:
- Each frame goes to its own folder's rejects folder. When there is
  more than one, the sheet says "a rejects folder in each of the N
  folders they are in".
- A shoot whose rejects folder cannot be made (a folder that is not
  the user's to write) keeps its rejects where they are. They are said
  in the report, and the other shoots' rejects go on. The first cut
  moved some, stopped at the error, and left the window holding old
  paths.
- A frame already in a rejects folder is not moved a folder deeper.
  Moving the rejects twice over the view used to nest them
  (`shoot/rejects/rejects/`).

The count on the sheet and in the status is of the frames still to go
out: a frame already in a rejects folder is not counted, and the
folders named are those the moved frames went to.

Rejects folders stay in the index and the all-roots view. The flag
travels with the sidecar, so "No rejects" hides them, and a folder
named `rejects` that is something else must not vanish from the
library.

**`--snapshot`.** `--roots DIR,DIR` gives the library these roots for
the run, leaving `roots.json` alone, and opens on the all-roots view.
`--all-roots` opens on the view with the roots kept beside the
library, so `--library PATH --all-roots` shows a scratch library's
view. A capture waits for the view as it waits for `--filter`'s chips,
and for the launch pass when there was nothing indexed yet. A fresh
library's `--roots tree2k --snapshot` now exits 0: the view comes up
empty, the pass indexes 2,000 files in 1.38 s, and they are merged in
on the pool.

**Measured.** Release build, on the 33 sample raws (1.2 GB) and on
synthetic trees of hard links to them. Other worktrees were building
beside it (load average 9 to 26 across the runs), so the cold numbers
are high. Where the reviewer measured the same thing on the same
machine, both are given.

- The launch pass over the samples as a root, cold (a fresh library,
  the files evicted from the page cache): 0.28 s here, 0.317 s in the
  review with fincore at 0 bytes resident; 33 added. Warm, the next
  launch on the same library: 0.000 to 0.001 s, 33 unchanged (below
  the log's millisecond).
- A tree of 40 folders of 500 links, 20,000 files: the warm launch
  pass takes 0.082 to 0.108 s. The same tree with an 8.8 KB sidecar
  beside every link takes 0.321 to 0.663 s, which is the sidecars read
  and hashed.
- The all-roots view of that tree at launch, the index warm:
  - listed in 4.7 to 14.0 ms here (the library holding 20,000 to
    40,000 rows), 8 to 10 ms in the review;
  - 20,000 sidecars read on the pool in 0.01 to 0.02 s with none on
    disk, or 0.10 s with every link carrying one;
  - the list put in the browser on the UI thread in 58 to 85 ms (the
    row ids 12 to 34 ms of it);
  - the list in the browser 300 to 423 ms after it was asked for, and
    the first frame showing it 562 to 747 ms after (536 to 560 ms in
    the review). Both count from before the window has come up.
  - The view's first frame waits on the thumbnail pool's deliveries
    (master since fc403f5). The window does not render until the
    20,000 cache hits are delivered: in the second read, the window
    was up at 0.25 s and the view's first frame came at 3.86 s, the
    same wait a flat 20,000-file folder has on master (5.7 to 6.3 s).
    That is §172's item, not this one's.
- The same 20,000 sidecars read one after another, as a folder's open
  reads them on the UI thread: 0.68 to 1.54 s with a sidecar each, and
  0.045 to 0.068 s with none. On the pool: 0.041 to 0.044 s and 0.003
  to 0.004 s (`sidecars_for_the_numbers`, an ignored test).
- The watcher, from a copy into a root to the grid showing the file:
  - on a root of a few files: 415 to 440 ms to the merge and 423 to
    452 ms to the frame, three rounds, most of it the 400 ms quiet
    (the review: 416 to 426 ms and 426 to 454 ms);
  - on the 20,000-file root: 513 to 963 ms to the merge (the merge
    itself 75 to 172 ms) and 761 to 1,527 ms to the frame, the rest
    being Slint building the list's cells again. Before the
    `rebuild_browser` change, the merge alone was 3.3 to 5.4 s.
- A raw copied into the 20,000-frame view 0.3 s after the list came up,
  while the thumbnails were still coming: 18,704 to 19,982 thumbnails
  delivered in the 15 s after the copy, against 1 before the fix.
- The frame on screen moved to another folder under its root: followed
  403 to 405 ms after the rename (402 ms in the review).

A cold index of synthetic links is not a number to quote for a
library. 20,000 links to 33 files is 600 rows sharing each hash, and
§160's move lookup asks the disk about every one of them for every
file added. The CLI's `library index --tree` over the tree with
sidecars, on a library already holding the tree without them, took
85.7 s for 20,000 added (4.3 ms a file). The review's cold CLI index
of the 20,000 links took 31.75 s. A real library has one row per hash.

**The review.** Read and run with the editor, with every finding
reproduced before it was fixed; the fixes are written into the
sections above. Four were blocking:
- after Move rejects, frames showed their neighbors' pictures;
- a merge while thumbnails were loading blanked the grid for good;
- a deleted frame was "followed" to its backup copy;
- the remembered filter hid the file a batch export or a double-click
  named.

Each has a test built from the reviewer's repro. The twelve others:
- a gone folder with a dot in its name went unseen;
- a save restarted the launch pass from the root;
- `loading` could stay set for the session and stop every refresh;
- one unwritable shoot half-moved the rejects;
- moving the rejects twice nested them;
- an unplugged root had its rows marked missing;
- one unreadable subfolder turned off watching for the whole root;
- the roots row could not grow past its width;
- `--all-roots` developed the last file first;
- an empty view failed a capture;
- two editors lost each other's roots;
- more than 500 new files dropped the selection.

The nits fixed: a folder pass left behind reported over the view; the
rows and facets were read twice after a merge, and every two seconds
while a pass found nothing new; reads under a root held a batch to the
5 s cap; a changed file's picture was not asked for again; a batch
export started the launch pass and the watcher; the deferred open was
keyed on the renderer; `roots.json`'s version was ignored; and a fifth
`windows-sys` was added.

**Left, and questions.**

- The grid and the strip build a cell for every row of the model, and
  a new list is a new model: 300 to 450 ms of the frame after a merge
  or a filter change over 20,000 frames. Every thumbnail of the view
  is kept in memory once made, about 60 KB a frame at 176 px (1.2 GB
  for 20,000). The view needs two things at that size, neither
  started: a grid that builds only the rows on screen, and a thumbnail
  memory that forgets what is off it.
- Which view the editor was in is not remembered. A launch opens the
  last file's folder, and the all-roots view is a click, or
  `--all-roots`.
- A tree pass reports a move across folders as one missing and one
  moved when the old folder is walked first. The row is right; the
  report double counts.
- `--roots` runs do not save a root added or removed in them.
- The filter is written on the way out, not when it changes. A crash
  loses it, as it loses the other panel choices.
- The watcher ran here on Linux only; its test runs on all three
  platforms in CI. FSEvents and ReadDirectoryChangesW may report the
  folder a write touched as well as the file. That becomes a tree pass
  over the folder, and costs a walk of it.
- §168's "no lens" chip question comes back with the roots, where a
  library of phone JPEGs has no lens. Still left.
