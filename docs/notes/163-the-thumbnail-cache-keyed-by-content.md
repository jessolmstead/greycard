# 163. The thumbnail cache, keyed by content (2026-09-24)

Roadmap v0.7.0's second line, built in wave B by an opus author on the
index of §160 and read twice by an opus reviewer.

Roadmap v0.7.0's second line, and the half of §72's "thumbnails stay
in the cache but keyed by content hash" that §160 left to
`Entry::hash`.

**What the browser did before.** There was no thumbnail cache on disk
at all, keyed by path or otherwise: §96 said so when the grid landed
and nothing had changed since. A folder open (`open_files`, or the
launch path in `startup`) sent one `Job::Thumbnail` a file to the
worker, and the worker's `thumbnail` asked rawler for the camera's
preview JPEG (`decode::preview_path`), decoded it — the full-frame
preview on a Canon, 8192×5464 on the R5 II — box-downscaled it to the
long edge asked for and turned it by the camera's orientation tag;
a file with no preview got a bilinear develop. The result lived in
`State::thumb_base` for the session and was forgotten with the
folder. So every open of a folder, the same one a minute later
included, decoded every raw's preview again, one at a time on the
worker, queued behind the first frame's develop. On the 35 sample
files that is 2.0 to 3.1 s of worker time (12 ms for a DNG's small
preview to 253 ms for an R5's full-frame one, median 59 to 69 ms),
and on a thousand-frame wedding a minute of it.

The "twenty seconds on a wedding" of §133 is not this. It is the
metadata probe (3 to 40 ms a file, §132) that the filter's camera,
lens and ISO chips would need if every raw's EXIF were read on the way
in, which is why those chips were left out; §160's index is what
answers that. The thumbnails were the other slow thing on the way in,
and the larger one.

**The key.** `greycard_library::hash_file`, the same function that
fills the index's `hash` column — BLAKE3 over the first 64 KB and the
size — so a thumbnail and its index row name the file the same way,
and a test indexes a file and checks the two agree. The editor now
depends on `greycard-library` for it rather than the hash moving to
core: the cache is the library's second layer (§72), it needs
`image` for the JPEG and `dirs` for the cache directory, neither of
which is core's business, and the editor will depend on the library
for the filter bar anyway. The cache lives in the library as
`thumbs::Thumbs`, with no UI in it.

The long edge is the rest of the key, one entry a size. The strip
asks 170 and the grid asks its cell's size, 96 to 360, and which the
first round of a folder is made at depends on when the grid's
request reached the worker: the first trial run found a frame made
at 170 in one run and asked for at 176 in the next, a miss on a warm
cache. So the worker now rounds every request up to one of four made
sizes, `grid::MADE` = 128, 176, 256, 360. The strip's 170 becomes 176,
which is 7 percent more pixels; the grid's smallest cell gets 128 for
96, 1.8 times the memory at that zoom only. Nothing else moves, since
`wants_bigger` already ignored a difference that small. The review
put the strip at 176 against master's 170 and found it draws the
same at the strip's 96 px (PSNR 49.9 dB). That change of the made
sizes needed no bump of the recipe: the size is in the key, so an
entry made at 170 is simply never asked for, and no release has
shipped the cache for such an entry to be sitting in.

**The entry.** One file a key, `greycard/thumbs/<first two hex>/
<hash>-<size>-r<recipe>-<stamp>.thumb` under the platform's cache directory
(`$XDG_CACHE_HOME` first, as the model store and the lens database
do), a 44-byte header and a JPEG at quality 90: magic `GCTH`, the
format, the caller's recipe, width, height, the caller's stamp, the
JPEG's length and the first 16 bytes of its BLAKE3. What is stored is
exactly what `thumb_base` holds, the preview sized and turned by the
camera's tag, not the picture as the strip draws it: the frame's own
quarter turns and mirror (the sidecar's `turn` folded with the edit's
geometry by `shown_turns`, §131) are applied by `show_thumb` at draw
time, and a crop is not shown on a thumbnail at all. So turning a
frame never makes its entry stale and costs no entry, which is the
answer to "a changed turn misses or re-keys": neither, because the
turn is not in the picture kept. What the entry does depend on is
covered: the camera's orientation tag is in the head the hash reads,
for the raws that put it there. At 176 px the 35 entries were 1.8 to
17.6 KB, 8.9 KB on average: 300 MB holds about 35,000 frames at one
size, and fewer once the grid's 256 and 360 are kept beside it.

The recipe and the stamp are in the name and not only the header.
The first cut had both in the header alone, and since a miss removes
the entry, anything that differed only in them removed the other's
entry on every open: two builds making pictures two ways, a release
and a development build on one machine; and, the second review
found, two copies of one frame with different times — a card and its
import by a plain `cp`, which keeps the head and the length and so
the key. Opening the two folders by turns remade those frames every
time ("33 from the cache, 2 made" on the reviewer's replay). With the
stamp in the name each copy has its own entry; an entry whose file's
time has since moved on is never asked for again and goes by the
eviction, like any entry not used. Checked here by copying the whole
sample folder with `cp -r` (new times) and opening the original and
the copy by turns: the copy's first open made 35, and after that
both opened with 35 of 35 from the cache.

**Invalidation.** An entry is a miss, and is removed, when its
header is short or not ours, its format is not this build's, its
length is not the file's, its checksum
does not match, or its JPEG does not decode to the size the header
says. The checksum is what catches a truncated or damaged entry,
since the JPEG decoder happily fills a cut-off scan with grey; the
tests cut an entry at six lengths, flip a byte in the JPEG, lie about
the width and write garbage of the right length, and each is a miss
and never a panic. `THUMB_RECIPE` in the worker is raised when
`thumbnail` makes a different picture: the downscale, the turn,
rawler's choice of preview, or the fallback develop, which is a
develop under `DevelopSettings::default()`; a comment beside that
default in core names the constant, so a change there that changes
the picture is not left with old thumbnails standing.

The stamp is the file's modification time, for every file. The first
cut stamped only picture files and DNGs, reasoning that a CR3, NEF,
ARW, RAF or RW2 keeps its orientation and its preview's directory in
the head and nothing rewrites it in place. The review found the case
that reasoning missed: a raw being copied. A copier that sets the
file's length before it writes — Windows' CopyFile, `rsync
--preallocate`, many card importers — leaves a file whose head and
length are already the finished file's, so its hash is already the
finished file's key, while the tail is zeros; the preview cut there
decodes mostly grey, and the first cut stored that grey picture under
the key the finished file would have, where every later open found
it. The review's recipe — `truncate` a copy to the raw's length, `dd`
the first 192 KB in, open the folder, finish the copy, open again —
showed "from the cache" and the grey picture for NEF, ARW, RAF and
RW2 alike. Now the copy's end moves the time and the entry is a miss.
And the file is stat'd before it is hashed and again after its
picture is made, and a picture made while the length or the time
moved is shown but not kept. The cost of stamping every file is one
more making of each thumbnail after a copy that does not keep the
times: `cp` without `-p`, and some desktops' drags between disks. A
move or a rename keeps the time and still hits. What is still open:
a copier that sets the finished time as well as the finished length
before it writes, and stalls long enough for a thumbnail to be made
between two of its writes, would still leave a grey entry; nothing
seen does that, and the stat around the making narrows it to a stall.

Entries are written to a temporary name and renamed into place, so
another editor never reads half of one. A temporary file left by an
editor that died mid-write is counted, evicted and cleared like an
entry, and a clear takes the fan-out folder it was in.

**The cap.** 300 MB by default, and a field in the Settings sheet's
THUMBNAILS row, "Keep up to … MB, 0 for none", taken on Enter or when
the sheet closes; it is `thumb_cache_mb` in `settings.json`, and the
field is there so nobody has to edit that file by hand, where one bad
value resets every setting. A cap past 2^44 MB saturates rather than
overflowing. A hit sets the entry's mtime to now, which is its
recency, and a write that takes the cache past its cap removes the
least recently used entries until it is under nine tenths, so a cache
at its cap does not evict on every write. A lowered cap evicts at
once rather than at the next write, and 0 typed in the sheet — the
cache off — empties it. A cache found past its cap when it is counted
at startup (a cap lowered in `settings.json` while the editor was
closed) is evicted by that count, on the counting thread, not left
until the next write; a cap of 0 found at startup is the cache off
and leaves what is there, shown in the row with Clear, for the user
to remove. The row says what the cache holds against its cap, and Clear
shows whenever it holds anything, the cache off included, so a cache
left from before the cap went to 0 can still be removed from the
app.

What the cache holds is counted once, by walking it: the review
measured the walk at 0.41 to 1.24 s for 400,000 entries on NVMe, and
seconds on a spinning disk. The first cut walked it on the UI thread,
under the worker's lock, every time the sheet opened and after every
Clear. Now the count is made once on a thread of its own when the
editor starts, without the lock, and kept up by every write, removal,
eviction and clear; the sheet reads the kept count with a `try_lock`
and never waits, says "Counting…" in the moment before the count is
in or while the worker holds the cache in an eviction, and a Clear or
a new cap runs on a thread of its own and fills the row in when it is
done. A sheet opened while the startup count is still running does
not walk the cache a second time: its thread looks for the seeded
count a few times a second, without holding the lock between looks,
and fills the row in when it is there; a new cap set before the count
is in is evicted to by the count. The worker still walks the cache under its lock for an
eviction, which holds up thumbnails, not the window.

**The numbers.** Release build, the 35 sample files (33 raws and 2
JPEGs, 1.2 GB) copied under the worktree, the editor on a headless
mutter's Xwayland with `--grid --snapshot`, which waits until every
cell the grid shows has its picture (all 35 fit). Each line is five
runs; the time is the editor's own log line, from the folder's open
to the last thumbnail arriving on the UI thread, with the worker's
own time on the thumbnails beside it. "Hot" is with the raws in the
page cache, "evicted" with them dropped by `posix_fadvise` before
each run and `fincore` confirming zero pages. Load average 13 to 28
for the first block (five other worktrees building), 10 for the
interleaved pairs.

| condition | folder open to last thumbnail, median | worker's time | process wall, median |
|---|---|---|---|
| cache off (today), hot | 4.09 s | 2.52 s | 4.67 s |
| cache empty, hot | 4.04 s | 2.45 s | 4.74 s |
| cache full, hot | 0.01 s | 0.00–0.01 s | 2.20 s |
| cache off, evicted | 3.52 s | 2.09 s | 4.08 s |
| cache full, evicted | 0.20 s | 0.01 s | 2.16 s |
| cache full, folder renamed | 0.01 s | 0.00 s | 2.11 s |

Five interleaved pairs of cache-off and cache-empty, evicted, at load
10: 3.37 to 3.38 s both, 2.01 to 2.04 s of worker time both, so the
write — a JPEG encode and a 9 KB file — is lost in the noise of a 60
ms decode. The first block's evicted cache-empty runs had come out at
6.1 s in the median at load 28 against off's 3.5 at load 14; the
pairs say that was the load. Five further runs with the cache's own
entries evicted as well as the raws: 0.19 to 0.20 s, 0.01 s of worker
time. The 0.19 s in the evicted rows is not the thumbnails: the
worker finished all 35 in 10 ms, and the UI thread's own start-up
(reading the sidecars from a cold disk) is what held the deliveries.

A file, from the debug lines: a hit is 0.10 ms in the median (0.7
at worst) with the raw in the page cache and 0.30 ms (0.6 at worst)
with it on disk — the 64 KB read, the hash, the entry's read and a
176-pixel JPEG decode together — against 59 to 69 ms for the decode
it replaces, which is 200 to 600 times the cost. So a warm folder
open is not slower than today at any file; it is the first frame's
develop, not the strip, that the open waits on now. The wall-clock
difference, 4.7 s to 2.2 s, is the snapshot waiting for the grid to
fill: today behind the develop and 35 decodes, now behind the
develop alone.

A renamed folder (`shoot` to `wedding-1`, and on through
`wedding-5`) logs `35 from the cache, 0 made` every time, and the
strip's snapshot after the rename shows every thumbnail; the grid
after the rename and the grid with the cache off agree to within a
pixel at 6 percent fuzz over the whole window (PSNR 46 dB), which is
the JPEG. The review reproduced all of it — the rename hitting 35 of
35, 46.07 dB against the cache off, 0.1 ms a hit, off against empty
within the noise — and ran two writers and two readers on one key
with no error.

After the review's fixes, stamping every file: at load 8, one cold
open (3.56 s, 2.16 s of making), five warm (0.01 s each, 35 from the
cache) and five renames (0.01 s each, 35 from the cache), so the
stamp costs a rename nothing. The review's half-copied recipe, run
again for NEF, ARW, RAF and RW2 beside a whole CR3: the open after the
copy finished logged `1 from the cache, 1 made` — the CR3 from the
cache, the finished copy made — and its grid agreed with the cache-off
grid at 60.8 dB, where the mid-copy grid, with the grey picture, sat
at 32.7 dB from it.

The editor's binary grows 0.5 MB, 85.0 to 85.6 MB stripped (the
review's measure). SQLite comes in with `greycard-library` and is
dead-stripped from the editor, which calls none of it; the release
packaging already compiles it for the CLI, so the build does no more
than it did.

**Tests.** In `greycard-library`: the key is the index's `hash` for
the same file; a hit after the file is moved into another folder
under another name, and then deleted, returns the stored picture;
another size is another entry; a byte changed in the head (the
orientation tag rewritten) is another key; another recipe misses and
leaves the other recipe's entry standing, both hitting after; another
stamp misses; a damaged entry, cut at six lengths, with a flipped
JPEG byte, a lying header or garbage, is a miss and removed, and a
good one hits after; eviction over thirty entries at a cap of ten
keeps under the cap, keeps the one used every time and the newest,
drops the oldest, and a clear empties it; a picture of the wrong
length is refused and nothing is written; the kept count follows
the disk through a count made off the lock, new and replaced
entries, a damaged entry removed, a lowered cap that evicts at once
and a cap of nothing that empties the cache; a leftover temporary
file is counted and a clear takes it and its folder. In the editor: a
file nothing can decode, with an entry for its hash, is a hit after a
rename into another folder, which it could only have been from the
cache; a PNG is made, then found, then made again after it is
rewritten with a new mtime, with the new pixels; the review's recipe
with a stand-in decode, a raw preallocated to its length with only
its head written, made grey, then made again once the copy ends with
the same hash and a new time, and the finished picture found after a
rename; a file that grows or is touched while its picture is made is
not kept, and left alone is; a cap of nothing and no cache each look
nothing up and write nothing; the count runs on a thread of its own
and seeds the cache; the folder's run counts each file once, a
failure included and a grid's second picture not; the made sizes
land every cell on one of four, never smaller than asked; the sheet's
words, the cap's parsing and its saturation.

**What is left.** The roots line: the index deciding what is in a
folder, and the browser reading `Entry::hash` from it rather than
hashing each file — which would save the 64 KB read, a tenth of a
millisecond a file, and is not worth coupling the browser to the
index before the filter bar does. Making thumbnails on a cold cache
is still one at a time on the worker behind the first develop; a
second thread for them would take a cold open of the samples from 2
s of decoding to a fraction, and is its own change. A truncated CR3
panics the worker with "capacity overflow" inside rawler, on master
as on this branch; it is caught at the job, but it wants fixing, and
upstream. A file whose thumbnail fails (the review's half-copied NEF
fails in rawler's NEF decoder) never fills its grid cell, so a
`--grid --snapshot` over such a folder waits until it is killed; that
is the snapshot's wait, as old as the grid, and not the cache's. An
entry's recency is its mtime, so a cache on a
filesystem that refuses `set_modified` evicts in the order entries
were written. Two editors running at once each count the cache's
size for themselves and may evict a little early; nothing is lost
but a render. The culling view's full-size previews (§123) are a
separate in-memory cache and are not kept on disk.

**The review.** The numbers held from the first pass; the design did
not, in three places the tests had not reached. A raw caught
mid-copy by a copier that sets the length first had the finished
file's key and a gray picture, cached for good; the Settings sheet
walked the whole cache on the UI thread under the worker's lock; and
two things that differed only in the header, a build's recipe or a
copy's time, removed each other's entry on every open. The reviewer
also ran two writers and two readers on one key, timed a sheet open
against a cache of 400,000 entries it made for the purpose, and found
the two pre-existing bugs now at the top of the roadmap: the
truncated CR3 that panics the worker and the grid snapshot that waits
forever on a cell that never fills.
