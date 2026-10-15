# §317. Virtual copies, part 2: the index and the window (2026-10-14)

Part 2 of the virtual copies design (§309), built on part 1's `Line` and
`Version` (§312) and part 4's export naming (§313): the library's versions
table, the window's cells, the makers' menu, the thumbnails per version
and the reduced develop's queue for them. Parts 3 (Compare) and 5
(collections) are still to come.

## The index

Schema 10 adds a `versions` table, a row a version: the file's row id,
the version's id, its name, when it was made, its position in the
sidecar, and the marks and develop key the files row keeps for version
0 (`edited`, `exported`, `develop_key`). The pass that reads a sidecar
fills both from the one read; the migration clears every sidecar hash,
as schema 8 did, so the next pass over each folder fills the table.
`RowMeta` carries the versions as `versions: Vec<VersionRow>`, read
with the meta in one query through a `json_group_array` subquery whose
`ORDER BY v.position` sits inside the aggregate, so the order is the
sidecar's and not the planner's. `has:versions` joins the filter
language. The salvage from the first build of this part, taken whole.

## The model: files once, cells many

The first build of this part kept a slot per cell with each version's
slot holding a swapped copy of the whole sidecar, kept in step by a
spread after every change. We rejected it: eight ways for the copies
to disagree were found before it shipped, and every one was a class of
bug the model invited. What we built instead:

- What is a fact about a file the window keeps once per file, as on
  master: `files`, `sidecars`, `from_row`, `seed_blend`, `index_ids`,
  `index_passed`, `trouble`. One parsed sidecar a file in memory.
- Over those it lists cells: `State::cells: Vec<Cell { file: usize,
  version: Option<Arc<str>> }>`, in display order, a file's own cell
  and then one for each version its sidecar keeps, by position. The
  strip, the grid, the selection, the arrows, the rubber band, the
  culling keys, the filter and the lightbox walk cells. `shown` (the
  browser's rows), `picked` (the selection), `current`, `menu_file`,
  `shown_turn` and the thumbnail vectors are by cell.
- A cell reaches its file's facts through `State::file_of(c)`,
  `path_of(c)`, `sidecar(c)`, and its own line through
  `State::line(c)` and `line_mut(c)`, which are
  `sidecars[cell.file].line_mut(cell.version)`. `line` is an `Option`:
  none when the sidecar no longer has the version (a join or a read
  took it out since the cells were made), and every caller says what
  it does then (skips the cell, refuses with a word), so no path acts
  on a line that is not there and none falls back to the file's own
  by mistake.
- The cells are made from the sidecars (`versions::cells_of`) and
  made again when a file's versions change: a maker, a join, a read
  that found other versions than the row listed, a row from the index
  that lists others (`versions::settle`). A relist carries each cell
  that stays (its picture, its sizes, its place in the selection) to
  its new number, as a merge of the list does; a cell gone takes
  nothing with it; the frame on screen whose version went gives its
  place to its file's own cell, opened as a click on it would be.
  Where the caller holds cell numbers it goes on using (a read landing
  under a request, a row from the index, a join), the relist is asked
  for the next turn of the event loop (`settle_later`) rather than
  done under it.

The stand-in a row makes for a sidecar not yet read lists the row's
versions too, each with the default edit, so the cells are the same
whether or not the sidecar is in hand, and a version's cell stands in
with the version's own marks and key (`FromRow::marks`,
`FromRow::versions`). A read that lands with other versions than the
row listed moves the cells at the next turn and writes nothing back.

### `current` is a cell index

We chose a cell index over `(file, version)`. The strip, the grid, the
selection and the browser's rows are all index spaces over the one
list in display order, and `row_of` is a binary search in it; keeping
`current`, `picked` and `shown` in that space leaves the selection
arithmetic (`selection.rs`, pure over `usize`), the arrows, the rubber
band and every row-to-frame map as they were. A `(file, version)` pair
would have put a lookup on every one of those paths and a second
representation of "the frame on screen" beside the row it is on. The
cost is that a cell index and a file index are both `usize`, and the
compiler does not tell them apart: every site that indexed a per-file
vector was visited by hand and classified (below), and the accessors
on `State` name which kind they take.

### Where the sites went

Of the window's accesses to the per-file vectors outside tests (some
490 of `files[`, `sidecars[`, `from_row[`, `seed_blend[`,
`index_ids[`), 58 now go through a cell's line (`State::line`,
`line_mut`) and 196 through the cell-to-file accessors (`file_of`,
`file_at`, `files_of`, `own_cell`, `cells_of_file`, `sidecar`,
`sidecar_at`, `path_of`, `path_at`); the rest index by file where the
index already was one (the list's landing in `roots::merge`, the
archive writes in `sync.rs`, the index's rows, the rename of a look).

Per line (the cell's own): the develop's edit and the panel
(`open_frame`, `pick_pending`, `save_edit`, `record_panel`,
`take_sources`, `take_current`), the history panel (rows, undo, redo,
a click, the snapshots, their names and removals), copy and paste, a
sync and a preset (`lay_over_targets` lays `apply_preset_to_line` on
each cell's line), the export's edit (`set_frames_of`) and record
(`record_export` on `line_mut(version)`), the thumbnail turns, the
marks (`frame_marks` through `greycard_library::line_marks`), the
seed's landing (`Outcome::Opened`), the culling loupe's edit for a
control reached in the mode.

Per file (shared by every cell): the meta and the culling keys
(`set_meta` over `files_of(cells)`, written once a file), the turn
(`turn_frames`, once a file by the panel's own cell), the XMP, the
save (`write_file`, `save_file`: one write, one revision, whatever
cells changed), the archive write-behind and the join, the look rename
and removal (a file counts once however many of its lines name the
look), the delete of a file (takes every cell), the rejects' move and
move back, the filter and its chips, the facets' counts, the index's
ids and passes, the collections' marks (the file's own cell wears it
until a version can be put in one).

The functions whose argument changed kind are named for it:
`rows::is_loaded`, `is_offline`, `load_frame`, `writable`,
`write_sidecar`, `save_sidecar`, `delete::held`, `say_offline_for`
take a cell; `file_is_loaded`, `file_is_offline`, `load_file`,
`file_writable`, `write_file`, `save_file`, `held_file`,
`say_offline_for_file`, `show_badges_of_file`, `refresh_file` take a
file.

### Ambiguous sites, and what we chose

- `Clipboard::from` was a path; a copy from a version and a paste back
  onto the file (or the other way) must not leave the target out as
  "the frame it came from", so `from` is a `versions::Key` (path and
  version), and the paste leaves out that frame alone.
- The ISO's seed (`Outcome::Opened`'s blend) landed on the open frame's
  line; a version copied from the not-yet-opened raw is at the same
  default and would otherwise keep the plain default for good. The
  seed goes to the open cell's line and to every other line of the file
  whose edit equals it before the seed, which is the copy and nothing
  edited. `take_seed` (the export's) does the same.
- `turn_frames` over a selection holding a file and its version turns
  the file once, from the panel's own cell when that is one of its
  (`once_a_file`).
- `frame_aspect`, `stance`, `camera_tag`, `migrate_frame` take a cell:
  the develop on screen, the culling cache and the thumbnail are the
  cell's, the file's stance is read through the cell's path.
- The lightbox's pictures, waiting and failed sets were by path; by
  `Key` now, so two cells of one file show their own.
- The loupe's `Want` and `Loaded` name the frame (`Loaded::frame: Key`
  in place of `path`), so a stale decode cannot land on a renumbered
  cell of the same file.
- `thumb_turns` for a version standing in from its row takes the file's
  row turns: the row does not carry a version's geometry, and the
  sidecar's own answer wins once read.
- The frame menu's Copy takes the cell right-clicked (a version's
  line); its Delete version acts on the cell right-clicked and not on
  the selection; the Delete key over a selection of versions alone
  opens the versions' sheet, over a mixed selection the files' sheet
  (a file's delete takes its versions).
- The look-removal sheet counts files: any line naming the look counts
  the file once, the panel's edit standing for the open cell's line.
- A paste or a preset over a set whose current frame shares its file
  with a target: the step recorded on the panel's line goes to disk with
  the set's write, and `take_current_with(.., false)` shows it without a
  second revision (`Synced::written`, `wrote_file_of`).
- `copy_settings_of` guarded the frame by `files.len()`; by `cells.len()`
  now (the test for a paste onto three cells of one file found it).
- The clipboard keeps the copied frame's name ("IMG_0001.CR3 (Warm)")
  from the copy, beside its `Key`: the status line and the paste's
  history row say the version by name, and a rename after the copy does
  not move the label of a step already recorded.
- A frame a request named that is gone when the request runs is said by
  its file's name, not by its version's id.

## Thumbnails and the reduced develop

The thumbnails' pool, the edit's pictures (`Edited`) and the reduced
develop (`Reducer`) are keyed by frame, `versions::Key { path, version }`,
in place of the path: `Job::Thumbnail`, `CachedThumbnail`, `Keep` and
`MasksFor` carry the frame, `Outcome::Thumbnail`, `NoThumbnail` and
`EditedKept` answer with it, and the window checks a landing against
`key_of(index)` so a picture for a renumbered cell is dropped. `Edited`
holds its wants by frame; its read of a sidecar for a key takes the
frame's line (`read_key`), and a version gone from the sidecar reads as
no edit. The reducer develops a version's line (`make_under` takes
`sidecar.line_mut(frame.version)`), so a version edited by a paste, a
sync, a preset or a join has its pictures made behind the thumbnails as
a file's are, and its cell never shows the camera's picture wearing the
pencil for longer than the reduced develop takes. The pool's previews
stay by path (a preview is the file's), and a thumbnail of a version
queues the file's preview under its own frame's number. The cache
changes by nothing: a version's pictures sit under the file's hash and
the version's edit key, as two edited copies of one file already do.
`Job::Open` carries the version so the worker's open frame is the cell
and not the file: the reducer may develop the other lines of the open
file.

A save of a file says `edited::saved` for every cell of it: the open
cell's `Keep` from the develop in hand, each other cell's picture asked
again by its own line's key.

## The makers, the sheets, the marks

New version (Ctrl+', and the frame menu under Copy and Paste), New
version from the original, and on a version's cell Rename version...,
Swap with the original and Delete version..., each through the edit
crate's makers (§312) on the file's one sidecar, written once, and the
cells settled at once. A new version's cell is opened as a click on it
would, and its name asked on the collections' name sheet, the field
holding "Version N", all of it chosen, so typing names it and Escape
keeps the name. The delete sheet asks in the version's words ("Delete
the version Warm of IMG_0001.CR3?", "Delete 2 versions?") with a
"Delete version" button in the trash button's place; the sheet's answer
deletes several at once and writes each file once. The strip's and the
grid's cells wear a name tag at the top left (`ThumbTag`), the grid's
label reads the version's name, the history panel's head names the
version open, the frame menu knows the version under the pointer
(`menu-version`), and the set's lines name a version's frame "IMG_0001
(Warm)" (`queue::label`, in the failed and the not-there lists). Each
cell has its own pencil and export mark from its own line
(`line_marks`); the rating badges are the file's meta on every cell.

## Departures from §309

- The design has `Edited` hold its wants by `(path, version)` and the
  reduced develop queued "through the same hook, `sidecar_written`
  naming the frames of the file that moved". We name every cell of the
  file at a save rather than the lines that moved: a cell whose line did
  not move is a cache hit asked again, cheap, and the hook needs no
  knowledge of which lines a save carried.
- The index's `versions` subquery orders inside the aggregate, which
  the design did not say; the planner is otherwise free to reorder.
- "Compare versions" in the frame menu is part 3's and not here; the
  menu has the five items of §309 §3 without it.
- A collection marks the file's own cell alone (§309 §7 is part 5): a
  version cannot be put in one yet, so its cell wears no mark.
- `Delete version...` from the menu acts on the cell right-clicked, as
  the brief asked, where §309 says only "Delete on a version opens the
  delete sheet".

## Review, five rounds

### First round

The review found the model sound and three defects, each reproduced on
a copy of the tree and closed here:

1. *False success onto a vanished version.* A paste, a sync or a preset
   onto a version cell whose line a read had just found gone said "the
   1 frame had these already". `rows::resolve` now answers `None` for a
   cell whose `line(c)` is none, so the request's `say_gone` names the
   frame as gone ("IMG_0001.CR3 (Warm) moved or is gone; the paste was
   not applied to it"), the action never sees the cell, and nothing is
   laid on it, written or claimed. One test drives all three entry
   points (`a_paste_a_sync_and_a_preset_onto_a_vanished_version_say_so`).
2. *Version 0 losing its kept pictures.* `Edited::settle_key` and
   `reset` took every picture under a frame's old key out of the cache
   when the frame moved to another key; after a New version's first save
   or a swap the old key was still the other line's. `wanted_elsewhere`
   asks whether any other held frame shows or is held under that key
   (`Want::Key` or the row's guess), and the removal is skipped while one
   does (`a_key_another_frame_shows_keeps_its_pictures`).
3. *The ISO seed per file and version 0's.* `State::seed_blend` (a bool a
   file) and `rows::Held::seed` are gone. The wait for a seed is a line's
   fact, read from the line: `rows::wants_seed(st, cell)` is true for a
   raw whose line was never developed (`files::line_never_developed`;
   the row's marks while the sidecar stands in), and
   `rows::seed_landed(st, cell, blend)` puts the blend on that line
   alone. The open job, the export set, the sync's and the preset's lay,
   the export record (`take_seed` by line, never for a picture), the
   launch preset (`preset_at_start` reads the sidecar's own wait) and
   the headless export (the named version's line) all ask the line.
   A version with a strength of its own keeps it when the file's seed
   lands; a new version from an unseeded original waits and is seeded
   on its own line when it is the one open
   (`the_seed_lands_on_the_open_line_alone`,
   `an_export_s_seed_goes_to_the_line_exported_alone`).

The minor list: `thumb_turns` for a version cell whose line is gone
uses the turn alone, not the file's own geometry; `Key` carries the
version's name beside its id (no part of its equality or hash), and
`said()` says the name, the id only when none was had; a mixed
selection's Delete leaves the version cells out, as before, and now
says so in the status line ("2 versions left out: a file's delete takes
its versions with it; a version is deleted from its own cell") — left
out rather than deleted, since the files' sheet asks about the trash
and a version goes by a save, and one sheet should not do both; the
parameters that take a cell are named `cell` (`lightbox::show`,
`frame_aspect`, `copy_settings_of`, `cull_select`, `start_placeholder`,
`placeholder_arrived`, the loupe cache's and `placeholder::Wait`'s).

### Second round

The review of the fix round ran the gates clean and found one
regression from it and one guard too broad, both closed here, and
closed a part 4 fault in `headless.rs` while it was ours:

1. *The culling keys lost on a vanished version.* The first fix made
   `rows::resolve` answer `None` for a cell whose line was gone, which
   dropped the rating, flag and label keys, the turn and the tags' undo
   on such a cell too, though the meta and the turn are the file's (§309
   §1). A request now carries a `rows::Scope`, `Line` or `File`, named at
   every call site: paste, sync, preset, export, the export queue and
   the open of a frame are line actions and resolve a vanished version
   to nothing; the culling keys, the turn and the tags' undo are file
   actions and resolve it to the cell while the cells have not
   followed, else to the file's own cell. `load_frames` keeps a file's
   cells whatever their line, since a line action asks the line itself
   (`a_culling_key_on_a_vanished_version_cell_rates_the_file`). The
   departure this was is gone.
2. *`wanted_elsewhere` too broad.* It ignored the path, so another file
   synced to the same develop key kept this file's old pictures in the
   cache, and it counted a frame's row guess after that frame had found
   its real key. It now asks for a frame of the same file, showing the
   key or still to read its own with the row guessing it
   (`another_file_at_the_same_key_keeps_only_its_own_pictures`; the
   version 0 test stands).
3. *`--preset` with `--version-name`* laid the preset on version 0 and
   exported the version without it. `preset_at_start` takes the version
   and lays on its line; the headless plan finds the version before the
   preset goes on (`a_preset_at_start_lays_on_the_version_named`).

The nits: `load_sidecar_said` no longer computes a seed (the test
helpers compute theirs); `show_history` with the open cell's line gone
clears can_undo and can_redo with the rows; Swap on a version the load
found gone says so; the panel takes the ISO's blend only when the line
took it; and a frame has one label form, the design's: the file's name
for the file's own frame and the stem with the version's name for a
version, "IMG_0001 (Warm)", in `Key::said`, `frame_name` and
`queue::label_of` alike (the file's own keeps its extension because a
file line needs it to tell IMG_0001.CR3 from IMG_0001.JPG).

### Third round

The landing review found two medium defects and two low items, all
closed here:

1. *A noted line losing its pictures.* A save says `edited::saved` for
   every cell of the file: the open cell's `Keep`, every other cell's
   `note`, which leaves that line's key to be read again. The guard on a
   key's kept pictures counted only a found key or a row's guess, so the
   open cell's move (a New version's first edit, a swap on screen) took
   the noted line's pictures when its thumbnail had not read its key
   yet. `wanted_elsewhere` now counts a frame of the same file whose key
   is not found yet (to be read, with or without a row, or unreadable)
   as possibly showing the key
   (`a_noted_line_keeps_its_pictures_when_the_open_one_moves`).
2. *A missing sidecar taken over a version.* The guard that refuses the
   default over a frame standing in asked only version 0's marks, so a
   file whose own line was default but which had an edited version took
   the default when the `.gcd` was not found at the read, and the next
   save would have written a sidecar without the version over the real
   one. `rows::take` refuses when the row lists versions too: a version,
   edited or not, is data
   (`a_missing_sidecar_with_a_version_on_record_is_not_taken`).
3. A turn from the panel on a version cell whose line a read found gone
   showed the default edit and started a develop; the panel branch of
   `turn_frames` now asks for the line, and the file turns as any
   other's does (the vanished-key test asserts the panel's edit and the
   generation stand).
4. A queued export of a version deleted before the run wrote the picture
   from the stored edit and dropped the record on a log line; the run
   now leaves the frame out of the entry at its start and says
   "IMG_0001 (Warm) was deleted; not exported"
   (`a_queued_version_deleted_before_the_run_is_left_out_and_said`).

### Fourth round

The review of the third round's fixes found one blocking defect in the
fourth item and one imprecision in the first, with a probe each:

1. *An unreadable sidecar read as a deleted version.* The run's check
   on a queued version asked the sidecar on the window's thread, and
   took any failure to read it (a later build's file, a corrupt one, a
   share that cuts the read short) for the version gone, dropping the
   frame from the entry for good. The check moved into the probe that
   already looks for the files off the window's thread, and its answer
   is three-way (`Presence`): there, deleted, or cannot tell. Only a
   sidecar that reads and lacks the id says deleted; one that will not
   read, or none where the version was, leaves the frame queued and
   counted with the files not there. For a file the window holds read,
   its own copy of the sidecar answers before the thread is asked. An
   entry whose every frame was a deleted version goes from the queue
   with them, and the run goes on at its place
   (`an_unreadable_sidecar_is_not_a_deleted_version`,
   `an_entry_of_deleted_versions_alone_leaves_the_queue`).
2. *The guard too wide.* The third round's `wanted_elsewhere` counted
   any same-file frame whose key was not found yet, so a version edited
   from one key to another kept its old pictures whenever the file's
   own line had just been noted by the save, which is every save. A
   held frame now keeps the key last found for it as a hint through a
   `note`; the guard counts a frame still to read its key only when its
   hint is the key in question, or it never had one. The file's own
   line at 5 keeps 5 and lets the version's 9 go
   (`an_edited_version_s_old_key_is_reclaimed`). The third round's
   three cases stand.
3. The recovery of a sidecar deleted on purpose was probed and found
   right: refused while the row lists the version, taken once the
   index's row says no sidecar and no versions; the probe stays as a
   test (`a_deleted_sidecar_recovers_when_the_row_follows`).

### Fifth round

The review of the fourth round confirmed the queue's three-way presence,
the emptied entry's removal and the run's continuation at its place,
and found one regression in the guard's hint: `note` carried the key
last found for the line, and nothing moved it when the line's edit
moved underneath. A swap trades the two lines' edits, so their keys
trade too; the save sends a Keep for the cell on screen and notes the
other line, whose hint was its key before the swap. When the Keep
settled the open cell on its new key, the guard asked the noted line
about the old key, the stale hint said not that one, and the pictures
the noted line now shows were removed. The same through a reset when
the open line ends at the defaults, and for any change to a line not
open that ends in a `note` (a paste, a sync, a preset onto a sibling).
`note` now takes the key the caller knows the line to be at: the save
computes it from the edit it wrote (`Edited::key_of`), as does the
worker when it lets an open frame go; a line changed on disk under the
window, whose row gives no key, passes none, which the guard reads as
"may show anything". A frame's hint is never older than its line
(`a_swap_keeps_the_pictures_the_other_line_now_shows`). A later build's
sidecar, read as far as the look can, is Unsure rather than Deleted
(`a_later_build_s_sidecar_is_unsure`).

## Tests

`versions::tests` (the cells from sidecars, the lookups, a relist
carrying the cells that stay); `panel::versions::tests`, twenty-two: the
cell after its file and the arrows; New version's cell, name sheet and
default name; Rename from the menu; Swap exchanging the lines and the
pictures; Delete after the sheet; two versions deleted at once, one
write; the menu's Delete version on the cell right-clicked; the panel on
a version editing its line alone; an export named for the version and
labeled "IMG_0000 (Warm)"; a folder from the disk listing versions and a
save keeping them; a roots view listing versions from the index with
their own marks; a version's picture asked by its own frame and held as
edited by its save; an export's record on the version's line marking
its cell alone; a culling key on a version marking the file and writing
it once; a paste onto a version and onto a file and its two versions
writing once; a read without the version taking its cell and writing
nothing; a row with other versions moving the cells; a paste, a sync
and a preset onto a vanished version saying so; the seed landing on the
open line alone; a culling key on a vanished version cell rating the
file. `sync::tests`: a copy without the version taking its
cell and writing nothing back. `edited::tests`: a key another frame
shows keeping its pictures; another file at the same key keeping only
its own; a noted line keeping its pictures when the open one moves; an
edited version's old key reclaimed; a swap keeping the pictures the
other line now shows. `panel::export_queue::tests`: a
queued version deleted before the run left out and said; an unreadable
sidecar not a deleted version; a later build's sidecar unsure; an entry
of deleted versions alone leaving the queue. `panel::versions::tests` gains a missing sidecar with
a version on record not taken and a deleted sidecar recovering when the
row follows (twenty-two). `headless::tests`: a launch preset laid on the version named.
`reduced::tests`: a version's line developed by its own frame, the
file's own apart. `clipboard::tests`: a version of the copied file is a
target, and a copy from a version is named for it. `edited::tests`: a
version held apart from its file. `panel::history::tests`: an export's
seed goes to every line at the default and not to one edited apart.
`queue::tests`: a set's line names a version's frame. The library's
tests from the salvage: two versions give two rows, a delete drops its
row, a schema-9 library gains the table.

Of the reviewer's probes against the first build, the two that still
mean something on this model are regression tests here (two versions
deleted at once; one key over a file and its version, one write); the
paste probe became the paste test's second half (one write for three
cells of one file); the stale-slot and in-place-seed probes have no
slots to be stale and no copies to lose a seed to, and the seed's rule
is tested on the one sidecar instead.

## Measurements

The UI crate's 1154 tests from before this part were green on the new
model before any of its own tests were added; with them, 1173. Clippy
with warnings denied over every target is clean. The whole workspace's
tests at the end: 2241 passed across 31 test binaries, none failed.

## Left open

- Compare versions (part 3) and the collections' reference (part 5).
- A version standing in from its row shows its file's row turns until
  the sidecar is read; the row could carry each version's turns.
- The culling loupe shows a version cell's own edit picture where one
  is kept; the plate's name for a version is part 3's.
- A collection's mark on a version's cell (part 5).
- The `settle_later` turn is a zero-length timer; a test runs it with
  `versions::settle_pending`.
- Two editors on one local folder are last-writer-wins at the sidecar
  save (the edit crate's `save_in`), which a version makes able to drop
  a whole line: pre-existing, and outside §309's scope.
