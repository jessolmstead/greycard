# 187. Keeping a list up with the disk, off the window's thread (2026-09-27)

Opening a library root on a NAS made the editor stop answering for
seconds at a time, again and again, until the desktop offered to kill
it. The log had 35 merges on the window's thread taking 79 s between
them, one merge of 10 to 48 new files taking 0.4 to 15.6 s.

**What it was.** §174's merge kept the list up with the disk by reading
it again at every pass the indexer reported, and each step of that
read was on the window's thread:
- `view_files` opened every folder in the list (`read_dir`) to leave
  out the ones that could not be read: one round trip a folder, every
  report, before the merge's timer started;
- `roots_of` opened every root to leave out the offline ones, and the
  roots' row did it again for each chip;
- a merge of up to 200 new files read their sidecars there too, a stat
  and a read each. A walk's batches are 10 to 50, so every one was
  under 200 and none went to the pool;
- the frame on screen was looked for with `exists` and `is_file` in
  `background_done`, in `follow_current` and twice in `merge`;
- the index's own reads asked the disk as well. `ids_of` made each
  folder of the list canonical with a `realpath`, at every merge and at
  every report that did not merge. `count_under` and `paths_under` made
  each root canonical at every report. The roots' row made the open
  folder canonical for "Add this folder", again at every report.

On a local disk each is a few microseconds, and nothing showed. Over a
network each is a round trip.

**Counted.** The tree: 300 folders of 10 hard links each (3,000 raws
from the samples), a fresh library, then 20 files linked into 20
folders every 3 s, ten times, which is 200 watcher passes of one
folder each. Debug build, local disk.
- Before, counted by a log line at each call in `roots.rs`: 307 calls
  on the window's thread for each report (300 folders, 2 roots, 1 to 4
  for the frame on screen, and the new files' sidecars), 61,708 in all
  for the 201 reports. That count missed the index's `realpath`s. The
  review counted every call with an `LD_PRELOAD` shim over libc, and
  found about 300 more for each merge on the first cut of this change.
- After, by the same shim: no call on the window's thread in the 45 s
  of rounds after launch in the last run, 1 in the run before it. That
  one is `remember_last_file`'s `canonicalize` of the frame on screen
  when a develop lands, once a develop, not in this path. The launch
  makes 4 by the review's count (5 in my run: the frame, the `--roots`
  root made canonical and looked at, and `start` and `watch` opening
  it). The 200 reports became 22 reads and 12 merges, with about 55,300
  calls off the window's thread, most of them the thumbnails. The
  second review had 21 reads and 12 merges, the same at any rate of
  reports.

**Now.** A read of the list is a `Look`. Everything it needs is taken
from the window's state when it is asked for; it runs on a thread of
its own, and lands on the window's thread when done. What it does off
the window's thread:
- in a view of the roots, each root looked at on a thread of its own
  (`Checks`), and one that has not answered in 3 s taken for offline.
  A hard-mounted share gone away does not answer at all, and a thread
  asking it is stuck for as long as it is gone. So a root keeps one
  look out at most: while it has not answered, every read takes the
  root for offline at once, with no new thread and no wait, and when
  it answers the next read looks afresh. The first cut started a look
  for every root at every read. With one root hung that was a stuck
  thread a read (38 over 40 reports, about 1,200 an hour) and 3 s more
  on every read. The chips' "offline" comes from the same look
  (`Library::offline`), kept rather than asked at every `show`;
- in a folder's view, no root looked at, since a root that does not
  answer is none of its business; the folder listed, and made
  canonical;
- for the roots view, each folder in the list the window does not know
  can be read looked at once;
- the list's rows asked of the index on a connection of the read's
  own, with each folder's canonical form in hand: the index's paths
  are canonical already, and a folder's view has its one folder made
  canonical by the read. `Library::ids_of_with` takes that form rather
  than asking the disk;
- the new files' sidecars read on the pool, whatever their number
  (`MERGE_AT_MOST` is gone);
- the frame on screen looked for, one stat, and followed through the
  index only when it is gone. Not when it is under a root this read
  found offline: that stat would not come back either. It is taken for
  there, and stays on screen and in the list while the rest of its
  root's frames leave. The first cut looked, and with the frame on
  screen under the hung root, no read came back: 35 reports and no
  merge in 105 s of hang, the chip never saying offline (it lands with
  the read), and the read sent after the give-up hanging the same way.
  With the fix, the same run merged 36 times, a merge 0.1 to 0.3 s
  after each report, the first read after the hang 3 s and every one
  after it 1 to 2 ms, one "greycard root check" thread in all.

The roots are canonical in `roots.json` (§174), so the counts and the
list ask the index by them as they are (`count_under_canonical`,
`paths_under_canonical`). The window keeps each folder's canonical
form (`Library::canonical`) as the reads hand it over, so `refresh_ids`
after a report that does not merge, `changed_rows`, and "Add this
folder" ask the disk only for a folder no read has seen. A folder's
view has its folder made canonical by every read, off the window's
thread, one `realpath`; and a folder opened by hand is made canonical
again as it opens. So a folder reached through a link that is pointed
elsewhere during the session is followed at the next read. The first
cut kept the first answer for the session, and a retargeted link kept
the old folder's rows: a frame could take another file's row by its
name, and "Add this folder" added the old target.

The merge stays on the window's thread, since it swaps the window's
lists, and asks the disk nothing: it takes the sidecars, the rows and
the frame on screen as the read found them (`Brought`). A test merges
a list whose folders do not exist, and lands a read after its folder
was deleted.

One read is out at a time. A report while one is out sets `stale` and
returns; the read that lands merges, then one more read goes out for
everything reported meanwhile. A test says six passes over six folders
are two reads and two merges, the second covering five of them. A
read that lands after a file became new to the list, with no sidecar
for it, is not merged with a blank one (which a save would write over
the file's own); it is read again.

A read that does not come back in 60 s (a share that stopped answering
in the middle of it, past the roots' look) no longer holds the list
for the session. The next refresh gives up on it and sends another,
and the one given up on is dropped whole if it ever lands. A view's
read, opened by hand, is given up on the same way: the status line
says the frames did not come in, rather than "reading N frames..." for
good, and a capture waiting on the view lets go. What a read saw (the
roots offline, the folders, their canonical forms) is kept only from a
read of the list the window holds now. The first cut kept it from any
read that landed, so a view's read that landed after 60 s put its old
offline set over a newer one. It also left `merging` set for good, and
every refresh after it only set `stale`.

**Which folders can be read.** The window keeps the folders a read
found it could read. A folder that could not be read is not kept
either way, and every read looks at it again: on a network share, a
failure can be a moment's (EIO, a timeout). The first cut kept it as
unreadable, which hid the folder's frames for the session unless a
pass went over it or the view was opened again, where §174 had them
back at the next merge. The cost is a look for each unreadable folder
at each read, off the window's thread; a `lost+found` at 0700 under a
drive's root is one.

A folder is looked at again when:
- it is new to the list, or could not be read last time;
- a pass finishes over it or over a folder above it
  (`folders_passed`, from `Told::Background`, not from the
  every-two-seconds progress word);
- the view is opened by hand, which looks at every folder again.

A pass that finishes while a read is out keeps the read from putting
back what it saw of the pass's folders, whether or not the pass
changed anything. Each finished pass is kept with a number, and a read
keeps a folder only when no pass newer than the read covers it. The
first cut moved the number only when the pass changed something, so a
read that saw a folder mid-blip could land after a clean pass over it
and keep the blip. Such a pass also marks the read stale, so one more
read follows it.

Which passes have the list read again, for the folders:
- a tree pass that could not read a folder under it says so as a folder
  (`Report::unreadable`). `Report::errors` also holds the files that
  could not be hashed or probed, and the first cut read the list again
  for those at every pass;
- a pass that failed outright. A folder locked under a watched root is
  an inotify event on it (locally, `chmod 000` produced `Tree(rootA/f1)`),
  and the pass over that folder fails at the folder itself: it comes
  back as an error with an empty report, not as `unreadable`;
- a pass over a folder the last read could not read
  (`Library::unreadable`), even one that found nothing changed: the
  folder unlocked, its files as they were.

The first cut had only the first. A folder locked and unlocked needed
an unrelated report to show either way: in the review's run its 15
frames stayed in the list until a file was added elsewhere. Now each
shows by its own pass: the frames left 0.16 s after the `chmod 000`
event and came back 0.14 s after the `chmod 755` one. A test does the
round trip. On a share mounted over NFS or SMB, inotify sees only what
this machine does: a folder locked on the server waits for the next
read, which any report brings.

**The frame on screen.** §174's check that the frame on screen is
still there after every pass, whatever the pass said, stays, off the
window's thread: a pass whose path is over the frame on screen has the
list read again even when nothing changed. So does a pass that could
not read a folder. In a folder's view, any pass over the folder does.

**What is still in the way.** The indexer's own thread still asks a
hung root and waits with it: a tree pass or a watcher's pass over a
share that does not answer blocks that thread, and while it is blocked
no report comes, for any root. The list keeps what it has, the window
answers, and the reads above do not hang, but nothing new is indexed
until the share answers. That was so before this change.

**What is still on the window's thread.**
- The index's reads over SQLite, on the local disk: the list
  (`paths_under_canonical`), each root's count, the facets and the
  filter's rows (`ids_passing`), and `refresh_ids` after a report that
  does not merge. A few ms, up to 60 ms over 20,000 frames (§174).
- At launch, and when a root is added or taken out: `start` and `watch`
  look at each root with a `read_dir`. A root that does not answer
  would still hold the window there.
- `remember_last_file` makes the frame on screen canonical after each
  develop.
- A folder's own open (Open folder, a double-click) still reads its
  sidecars on the window's thread, as §174 left it ("a folder is a few
  hundred at most"). On a NAS that is not true either, and is the next
  item.

**The merge's own time**, the "list merged" log line, same tree and
rounds, thumbnails still being made:
- before: 20 new in 47 to 226 ms (median 80), the 3,000 at once 41 to
  68 ms;
- after: 20 new in 31 to 41 ms, the 3,000 at once 29 ms. The rows no
  longer make each folder canonical, and that was most of the rest,
  even locally.

A read takes 21 to 30 ms off the window's thread for 3,000 files, and
52 ms for the first 3,000 with their sidecars and every folder looked
at. What it means on the NAS was not measured, there being none to
measure on. The count says what went: about 600 round trips on the
window's thread for every report (the folders looked at and made
canonical, the roots, the frame on screen), plus the sidecars of up to
200 new files, are now none, and 200 reports are 22 reads. The 600 is
an estimate from the two counts (307 counted a report on master, and
the review's about 300 `realpath`s a merge), not one measurement. At
1 ms a round trip that would be about 0.6 s of a frozen window for
each report, with a walk reporting every two seconds.

**Not done.**
- The indexer's thread hangs with a hung root, and reports stop for
  every root until it answers (pre-existing). For the roadmap: give
  the indexer's passes the roots' 3 s look, and skip a root that has
  not answered.
- `start` and `watch` still look at each root on the window's thread at
  launch and when a root is added or taken out.
- A folder's own open still reads its sidecars on the window's thread.
