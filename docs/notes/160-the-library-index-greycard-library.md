# 160. The library index: `greycard-library` (2026-09-24)

Roadmap v0.7.0's first line, built in wave A on Fable 5.1 and read
five times by an opus reviewer. Most of what follows was found by the
review and is written as the design now is; the cuts it replaced are
named where the reason matters.

§72 laid the library out as three layers — truth on disk, virtual
structure in one small file, a rebuildable index — and put the index
first after the meta section. This is the index, as a crate with no
UI in it, and the CLI listing §72 named as the testable surface for
the filter language before the filter bar exists.

**The schema.** One SQLite file, `greycard/library.sqlite` under the
platform's local data directory (`$XDG_DATA_HOME` first, as the
camera profiles do in §122; `data_local_dir` rather than `data_dir`,
since on Windows the latter is the roaming profile and an index of
this machine's disks has no business following a profile to
another), rusqlite with SQLite bundled so every platform runs the
same build. One table `files`, one row a raw or picture: `path`
(unique) and `folder` as BLOBs holding the path's exact bytes — the
OS string's bytes on Unix, the UTF-16 units on Windows — beside
`folder_text` and `name` as text for the `folder:` and `name:`
filters and the log; the folder indexed, since a pass over a folder
and the filter bar's folder view both ask by folder; `size` and
`mtime` (nanoseconds); the content `hash`, indexed; the EXIF —
`make`, `model`, `camera` (the two joined as `camera_name` shows
them, so `camera:R6` is one test and not a guess about which tag
Canon put the body in), `lens`, `iso`, `focal`, `aperture`,
`shutter`, `taken`; the sidecar's path, a hash of its bytes, and its
mtime; `rating`, `flag`, `label` and `keywords` as a JSON list; and
`missing_since`. A second table `keywords(file, word)` is the
keyword search's index, the JSON column what a row reads back.

The exact bytes for the path were the second cut. The first stored
`to_string_lossy`, and two names that lossy to the same text — a
Latin-1 `café.tif` beside a corrupt one — met the UNIQUE constraint
and took the whole folder down with them, while a copy of such a
file flip-flopped between "moved" and "added" on every pass because
the stored spelling was never a path that existed. Bytes match what
is on the disk; the text columns are for reading, never for
matching.

`PRAGMA application_id` is set to `GRCY` on creation and
`user_version` to the schema. A SQLite file without the id is not a
library and is refused, whatever tables it has; an older library is
dropped and rebuilt with a warning, since it is a cache and a
migration would be more code than the rebuild costs while nothing
but the index reads it; a newer one is refused with a message, since
the later build may still want it. Write-ahead logging is a property
of the file and is set once, when the file is made: an open that
sets `journal_mode` on every connection needs the write lock, and
the first cut's read-only listing did exactly that and reported
"database is locked" after five seconds whenever an index held its
transaction longer. Now `open_read_only` sets nothing, a listing
opens that way, and under WAL a reader never waits for a writer.
`synchronous=NORMAL` is per connection and safe under WAL: a crash
loses the last transaction, never the file.

`taken` is the EXIF date with its colons turned to dashes in the
date part, `YYYY-MM-DD HH:MM:SS`, so it sorts lexically and a
filter's `2026-09` is a prefix of it; a camera with no clock writes
spaces, and that is stored as no date. The split is by characters:
the first cut split by bytes, and a tag with a multibyte character
where a digit should be (rawler's replacement character for a byte
it could not decode is three bytes long) split inside the character
and took the process down, the rest of the folder with it.

**The probe grew.** `decode::Probe` had make, model, ISO, shutter
and aperture, for the learned blend's ISO and the bench's grouping.
It now carries the lens, the focal length and `DateTimeOriginal`
(or `CreateDate` when only that is there), from the same metadata
read, built by one `Probe::from_metadata` that a new
`picture::probe_path` shares for a JPEG's or PNG's EXIF chunk, so a
folder of exports is indexed on the same terms as the raws beside
them. The picture probe reads the file's head — 128 KB — and walks
the JPEG's segments to the start of scan, or the PNG's chunks to the
first `IDAT`, growing the read only when a segment runs past what it
has; a file cut off after an intact EXIF block still probes, and one
cut off inside it says nothing rather than failing. A PNG that put
its `eXIf` after the picture data, which the standard allows and
nothing writes, is not read past the first `IDAT`. A TIFF's
directories can be anywhere in it, so a TIFF is read whole. The
rationals are read in double precision straight from the tag rather
than through `Shot`'s f32: the first cut went through the f32 and
f/2.8 arrived as 2.7999999, under `aperture<2.8`. The raw extension
list moved from the browser's `files.rs` into
`decode::RAW_EXTENSIONS` so the index and the browser list the same
files.

**The hash** is over the first 64 KB and the size, as §72 chose;
BLAKE3 is this crate's choice of function, several times faster
than SHA-256 on 64 KB and a key rather than a signature. What the
head holds depends on the container. In a CR3, NEF, ARW, RAF or RW2
the EXIF and, in most, a thumbnail are inside the first 64 KB — a
NEF keeps its JPEGs further in, at 247 and 265 KB in the samples —
and two frames from one body a fraction of a second apart differ
there in the counter, the timestamp and the thumbnail's pixels,
thousands of bytes (the two sample NEFs, with no thumbnail in the
head, still differ in 27,538 of the first 65,536). A DNG written by
DNGLab is the other way round: IFD0 and the
EXIF are at the end of the file (the four sample DNGs, 50 to 66 MB,
have IFD0 in their last 500 bytes) and the first 64 KB is the header
and raw tiles, so a DNG's identity rests on its sensor data, which
is noise and differs between any two frames, and the size. A byte
changed past the head does not change the hash and a byte in the
head or a byte of length does, and the test says so. Sixty-four hex
characters, indexed.

**The incremental rules**, tried in order for each file on disk:

1. A row at this path with the same size and mtime: the file is not
   read. Its sidecar is looked for through `Sidecar::find`, beside or
   under `.greycard/` (§153), read — it is small — and hashed, and
   when that is not the sidecar the row knows, a different place or
   different bytes or none where there was one, the meta is written
   again and only the meta. The hash rather than size and mtime
   because a save that changes one digit of a rating changes neither
   the file's length nor, within one timestamp tick, its mtime, and
   the editor will call `index_file` after every save.
2. A row at this path with another size or mtime: the file changed
   on disk, and is hashed and probed again under its old id.
3. No row at this path: the file is hashed. A row with that hash
   whose file is gone from a folder that is still there is this file
   moved, and keeps its id and its EXIF; only the path and the meta
   are written. A row whose folder is gone, or is there but empty,
   is not a move's other end: an unmounted drive is a folder gone
   or, with `fstab` and a manual mount, an empty folder left
   behind, and the first cut let a copy on the laptop take the
   drive's row, so that the drive's own file got a new row when it
   came back. A copy — the old path still there — is a new row with
   the same hash, and `by_hash` lists both.

What was in the folder's rows and not on disk is marked missing with
the time and kept, which is how a move is found from either end: the
old folder indexed first marks the row missing, the new folder claims
it; the new folder first finds the old path gone and claims it before
the old folder ever notices. A folder that is gone marks its rows the
same way, from a pass over the tree above it — the tree pass ends by
looking for folders the index holds under the root that it did not
walk and that are not there — or from a pass over the folder itself.
The first cut had neither, and a deleted shoot was listed forever.

Two things that look like a deleted folder are not. A folder found
empty on disk, no entries at all, with rows in the index under it,
is a mount point with nothing mounted as likely as a shoot deleted,
so nothing under it is marked missing or claimed as a move, a tree
pass does not mark what is under it either, and the report counts
it unavailable, by name: marking a shoot missing because its disk is
in a drawer is Lightroom's exclamation mark, the thing §72 set out
not to build. The trade-off is real and is chosen: a folder emptied
on purpose keeps its files listed as present until the folder itself
is removed, and a move that leaves its old folder empty shows both
paths with nothing flagging the stale one until then. The other
reading — an empty folder is a deleted one — would mark a whole
drive's shoots missing at every unmount and let a copy on the laptop
take the drive's rows, which is the worse mistake and the one that
cannot be undone by `--prune` later. A folder with other files still
in it is a real change. And
a folder that is not there and that the index knows nothing of is a
name mistyped, and an error rather than a pass that "indexed 0
files"; one with rows under it is the deleted-folder case, and a
parent gone too is an error either way. `prune_missing` forgets the
missing across the whole library, and a listing leaves them out
unless the filter says `missing:yes`. A tree pass does not go into
hidden folders or follow links to folders, which could lead back up
the tree, and names the links it skipped in the report. A link to a
file is one entry of its folder under its own name, and a lookup or
a save through the link keys the same way — the folder canonical,
the name kept — so a file has one row and not one for the link and
one for its target.

A file the probe cannot read still gets a row, with no EXIF, so it
lists by name and rating; the error is in the report and it is not
retried until the file changes. A decoder that panics on a file's
bytes is caught at the file and is the same case: one line in the
report, the rest of the folder indexed. The test for it is a real
one: a head of the sample `P1000247.RW2` with fifteen bytes changed
by a fuzz pass, on which rawler 0.8's RW2 decoder divides by zero,
kept under the crate's `tests/fixtures` as 8,192 bytes with the
camera's serial numbers zeroed and every byte of the embedded preview
zeroed, since a preview is a picture of a person and this repo holds
no raw; a 6,144-byte cut does not reach the division.

The pass works in batches of fifty files, or of a second, and each
batch is two phases: the first stats, hashes and probes the files
with no transaction open, which is where the time goes, and the
second writes the batch's rows in one short write transaction taken
as a write from the start (`BEGIN IMMEDIATE`). It took three cuts
to get here. The first held one deferred transaction a folder: a
deferred transaction begins as a read and asks for the write lock
at its first write, and SQLite answers that upgrade with "busy" at
once rather than through the busy handler, since waiting there can
deadlock, so two passes on two folders, or a pass and the editor's
`index_file`, collided instead of taking turns — a pass over four
thousand files with an `index_file` every 20 ms beside it died with
"database is locked" within 75 ms. The second took the transactions
as immediate, which waits, but held each through the probing of its
fifty files and took the next the moment it committed; a second
writer polls for the lock through SQLite's busy handler, at
intervals up to 100 ms, and in tens of thousands of polls never once
found the lock free, so two passes started together still failed
two runs in twelve at the five-second timeout. With the file work
outside the transaction the lock is held for the writes alone, a
few milliseconds a batch, and the gap between batches is the next
batch's probing. The folder's rows are read once before the pass, so
a path that read did not hold is looked up again inside the
transaction before anything is inserted, or a row another writer
added meanwhile would meet the UNIQUE constraint and end the pass.
The making of the library is one such transaction too, with the
file judged again under the lock: two processes opening a library
that was not there both found it fresh and both made it, and the
three questions that judge a file — application id, version, tables
— are asked in one read transaction, since asked one at a time they
straddled the other process's commit and read "no id, no version,
tables", which is somebody else's file. The checkpoint the CLI runs
after a pass is passive: a truncating one waits for every other
connection and resets the log under them. Tested with a folder pass
and an `index_file` on the folder's last file running beside it
from another connection on another thread, with two passes on two
folders on a library that is not there yet, and with four openers
of a fresh library at once: every row, no error on either side; and
at the command line, two passes on two folders of 4,200 and 2,100
hard links to 21 CR3s started together, forty-eight pairs without a
failure. A listing opens read-only and under write-ahead logging
never waits at all.

Two phases have a seam, and the seam has a rule. If the editor
saves a rating and calls `index_file` between the phases, the row's
meta in the index is newer than anything the pass read before the
lock, and writing that back would undo the save. So the sidecar is
not read in phase one at all: phase two looks the row up again
under the lock and reads the sidecar there, every time, and settles
the row against both as they are now. A cut that re-read only when
the row's sidecar hash had changed missed the case of a rating
changed on disk and then set back in the seam — the hash is the
snapshot's again, and the pass would have written the change it read
before the lock. The sidecar is small and the lock is brief; a test
saves a five over a four in the seam, and a two changed back to one,
and the disk's word stands both times. The other cost of many copies
of one file: the move lookup asks, for every row sharing a hash,
whether that row's file is on disk and whether its folder is in use,
and a folder of 4,200 links to 21 raws asks it of 200 rows a file —
with a `read_dir` for each, the pass went from 1.9 s to 40 s. Now a
row in the folder being indexed is on disk if the listing has it, a
row elsewhere gets a stat, and a folder's in-use answer is kept for
the pass. `index_file` has no listing — its one path would say every
other file in the folder was gone, and a copy indexed on its own
took the original's row — so it stats every candidate.

The meta is read from the sidecar's JSON as `meta` alone, through
`serde_json::Value`, not through `Sidecar::load`: the edit and its
history are not needed and not migrated, and a sidecar whose edit
this build cannot read still gives up its stars. An XMP beside the
raw is not read; that is the browser's adoption in memory (§133's
`xmp::adopt`) until the frame's next save, and the index mirrors
what is saved.

**The filter grammar.** Terms separated by spaces, a file passing
when it passes all of them, §133's rule. A term is a bare word —
somewhere in the name or in a keyword, two words each having to land
— or `field op value`. Text fields (`camera`, `make`, `model`,
`lens`, `keyword`, `name`, `folder`) take `:` for contains, `=` for
the whole value and `!=` for not it, a value with a space in double
quotes. Number fields (`iso`, `focal`, `aperture`, `shutter`,
`rating`) take `:` and `=` for equal and the four orderings, and
read their units: `50mm`, `f/2.8`, `1/250`, `ISO800`. ISO and rating
are whole numbers and refuse a fraction: the first cut rounded, and
`rating>2.5` became `rating>3` and dropped the three-star frames it
was asking for. A date is a prefix, `2026`, `2026-09` or
`2026-09-21`, spelled with dashes or EXIF's colons, and every
operator compares that much of the file's date: `substr(taken, 1,
len) <= '2026-09'` is everything through September, which a plain
string comparison would not give. `flag` is pick, reject or none,
`label` one of the five or none, `missing` yes or no.

`!=` leaves out a file that does not say, on every field. The first
cut left NULLs in on the real and text fields and out on the integer
and date ones, so `iso!=100` and `aperture!=2.8` disagreed about the
two EXIF-less JPEGs; a JPEG with no EXIF is not "not ISO 100", it is
unknown, and that is what `!=` means to most people. No keywords is
known, and `keyword!=wedding` keeps such a file.

Case is folded with Unicode's rules on both sides, as the browser's
box folds it: SQLite's `LIKE` and `NOCASE` fold ASCII only, so the
first cut found `Ärger_50%.jpg` by none of `Ärger`, `ärger` or
`ÄRGER`. The library registers `ulower` with every connection, the
parser lowercases its values once, and the SQL compares
`ulower(column)`. Where the two still differ: the box has no quotes
and no fields, so there `"low tide"` is two words with quote marks
and `a:b` is a word, and here the quotes make a phrase and `a:b` is
refused as a field this language does not have, which is what
catches `camra:R6`. The box is §133's and will take this parser when
the filter bar lands. The parser refuses what it cannot answer — an
unknown field, a rating of nine or of two and a half, `lens>50`, a
lone quote (which the first cut read as an empty word matching
everything) — because a filter that silently matched nothing, or
everything, would look like the library and not like a mistake. A
token that starts with a digit is a word, so `12:30` searches for
it.

A real is never compared exactly. A shutter of 1/250 is 0.004 in the
file and not quite that as a float, and f/2.8 is written 28/10 by
one body and 2.8 by another, so for aperture and shutter equality is
a window half a percent wide (under a tenth of a stop), `<` and `>=`
are against its near edge and `<=` and `>` its far one:
`aperture<2.8` leaves the f/2.8 frames out and `aperture<=2.8` takes
them. A focal length's window is a tenth of a millimetre instead: 24
and 23.9 are different settings of a zoom, and a window of a percent
joined them. Integers compare as integers. The translation is
`Filter::to_sql`, a `WHERE` body and a list of parameters, the
parser and the translation both pure and tested without a database;
the library binds them.

On the command line each argument is one term. The first cut joined
the arguments with spaces and parsed the result, so `lens:"RF 24"`
typed in a shell — which arrives as one argument `lens:RF 24` — was
split into `lens:RF` and `24`, and the help's own example put
`iso>=3200` unquoted, which in bash is a redirection that creates a
file called `=3200`. Now `'lens:RF 24'` is a lens test with a space
in it, `'iso>=3200'` is quoted in the help and the README, and a
blank argument is nothing.

**The API** the filter bar and the roots will call: `open`,
`open_read_only`, `open_user`, `open_in_memory`;
`index_folder(dir, progress)`, `index_tree` (hidden folders left
alone, links named), `index_file` for the row after one save;
`query(&Filter)` and `count`; `by_hash`, `by_path`, `folders`,
`prune_missing`. A `Report` says what a pass did — added, moved,
changed, meta refreshed, unchanged, returned, missing, the errors and
the links skipped — and the CLI prints it. `greycard library index
DIR... [--tree] [--prune]` and `greycard library list TERM...
[--paths] [--count]`, `--db` for a database other than the user's;
`--prune` forgets the missing across the whole library, not only
under the folders given, and its help says so.

**The numbers.** The 35 sample files (33 raws from Canon, Sony,
Nikon, Fujifilm and Panasonic, 2 JPEGs, 1.2 GB), release build, on a
Samsung 990 PRO NVMe under LUKS on btrfs. Cold, the files evicted
from the page cache with `fadvise` before each run and `fincore`
confirming zero pages cached: 0.27 to 0.33 s for the 35 on a quiet
machine, 8 to 9 ms a file, which is rawler's metadata read from disk
and not the index. The same runs on this machine while five other
worktrees were building (load average 14 to 18) reported 0.29 to
0.51 s, which is what the load costs. The second pass, everything
unchanged, is 3 to 6 ms as a whole process: a stat of the file, a
read of its sidecar where it has one, no bytes of the raw. With the
files in the page cache, the first pass into an empty database is
0.07 s, 2 ms a file, the process 0.11 s. A two-term listing over the
35 is 2 ms as a process, opened read-only.

A rename within the folder came back as `1 moved, 34 unchanged` with
the row's id kept; moved to a sibling folder, the old folder reported
`1 missing` and the sibling `1 moved`; moved back before the old
folder was looked at, the sibling reported the move and the old
folder had nothing to say. A `day1` folder of two raws deleted from
under a tree: the tree pass reported `2 missing`, the listing dropped
them, `missing:yes` listed them, a pass over the deleted folder
itself was `0 files` and not an error, and `--prune` forgot two. A
raw copied to a laptop folder with its drive then renamed away was
`1 added, 0 moved`, and the drive back was `1 unchanged` with both
rows standing; the same with the mount point left behind as an
empty folder was `1 added` on the laptop and, over the empty mount
point, `unavailable 1`, nothing missing and `--prune` forgetting
nothing. Two passes on two folders of 4,200 and 2,100 hard links to
21 CR3s, started together on a library not there yet, both finished
with all 6,300 rows, forty-eight pairs without a failure. Alone, on
a fresh library, the 4,200 index in 1.92 s and the 2,100 in 0.89 s
(0.45 ms a file, the files page-cached), and a second pass over
either is 10 ms as a process; with the move lookup asking the disk
about every one of the 200 rows that share each file's hash, the
same two passes took 40 and 9.7 s.

Size: the 35 rows are 61,440 bytes, which is page granularity. A
thousand synthetic frames (small TIFFs with real EXIF through the
export writer, half with sidecars carrying two keywords, paths of
about seventy characters) come to 868,352 bytes after a checkpoint,
868 bytes a row with the three indexes, the keyword table and the
sidecar hashes: about 87 MB for a hundred thousand frames. That pass
indexed the thousand in 20 ms and re-indexed them in 3 ms, and a
four-term query over them ran in 0.2 ms.

**What is left.** The filter bar takes the language and the API and
needs one thing the index does not yet have: facet counts, the
number of files a chip's value has among the files the other terms
leave (§133's rule), which is one `GROUP BY` a facet and belongs
with the bar. Roots — which folders make up the library, the
all-roots view, the watcher and the mtime pass on launch — are the
next roadmap line; `index_tree` and `folders` are their start. The
thumbnail cache rekeyed by hash reads `Entry::hash`. rawler logs a
warning a file for lenses its table does not know, which the CLI
shows at the default level; the index does not need the lens table,
only the tag, and the noise is rawler's to quiet. Printing to a
closed pipe (`list --paths | head`) panics, as every CLI command
does; a `SIGPIPE` default is the binary's to set, not this crate's.

One thing to file upstream before the index runs unattended over
whole cards: a corrupt CR3 makes rawler 0.8's `raw_metadata` spin,
allocating to 29 GB of resident memory before anything stops it.
The repro is the first 300,000 bytes of the sample `5M0A3021.CR3`
with nine bytes changed — at offsets 46 (17 to 231), 188 (0 to 43),
196 (0 to 22), 250 (0 to 255), 301 (14 to 7), 306 (0 to 6), 347 (0
to 255), 385 (0 to 255) and 491 (0 to 255) — which a fuzz pass over
the samples' heads turned up; the editor's own open of the same file
would do the same, since master makes the same call. No guard is
built here: a decoder that must be watched from outside is rawler's
bug to fix, and the roadmap's rule is to contribute the fix upstream
rather than work around it in a fork. Until it is fixed, a
truncated or damaged CR3 on a card is the one way an index pass can
be brought down.

**The review.** Five passes, each finding something the tests had
not: a panic on a non-ASCII EXIF date that took the folder down; rows
under a deleted folder listed forever; a read-only listing that could
not open while an index ran; a copy on the laptop taking an unmounted
drive's row; two writers colliding, then starving; a mount point left
empty read as a deleted shoot; a per-pass disk cache that made
`index_file` on a copy steal the original's row; and a rating changed
and put back in the seam between the two phases, lost. The reviewer
also found, by reading the branch's binaries, that the first panic
fixture was 300 KB of a real portrait's preview with the camera's
serial numbers; the branch's history was rewritten so that blob never
existed. And it found rawler's 29 GB spin on a corrupt CR3 by fuzzing
the samples' heads. The numbers in this section are the reviewer's
where the two differed, taken on the quieter machine.
