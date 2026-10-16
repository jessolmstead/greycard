# §320. Virtual copies, part 3: Compare (2026-10-15)

Part 3 of the virtual copies design (§309 §4, §10 item 3), on part 2's
cells (§317). Commits 3f958f3 (the plate and the tile names), 6dddf4b
(the menu item, the compare set, the delete in culling), a23cbd8 (the
set made exactly the file's cells, three up for two versions), a3f22e1
(the first review round), the merge of master with part 5 (b2aa4d2),
8a8295a (the second review round) and 69c5e5d (the third).

## What we built

**Compare versions** in the frame menu, under Delete version..., offered
when the cell right-clicked is a file with versions or one of its
versions (`menu-compare`, set in `menu::menu_asked` from
`versions::compare_set`). It enters culling (or, already culling, changes
the view) on the file and its versions and no other frame: two up for a
file with one version, three up for two (the compare grid's 2×2 with the
fourth tile empty, which `tile_grid(3)` already drew at a folder's end),
four up for three or more. The view's tiles are those cells by name
(`Cull::set`), in list order wherever they stand: in a folder they are a
run, in a collection (§319) they may stand apart with other frames
between. The focus is the cell right-clicked when
it is in the set, else the file's own; the selection collapses to it, so
Delete there is that one frame's delete. A file with four or more
versions compares the file and its first three versions, and the status
line says how many were left out ("1 more version of IMG_0001.CR3 left
out"); the culling status keeps that note after its own words
(`Cull::note`).

**Three up is Compare versions' alone.** V's cycle stays 1, 2, 4; from
three it goes to four, the next step after two. The compare field in the
culling section offers 3 only while the view is at three (its options
read `["1", "2", "3", "4"]` then, `["1", "2", "4"]` otherwise), and a
press on 3 changes nothing. `--cull-compare` and `enter_cull` still take
2 or 4.

**The view stays fitted to the file.** `Cull::versions_of` keeps the
file's path while the view is a versions compare. Every caller of
`relist` with a culling view (`versions::settle`, the list's merge in
`roots::merge`, `panel::cull::drop_files`) takes it before the relist
(`versions::compared_file`) and fits the view to the file's cells after
(`versions::refit_compare`, then `fit_compare`). So a delete under
compare closes the set up on the remaining cells (three up becomes two;
with the last version gone, one up on the file), a join that brings or
takes a version refits, and a file dropped or listed before the compared
one moves the set with the file's cells; it never takes in a frame of
another file. The view stops being the file's compare
(`Cull::leave_versions`: the note and `versions_of` go, three up becomes
four) when an arrow or a click leaves the file's cells, when the file
goes from the list or is filtered out wholly, and when V or the field
changes the view (those set the count themselves).

**From the grid and the lightbox.** The menu's item leaves the lightbox
and closes the grid before it enters culling, so the compare is what
shows; both are done before the state is held, since they answer the
window's own callbacks.

**Each tile its own picture.** Nothing new: part 2 already asks the
loupe's pictures by frame (`cull::Want::of_version`), so a version's tile
is the 2048 of its own edit by its own key, and the camera's JPEG while
that is not made.

**The plate.** The status plate names the version before what the
picture is: "culling, 3 up: Warm, the edit, 2048 × 1365, fitted; Enter
develops" (`cull::plate_words`, from a `version` field on `Named`). The
file's own tile keeps today's words. The tile's own name tag at its top
reads the frame's label form, "IMG_0001 (Warm)", where it read the
file's name for every tile (`cull::tile_name`, through `Key::said`), so
two tiles of one file are told apart without the focus on them; a
version with no name says its id, as every other label does. The shot
lines in the side panel are the focused frame's, as they already were
(`cull_select`).

**Delete under compare.** A version's delete ran through the makers'
refusal, which refused anything while culling ("a version is made from
the editor"); the delete now asks only what does not concern the panel
(`unready`: a delete under way, the sidecar in hand). Delete on a
version tile opens part 2's sheet in the version's words; after a yes the
sidecar is written once, the cells settle, the file's own cell takes the
focus (settle's rule for a gone version), culling stays, and the view is
fitted to the file's cells again.

## Sites

- `panel/versions.rs`: `compare_set`, `compare` (the action),
  `fit_compare` (count, anchor, note, `versions_of`; the view left when
  the file is gone or filtered out), `left_out`, `unready` split out of
  `refused` for the delete, the `version-compare-asked` callback in
  `install` (the lightbox left and the grid closed first).
- `panel/menu.rs`: `menu_asked` sets `menu-compare` (one statement).
- `ui/panel/frame-menu.slint`: `can-compare`, `compare-versions`, the
  item after Delete version... (one block).
- `ui/app.slint`: `menu-compare`, `version-compare-asked`, the wiring
  with `menu-done`; V's cycle goes from 3 to 4.
- `ui/panel/cull.slint`: the compare field offers 3 only at three.
- `panel/cull.rs`: `Cull::note`, `Cull::versions_of` and
  `Cull::leave_versions` (called from `cull_select` when the set leaves
  the file, and from `relist`); `compare-changed` ignores a 3 and clears
  both itself; `Named::version`, `version_name_of`, `tile_name`,
  `plate_words`; `cull_status` uses the plate words and the note; the
  compare tile's name; `start_placeholder` asks the cell's version;
  `drop_files` refits.
- `versions.rs`: `relist` leaves the versions compare when the list is
  renumbered; `compared_file` and `refit_compare`, used by `settle`.
- `roots.rs`: `merge` refits; a test's `Named` gains `version: None`.
- `panel/viewport.rs`: the click on a compare tile takes its row from
  `Cull::rows`.

## Departures from §309

- **Three up.** §309 says "two or four up"; a file with two versions is
  three up, the 2×2 with the fourth tile empty, so the view named Compare
  versions never shows another file's frame. Three is this entry's alone:
  V and the field do not offer it otherwise, and the view leaves it for
  four as soon as it stops being the file's compare.
- **An explicit list of cells, not an anchor and a count.** A versions
  compare names its tiles' cells (`Cull::set`); V's views are a run of
  rows from the anchor as before. `Cull::rows` is the one maker of the
  compare's rows, read by the culling decodes (`cull_refresh`), the
  frame drawn (`cull_frame`), the click on a tile (`viewport`) and the
  tests; the count (`Cull::compare`) stays the set's length, so the tile
  grid, the zoom's cell and the status line read it unchanged. The set is
  kept by cell, not by row, so a filter that moves the rows leaves it on
  the same frames. A focus on any cell of the set keeps it, wherever it
  stands; a focus off it (an arrow, a click on another frame) leaves the
  compare. So no slide at the list's end ever happens for a versions
  compare; it still happens for V's views, and for the view a versions
  compare leaves behind.
- **The view refits after any change to the list under it** where §309
  says only that the loser goes by Delete: a delete, a join, a file
  dropped or a merge of the list; the set follows the file's cells and
  shrinks with them rather than taking in another frame.
- **The tile's name tag.** §309 names the version on the plate; we also
  put the label on the tile's tag at its top, since the plate speaks only
  for the focused tile and the tags are what tell the tiles apart.
- **A version's delete in culling** was refused before this part; it is
  allowed now, as §309 §4 needs. New version, Rename and Swap stay
  refused in culling (§309 §10).

## Review, four rounds

### First round

The review ran the gates clean and found the slow tile's cause and three
defects in the set's invariant, all closed in a3f22e1:

1. *A version's placeholder asked as its file's (part 2's bug).* Opening
   a version in the editor asks the camera's picture to stand in for its
   develop (`start_placeholder`), and asked it without the version. The
   delivery is checked against the cell's `Key`, path and version, so it
   was dropped: a version's cell never showed its camera placeholder, and
   the stray decode held one of the three culling threads. That was the
   slow tile of the first snapshot: entering compare from the editor on
   a version, one of three threads was decoding a picture nobody would
   take. The want now carries the cell's version
   (`a_version_s_placeholder_is_asked_for_the_version`).
2. *Compare from the grid or the lightbox showed nothing:* culling under
   the grid keeps the grid on top. The item leaves the lightbox and
   closes the grid first
   (`compare_versions_from_the_grid_or_the_lightbox_closes_it`).
3. *A renumbering outside `settle` lost the set.* `relist` dropped the
   compared file and only `settle` restored it, so a reject moved out, a
   file deleted, or a merge of the list (a folder read again, an index
   pass) left the anchor on the old number and the set took in another
   file's frame. One pair, `compared_file` before the relist and
   `refit_compare` after, is called from `settle`, `roots::merge` and
   `drop_files` (`a_drop_of_files_under_compare_keeps_the_set_on_the_file`,
   `a_settle_from_a_join_refits_the_compare`).
4. *Three up outlived its set.* An arrow off the file, the file gone, or
   item 3 left the view at three over any frames with the field offering
   3. `Cull::leave_versions` is the one way out other than V and the
   field, and it makes three four
   (`three_up_stands_through_the_culling_keys_and_leaves_with_the_file`).
5. Nits: `tile_name` takes the version's id as well as its name, so a
   version with an empty name says its id as `Key::said` does; and
   `fit_compare` with every cell of the file filtered out leaves the
   compare as when the file is gone, rather than keeping the note and the
   file.

### Second and third rounds

Part 5 (§319) landed on master under this part, and the merge (b2aa4d2)
broke two things, closed in 8a8295a with the review's nit:

1. *It did not compile merged.* Part 5 made `State::cells_of_file` a
   `Vec` (a collection's cells of one file need not be a range);
   `compare_set` iterates it.
2. *A stranger in a collection's compare.* A collection may list
   IMG_0000 (Warm), IMG_0001, IMG_0000, and the compare read the run of
   rows from the file's first cell, so Compare versions from Warm showed
   Warm and IMG_0001. We gave the compare an explicit list of cells
   (`Cull::set`) in place of the anchor and count for a versions compare,
   behind one maker of the rows (`Cull::rows`); `cull_select` keeps it
   while the focus is one of its cells and leaves it otherwise
   (`compare_in_a_collection_takes_the_file_s_cells_where_they_stand`:
   the cells apart, the set exactly the file's, a click within it, an
   arrow off it, Delete under it).
3. Nit: a renumber under a three-up compare took the view through four
   (`relist` leaving the compare) and back to three, and the refit read
   that as a change of count and fitted a 1:1 view. The refit now
   compares with the count before the relist, and the zoom stands when it
   comes back the same (the drop test asserts it).

### Fourth round

Another folder, collection or the empty view opened during a versions
compare kept the compare over the new list: `open_loaded` keeps culling
on and makes the cells with `versions::list`, not `relist`, so the set's
old cell numbers named the new list's first frames and the view still
counted as a versions compare. It now leaves the compare beside the
anchor's reset (`a_new_list_ends_a_versions_compare`). `take_empty`
goes through `open_loaded`; `list` and `relist` are the only writers of
the cells outside tests, so no other path needed the call. Closed in
69c5e5d.

## Tests

Eleven in `panel::versions::tests`:
`compare_versions_is_offered_on_a_file_with_versions_and_its_versions`
(the menu item by cell kind, and gone with the file's last version);
`compare_versions_sets_the_file_and_its_versions_side_by_side` (from a
file with one version: two up, anchor, focus; from a version of the
folder's last file, which has two, while culling: three up on exactly the
three cells, the anchor on the file with no slide, the version in focus,
the selection collapsed, the field at "3"; V from three to four, the
view no longer the file's; a file with four versions: four up on the
first three and the note); `a_version_s_tile_names_the_version_and_the_file_s_does_not`
(the plate's words for a version's and the file's tile, the camera's
JPEG fallback, the note's place, the tile name with its id fallback, the
state's lookup); `delete_under_compare_takes_the_version_and_the_set_closes_up`
(the version's sheet from compare at three up; after it two up on the
file and the other version, no other frame; the last version deleted
from the menu, one up on the file); the first review's five, the
second's one and the third's one above.

## Measurements

Workspace tests 2269 passed, none failed, 47 ignored, across 31 test
binaries, merged with master at 217c5dd (eleven of them this part's, all
in the UI crate). Clippy
over every target with warnings denied and fmt clean. Snapshots under
headless weston, debug build, on a copy of two sample raws with a
hand-written sidecar of two versions on the first:

- `--menu 1 --keys "down ×8 enter ..."` from the editor (Compare
  versions picked from the menu on Warm): the file, "066A3439 (Warm)" and
  "066A3439 (Cool)" in three tiles of the 2×2, the fourth empty, every
  tile's picture up by the capture 11.8 s after the pick, the plate
  "culling, 3 up: Warm, the camera JPEG, 5464 × 8192, fitted; Enter
  develops". Before the review's first fix the same run had two of the
  three still decoding at 11 s. Again after the second round, merged:
  every tile up by the capture 11.0 s after the pick, and with the
  versions' pictures made by the earlier runs each tile is its own edit
  (Warm brighter, Cool darker), the plate "culling, 3 up: Warm, the edit,
  1366 × 2048, fitted; Enter develops".
- The same picked while culling (`--cull`): the three at once.
- `--menu 1` shows Compare versions under Delete version...

## Left open

- The "comparing X and its versions" status the action sets is replaced
  by the culling status at the first frame drawn; only the left-out note
  stays.
- No key for Compare versions; it is a labeled menu item.
- In a collection whose cells of the file stand apart, the arrows move
  through the list, so they cannot reach the set's other tile: the frame
  between is off the set, and stepping onto it leaves the compare. A
  click on the other tile reaches it.
