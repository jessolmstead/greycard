# 172. Thumbnails made in parallel, and a cell that has none (2026-09-25)

Roadmap line: thumbnails made in parallel on a cold cache, and beside
it the bug that a `--grid --snapshot` over a folder where one
thumbnail fails waits until it is killed. Both from §163. Wave C, an
opus author and an opus reviewer.

**What was there.** A folder open sent one `Job::Thumbnail` a file to
the worker, whose `run` loop took them last, after a develop, a mask
and an export: one at a time, on the thread that develops, behind the
first frame's develop. On the 35 sample files that was 2 s of decoding
(12 to 253 ms a preview) that a warm cache (§163) answers in a
hundredth.

**The pool.** `crates/greycard-ui/src/thumbpool.rs`: a fixed set of
plain threads of its own, not rayon's global pool. The develop runs
rayon over every core, and a thumbnail job put on that pool would
queue behind the develop's own splits, or split the develop's; plain
threads can be held and let go without touching rayon. Its size is
half the cores, at least two and at most eight (`threads_for`). The
ceiling is memory and the disk as much as the CPU: each thread holds
a full-frame preview while it downscales it, 134 MB for an R5 II's
8192 by 5464, and sixteen threads made a folder of 300 no faster than
eight (4.40 s against 4.31 s in the median) while slowing the
window's start-up enough to cost the first paint a second (3.58 s
against 2.61). The threads start with the first thumbnail asked for,
so a window with nothing open, and every test that builds a `Worker`,
never starts them.

The worker keeps its API: `send(Job::Thumbnail)`, `want_thumbnails`
and `set_thumb_size` go to the pool, and `forget_thumbnails`,
`hold_thumbnails` and `thumb_threads` are new. What the pool keeps is
what the worker's queue kept for thumbnails: the waiting jobs in the
order wanted, the range they are ordered for and the size to make
them at, read as each is begun, so the grid's step-up to a larger
cell still takes effect for anything not yet begun.
`order_thumbnails` is the worker's function, unchanged; a re-order
sorts what is waiting, and what is in hand is finished.

Three rules make parallel safe:

- **One file, one thread.** A thread takes the first waiting job whose
  file no other thread has in hand, so a second ask for a file being
  made (the grid's larger picture) waits for the first to finish
  rather than decoding the same preview twice at once; and an ask for
  a file already waiting with the same index is dropped, since the
  waiting one will be made at whatever size is wanted when it begins.
  The cache's entries were already written to a temporary name of
  this process's and this moment's and renamed into place (§163).
  The cache's mutex covers every use of it: a lookup (`Thumbs::get`,
  read, check and JPEG decode), a write, the first write's `usage()`
  walk when no count is known yet, and an eviction's walk. So two
  copies of one frame with one content key (a `cp -p`) made on two
  threads write one after the other and the later rename wins with the
  same picture; and while a walk runs, the other threads' lookups
  wait. The startup count (`count_thumb_cache`, §163) seeds the count
  from a thread of its own without the lock, so the first write's walk
  under the lock happens only when a write beats that count. Moving
  it off the lock would mean a write that does not know the count
  yet skips its eviction check, which is a change to the cache's
  contract and not this item's; the comment in `thumbs.rs` that said
  the walk ran on the worker's thread now says what it does.
- **A folder change drops what is pending.** `open_files` calls
  `forget_thumbnails`, which empties the waiting list and raises an
  epoch; a picture in hand when the folder changed is not delivered.
  The UI's own check stays behind it: `Outcome::Thumbnail` carries the
  path, and one lands only on the slot whose file is that path, so a
  delivery that raced the change can land only where the new list has
  the same file at the same index, which is the right picture. The
  cull's renumbering after rejects are moved away (`cull.rs`) is not a
  folder change and is left as it was: it asks again for what has no
  picture, and a stale job's path no longer matches its index.
- **A panic costs one cell.** Each job is made under its own
  `catch_unwind` on its thread. On the worker a panic in a thumbnail
  was caught but blamed on nobody and delivered nothing, so the cell
  never filled; now it is `Outcome::NoThumbnail`, logged at warn with
  the panic's text, and the thread goes on serving.

The folder's log line now reads "N s of decoding across T threads":
the seconds are summed over the threads and are more than the wall
time.

**The develop first.** Two holds, both measured.

The worker holds the pool while any develop runs: it sets the pool's
limit to nothing when it takes an `Open` or a `Develop` and back to
the pool's size when it is done (panicked included). Pictures in hand
finish; nothing new is begun. On a folder of 300, eight threads left
running took the first develop from 1.28 s to 1.61 s in the median
and four threads from 1.28 to 1.49; held, it was 1.21. An export, a
mask or a set's frames do not hold it: the item was that thumbnails
never queue behind those.

That hold starts only when the worker begins the open, and on a cold
folder the pool has had most of a second of the window's start-up by
then. The review measured the first paint on the 300 at 2.64 s on
master against 2.79 on the branch, five runs of six above master's
median: the pool's eight decodes were taking the CPU from the window's
own start-up and the GPU context's. So a folder opened in the loupe
now holds its thumbnails from the moment it is listed until the first
develop is delivered (`hold_thumbnails_for_develop` in `browser.rs`,
from the launch and from `open_files`; let go by the `Developed` or
`Failed` delivery, by the grid reporting its range, or after 10 s,
`HOLD_AT_MOST`, for a develop that never comes). On the grid nothing
is held: the thumbnails are what is on screen.

Held, the threads still look pictures up in the cache and deliver the
hits; only the making waits, and a miss is looked up once and left in
its place in the queue. The first cut of this hold held the lookups
as well, and a warm folder's strip, which the cache fills in a
hundredth of a second, came up at 1.58 s, when the develop landed. A
hit is a tenth of a millisecond and a 64 KB read; it takes nothing the
develop wants.

**The bug.** A file no picture could be made of now marks its cell:
the grid and the strip draw the same quiet bordered box saying
"Unreadable" where the picture would be. `Thumb` has a `failed`
field, `State::thumb_failed` keeps it per file across a filter's
re-made rows and the cull's renumbering, a picture that arrives after
all clears it, and a larger picture that fails after a smaller one
arrived keeps the smaller one unmarked. `grid_filled_rows` counts a
failed file as filled, so a `--grid --snapshot` over such a folder is
taken as soon as the rest are in. And the snapshot's wait for the grid
has a limit: `GRID_WAIT`, 30 s from launch, after which
`give_up_on_grid` logs at warn which cells are still waiting for a
picture and which for a larger one, named apart, and the grid is taken
as it stands. That covers what the mark cannot, a thumbnail that never
comes back at all.

Reproduced first on master: a folder of a whole CR3, a whole NEF and a
CR3 cut to its first 1 KB (in the work dir, not committed);
`--grid --snapshot` was killed by `timeout 60` with no picture written
(exit 124), the log saying `3 files ... 2 made, 1 failed`. On the
branch the same run writes the snapshot at 1.8 s, exit 0, the third
cell marked; in the loupe the strip's cell is marked the same way.
The limit was checked with a named pipe named as a raw in the folder,
whose hash never returns: the log at 30.17 s said `the grid still
waits for 1 picture after 30 s (HANG.CR3)` and the snapshot was
written at 30.5 s. The library's indexer blocked on the pipe as well,
and quitting waited 5 s on it.

That pipe also showed that the folder listing took anything with a
raw's name. `files::list_files` now lists only regular files (or
links to them), and so does the index's `list_folder`, which had
excluded folders only; a folder named `x.CR3` is not listed either.
Over the same folder the pipe is now not listed, the snapshot is
written at 2.2 s and quitting does not wait on the indexer. The give-up
is therefore checked by the unit test and by the run above, made
before the listing changed; nothing in the folder listing can make a
thumbnail hang now that is known. Truncations of a CR3 to 192 KB and
2 MB fail cleanly in rawler rather than panic, so the real "capacity
overflow" panic of §163 was not reproduced here; the pool's panic
path is tested with a maker that panics.

**The numbers.** Release builds of master (fc403f5, "before") and the
branch rebased on it, the editor on a headless mutter's Xwayland at
1500 by 950, every run with its own empty `XDG_*` folders; each run
opened the folder and was ended once the log had the folder's
thumbnails line and the first develop. "Last thumbnail" is that line's
time from the folder's open; "paint" is the process's uptime at the
first `developed` line, and "develop" that develop's own total. The
35 are the sample files hard-linked into the work dir (33 raws, 2
JPEGs); the 300 are hard links to them under 300 names, run with the
cache off (`thumb_cache_mb` 0), since 300 links to 35 contents would
otherwise mostly hit the cache. Raws in the page cache throughout.
Two blocks of eight interleaved runs each, load average 3 to 14;
medians over the sixteen, the last thumbnail's from the final block.

| | last thumbnail, before | after | paint, before | after |
|---|---|---|---|---|
| loupe, 35, cold cache | 3.49 s | 2.35 s | 2.30 s | 2.09 s |
| loupe, 300, cache off | 17.00 s | 5.12 s | 2.31 s | 2.18 s |
| loupe, 35, warm cache | 0.01 s | 0.01 s | 1.68 s | 1.65 s |
| grid, 35, cold cache | 3.60 s | 0.63 s | 2.35 s | 2.25 s |
| grid, 300, cache off | 16.94 s | 4.11 s | 2.31 s | 2.49 s |

The develop's own total was 1.17 to 1.21 s in the median on every
line, before and after. In the loupe the hold takes the first paint
below master's, by 0.2 s on the 35 and 0.13 s on the 300, since
master's worker was decoding thumbnails through the start-up until
the open arrived, and the strip still fills well before master's: 2.35
s against 3.49 on the 35, 5.1 against 17.0 on the 300. What the hold
costs is the strip's first second: before it, the 35 were all in at
0.6 s, before the picture. On the grid, unheld, the 35 fill in 0.63 s
against 3.60 and the first paint is 0.1 s earlier; on the 300 it is
0.18 s later (2.49 s against 2.31), which is the pool's decodes beside
the window's start-up and is the price of the grid's 300 filling in
4.1 s rather than 16.9. On the grid the develop is not what is on
screen.

The pool's own seconds (the "of decoding across 8 threads") are 2.6 to
4.4 s over the 35 against the worker's 1.8 to 2.3: eight decodes at
once are each slower, sharing memory bandwidth and cores, and the wall
time is what falls. The grid's snapshot over the 35 on a cold cache is
pixel for pixel master's (ImageMagick `compare -metric AE`: 0),
written at 2.5 s against master's 3.8 s.

**Tests.** In `thumbpool`: the pool's size from the cores; held while
the folder is queued and let go one at a time, the frames shown are
made first and the rest outward (4, 5, 6, 3, 7, 2, 8, 1, 9, 0); a
re-order while a picture is in hand applies to the rest and the one in
hand finishes; a file in hand is never begun by a second thread while
three are free, a repeat of a waiting ask is dropped, and the second
ask is made after the first; a folder change drops the waiting ones,
the one in hand is not delivered and the new folder's is; a panic in
one decode and an error in another cost those two cells and the
threads go on serving; four slow pictures on four threads take about
one's time; the size is the one wanted when a picture is begun,
rounded to a made size; a hold outlasts the worker's limit going back
up and ends with its release; held, the cache's hits are delivered,
each file is looked up once, nothing is made, and after the release
the misses are made in order. In the browser, on Slint's headless
backend: a `NoThumbnail` marks the cell, fills the grid, survives the
rows being made again, is ignored for a path not the slot's, and is
cleared by a picture that arrives after; a larger picture that fails
keeps the smaller unmarked and filled; the snapshot's give-up does
nothing without a snapshot or with the grid filled, gives up on
missing pictures and on cells waiting for a larger one, and fills the
grid. In `files`: a folder named as a raw, and on Unix a named pipe,
are not listed. The worker's existing thumbnail and cache tests pass
unchanged.

**What is left.** The hold on every develop could become a smaller
limit if thumbnails stalling during a slider drag on a cold folder
turn out to matter; the cache's hits are not held. The first write's
count walk under the cache's lock, when it beats the startup count. A
decode that hangs rather than fails holds its thread for good; the
snapshot's limit covers the snapshot, not the thread.

**The review.** Land after small fixes, no concurrency or correctness
bug found: the reviewer built its own master binary and reproduced
every number, then found the one the author's runs had hidden, a
first paint on the 300 consistently 0.15 s later with the pool
running beside the window's start-up, five runs of six above master's
median. The author had left the hold-until-first-develop as a
judgment call; the reviewer's answer, hold in the loupe where the
picture is what the user waits for and never on the grid where the
thumbnails are the content, is what was built, and the first cut of
it held the cache lookups too and put a warm strip back to 1.58 s,
which the author caught measuring. The rest was a doc comment that
had landed on the wrong test, a give-up warning that could read
"waits for 0 pictures ()", a log line whose seconds now sum across
threads, and the folder listing taking a named pipe as a raw, which
was the reviewer's follow-up suggestion and became the listing
change. One round.
