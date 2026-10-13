# §309. Virtual copies (2026-10-12)

The roadmap's line: "named versions in the same sidecar, each with its
own edit and history; a collection references hash plus version."
Collections are built (§304) and left the reference a place for the
version. §53 said what a version is to the user: a picture of its own
in the strip, with a thumbnail, an export name and a history, where a
snapshot is a state kept inside one line. This is the design the build
follows, in the order at the end.

**What the others do, and what we take.** Lightroom's virtual copy
shares the file, carries a copy name, starts its history where the
master stood, and is stacked under the master with a page-curl mark;
"Set Copy as Master" swaps them; its meta is per copy; the export name
takes a Copy Name token. Capture One's variants are the same idea with
two makers, New Variant (default settings) and Clone Variant (the
adjustments), a primary variant, and a number appended on export.
darktable's duplicates are each a file: `IMG_0001.CR2.xmp` is version 0
and `IMG_0001_01.CR2.xmp` version 1, grouped in the lightbox, with a
duplicate (the history copied) and a virtual copy (blank). We take: two
makers (a copy and a fresh one), a name the user types, the swap, the
name in the export's file. We reject darktable's file per version (one
sidecar per frame is what `Sidecar::find`, the index, the write-behind
of §244 and the file type of §120 all stand on; a second naming scheme
would have to be taught to every one of them), Lightroom's and Capture
One's meta per copy (below), and the stack as the way a version shows
(below; stacks are a cross-file item of their own in the collections
file, §72).

Reviewed 2026-10-12 and taken as designed: the meta stays shared across
versions for now, and per-version marks stay out of scope until a
tester asks.

## 1. The data model

**Versions live in the one sidecar, as the roadmap has it.** The
sidecar's own line, `current`, `step`, `exported`, `id`, `history` and
`snapshots`, is version 0, the file's own version, and does not move
by a byte. A new top-level list, `versions`, holds the rest, each a
`Version`: a random `id` of 16 hex digits made when it is created, a
`name`, `made` (seconds since the epoch), and a line of its own: its
`current` edit, `step`, `exported`, `id` of the state, `history` and
`snapshots`, the same keys in the same shapes. In the code the line
becomes a struct, `Line`, flattened into `Sidecar` for version 0 and
held whole by each `Version`, so `record_as`, `undo`, `redo`, `go_to`,
the snapshots, `record_export`, `fill_ids` and the history cap are
written once over a `Line` and the sidecar hands out the line a version
id names (`line(v)`, `line_mut(v)`, `None` for version 0). The redo
stack is per line and not written, as now. §166 counted some 130 reads
of `sidecar.current`; they stay as they are, version 0's, and the paths
that act on a frame in the window go through the line of the frame's
version.

**What is shared.** Everything that is a fact about the file and not a
step in developing a picture, which is the rule §117 drew for the meta
and `turn` already: the meta (rating, flag, label, keywords, title,
caption), `meta_at`, the turn, the XMP mark, `saved`, `revisions` and
`absorbed`. Everything in the edit is the version's: the crop, the white
balance, the camera match, every look and mask. `last_export` is the
version's (§276 made it a fact about what went out, and what went out
is a version's picture), so each version's cell wears its own export
mark; the pencil is per version too.

*Why the meta is shared, against Lightroom and Capture One.* The XMP
beside the frame carries one rating; the filter bar and the facets ask
the file's row; the rejects move moves the file; a keyword names what
is in the picture, which a version does not change. A rating per
version would put two ratings on one file in the XMP and the index,
and X on a version while comparing would have to mean something other
than "reject the file". Choosing among versions is done by deleting the
losers or swapping the winner into version 0, not by rating them. The
cost is that a user who wants "this version is the one" has to say it
with the swap; Lightroom users will look for a per-copy flag and not
find it.

**Identity.** A version's id is random, 16 hex digits, made once, kept
through a rename, a swap and a join. Not the hash of anything: a name
changes, the edit changes, and two machines making a version each must
get two ids (a hash of the first state would give one id to two
versions made from the same original and then edited apart). Version 0
has no id and needs none; it is the file. A collection, the index, the
export record and the window name a version by (file, id).

**Making one.** "New version" copies the source version's current edit
as the new line's first and only state, labeled "From <source's name>"
("From the original" for version 0), with no history, no snapshots and
no export records: a history is fifty whole edits and a mask-heavy one
is megabytes (§233), and the new line's story starts here. "New version
from the original" starts from the default edit (`Edit::for_picture`
for a picture file), Capture One's New Variant. The default name is
"Version N", N the count of versions plus two, so the first one made is
"Version 2". "Swap with the original" exchanges the two lines whole
(edit, history, snapshots, exports, `last_export`) and the version
keeps its id and name; nothing else in the file moves, and the strip's
cells trade pictures. Deleting a version removes it from the list, a
save and nothing to undo: the delete sheet asks, in the version's words
("Delete the version Warm of IMG_0001.CR3? The file and its other
versions stay"). Version 0 cannot be deleted as a version; Delete on it
is the file's delete, as today.

**The schema, and the two directions of reading.** `versions` is read
loosely like every optional field: a list that is not a list is none, a
version that will not read costs itself and is said in the log. An old
sidecar has no `versions` and reads as a file with version 0 alone,
which it is; no migration. The other direction is the one that matters:
an older build reads a sidecar through `#[serde(default)]` structs that
pass over unknown fields and drops them on its next save, which §166
accepted for a label and §243 for ids. For a version that is an edit
lost. So a sidecar that carries versions is written with its edits at
schema version 5 (`VERSIONS_SCHEMA`), and `migrate` accepts 5 as the
identity; a sidecar with no versions keeps writing 4 and is read by
every build as today. An older build refuses a version-5 edit whole
("schema version 5, this build reads up to 4"), and the editor treats a
sidecar it cannot read as edited and never writes a default over it
(§276's guard): the frame shows the camera's JPEG with the pencil and
the log says why, and nothing is lost. Presets carry no versions and
stay at 4. The alternative, raising `VERSION` to 5 for every write,
would lock 0.6.x out of every sidecar 0.7 touches, with versions or
without, which is the cost §166 refused to pay for a label. The one
oddity accepted: an edit's stored `version` field is in the bytes a
state id hashes, so the same step recorded on a frame with versions and
on one without gets two ids; ids are never recomputed from a file, and
the join's content match (§243 rule iii) compares edits, not ids.

## 2. The index

**One row per file stays, and a `versions` table is added** (schema
10): `file` (the row's id, cascading), `version` (the id), `name`,
`made`, `position`, `edited`, `exported`, `develop_key`, keyed by (file,
version). The files row keeps version 0's `edited`, `exported` and
`develop_key` as it does. The pass that reads a sidecar (`summary_of`)
fills both from the one read; the migration clears the sidecar hashes
as schema 8 did, so the next pass over each folder fills the table. The
alternative, a row per version in `files` with a `version` column in
the key, lost because `by_path`, `by_hash`, the rehash heal, the move
detection, `archive_writes` and `pending` all take a path to be one
row, and a file's size, mtime, hash and EXIF would stand twice.

What each consumer does with it:

- The strip and the grid list a view as (row, version) frames: the
  row, then its versions by `position`. A folder's listing from disk
  (no index) reads each sidecar's version list as it reads the meta.
- The filter bar and the facets filter rows; a row that passes brings
  its versions with it. A `has:versions` term in the language is cheap
  and goes in.
- The marks come from the row for version 0 and the table for the
  rest, through the same `develop_marks`.
- The develop key per version is the table's, read as the row's is:
  a first guess, the sidecar's key winning when it can be read (§285).
- The key cache changes by nothing: an entry is `<hash>-<size>-r<recipe>-
  <key>` and a version's key is its edit's, so a file with three
  versions holds three keys under one hash, as two copies edited apart
  do today (§278).
- `ids_by_hashes` goes on giving a collection's member rows; the mark
  is placed by the cell, which checks its (hash, version) against the
  collection's references (§7).
- The archive tables are per frame and unchanged: the sidecar is one
  file, written through and compared whole.

## 3. The strip and the grid

**A version is a cell of its own, right after its file's**, in the
file's place in the sort, each with its own picture, pencil and export
mark, its rating badges the shared meta's. Versions carry a small tag
at the top left of the picture with the name ("Warm"); version 0 wears
none, so a folder without versions looks as it does. The grid's label
under the cell reads the file's name for version 0 and the version's
name for the rest. A stack (one cell, a count badge, expand to see)
lost: comparing versions side by side is the main use, the stack hides
it, and the grid's cells are already keyed by row and need no new
widget. Opening a folder lands on version 0 as today; the arrows walk
every cell, versions included. Shift-arrows and the rubber band select
versions as frames. The culling keys act on the shared meta and every
cell of the file follows.

In the window the list's unit becomes a frame = (file index, version
id). `files` and `sidecars` stay one per file; the browser's list is of
frames, each pointing at its file, and the sidecar in memory is held
once per file so two frames of one file never race their saves. A save
of any of a file's frames saves the file once, one revision.

**The frame menu** gains, under Copy and Paste: "New version"
(Ctrl+', Lightroom's key), "New version from the original", and on a
version "Rename version..." (the collections' name sheet), "Swap with
the original", and "Compare versions" (§4). Delete on a version opens
the delete sheet with the version's words. The history panel's head
says which version is open ("Warm" under the file's name, nothing for
version 0); the panel's rows, snapshots and undo are the line's.

## 4. Culling and compare

Versions are adjacent rows, so compare already works: C then V with the
anchor on the file puts the original and its versions side by side,
each tile the 2048 picture of its own edit by its own key (§278's loupe
rule), the camera's JPEG where a version's picture is not made yet.
"Compare versions" in the menu enters culling at two or four up
(capped at four, the compare grid's most) anchored on the file, so one
click does it. The shot lines show the focused frame's; the plate names
the version ("Warm, the edit, 2048 × 1365") so two tiles of one file
can be told apart. Delete under compare on a version is the version's
delete, which is how the loser goes.

## 5. Thumbnails

The develop key is the edit's and the cache is keyed by hash and key, so
a version's pictures are kept and found with no change to the cache. What
moves to the frame: `Edited` holds its wants by path today and holds
them by (path, version) instead; `Job::Keep` from the open frame carries
the version, as does `set_open_frame`; `Thumbs::remove_tag` on a save
takes the frame's own old key, which is per version (two versions
sharing a key, both at the same edit, are the two-hard-links case §278
left to the eviction, and are left so). The reduced develop's queue
(§285) is fed through the same hook, `sidecar_written` naming the frames
of the file that moved: a paste onto a version, a join that brought a
version from another machine, and a folder opened cold with versions in
its sidecars each queue the version as a frame, visible cells first.
`Thumbs::newest` for an offline root takes the table's key per version,
so an offline file's versions each show their own picture.

## 6. Export

**The version's name joins the stem.** `named` takes a frame's version
name and makes `<stem>-<name>.<ext>` (`IMG_0001-Warm.jpg`), the name
made file-safe (slashes, colons, quotes and control characters to `_`,
whitespace trimmed, empty to `v<N>`); version 0 keeps `<stem>.<ext>`.
Everything after that is as §301 has it: beside the raw the infix goes
after the stem and the name, `IMG_0001-Warm.greycard.jpg`; "If it
exists" holds and Increment gives `IMG_0001-Warm-1.jpg`. A version
named "1" makes `IMG_0001-1.jpg`, the very name Increment would give the
original; `named` takes every name of the set before it numbers any, so
the pair sorts itself out, and the confusing name is the user's own.
`queue::Frame` carries the version id and name; the export is recorded
on the version's state and in its `last_export`; the queue's rows and
the status line say "IMG_0001 (Warm)". A set of several versions of one
file is several files, as it should be. The headless `--export` takes
`--version <name>` for the frames it names (version 0 without it); a
name two versions share exports the first by position and says so.
Exporting every version at once is not in this build (§10).

## 7. Collections

A reference gains `"version": "<id>"`, absent for version 0, so every
collections file written so far reads unchanged. A file holding any
reference with a version is written at the collections' `VERSION` 2 and
an older build leaves it alone, as §304's rule has it; one without stays
at 1. A collection with two versions of one picture shows two tiles, in
its order; dedupe on open is by (hash, version), each once. Resolving a
reference finds the file by §304's rules and then the version by id in
its sidecar: found, it is the tile; a version gone from the sidecar
(deleted on this machine or another) is missing, counted in the status
line as a moved file is and never dropped, since a join may bring it
back (§8). The hint is a path and unchanged; the heal by hash is
unchanged, the version id living in the sidecar and surviving a rewrite
of the raw. The mark: the window asks the collections for their (hash,
version) pairs, the index for the rows of those hashes, and a cell is
marked when its own pair is among them, so the original's cell is not
marked for a version in a collection. B and "Add to collection" add the
selection's frames as pairs; "Remove from" takes the pair. A smart
collection is a query over rows and lists a passing row's versions with
it.

## 8. Sync across machines

The comparison is the file's and unchanged: revisions are per sidecar,
so `Same`, `SameEdit`, `Behind` and `Diverged` are decided for the whole
file by §243's cases, and `Behind` copies whole. The join joins each
line: version 0's by the rules §243 built; a version on both copies (by
id) has its line joined by the same function, since a `Line` is what
the join already walks, and takes the newer copy's name when the names
differ; a version on one copy alone is kept, whether it was added there
or deleted on the other side, which is §243's "a join keeps rather than
loses", the same it says for a snapshot taken off. The list is ordered
by `made`. Two machines each making a "Version 2" of one frame get two
versions of that name with two ids, both kept; names are labels, not
keys, and the user renames one. The write-behind, the pending list, the
catch-up and the follow (§244) move the one file and know nothing of
versions. The simulation gains versions among its moves (made, renamed,
edited, deleted, swapped) and the check that no version of either input
is lost from a join.

## 9. The copy sheet and presets

Nothing new to design. The selection is of frames, so Ctrl+C on a version
copies its edit, Ctrl+V onto versions pastes onto each, a sync from a
version onto the file's other versions is a sync onto frames, and a
preset over the selection lays on each frame's line; "Check all" is
over sections and parts and knows no frames. One file whose three
versions moved in one paste is saved once with one revision and three
labeled steps, one a line. A paste onto frames of one file the index
listed as rows standing in reads the sidecar once (`rows::take`) and
takes every version with it.

## 10. Out of scope, and the order

Not in this build: per-version meta; stacks; exporting every version
of a frame in one go, headless or from the sheet; a version in the
lightbox (the lightbox is itself not built); Lightroom's `.lrcat`
virtual copies in the import (§79); writing a version's meta or edit to
XMP (the one XMP carries the shared meta, as now); "Make this the
original" by any means but the swap; a version made in culling mode
(the panel is a stand-in there, §280).

The parts, each landable alone:

1. **The edit crate.** `Line`, `Version`, `versions`, the loose read,
   the conditional schema 5, the per-line join, the simulation's
   moves, and `greycard presets`'s listing naming a sidecar's versions.
   For a tester: a sidecar with a `versions` list written by hand opens,
   saves and keeps it, and an older build refuses it instead of
   emptying it.
2. **The index and the window.** Schema 10, the frames list, the
   cells and their tag, New version (both), Rename, Swap, Delete, the
   menu and Ctrl+', the history panel's head, the thumbnails per
   version, the reduced develop's queue. For a tester: Ctrl+' on a
   frame makes a second cell beside it with its own edit, name and
   thumbnail, which survives a restart and a move of the folder.
3. **Compare.** "Compare versions" and the plate's name. For a
   tester: Compare versions on a frame with two versions shows the
   three side by side, each its own look, and Delete on a tile removes
   that version.
4. **Export.** The name, the queue, the records, `--version`. For a
   tester: exporting a frame and its version Warm writes `X.jpg` and
   `X-Warm.jpg`, and each cell wears its own export mark.
5. **Collections.** The reference's version, file version 2, the mark
   per cell, dedupe, B. For a tester: adding a version to a collection
   shows that version's tile in it and marks that cell alone.
