# §319. Virtual copies, part 5: collections (2026-10-15)

Part 5 of the virtual copies design (§309 §7), on part 2's cells (§317):
a collection's reference names a version, a collection lists its frames
as cells in its order, and the mark is per cell.

## The file format

A reference gains `"version": "<id>"`, the version's id in the sidecar,
left out for the file's own (`skip_serializing_if`), so every collection
file written so far reads as it was and a collection with no version in
it writes the same bytes. `VERSION` is 2 (what this build reads up to);
a plain collection is written at `PLAIN_VERSION` 1 and one holding any
reference with a version at 2 (`Collection::file_version`), so the bump
is per file and only when needed. An older build reads up to 1 and
leaves a file at 2 alone by §304's rule (logged, never written over);
taking the last version out writes 1 again. Smart collections carry no
references and stay at 1. The join of a file changed on disk (§304's
two editors) and its base are by (hash, version) pairs now.

## The model

- `Reference { hash, hint, version }`; a frame is a `Pair` (hash,
  version) and a `Frame` to add is (hash, path, version). `has`, `add`,
  `remove`, `reorder`, `take_hash` and the join act on pairs; the new
  `insert_after` puts a frame beside another.
- `resolve` dedupes by pair: two versions of one picture are two
  frames, a pair named twice is one. A version is looked for in the
  rows of its hash whose sidecar (as the index's versions table lists
  it) has the id: a version lives in the sidecar of the copy it was made
  on, not in every copy of the picture, so a reference to it is shown
  from that copy, its hint following there. None has it: missing,
  counted, kept, with the file it was looked for in (`Shown::lost`), or
  counted away when that file is under a root that is offline.
  `Shown::frames` is the frames in order (capture order sorts frames,
  not files); `Shown::listed` is each file once, in the order its first
  frame comes.
- The heal by hash is unchanged in kind: every reference of the old
  hash follows the file, whatever its version, the id living in the
  sidecar, which a rewrite of the raw leaves alone.
- The window: `State::collection_frames` (set where a read lands, from
  the read's frames; none for every other list) and `wanted_cells`. In
  a collection the cells are its frames whose file is listed and whose
  version the sidecar in hand has, each once, in its order; elsewhere
  each file's own and all its versions, as before. `list`, `relist`,
  `settle` and `rows::versions_differ` all ask `wanted_cells`. A file's
  cells need not stand together or start with its own, so
  `cells_of_file` returns a `Vec` (a scan in a collection, the binary
  search elsewhere), `own_cell` may be none, and `file_cell` (its own,
  else its first version listed) stands for the file where what is done
  is the file's (the XMP's turn, a culling key on a vanished version, the
  frame a list opens on, the tags' step).
- The mark: `Store::member_ids` maps each member row (by hash, the rows
  asked of the index a hash) to the set of versions of that picture in a
  collection, and `collected(st, cell)` is true when the cell's own
  version is in its row's set. So a version in a collection marks that
  version's cell alone, and the original's cell marks no version.
- B, Add to collection and Remove from act on the chosen cells' pairs
  (`frames_of` by cell, the hash asked once a file); the drag moves
  pairs.
- Forget missing: a version whose file is there is dropped only when its
  sidecar on disk reads and lacks the id (`export_queue::version_presence`,
  reused); one the sidecar has (the index behind it) counts as on disk,
  and one whose sidecar will not read, or is not there, cannot tell and
  is kept (`Forgot::unsure`, said "nothing forgotten: the sidecar of 1
  frame could not be read"). Its folder is looked at through the same
  bounded look first, and a network mount is never looked at.
- The status line counts the frames shown (cells), not files.

## Sites

- `collections.rs`: `Reference::version`, `Pair`, `Frame`,
  `PLAIN_VERSION`/`VERSION` 2, `file_version`, `load_reading`, pairs in
  `has`/`add`/`insert_after`/`remove`/`reorder`/`take_hash`/the join/
  `bases`, `Store::pairs`, `member_ids` per version, `resolve`'s pair
  dedupe and version lookup (`Shown::frames`, `Shown::lost`),
  `Change::version`, `forget_missing`'s version rule and `Forgot::unsure`.
- `versions.rs`: `cells_of_frames`, `State::wanted_cells`,
  `wanted_of_file`, `cells_of_file` as a `Vec`, `own_cell` in a
  collection, `file_cell`; `settle`, `relist` and `list` through
  `wanted_cells`.
- `main.rs`: `State::collection_frames`.
- `roots.rs`: the read carries the frames (`Found::frames`), `land` sets
  `collection_frames` from a read of the list held, and a merge settles
  the cells when the frames moved and the files did not; the forgettable
  count takes the lost versions' files.
- `rows.rs`: `versions_differ` against the wanted cells; a file action on
  a vanished version falls to `file_cell`.
- `panel/browser.rs`: the thumb's mark per cell; `open_loaded` puts the
  files in before the cells are made; `file_cell` for the frame to open
  and the tags' step.
- `panel/edit.rs`: the XMP's camera tag through `file_cell`.
- `panel/collections.rs`: `refresh_members`, `collected` per cell,
  `frames_of` per cell, `pairs_of`, B/Add/Remove/drag by pairs,
  `version_made`, the forget's words.
- `panel/versions.rs`: New version in a collection's view adds it there;
  Swap on a version whose original is not listed.

No `.slint` and no `menu.rs` change: the frame menu's Add and Remove
count the selection, which is cells already.

## Departures from §309

- The version is found by its id "in its sidecar" as the index's
  versions table lists it, not in the sidecar on disk: `resolve` asks
  the index alone and never the disk (§310's rule, and it runs off the
  window's thread with the rest of the read). The cells then follow the
  sidecar itself when it is read (`settle` drops a cell whose version a
  read lacks). Forget missing, which looks at the disk anyway, asks the
  sidecar on disk.
- "The hint is a path and unchanged": its kind is, but the copy a
  version is shown from can only be one whose sidecar has it, so a
  version's hint follows to that copy as a moved file's does.
- A version made while a collection is the list goes into the
  collection beside the frame it came from (Lightroom's virtual copy
  does the same). §309 does not say; without it the new cell would have
  no place in the list, and New version's open and name sheet would
  land on nothing.
- Swap with the original on a version whose original the collection
  does not list swaps the lines as ever; the version's picture is made
  again, there being no cell to trade it with.
- A smart collection needed nothing new: part 2's filter already brings
  a passing row's versions with it. A test holds it.
- The status line counts the frames shown, the cells, not the files: a
  collection holding a file's own and its version says "2 frames" for
  one file. The pane's count is the collection's frames, each (hash,
  version) once.
- A version the index does not list is still put among the frames the
  read hands the window, at the file it was looked for in, when that
  file is listed for another frame: the window makes a cell for it
  only while the sidecar it holds has the version. This keeps a version
  just made (index behind, or never caught up under `--no-sidecars`)
  through a merge read, and drops it once a read of the sidecar says it
  is gone. It is still counted missing in the status line meanwhile.

## Tests

- `collections::tests`: a reference with a version round-trips, the
  file is at 2 with it and at 1 without, the file's own writes no
  version key, a build reading up to 1 leaves a file at 2 alone and on
  disk as it was; two versions of a picture are two frames (add once
  each, hints apart, reorder, `insert_after`, remove by pair); a
  version found by its id in the copy whose sidecar has it, the hint
  following, the file listed once for two frames, a pair named twice
  shown once, a version the sidecar lacks missing and kept (`lost`), and
  counted away under an offline root; Forget missing drops a version
  only when its sidecar reads without it, keeps one the sidecar has
  (index behind) and one whose sidecar will not read.
- `versions::tests`: a collection's cells are its frames in its order,
  not standing together, a version the sidecar lacks and a file not
  listed making none, each once; the lookups and `file_cell`.
- `panel::collections::tests`: a version in a collection from the menu
  marks its own cell alone; B adds the file's own as a second frame, B
  on the version all in takes it out (the file back to shape 1) and in
  again; opened, the two tiles of one file in the collection's order,
  a duplicated reference shown once, Remove from on the version's tile
  takes the pair; a version deleted from its sidecar (another machine)
  is missing in the status line, kept in the file, offered to Forget and
  forgotten; a version made in a collection goes in beside its source,
  opened and marked; a smart collection lists a passing row's versions.
- The existing collections tests moved to frames with `None` and pass
  unchanged in substance.
- From the review (its probes, under real names): `versions::tests`, a
  followed frame keeps its cells in a collection;
  `panel::collections::tests`, a version made survives a merge before
  the index reads it, and a rewritten file's version is not forgotten by
  a copy; `collections::tests`, a frame named twice counts once.

## Review round

The review ran the gates clean and found three defects, each with a
failing probe, kept here as tests under real names:

1. *A followed frame lost its cell in a collection.* Following a frame
   to its archive's copy (`sync::follow`) changed the file's path in
   `files` alone; the collection's frames, keyed by path, no longer
   found it, `versions_differ` asked a settle, and every cell of the
   file went, a neighbor opening in its place. `State::move_file` now
   changes the path in `files` and in `collection_frames` together, and
   is the one writer of a listed file's path: `sync::follow`,
   `roots::follow_current` (safe before only because `land` had just put
   the read's frames in) and the rejects move's swap and swap back.
2. *A version just made vanished at the next merge read.* The read
   resolves by the index, whose row did not list the version yet, and
   the frames it handed over lacked it. `resolve` now puts a lost
   version among the frames, in the collection's order, at the file it
   was looked for in when that file is listed, and the window's cells
   keep it while the sidecar in hand has it (departures).
3. *A rewritten file's live version forgotten through a copy.* With the
   hint no longer among the hash's rows (the file rewritten in place, a
   copy of the old content surviving, no heal yet), the version was
   looked for in the copy, whose sidecar lacks it, and Forget missing
   dropped it. A version reference with no row of its hash keeping it
   now tries the file at the hint: when that row lists the id it is
   shown from it and the reference takes its hash; otherwise the hint's
   file is where the version was looked for, never another copy.

The small items: `settle` falls back to `file_cell` for the frame whose
version went; the pane's count and Remove's "N frames taken out" count
each (hash, version) once (`Collection::count`, `remove`), and `resolve`
counts a pair named twice as missing once; the departures and the
measurements below say what the review asked them to.

## Measurements

The UI crate's tests before this part: 1194 passed, 19 ignored. After:
1203 passed (nine new), 1206 after the review round (its three probes
as tests). The whole workspace after the round: 2257 passed across 31
test binaries, none failed; fmt and clippy --all-targets -D warnings
clean.
Nothing timed: the
mark now asks the index one query a member hash, as `ids_by_hashes`
already did inside; `cells_of_file` is a scan of the cells only while a
collection is the list, and a collection is hand-made and small.
`wanted_of_file` makes every wanted cell again on each call in a
collection, so a loop over files asking `versions_differ` (the look
rename's, look_rename.rs) is quadratic in the collection's size; for
collections of tens or hundreds of frames that is nothing, and we
accept it for now. A per-file map built once per loop is the fix if a
large collection ever makes it show.

## Left open

- A version made and added to a collection before the index has read
  the save keeps its cell (see departures) but is counted missing in the
  status line until the index catches up and the collection is read
  again.
- A version whose sidecar, once read, lacks it loses its cell but the
  status line's missing count is the open's, until the collection is
  opened again.
- Deleting a version keeps its references, missing, until Forget
  missing (as §309 asks: a join may bring it back). The review judged
  that the delete sheet saying the version is in a collection (and
  which) would help; not built here.
- A frame on screen kept in the list by a merge although the collection
  no longer names it has no cell (the merge's "stays in the list" rule
  is a folder view's).
- A pair named twice in the file is shown once and left twice in the
  file; nothing compacts it.
