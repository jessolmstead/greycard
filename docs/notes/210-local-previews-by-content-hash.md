# 210. Local previews by content hash (2026-09-30)

Roadmap line (Next 0.4.0): "Local previews by content hash: a mid-size
picture per frame in the local cache, made by the pass, so a shoot on a
NAS is culled in the loupe and the compare view without reading the raw
over the network. Lightroom's smart preview, minus the editing."

**What it was.** The culling loupe and the compare view had one source
for a frame's picture: the camera's JPEG, decoded from the file by the
cull threads (§123) at the view's size. A frame under an offline root
(§207) was listed, dimmed, badged and filtered from its row, and its
thumbnail came from the cache by the row's key; culled, it had nothing,
since the decode asked the file and the file was not there, and the
status line said "no camera preview". On a network mount the decode
read the embedded JPEG over the share for every frame the arrow came
near, two or three megabytes each. §207's "Not done" named it: "an
offline frame's picture comes from the cache or not at all; there is no
smaller stand-in".

**Now.** Every frame under a root has a second entry in the thumbnail
cache beside its thumbnails: a local preview, the same picture the
thumbnail is made from at a long edge of 2048, under the thumbnails'
own key (`<hash>-2048-r<recipe>-<stamp>.thumb`). The eviction, the
stamp, the damaged-entry check, a deleted file's removal
(`Thumbs::remove` takes every size of its hash and stamp) and the
settings sheet's count and clear all cover it without a line of their
own. `greycard-library` grew the calls for it: `Thumbs::has`, whether
an entry's file is there, one stat and no decode; `Thumbs::put_at`, a
put at a JPEG quality of the caller's; and the halves of a get and a
put, so a caller can do the JPEG work outside the cache's lock:
`thumbs::encode` and `Thumbs::put_bytes`, `Thumbs::read`,
`thumbs::decode` and `Thumbs::settle` (the recency on a good entry,
the removal of a damaged one). Every thumbnail lookup takes that lock,
and so does the window's delete; a 2048 px encode under it, 60 to 100
ms, held all of them, so the previews do their encoding and decoding
outside it and take it only for the write, the rename, the read of the
bytes and the count.

The loupe plans each frame by its root (`cull::source_for`, from
`panel::cull::loupe_plan`): the local preview under a key, the file, or
both.
- Under a root on a local disk, the file, as before.
- Under an offline root, the local preview alone, found by the key the
  frame's row gives (`FromRow::key`); nothing of the file is read, not
  a stat and not a head. With no key (a frame read from disk rather
  than from a row, or `--no-sidecars`, which holds every frame as read)
  there is nothing to find it by, and the frame is marked as having no
  picture without asking anyone: "culling: nothing to show; its root is
  offline, and no local preview of it is kept". A key with no preview
  kept under it says the same once the cache has said so.
- Under a root on a network mount (`Library::remote`, which the roots'
  look already fills), the local preview and the file, as two wants in
  two bands: every local preview the window wants goes ahead of any
  file, so the window's previews are all up, a few milliseconds each,
  before any read goes over the share. The first cut had one want do
  both on one thread, preview then file; with the loupe's one to three
  threads, a few frames on a hung share parked every thread and no
  preview came after. The file's picture replaces the preview when it
  comes. A file that fails with the preview up leaves the preview up
  and is not asked for again (`Cull::file_failed`): the first cut logged
  the failure and asked again at every step, each time reading and
  decoding the preview once more and parking a thread on the dead
  mount. A preview not kept is not asked for again either
  (`Cull::no_local`), and the file is what shows. The two land in
  either order: a preview that lands after the camera's JPEG is dropped
  rather than put over it, and a file failure that lands before the
  preview is moved from the failures to `file_failed` when the preview
  comes, so the plate names the preview and not "no camera preview". A
  frame on a share with no row key gets the file alone: its key would
  be read from the file's head over the share, which is the read the
  preview is there to go ahead of.

The compare view is the same loupe with more tiles, so it takes the
same sources. The status plate names the picture: "culling: the local
preview, 1366 × 2048, fitted, through the monitor profile only", in
place of "the camera JPEG" or "a small camera preview";
`panel::cull::picture_words` is the one place the three are worded, and
the develop view's placeholder line takes it too for the one case a
local preview can reach it (culling left over a frame whose picture was
the preview). `Preview::local` marks the picture, and a local preview
is never called small: it is small by design and its words say so.

The develop view is not the preview's business. A frame chosen there
asks for the camera's JPEG as it did (`Want::of_file`), and a frame
that cannot be decoded says so as it did.

**Who makes it, and when.** The thumbnails' pool, behind the
thumbnails. A thumbnail's lookup already stats the file and hashes its
head for the cache's key; now it also notes the frame with the
previews (`Previews::note`) when the frame is under a root and the
cache has no preview under that key, which costs one stat of a file in
the cache and nothing of the raw. When the thumbnail is delivered, a
frame noted is queued as a preview (`thumbpool::PreviewHooks`). A thread
begins a preview only when no thumbnail can be begun (none waiting, or
every one waiting already in another thread's hands), the pool is not
held, and two more may be made: one thread is always left for a
thumbnail. A preview of a raw with no JPEG in it is a develop, seconds
long, and one on a slow share waits on the share; without the spare
thread they could take the whole pool, and the next folder's
thumbnails would wait for them. So previews are made one fewer at a
time than the pool's size, the develop's hold (§172) holds them too,
and the loupe's first develop is not slowed by them. Nothing is made
until the cache's startup count is in: a write before the count walks
the whole cache under its lock, and the first cut would have let the
first previews do that. The
previews wait in the thumbnails' order and are sorted by the same
`want` range, the frames on screen first. A folder change drops the
previews waiting with the thumbnails; one in hand finishes, since it is
keyed by content and is as good for the next list. Nothing is delivered
to the window: a preview is for the cache. What is owed is the open
list's: the roots handed over with a new list clear it, and a frame
taken to be made is checked again against the roots, so a frame of a
folder forgotten or a root removed is not made.

The preview is made again from the file (`Previews::make`): the
camera's embedded JPEG, or the picture itself for a JPEG, PNG or TIFF,
brought to 2048 on its long edge by `image::imageops::thumbnail` (area
averaging, to the size, not to a whole factor: the thumbnails' whole
factor would make a 24 MP frame's 6000 into 1500, and an R6 II's 6240
into 1560), turned by the camera's orientation tag, nothing of the edit.
A raw with no JPEG in it goes the thumbnail's way, the quick bilinear
develop at the same size. The file is stat'd before and after, against
the stat the note recorded, and a file that moved meanwhile is not
kept under the old key; the next thumbnail notes it again.

The roadmap said "the first time it is decoded anyway". A frame the
culling loupe decodes from its file, and which is owed a preview, has
it made from the picture already in hand, with no second read. The
cull thread takes it off the owed list (memory alone) and hands the
picture to rayon (`Previews::make_taken`), so the thread goes on to the
next decode at once: the first cut made it on the cull thread, and on
a machine with one cull thread every step onto a frame owed a preview
held the next frame's decode by the resize and the encode. Each such
job holds the whole camera picture (72 MB at 24 MP, 135 at 45), and a
develop begun meanwhile queues its work behind them on the same pool,
so no more than two are in hand at once (`FROM_PICTURES_AT_ONCE`); past
that the loupe leaves the preview owed and the thumbnails' pool makes it
later from the file. The job catches its own panic and logs it: a panic
on rayon's pool with no handler ends the process, where the first cut's
in-line make sat inside the cull thread's catch. The offline thumbnail's
cache lookup, also spawned on rayon (§207), is caught the same way and
delivers no thumbnail. The develop
view's placeholder decode (§134) is the same threads' and makes none
(`Want::make_preview`): the first cut made the first frame's preview
there, beside the first develop, and the develop's cores are the
develop's.

Only frames under a root get one. A folder opened on its own is where
the user put it, and read from there; the roots are what can go offline
or live on a share. The roots are handed to the worker with each list
the browser opens (`Worker::set_preview_roots`, in `open_loaded`), less
the ones offline.

**The decisions, for the user to move.** Each is a constant with its
reasons beside it, in `previews.rs`.
- `SIZE`, 2048 on the long edge: past a 1440p loupe's fitted view and
  half a 4K one. 2560 would fill a 4K loupe at about 1.6 times the
  bytes. At 1:1 the loupe shows the preview's own pixels and the plate
  says it is the local preview; where the file can be read, the
  full-size JPEG is asked for all the same (a local preview never
  counts as the JPEG's every pixel), and replaces it. A thumbnail is
  never made at that size (`grid::MAX_RENDER` is under it, a compile-time
  check), so the two can never be one entry.
- `QUALITY`, 88: a little under the thumbnails' 90, where the bytes
  start to count, and well past what culling for focus and expression
  can see.
- `ROOM`, three quarters of the cache's cap: a preview is made only
  while the cache is under it. Without a limit a library larger than
  the cache would make previews that evict one another round and round,
  and the thumbnails with them, at every open of the roots' view. At
  the default 300 MB cap that is about 225 MB of previews, some 480
  frames at the measured 472 KB each; past it the rest cull from their
  files as before, and the thumbnails keep the last quarter. The real
  question is the cap: a NAS library of 11,711 frames wants about 5.4
  GB of previews. A separate cap for previews, or a larger default when
  a root is on a network mount, is the user's call; the settings sheet
  shows one number for both today.
- The source: the camera's JPEG (as the loupe shows it) rather than our
  own develop, since this is culling's picture and not the develop's,
  and "thumbnails from the edit" is its own roadmap item. A DNG whose
  only preview is 1024 wide keeps that size; its develop would be the
  honest larger picture, at a second or two a frame on the pool.
- When: behind the thumbnails, on the pool, never while it is held. The
  other reading of the roadmap line, making the preview in the
  thumbnail's own decode, would save the second read of the embedded
  JPEG and cost the strip its fill time on a cold folder (the resize and
  encode are about 100 ms a frame on top of the thumbnail's 50 to 250),
  which the pool's order exists to protect.

**Measured.** Release, `previews::tests::the_samples_previews`
(ignored; `GREYCARD_SAMPLES` names the folder), over the 33 frames of
the sample folder hard-linked into the work dir (26 CR3s at 24 to 45 MP,
four DNGs, two NEFs, three RAFs, an ARW and an RW2), one thread, the
files in the page cache:
- bytes: 15.9 MB for the 33, 472 KB a frame in the mean; 143 KB to
  1,019 KB across the CR3s by content (a clean studio frame against
  foliage), 26 to 150 KB for the DNGs' 1024 px previews;
- making: 151 to 170 ms a frame in the mean over two runs (5.0 and
  5.6 s for the 33); a 24 MP CR3 150 to 175 ms, a 45 MP R5 II 205 to
  257, the RAFs 138 to 162, the NEFs 156 to 175, the ARW's 1616 px
  preview 31, the RW2 45 to 47, the DNGs 19 to 27. Of that, the decode
  of the embedded JPEG is what §123 measured (35 to 108 ms); the rest is
  the resize and the encode;
- reading back, as the offline loupe does: 6.8 ms a frame in the mean
  (the entry's read, its checksum and the JPEG decode).

In the editor, a debug build in a sandbox of its own, the 33 as a root:
the pool made 33 thumbnails and then 33 previews (16 MB in the cache,
counted on disk). In the log the first preview begins after the 26th
thumbnail, when the queue has none left to begin and the other seven
are in other threads' hands; those seven land among the first previews,
and the rest follow. The pool's line for the thumbnails: 33 files in 35 s from
the open, all made, in a debug build on eight threads; the previews
followed in the next 20 s or so. No run without previews was made to
set beside it; the pool's order is what keeps them out of the
thumbnails' way, and the pool's tests are what check it. Then the
folder renamed away, a second run with `--all-roots --cull`: the root
said offline, the loupe up on the first frame's local preview, "culling:
the local preview, 1366 × 2048, fitted, through the monitor profile
only", and `--cull-compare 4` the same over four tiles. `--time-cull 20`
over the offline root: mean 326 ms a step in debug, which is the debug
build's JPEG decode; the release read-back above is 6.8 ms. Nothing
under the offline root was read in either run (it was not there to
read). No network share was at hand: the preview-then-file order is
checked by the tests and not in the editor.

**Tests.** The library: a picture kept at its own quality is an entry
like any other, named by its size, found by `has` without a read, read
back whole, and smaller at a lower quality. `previews`: the key is the
thumbnails' own with 2048 for the size, and the entry on disk is the
bytes reported; made, then read back with the file gone, with another
stamp a miss, and a frame kept already owed nothing; a file changed
after its note is not kept; only frames under a root are owed one, none
is owed with the cache off, none is made past the cache's share of its
cap, and one made from a picture in hand is turned. The pool: every
thumbnail comes before any preview, and a file with its preview kept
is owed none; held, the cache's thumbnails are delivered and no
preview begun, a folder change drops the previews waiting, and let go a
new folder's is made. The loupe's plan: the file on a local disk, the
preview alone offline and nothing without a key, the preview and the
file on a network mount with a key and the file alone without.
`cull::fetch`: the develop view's decode of a frame makes no preview,
and the loupe's decode of it makes its owed preview, at 2048; with the
file gone the kept preview comes back marked local. And through the
window (headless, on §207's
offline-root fixture): a frame under an offline root, entered in
culling, is asked for from its preview alone under its row's key, the
preview is delivered and marked local, its file and its neighbor's are
never read (`cull::FILE_READS`, by path), no sidecar is read under the
root, the plate reads "culling: the local preview, 300 × 200, fitted";
a neighbor with no preview kept is told so; a frame of the online local
root asks for its file, and with that root said to be on a network mount
asks for its preview and its file, the preview ahead of every file; its
preview delivered and then its file failing leaves the preview up, no
failure over it, and the next step asks for neither again.

Added with the review's fixes: a local preview is not the JPEG's every
pixel, so 1:1 still asks for the file's; a preview want and a file want
of one frame are two wants to the threads; a preview not kept says so
with no read of the file; the loupe's decode makes its preview on
another thread (the test waits for it to land); a cache whose count is
not in makes none; new roots clear what was owed and a frame taken
under a root no longer one is not made; a picture made from one in hand
reads nothing of the file; on a pool of two, the previews waiting are
made one at a time in the thumbnails' order about the range shown, and
a thumbnail asked for while a preview is in hand is made at once beside
it; and a frame's full-size copy held with no view-size one (the compare
view entered from 1:1) still asks for the view-size copy. That last was
a regression the review found in the first cut, which took the full
copy for the view's and left the tile blank until the next move. From
the second review: past two previews in hand from pictures the next is
skipped and left owed, and room comes back as one is let go; a late
local preview leaves the camera's JPEG up; a file failure before the
preview leaves the preview up with no failure said and the file not
asked again.

**Not done.**
- The cap. Previews share the thumbnails' one cap and take at most three
  quarters of it; a library past about 480 frames at the default gets
  previews for the first that the pool reaches and none for the rest.
  See the decisions.
- The preview is made from a second read of the embedded JPEG, after
  the thumbnail's. On a share that is the JPEG's megabytes twice for a
  cold frame, unless the client's page cache still has them.
- A folder opened on its own, not under a root, gets no preview; a
  folder on a share that is not a root is not known to be remote, so it
  is culled from its files as before. `mounts::remote` could be asked of
  the folder.
- A root added while a list is open is handed to the previews with the
  next list, not at once.
- A frame on a share with no row key (a folder the index does not know
  yet, opened under a root) culls from its file alone, as before.
- A file that failed with its preview up is not tried again until the
  list is opened again; a share that comes back meanwhile is not
  noticed by the loupe.
- `--no-sidecars` holds every frame as read, and so drops the row's key:
  an offline root under it has neither thumbnails nor previews. That is
  §207's, and its tests say it.
- At 1:1 an offline frame shows the preview's own pixels; the plate says
  so by name but does not say "offline". The "Enter develops" at the end
  of the plate is as true as it is for an offline frame elsewhere: Enter
  says the root is offline.
- Nothing measured over a real share; the network-mount path is checked
  by the tests alone.
