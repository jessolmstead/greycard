# §312. Virtual copies, part 1: the edit crate (2026-10-12)

Part 1 of the virtual copies design (§309), built in `greycard-edit` alone:
the line as a struct, versions in the sidecar, schema 5 and the join
per version. Nothing in the window knows of versions yet; the other
crates moved only where the struct's shape made them.

**The line.** `Line` holds what was the sidecar's own development:
`current`, `current_label` (`step`), `current_exports` (`exported`),
`last_export`, `current_id` (`id`), `history`, the unwritten `redo`
and `snapshots`, with the keys and shapes they had. `record`,
`record_as`, `undo`, `redo`, `go_to`, the snapshots, `record_export`,
`rows`, `describe`, `take_sources`, `rename_look`, `fill_ids`,
`push_state` and the history's cap are written once, on `Line`. The
sidecar holds version 0's line as its `line` field, flattened into the
file (`#[serde(flatten)]`), and derefs to it, so `sidecar.current`,
`sidecar.history` and `sidecar.describe(i)` read version 0 as they
always did and the some 130 reads of `sidecar.current` did not move.
`Sidecar::line(v)` and `line_mut(v)` hand out a line by
`Option<&str>` (None for version 0), none for an id the sidecar does
not have. Each line field and `versions` is read loosely through
`deserialize_with`, as the other loose fields are, and a sidecar with
none of the line's keys, or no `versions`, reads as the defaults.

What acts on the whole file stays on `Sidecar` and now reaches every
line: `turn_by` (the turn is the file's), `rename_look`,
`take_sources`, `needs_frame`, `migrate_with_frame`, `placed`,
`names_look`, `looks_named`, `fill_ids`. `Sidecar::lines()` walks
version 0 then each version.

*The deref's one cost.* A call through `DerefMut` borrows the sidecar
before its arguments are read, so `s.record_export(&s.current.clone(),
..)` stopped compiling: the two-phase borrow that lets a method read
its receiver in its arguments does not reach through a deref. Rather
than rewrite such calls across the workspace, the line's mutating
steps (`record`, `record_as`, `record_export`, `go_to`,
`take_snapshot`, `restore_snapshot`, `undo`, `redo`) are said again on
`Sidecar`, one line each, acting on version 0. Five places outside the
crate moved a field out of a sidecar (`let edit = sidecar.current`),
which a deref cannot do; they say `sidecar.line.current` now. The
twenty struct literals that set `current:` set `line:
Line::of(edit)`; `Line::of` is a line whose one state is the edit.

**What moved in the file.** With the line flattened first, a sidecar
writes the line's keys together: `current`, `step`, `exported`,
`last_export`, `id`, `history`, `snapshots`, then the meta,
`meta_at`, the turn, the XMP mark, `saved`, the revisions, `absorbed`
and last `versions`. Before, `history` and `snapshots` came after
`absorbed`. Readers find keys by name, so nothing reads it
differently; only the bytes moved, which the first save of a frame
on this build does anyway now that its edits say 5.

**Versions.** `Version { id, name, made, line }` in
`Sidecar::versions`, written as `{"id", "name", "made", "line":
{...}}`. The line is nested under `line` rather than flattened into
the version's object because a line has an `id` of its own, the
current state's; the design did not say. A `versions` that is not a
list (or is null) is none; an entry that is not a version at all (no
id, an empty id, an id already read) is left out and said in the log.
A version with an id of its own that will not read refuses the whole
sidecar, as the same fault in version 0 does: left out, it would be
gone at the next save, and the window never writes over a sidecar it
cannot read. A name or a time that is not one reads as empty or 0 and
keeps the version.

The id is sixteen hex digits from the standard library's randomly
keyed hasher over a count, the clock in nanoseconds and the process
id: random, made once, and checked against the sidecar's other ids.
No new dependency.

**The makers.** `new_version(from, name)` copies the source's current
edit as the new line's only state, labeled "From <the source's name>"
or "From the original", with no history, snapshots or export records,
and returns its id (none when `from` names a version that is not
there). `new_version_from_original(default_edit, name)` starts from
the edit given and labels it "From the defaults": the design names the
label of a copy and not of a fresh one, and "From the original" there
would say the original's look was copied when it was not. A blank or
missing name is `next_version_name()`, "Version N" with N the count
plus two; where that name is already taken (Version 2 deleted, Version
3 kept) it steps on to the first free N rather than make a twin, which
the design's count alone would. `rename_version` trims and refuses a
blank name; `swap_with_original` exchanges the two lines whole (the
redo stack and `last_export` with them), the version keeping its id
and name; `delete_version` returns the version taken. The two labels
are `history::version_from_label` and `version_from_defaults_label`.

**The schema: 5 for every write.** §309 kept a sidecar without
versions at 4 and wrote 5 only where versions were, for the reason
§166 refused a bump for a label: raising `VERSION` locks every build
before it out of every sidecar the new one touches. That reason has
gone: 0.6.x has next to no users, and neither byte stability of a
sidecar from before versions nor an older build reading a sidecar
0.7 touched is a goal any more. So `VERSION` is 5 and every edit,
sidecar and preset is written at it. The step from 4 to 5 moves
nothing in an edit and is the identity in `migrate`; the bump is
there so that a build from before versions refuses any sidecar this
one wrote ("schema version 5, this build reads up to 4") rather than
reading version 0 and dropping the rest at its next save. Gone with
it: the conditional schema, the thread-local the sidecar's writer set
for it, the edit's custom `version` serializer and reader, and the
read with a lowered ceiling. An edit serializes its own `version`
again. The gap the conditional schema left (a sidecar whose version 0
was all edits still at 3, waiting on the frame, had nothing at 5 for
an older build to refuse) is gone too, since the version-0 edits that
are not waiting are at 5 and a sidecar of nothing but waiting edits is
from before schema 4, which no build since writes.

`Edit::needs_frame` asks `version < 4`, not `version < VERSION`: the
step that wants the frame is version 4's, and an edit at 4 read
without `migrate` (serde alone) is not waiting on anything.

The fixture that pinned a sidecar's bytes from before versions
(`fixtures/sidecar-before-versions.gcd`, 24 KB, synthetic, written by
the build before) now holds the migration: read at schema 4 it comes
out whole, its states, words, records, snapshots, meta, times, turn,
XMP mark, save count, revisions and absorbed list as written, its
stored ids kept; written, every edit says 5; read and written again,
the same bytes.

**Sync.** The comparison stays the file's, by revisions. For a pair
from before revisions it goes a line at a time: the same edit when
every line has the same states by id and the two have the same
versions; behind when the meta and the turn agree and every line of
the one copy, version 0's and each version's by id, stands in order
in the other's line of that version with nothing outside its states
the other lacks. A copy that lacks a version is then behind one that
has it: without revisions a delete and an add read alike, and keeping
is the side to err on.

The join pairs lines by version id. `join_line` is the old body of
`join` over two lines (the states by §243's four rules, the cap, the
snapshots by name, the later `last_export`); `join` runs it on version
0 and on each version both copies have, taking the newer copy's name,
and keeps a version on one copy alone. The list is ordered by `made`,
the id breaking a tie, so both sides of a join make the same list. The
meta, the turn, the revisions and `absorbed` stay the file's.

*A departure from §309 §8: a version deleted on purpose stays
deleted.* §309 keeps a version on one copy alone "whether it was added
there or deleted on the other side". We keep it in every case but one:
when the newer copy has seen the whole of the older (the older's
latest revision is in the newer's lineage, the case §243 already uses
to treat the older copy's missing states as let go on purpose), a
version only the older copy has was deleted by the newer, and the join
leaves it out. Concretely: A deletes V and B has not touched the file
since it last met A's lineage: V is gone (and `compare` says B is
behind, so the editor copies A over B anyway). A deletes V and B edits
V: V is kept, with B's line, whichever was later. B never received V
(A made it after they last met): V is kept. The exception keeps a join
met again with its own input giving itself back, which the simulation
checks.

*The name.* The newer copy's, as the design has it, which means a
rename on the older copy is lost when the other copy saved later for
any reason; a test holds the rule. A time per name, as the meta's
fields have, would fix it if a tester trips on it.

*A swap on one copy.* Version 0's line on the swapping copy is the
version's old line, with different ids from the other copy's version
0, so the join keeps both branches of version 0 and of the version:
nothing is lost, and each line's history holds both pictures, the
reconciled state on top. Rare and lossless; left.

**The simulation.** A mode with versions among the moves: made (a
copy of the original or of a version, its id from the run's generator
so a seed runs the same again), renamed, swapped, deleted, and a
version's line edited, undone, exported from and kept in a snapshot.
After every join it checks that no version of either input is lost
(but one the newer copy had seen and deleted), that a version on both
keeps the edits and snapshots of both lines below the cap, that none
comes from nowhere, that the list is in the order made, that a saved
join rejoined with an input keeps its versions, and through the union
rules too. The end checks (every copy the same, nothing the archive
held lost but by the cap, an undo or a delete) cover every line. The
older-builds mode now stands for copies a build from before ids wrote
before this build met them; such a build refuses whatever this one
writes, and a copy carrying versions is left alone by it. Two in-tree
tests, twelve seeds light and six mixed with older builds, skew and
the cap. By hand, before the schema change: 300 seeds in the versions
mode, 150 with versions and heavy, 150 with versions, older builds and
skew, all clean (190 s, 580 s and 59 s in a debug build at eight
threads); after it, 20 seeds in the versions mode, clean. The three
simulation tests from before run unchanged and clean.

**The command line.** `greycard presets FILE...` with no `--apply`
lists each file's sidecar: the original's states and step, and each
version's name, id and states. With `--apply` the preset goes on the
original, as before, and the line says when the file has versions it
did not touch.

**Tests outside the crate.** The export queue's
`an_edit_in_the_queue_is_migrated_and_a_newer_one_set_aside` uses
`VERSION + 1` as before and so now asks about 6. In the crate the
preset test that pins a preset's first lines says `"version": 5`, the
labeled-step test's hand-built old shape puts `saved` after the
snapshots, and `a_pair_from_before_revisions_is_compared_by_its_state_ids`
dates one copy so the newer is not chosen by a hash the bump moved.

**Tests.** In `versions.rs`: a new version copies the source's
current edit and nothing else; from the original starts from the edit
given; the default name counts from two and steps past a taken name;
a rename keeps the id and refuses a blank; a swap exchanges the lines
whole and back; a delete takes the version alone; a line is named by
its id and walks its own history, cap included; a sidecar with
versions round-trips (through the migration and by serde alone) and
writes the bytes of before once the last is deleted; every edit is
written at 5; a waiting edit keeps its 3 and an edit at 4 is not
waiting; a version from a newer schema refuses the whole sidecar, by
string and by the file read; junk in the versions is left out and a
version with an id that will not read refuses the sidecar, as version
0 does; a sidecar with no line keys reads as the defaults; a turn, a
look renamed and the frame's shape reach every version; makers at
random over forty seeds keep every line and round-trip. In `lib.rs`:
the schema-4 fixture reads whole and writes at 5. In `sync.rs`:
versions made on each copy both kept in the order made; a version
edited on both joined as the original's line is; deleted on one and
edited on the other kept, both orders; deleted on one copy the other
never touched stays deleted; copies from before revisions compared a
line at a time; and the two simulation tests.

**Left.** The index, the window and the rest of parts 2 to 5. A
per-version name time. The editor's paths still act on version 0
through the deref; part 2 moves the ones that act on a frame in the
window to `line_mut` of the frame's version.
