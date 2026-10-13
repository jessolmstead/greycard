# §310. Forget missing, built (2026-10-12)

§304 left a "Forget missing" open: a reference whose
picture the index has nowhere stays in its collection for good, untiled,
and the status line counts it. This is the way to let it go.

## What it does

The open collection's row in the COLLECTIONS section gains a menu item,
"Forget 1 missing frame" / "Forget N missing frames", between Capture
order and Delete. It is offered only on a plain collection that is open,
while no index pass is under way or owed, when its last read found at
least one reference it may forget; a smart collection has no references
and never shows it. The number is asked of the window as the menu opens
(a pure callback), not carried on the row, so a pass that starts or ends
between two opens of the menu is seen at the next one. It asks nothing
first: what it drops is already not shown, and the frames themselves are
not touched.

Pressed, it reads the collection afresh, drops every reference that read
finds gone, writes the file through the usual save (so a file changed on
disk by another editor is joined first, and our removal stands), asks
the marks again, and says the status line again from that read's own
counts: "Kept: 1 frame shown, 1 missing (moved or deleted, and kept
until found); forgot 1 missing", or "Kept: 3 frames; forgot 2 missing"
when none is left. When it finds nothing to forget it says why only when
it knows: "...; nothing forgotten: 1 frame on a drive that is offline or not
answering",
"...; nothing forgotten: the index has yet to read some of them", or
plainly "...; nothing to forget".

## The test is the status line's, and then some

Missing is decided by `collections::resolve`, the same read that counts
the missing for the status line: no row of the hash that the index
holds as present, and no file at the hint whose hash the collection does
not already hold. `resolve` asks the index alone and never the disk, at
the open as before. Forget does not trust the count from the last open;
it runs `resolve` again on the window's index reader at the press, and
of what that finds missing it keeps, still counted in the status line:

- **Hinted under an offline root.** A frame under an unplugged root is
  not missing in the first place (no pass, its rows stay present, shown
  dimmed); the one that can count missing is a file the index saw
  deleted before the drive went away. We cannot tell that from a file
  copied back meanwhile, so it stays until the root is back.
- **Hinted under a network mount** (`library.remote`: a root on one, or
  a mount below a local root, left out of the root's walk). Its folder
  is never looked at: a hard-mounted share gone away would hold the look
  for good. It is not offered either.
- **In a folder that does not answer.** The rest of the hints are looked
  at through the editor's own bounded look (`roots::answers_of`, the
  roots' 3 s wait): by their root when they are under one, so a press
  makes a handful of looks, else by their folder. The looks go at most
  16 at a time, and once one batch has a path that did not answer, the
  rest are taken for no answer without a look, so the press waits one
  3 s at most and never asks for more than 16 threads. A share gone
  quiet that is not yet known offline, or a hint under no root on a
  dead server, is kept. A root that answers no is offline by the roots'
  own test, even when the window's offline set (from the last look) does
  not say so yet, as with a drive pulled just before the press: its
  hints are kept too.
- **A file the disk does not say is gone.** Only once its folder (or
  root) has answered is the hint itself asked, with `symlink_metadata`:
  there, it is a file a pass has yet to read and is kept; not found, it
  is gone; any other error (a folder that cannot be read, a FUSE or
  sshfs mount that died and answers at once, a stale NFS handle, an I/O
  error) keeps it. A folder that answered no is taken for gone only when
  the folder itself is not found. This matters most after a rebuilt
  index: a pass leaves a folder it cannot read with no rows, the indexer
  goes idle, and without this every frame in it would be offered and
  forgotten though the files are there.

## Never while the index can still change its mind

The index alone cannot tell "not on disk" from "not read yet". Two real
windows: a pass marks a folder's absent rows missing as soon as it
finishes that folder, and sees a move only when it walks the folder the
file went to, so a shoot reorganized while the editor was closed is
missing, with hints that are no files, until the pass gets there; and a
rebuilt index (a schema change drops the files table) has every frame
missing until its passes are done, which on a large library is hours.
Forget pressed in either would drop references for good.

So Forget is neither offered nor done while the index is busy: the
launch's look at the roots out (`starting`), a launch pass left
(`launch_left`), or anything asked of the indexer's thread not yet done.
The last is new on the indexer, `Indexer::busy` (`library::Busy`): a
count of asks owed, up before each send and down as the thread takes
each, so an ask in the channel is always counted; a flag for what the
thread has in hand, true from its start (the open, and any rebuild, come
before the first ask) and false only while it waits; and the roots of
the passes set aside because their root stopped answering. A pass set
aside counts as busy unless its root is in the window's offline set or
under a network mount in `library.remote` (a hung mount below a live
local root is never offline): those frames are kept by the offline and network rules anyway, and
without the exemption a dead root left in the library would keep Forget away for the
whole session. A root that stopped answering in the middle of a pass but
is not yet found offline does keep it away until the next look finds it
so. A press while busy says "the library is being indexed; nothing forgotten from
Kept until it is done".

The count the menu shows is kept from the last read of the view the
window holds now: a read of an older view (its generation passed) does
not set it, so a read landing after a forget does not bring the old
count back.

## What we measured

Nothing to measure in time: `resolve` asks the index one or two small
queries a reference, and at the press the missing hints' folders get one
bounded look together, plus an `is_file` a hint whose folder answered.
That runs on the window's thread; a folder that does not answer holds
the press for up to the roots' 3 s wait, once for all of them. A
collection of thousands of references would make the press take a
visible moment; none of ours are near that.

## Tests

- `collections::tests::forget_missing_drops_the_gone_and_keeps_the_offline`:
  a frame deleted on an online root and a hash never seen are forgotten;
  a frame on an offline root, both one whose row is present and one the
  index last saw missing, is kept; with no index nothing changes; the
  file on disk is rewritten; the counts come back from the fresh read
  and the status line's count drops from 3 to 1; with the drive back,
  its missing one goes too.
- `collections::tests::forget_missing_keeps_a_file_on_disk_and_never_looks_at_a_far_one`:
  an index that has read nothing (as after a rebuild); a hint that is a
  real file is kept, one under a network mount is kept and its folder
  never looked at, one in a folder that does not answer is kept, and the
  one whose folder answered with no file is forgotten.
- `library::tests::the_indexer_is_busy_from_an_ask_until_its_pass_is_done`:
  busy at start, idle after, and idle again once a launch pass asked of
  it is told done.
- `library::busy_tests::busy_counts_an_ask_owed_and_a_pass_aside_on_a_root_not_offline`:
  an ask in the channel is busy before it is taken; a pass set aside is
  busy until its root is offline, and not at all under a network mount.
- `collections::tests::forget_missing_keeps_the_frames_of_a_folder_that_cannot_be_read`
  (Unix): a folder of mode 000 answers no and its file's stat fails
  with a permission error: kept; a hint in a folder not found: forgotten.
  Skipped in effect when run as root, where the mode stops nothing. The
  mode is put back by a guard however the test ends.
- The panel test also pulls the drive behind the window's back (its
  offline set cleared): the root answers no and the reference is kept.
- `panel::collections::tests::forget_missing_drops_the_gone_and_keeps_the_offline`:
  headless window, a collection with one frame, one reference found
  nowhere and one hinted under an unplugged root of the library; 1
  offered only while open; with a launch pass left nothing is offered
  and a press is refused; the press rewrites the file and the row's
  count; pressed again it says the drive is the reason; opening again
  counts 1 missing.

## Left open

- No undo: the history is the edit's, not the browser's. A reference
  forgotten by mistake comes back only by adding the frame again, which
  puts it at the end of the collection's order.
- The watcher's own wait: a move made while the editor runs is a
  missing file with no file at its hint until the watcher hands the
  change on, and the indexer is not busy in that moment. A press inside
  it would forget the moved frame.
- The press can hold the window for up to 3 s when a hint's root or
  folder does not answer, and the status line says nothing meanwhile
  (a word set before the wait would not be drawn until the window's
  thread is free again); moving the press's read off the window's
  thread would remove both.
- The editor's looks (`CHECKS`) keep an entry a path ever looked at, so
  each folder a press looks at (a hint under no root) stays in that map
  for the session.
- A missing frame under a network mount is never forgotten, even when
  the share is up and the file truly gone; it would need a bounded look
  that we trust on a hard mount.
- A merge read asked before a forget and landing after it, of the same
  view, brings the old forgettable count back to the menu; the press
  reads afresh, forgets nothing and says so.
- The offer is on the open collection only; a forget across every
  collection (or from a closed one's row) would need a read of each.
- Frames in a folder that cannot be read are still neither shown nor
  counted (as before), so Forget does not reach them either.
