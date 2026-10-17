# §334. Stacks (2026-10-17)

The roadmap's "Stacks, in the collections file", the layer above the
RAW+JPEG pairs (§332): a pair is one press as one frame, a stack is
several frames put behind one by hand. A burst, a bracket, the five
tries at one expression: the grid shows the one on top, and the rest
are a click away. The Culling and search track's burst grouping puts
frames in stacks; this builds the stacks and no grouping.

## What a stack is

A group of frames made by hand, in one folder, one of them on top.
A frame is a version of a picture as everywhere since §317: a file's
own, or a version of it, so a version is a member by itself, and a
file's own and its version can be stacked together. A frame is in one
stack at most: stacking a frame that is in another stack takes it out
of that one.

**One folder.** Lightroom's stacks are within a folder, and we follow
it. A stack exists to fold a run of frames into one place in the list,
and the list it folds is a folder's: a burst is one card's run in one
folder. A stack across folders would have no place to stand in a view
of either folder alone, and a view of both would gather frames from
two places into one. Stack refuses a selection across folders ("a
stack is made of frames in one folder"). A view of a root or of a
branch with its subfolders lists several folders at once and shows
each folder's stacks in it.

## The file

Beside the collections, per machine as they are (§273, §304): one
small file a stack, `collections/stacks/<id>.json`,

    {"version": 1, "made": <seconds>, "open": true,
     "refs": [{"hash": "...", "hint": "/photos/shoot/IMG_0001.CR3"},
              {"hash": "...", "hint": "...", "version": "<id>"}]}

The references are a collection's (`collections::Reference`): the
picture's content hash, the path as the index keys it, and the
version's id, left out for the file's own. Their order is the stack's,
the top first. `open` is left out while the stack is closed. The id is
made at random when the stack is; unstacking deletes the file.

**A folder of its own.** A build before this one reads every `*.json`
in `collections/` as a collection, and a stack's file there would be
said in its log as broken at every launch; a folder in it is passed
over in silence, so 0.7.0 reads the collections of a library this
build has stacked, exactly as before (a test holds it).

**One file a stack**, for §273's reason: a change is a small write of
one file, never the rewrite of every stack in the library. The cost is
a directory read of one file a stack at launch, which a library of
bursts may make thousands of; if that ever shows, the stacks of one
folder can share a file without changing the reference.

**Two editors.** A stack's file changed on disk since it was read is
taken by §304's rules for a collection's, less the join: a stack is an
order and a top, a whole, and not a list two sides add to, so the join
has nothing to join. A file gone since it was read was unstacked by
the other side, and the stack goes here too instead of coming back at
our next write. A later build's file is never written over. One that
will not read is kept beside itself as `.json.bad` before ours goes
over it. One that reads is written over. Each is said in the log
(`Store::save`).

**One order everywhere.** The stacks are kept sorted by when each was
made, then by id (`stacks::sort`), not in the order a directory read
gives. Two stack files can claim one frame (two editors, two machines
once the carry exists); the frame is the first stack's, so it resolves
the same on every machine and in the command line.

**Open is kept.** Whether a stack shows its members is in its file, as
Lightroom keeps it in its catalog: a stack opened to judge stays open
in the next session, and an S costs one small write.

**Carried across machines** with the collections, the same follow-up
on the roadmap: the folder travels as the collections' does.

## Found by the path, followed by the index

A collection finds its pictures by hash (§273): every copy of a member
is the member. A stack cannot: the selects folder holds copies of a
shoot's best frames, and by hash a stack in the shoot would turn up
again among the selects. A stack is found by its members' paths. The
list's files are the index's keys already in a view of a root, and a
folder opened from the disk asks its own once a session
(`greycard_library::key_folder`); a frame the window follows on an
archive's copy (§233) is found by its local path. Nothing else is
asked of the disk, and nothing of the index, on the window's thread.

The hash is kept so the reference follows the file, as a collection's
does. The index's report gains `moved_files`, the old path and the new
of each file a pass found moved by its hash, beside `rehashed`. A pass
that finds a member renamed within its folder carries its path; one
that finds it moved to another folder (a file manager's drag) takes it
out of its stack, since a stack is one folder's and the member would
otherwise stay hidden behind a top in a view of the root; one that
finds it rewritten in place gives it the new hash (`Store::heal`, at
the same three places the collections heal). No schema change: the
report is what a pass says, not what the index keeps.

A member is kept at the path the list finds it by
(`stacks::hint_of_file`), so a frame the window follows on an
archive's copy is stacked at its local path, and the rule of one
folder is asked of the local folder too.

## The list

A stack is laid over the cells (`stacks::arrange`, from
`State::wanted_cells`, the one maker of the cells):

- A member is listed when its path is a listed file's and its version
  one the sidecar has. A frame is shown in the first stack, in the
  kept order, that has it. The first member a stack keeps is its top. A
  stack keeping fewer than two is no stack in that list, and gives back
  what it claimed: its members are plain frames, and the file keeps
  them all. This is one function, `greycard_library::stacks::shown`,
  which the window's `listed` and the command line's `collapsed` both
  call, so the two cannot disagree over a frame two stacks claim or a
  version gone from its sidecar.
- **Closed**, the stack is its top's cell, where the top stands in the
  list, and its other members have no cell at all. Every walker of the
  cells, the strip, the grid, the arrows, the rubber band, the filter,
  the lightbox, culling and the export queue, sees one frame without a
  word of its own.
- **Open**, the members follow the top in the stack's order, gathered
  from wherever the folder's order put them, as Lightroom shows an
  expanded stack.

An open stack whose members stand apart in the folder breaks the rule
the lookups by file leaned on, that a file's cells stand together, its
own first, in file order. When the cells are made and do not stand so,
`State::cells_by_file` keeps each file's cells, and `cells_of_file` and
`own_cell` read it in place of the binary search (a closed stack only
leaves cells out, which keeps the rule, and the index is not made).
`own_cell` also checks that the cell it finds is the file's own, since
a closed stack may keep a file's own out and its version in. A read
landing asks of its one file whether its cells are still what the list
wants (`rows::versions_differ`); laying every stack over the whole
list for each would be the square of the list, so the stacks as the
cells were last made from them are kept (`State::stacks_listed`) and
a file of one is answered by its stacks' files alone
(`stacks::wanted_of_file`). A collection's list is its own order and
shows no stacks.

In the store a member is told from another by its path and its
version, not by its hash: two identical copies in one folder are two
frames, and Set as top and Remove act on the reference itself.

**The badge** is on the picture's bottom right, the corner the
rating's badges (bottom left), the marks (top right) and a version's
name (top left) leave free, in the marks' size and scrim: the layers
glyph and the count on a closed stack's top, "2/5" on each member of
an open one. A click on it opens or closes the stack; it is drawn over
the cell's own press so the cell does not take the click. The lightbox
draws nothing over its pictures but this: a closed stack's top there
would otherwise be taken for one frame.

## The actions

Lightroom's keys, which a hand coming from it already has:

- **Stack, Ctrl+G** (the frame menu, "Stack 3 frames"): the frames
  chosen, the frame on screen on top and the rest after it in the
  list's order, closed. A closed stack's top chosen stands for its
  stack and brings its members along, since that is what the user sees
  chosen, and the menu's count is of what the stack will hold, that
  stack's members with it. A collection's list makes none ("a stack is
  made in a folder's list, not in a collection's").
- **Unstack, Ctrl+Shift+G**: each stack a frame chosen is in.
- **Open or close, S**, the menu's Expand stack and Collapse stack in
  Lightroom's words, or a click on the badge. Closing a stack whose
  member is on screen opens its top in its place. S and Shift+S are the
  grid's alone, as in Lightroom's Library; everywhere else, culling
  included, S stays the soft proof it was, and in culling a stack opens
  and closes by its badge on the strip.
- **Set as top, Shift+S** in the grid: the frame right-clicked, or on
  screen; the stack then stands where its new top does.
- **Remove from stack**: the frames chosen out of their stacks. A
  stack left with one member is no stack, and its file goes.
- **Delete version** on a member takes its reference out of the stack,
  as a delete of a file does. A version gone from the sidecar another
  way (a join, a read) is not shown and stays in the file, as a missing
  file does.
- **Compare stack**: below.

What acts on frames acts on what is seen of a closed stack, its top: a
rating, a flag, a label, a turn, a paste, an export, a delete, Add to
collection. Every one of them already takes the chosen cells, and the
members have none.

## Culling and Compare

The arrows step over a closed stack as one frame and through an open
one's members, with nothing added: they walk the shown rows. Compare
stack opens a closed stack and enters culling on its members side by
side, two, three or four up, the frame right-clicked in focus. It is
§320's named compare taken one step: `Cull::stack_of` beside
`versions_of`, the set the stack's member cells by name, refitted by
the same `compared_file` and `refit_compare` (now over a `Compared`,
a file or a stack) after every relist, so a member taken out closes
the view up and an unstack leaves it. Three up, Compare versions'
alone until now, is a stack of three's as well: a named set shows
itself and no stranger.

## Collections and the filter

**A collection takes the frame, not the stack.** A closed stack's top
added to a collection (B, the menu) adds that frame alone. A
collection holds pictures by hash wherever they are; a stack is one
folder's grouping and has no place in a list of pictures gathered
across folders, and putting the members in quietly would fill a
collection with frames the user never saw chosen. A collection's view
shows no stacks; Lightroom keeps a collection's stacks apart from a
folder's, and we leave a collection's own order as it is.

**A closed stack passes the filter when its top passes.** It is shown
as its top: a top shown under a filter it fails would be a frame the
filter says no to, and a member that passes stepping up in its place
would move the stack's face as the filter moves. To find a member that
passes behind a top that does not, open the stack, and each member is
judged on its own. The bar counts what it would show: a file behind a
closed top is left out of the chips' counts, the "2 of 2", and the
facets' (`stacks::behind`), so the bar over a folder of four frames, two
of them behind a top, says two.

## Moves and deletes

`State::move_file` is the one writer of a listed path, and the
window's moves of files are the rejects move, Move back and the
delete. Each acts on frames one by one: the rejects move is a verdict
on each frame. So a stack does not travel with its top: rejecting the
top and moving the rejects out takes the top alone, and the next
member comes up. Taking the top's unrejected alternates to the rejects
folder with it would move frames the user did not reject. Lightroom's
"move takes its members" is a drag to another folder, a filing action
we do not have; when it comes, it takes the stack.

Move rejects is the folder's, as it was: it takes every rejected
frame of the folder, a rejected member behind a closed top among
them, which the user rejected while the stack was open.

A frame the window moves out of its folder or deletes leaves its stack
(`stacks::left_files`, in `drop_files`, through which all three pass):
a stack is one folder's, and the frame is no longer in it. Move back
brings a frame back plain. A frame gone by another hand stays in the
file, not shown and not counted, as a collection keeps a missing
reference: it may be on a drive that comes back.

## The command line

`greycard library list` lists a closed stack's top alone with "stack
of 3" among its marks, and drops the members behind it, `--count`
with them; `--stacks expanded` lists every frame. A member is there
when the index has a row at its path that is not missing and, for a
version, when that row's sidecar keeps it (the index's versions table,
each member's folder read once). `stacks::collapsed` lays the window's
rule (`shown`, above) over those rows; the list being of files, a file
is left out when every frame of it, its own and each version, is a
member behind a closed top.

## Sites

- `greycard-library`: `stacks.rs` (the file, `Stack`, `read_folder`,
  `sort`, `shown`, `collapsed`); `collections.rs` lends `new_id`,
  `now` and `write_atomic`; `index.rs`'s `Report::moved_files`.
- `greycard-ui`: `stacks.rs` (`Store`, `listed`, `arrange`, `marks`,
  `by_file`, `wanted_of_file`, `behind`, `stack_of`, `left_files`,
  `version_left`, `hint_of_file`, `folder_of`); `panel/versions.rs`
  (Delete version's leave);
  `panel/stacks.rs` (the actions, the menu's state, `fit_compare`,
  `healed`); `versions.rs` (`wanted_cells`, `wanted_of_file`,
  `cells_of_file` and `own_cell` by `cells_by_file`, `Compared`);
  `main.rs` (`cells_by_file`, `stacks_listed`, `stacks`);
  `panel/cull.rs` (`Cull::stack_of`, the leave in `drop_files`, the
  bar's counts);
  `panel/browser.rs` (the thumb's marks); `panel/menu.rs`;
  `panel/startup.rs` (the load); `library.rs` (the heal, the facets'
  counts); `sync.rs` (`local_of`); the slint: `ThumbStack`, the strip's,
  the grid's and the lightbox's badge, the frame menu's six items, the
  keys.
- `greycard-cli`: `library list --stacks`.

## Tests

- Library: a stack's file round-trips (order, versions, open, no
  `open` or `version` key when there is none), a later build's and a
  broken file left alone; a collections folder holding the stacks'
  folder reads as 0.7.0 reads it, and one of 0.7.0's has no stacks;
  the list's closed stacks for the CLI (the first there the top, a
  version gone not there, one there no stack, a file with a version
  shown listed though its own is behind, open hides none); a frame two
  stacks claim the older's whatever the read order; a pass's move says
  both paths as the index keys them.
- UI, `stacks::tests`: a stack laid over the list closed and open, the
  order, the marks; the store keeping a frame in one stack, dropping a
  stack of one with its file, the top, open, the load, a pass's
  rename followed, a move to another folder leaving, a rewrite, the
  window's leave; a file gone on disk dropped and not written back, a
  later build's left alone, a broken one kept as `.json.bad`.
- UI, `panel::stacks::tests`, thirteen with those, on an indexed root
  of five frames: Ctrl+G, S, Set as top from the menu, Remove down to no
  stack, the menu's Stack and Ctrl+Shift+G, each on disk and read back,
  and the lookups by file under an open stack; culling's arrows over a
  closed stack and through an open one, S there the proof and the
  badge opening it, closing from a member; a
  closed stack as its top to the bar's counts, the filter and Add to
  collection; a member gone from the disk, one moved out by the window,
  one renamed and carried by the pass; versions in a stack (a version
  on top, its file's own behind it, a closed top bringing its stack, a
  version gone from the sidecar); Compare stack three up, closing up to
  two and left at an unstack; Delete version taking its reference
  out; two stack files claiming one frame, the older's, and the menu
  counting what Stack will hold; a frame followed on an archive's copy
  stacked at its local path; one folder, no collection. At each step
  of three of them every file's own answer (`wanted_of_file`,
  `versions_differ`) is checked against the whole list's.
- CLI: `library list`'s stacks closed and open, three members of one
  hash told apart by their paths, and a later stack claiming a frame
  hiding nothing more, as the window has it.

By hand, under headless weston on hard links of four raws with a stack
of three written beside the scratch library's collections: the grid
lists two frames, the top wearing the layers glyph and 3 at its
picture's bottom right, the bar saying 2 of 2 and its facets counting
two; S opens it, "1/3", "2/3", "3/3" after the top and the fourth frame
after them, the members' pictures made once asked; the frame menu on
the top offers Expand stack, Remove from stack, Compare stack and
Unstack; the strip in the loupe wears the same badge. After the review
round, the lightbox over the same folder: the stack's top with its
badge at its picture's bottom right, beside the other frame.

## Review round

The review found the window sound (nothing vanished, no action reached
a hidden frame, 0.7.0's reading proved) and asked for these, closed in
the same commit:

1. *The command line's rule was not the window's.* Two stack files
   claiming one frame both claimed it in `library list`, which dropped
   a frame the window showed; and a reference to a version gone from
   its sidecar was taken as a top there and hid the next member. Both
   now call `stacks::shown`, and Delete version takes its reference
   out of the stack.
2. *A member moved to another folder stayed hidden behind its top* in
   a view of the root, its path carried there. A pass's move follows a
   rename within the folder only; a move away leaves the stack.
3. *S and Shift+S had taken culling's proof.* They are the grid's
   alone; in culling the badge on the strip opens a stack.
4. *The stacks were judged in the order the directory read them*, so
   a frame two stacks claim could resolve differently on two machines;
   they are sorted by when each was made, then by id.
5. *A stack changed on disk was written over whatever it was*: one
   gone came back, a later build's was overwritten, a broken one lost.
   §304's three rules now hold, less the join.
6. *Stacking a frame followed on an archive's copy kept the copy's
   path*, which the list did not find it by; it keeps the local one,
   and the folder rule asks the local folder.
7. Nits: the menu's "Stack N frames" counts what the stack will hold
   when a closed top is chosen; the lightbox shows the badge; Move
   rejects being the folder's is said above.

## Measurements

The workspace's tests: 1154 passed and 28 ignored outside the UI
crate, across 28 test binaries, and 1278 passed and 20 ignored in it;
fmt and clippy over every target with warnings denied clean.

Timed once in a debug build on a list of 20,000 files with 300 stacks
of five: the stacks the list shows found in 14 ms; the cells made with
them laid over, 18 ms with the finding; `versions_differ` asked of
every file, 90 ms; 2,000 lookups by file under open stacks, 0.08 ms
through `cells_by_file` (338 ms scanning, before it). With no stack in
the store the cells are made in 0.5 ms, as before. A list is made
again only at a merge, a relist or a change to a stack.

## Left open

- Carrying the stacks across machines, with the collections.
- A member moved where no pass of the index sees it (outside every
  root) is not found again; the next pass under a root that finds it
  renamed within its folder carries it, and one that finds it in
  another folder takes it out of the stack.
- Renaming the folder itself loses its stacks: the index sees new
  files and missing ones, not a move, so no heal runs and the stack
  file stays, showing nothing.
- A stack is never found by its members' hashes, so a shoot copied to
  another folder whole does not bring its stacks; the copy is another
  folder's frames.
- The Lightroom catalog import does not bring Lightroom's stacks.
- No Shift+[ and Shift+] to move a member up or down in its stack;
  Set as top is the one reorder.
- `listed` is asked afresh by every making of the cells and every
  rebuild of the browser's rows, one path made and looked up a listed
  file whenever any stack exists (the 14 ms above, in a debug build).
- A stack open shows its members by their badges alone; Lightroom
  draws a band round an expanded stack.
