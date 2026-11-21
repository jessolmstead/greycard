# §724. States that are never rewritten (2026-11-20)

A design, agreed and not yet built. It answers why two copies of a
sidecar cannot be joined by a simple rule, and changes the file so
they can. It follows §722, whose save keeps the other copy aside
until this is in.

## The problem

§243 gives every state an id, blake3 over its parent's id, the edit as
compact JSON, and its label (`sync::state_id`, sync.rs:85), computed
once at `Line::record_as` (line.rs:433) and stored. Joining two copies
should then be a union of states and one rule for which is current. It
is not, because about a dozen writers change states that already have
ids and keep the ids. An id no longer says what the state holds.

Three failures show it:

- **The checked save.** Five rounds of a merge on the save path each
  failed on this. In the last, a turn with masks left a geometry
  neither side made. "An edit hashes to its id" is false for any frame
  ever turned with a crop, so every rule built on it misfired, and two
  windows never converged.
- **The archive join.** `sync::join` takes the turn field by field
  (sync.rs:514) and the states from both copies as they are. States
  from the copy whose turn lost are in the other frame: their crop,
  keystone, and masks land a quarter turn off.
- **§243 itself.** Its join grew four rules for "which of the older
  copy's states are its own" (sync.rs:593 to 1089, about 500 lines)
  and ten review passes, mostly to survive ids that do not match
  their content.

## The rule

**A state is never changed after it is recorded.** Its id is the hash
of its parent's id, its content as written, and its label, and stays
true for as long as the state exists, on any machine and in any build.
Everything that today rewrites states either records a new state,
moves out of the state, or is applied when a state is read, never
when it is written.

The rule is about bytes, not pictures. A look file refitted, a lens
database updated, or a profile missing on one machine still changes
what a state renders. That is true today and stays true.

## What rewrites states today

Every place that changes an `Edit` inside an existing state, snapshot,
version, or redo entry without recording a state. "Every state" means
history, current, redo, and snapshots of every line (`Line::edits_mut`,
line.rs:759).

| Writer | What it changes | Why in place | How often | Where |
|---|---|---|---|---|
| Frame turn | Crop, keystone, held aspect in every state; with the aspect, also every mask shape, brush stroke, AI pick and box, Part person, and retouch patch and source | A turn is not a step; undo must hand back states in the current frame | Each Turn left/right, from the browser, culling, or a selection | `Sidecar::turn_by` sidecar.rs:577, `Edit::turn` edit.rs:565, `Geometry::under_turned_source` geometry.rs:485; callers browser.rs:823 and :846, `frames::turn_into` frames.rs:29 |
| XMP orientation adopted | As the turn, but always without the aspect: crop turns, masks stay | No picture decoded when a folder opens | When an XMP beside the raw says a new orientation | xmp.rs:1626 |
| Look renamed | Every state and snapshot naming the look; states re-id'd from the first renamed one, snapshots not | Undo must never go back to a name that is gone | Each Rename in LOOK, on the open folder's sidecars only | `Line::rename_look` line.rs:782; look_rename.rs:777, :914, :932, :940, :1132, :1274 |
| ISO seed | `noise.learned_strength` of the current state | A frame nobody developed should start at its ISO's blend | First open, first export, or a sync onto an undeveloped frame | `rows::seed_landed` rows.rs:108 (from deliver.rs:569, sync.rs:430) |
| Seed taken at export | The blend, and `lens.enabled` flipped to match the render | The export rendered the frame at the other default | Export of an undeveloped frame | `export_record::take_seed` export_record.rs:107 to 138 |
| Seed before a preset | The blend of the current state, then a state recorded on top | As the open's seed | Preset onto an undeveloped frame | preset_fit.rs:191 |
| Lightroom beside | `current` replaced by the import's starting edit, then the import recorded | Only when the sidecar has "no edit of ours" | Opening a raw with a Lightroom XMP | lightroom/beside.rs:341 |
| Repair sources | `source` filled in every state holding the patch; equal neighbors then merge, the earlier taking the later's id | A source is part of placing a patch (issue 7) | After each develop that chose a source | `Line::take_sources` line.rs:683 to 731; panel/edit.rs:367 |
| Version 4 crop | `geometry.portrait`, `frame_pending`, `version` | The step needs the frame's shape | First open of a sidecar from before version 4 | `Edit::migrate_with_frame` migration.rs:40; sidecar.rs:542; browser.rs:1228, on_disk.rs:337, cli presets.rs:193, rawtherapee.rs:680, lightroom_catalog.rs:1614 |
| Migrations at read | Version, baseline (v6), chroma blend (v7), brush ops (v8), the dehaze move | Steps in `migrate` | Every read of an older state; written at the next save | migration.rs:151 to 256 |
| Lenient readers | A null crop read as none (geometry.rs:42); an unknown variant read as the fallback (`serde(other)` in vignette.rs:28, white.rs:108, mask.rs:252); a later build's shape dropped on write (`known_only`, mask.rs:740) | Read what we can rather than refuse | Every save after such a read | as listed |
| Older build | Every field it does not know, from every state | serde drops unknown fields | Every save by a build older than the file | `Sidecar::to_json` sidecar.rs:716 re-serializes all states |

Not rewrites, but they bear on the join:

| Writer | What it does | Where |
|---|---|---|
| History cap | Drops states (repeats first, then the oldest after the first) | `Line::cap_history` sync.rs:1231 |
| Swap with original | Exchanges two lines under the version id: a join by id then joins unrelated lines | `swap_with_original` versions.rs:224; `join_versions` sync.rs:567 |
| Version rename and delete | No time on the name, no tombstone on the delete | versions.rs:208, :237 |
| Snapshot rename and remove | Snapshots are keyed by name and have no id | panel/history.rs:916, :940 |
| Export records | Ride on states, outside the id | `Line::record_export` line.rs:537 |
| Ids filled at read | For a sidecar from before ids, hashed from the migrated content | `Line::fill_ids` sync.rs:1182 |

The one source of most of these is the save: `to_json` writes every
state from the parsed `Edit`, so anything a read changed lands on disk
under the old id.

## The design

### Two turns, and where each lives today

There are two turns, and they are different things.

- **The frame's turn**, `Sidecar::turn`: quarter turns clockwise over
  the camera's tag, 0 to 3. Outside the edit (§117), never a step,
  joined as a meta field with a time (`meta::Field::Turn`, meta.rs:545),
  written into the XMP's `tiff:Orientation` composed with the tag.
  Turn left and right in the browser and culling.
- **The edit's quarter turns**, `Geometry::turns`: counter-clockwise on
  the screen, in the Rotate section, a step like any slider, undone
  and redone. Presets and syncs never carry geometry.

The develop applies the tag and the frame's turn together when the
raw goes into the working space (`Stage::IntoWorking`'s `orientation`,
stages.rs:1235; `Stage::Orient` for a picture, :1243). Everything after
that sits in the *oriented* picture. The edit's geometry is applied
later, by `Geometry::matrix`. Thumbnails of the unturned source fold
the frame's turn into the edit's quarters instead
(`Geometry::shown_turns`, geometry.rs:506).

| Placed thing | Frame it is stored in |
|---|---|
| Crop, held aspect, `portrait` | Fractions of the leveled plane: oriented picture, then the edit's turns, angle, and keystone |
| Keystone | Degrees on the plane's axes |
| Angle (the level) | About the center; a quarter turn leaves it |
| Mask shapes (linear, radial, brush strokes), AI picks and boxes, a Part's person | Units of the oriented picture's width, both ways (`mask::Pos`) |
| Repair patches, their sources (offsets), a Fill's further strokes | The same units |
| Luminance, Color, Landscape, Subject, Background | Nothing placed |
| AI rasters, fills | Not in the sidecar; cached under the base's print, which holds the turn (mask_base.rs:82, ai.rs `cache: (Key, u8)`) |
| Vignette | On the framed picture, symmetric under a quarter turn |

So everything placed is in the frame the tag *and the frame's turn*
make, which is why a turn has to rewrite it.

### A state says which turn it was placed under

We considered one fixed frame for every state, the tag's (turn 0),
with the frame's turn applied at render and at the pointer. It needs
a map at every place a tool reads or writes a position (crop.rs,
mask.rs, retouch.rs, parts.rs, viewport.rs: 42 calls through
`view_to_source`, `source_to_view`, `to_source`, and `to_plane`), the
AI models turned upright and their rasters turned back, and an
un-turn at every record, which moves untouched masks by rounding.

The design instead puts one byte in each state: **`Edit::placed_under`,
the frame's turn its positions were placed under**, left out of the
file at 0. A state is recorded in the frame as it is shown, so
`placed_under` is the sidecar's turn at that moment. A turn changes
`Sidecar::turn` and nothing else.

In memory every state is held twice: as written, and as shown. The
shown copy is the written edit migrated and turned from
`placed_under` to the sidecar's current turn, by today's
`Edit::turn` (crop and keystone by `under_turned_source`, positions by
`mask::Turned`). Everything that reads `Step::edit` or `Line::current`
today reads the shown copy, so the develop, the panel, the history,
copy and paste, syncs, and presets do not change at all. A new state
is the shown edit, `placed_under` set to the current turn. A turn
re-makes the shown copies from the written ones, never from the
previous shown ones, so nothing drifts.

The shown copy needs the picture's shape only where a state's
`placed_under` differs from the turn and it has something placed:
`Turned::new` takes the aspect. Most frames are never turned, and
their shown copies are the written edits. For the rest the shape
comes from `Stance::shown_size` (a header read, as `frame_aspect`
does now), and where no file can say, the state waits as
`needs_frame` makes a version 3 crop wait today. The version 4 crop
step moves here too: it is applied to the shown copy, which knows the
shape, and the written state keeps its bytes.

What this fixes at once: the join's turn fault (each state carries its
own frame, so the joined turn shows every state right); the XMP
adoption that leaves masks behind; and a later build's shape, which
a turn by this build no longer misplaces.

Code: greycard-edit's `Step`, `Line`, `Sidecar::read`, `turn_by`, and a
`show` pass (about 400 lines with tests); `turn_by`'s callers lose
their aspect plumbing (browser.rs, frames.rs, xmp.rs, painted.rs,
about 100 lines out). The develop, the panel's tools, and the AI
rasters are untouched; a turn still invalidates rasters and fills,
as now.

### States kept as written

The older-build case, the lenient readers, and the migrations all
come from re-serializing states from a parsed `Edit`. So we stop.
**Each state keeps its content as written**, as the canonical JSON of
its object (keys sorted, compact: `serde_json::Value` without
`preserve_order` already sorts), and that string is what the id
hashes and what the save writes. A save writes the states it read
byte for byte (up to layout) and serializes only the states this
session recorded. An unknown field, a later build's shape, a variant
read as a fallback, a migration's output: none reaches the file.

`Step` becomes `{ content: Arc<str>, parent: String, id, label,
exports, edit: Edit }`, `edit` being the shown copy. The parent is
written, so an id can be checked without its parent present (after
the cap) and the history is a graph, not a list.

Is canonical form stable? It depends only on `serde_json`'s printing
of a parsed number, which `float_roundtrip` fixes. A property test
writes, reads, and re-canonicalizes random states and demands the
same string. Ids are stored and never recomputed to join, so a future
change in printing would only fail the check, not the join.

Memory and time. Our test folder's 36 sidecars hold 616 states, a
median of 3.1 KB of compact JSON each (90th percentile 21 KB, largest
34 KB); the largest file, 5.4 MB pretty-printed, is 1.1 MB compact over
50 states. Keeping the string beside the parsed edit adds that compact
size per open sidecar, about 1 MB at worst. Reading already goes
through a `Value` for every state (`migrate_line`), so canonicalizing
costs one more print. To measure before building: parse and save time
of the 5.4 MB sidecar, and the window's memory over a 2,000-frame
folder.

The file shape changes: a state's edit moves under `"edit"` (today its
fields sit beside `step`, `exported`, and `id`), so the reader can take
it as a raw value. Unknown keys at the top of the sidecar are kept in
a catch-all on `Sidecar` (one only; `Line` is flattened into it).

### Migrations

A migration becomes a pure function of a state's bytes and the
frame's facts, applied when the shown copy is made. The written
state keeps its version and its bytes.

The one exception is the upgrade to this schema, the last rewrite
there will be. At first read of a version 9 sidecar, each state:

1. gets `placed_under` = the sidecar's turn (every state was turned
   in place to it), inserted into its object;
2. is canonicalized;
3. gets its parent written and its id made again, from the first
   state forward.

No aspect is needed, so it runs on every sidecar at read, offline or
not. It is deterministic: two machines migrating the same file, in
any build from this one on, get the same ids, so the first join after
the upgrade finds them shared. Two copies whose states differ (one
turned in place, or saved by an older build) get different ids, which
is the truth. Their first join keeps both (below), once.

Nothing outside the sidecar holds a state id (only `sync` reads them),
so recomputing them breaks nothing. The revision chain is of saves,
and the migration's save is one more.

### Seeds: a line nobody developed holds no state

Four of the writers (the open's seed, the export's, the preset's, and
Lightroom beside) act only on a line nobody developed
(`line_never_developed`, on_disk.rs:317). Such a line's one state is a
default the file does not need. So **a line nobody developed holds no
recorded state**: no id, and nothing of it written. The window makes
its edit from the settings and the file (lens for new edits, the ISO's
blend), and the first thing that needs a state records it whole as
the root: a step, an export record, a snapshot, or a version made from
it. The seed and the lens flip then happen before the root exists, so
nothing is rewritten. The migration drops the state of a line that
has nothing else (no history, snapshot, or export).

We weighed the blend as an `Option`, `None` meaning the ISO's, read at
develop. It covers the seed but not the lens flip, which follows a
setting, not the file; a setting's default read at develop would make
one id mean two pictures on two machines.

### Repair sources beside the states

A source the engine chose is derived, not placed: the same patch on
the same picture gets it again. It moves out of the states into a
sidecar map, `Line::sources`, keyed by a hash of the patch (method,
points, radius, more strokes, `placed_under`) to the offset. The
develop takes a state's own source when the user dragged one, else
the map's, else chooses and the window adds it. Two copies join the
maps by union, the lower offset on a tie, so both sides agree. The
merge of equal neighbors (line.rs:695) goes with it, along with the
id hand-over that breaks the chain there.

### Looks by id

A state names a look by its file stem (`LutChoice::Named`, look.rs:44).
A rename rewrites every state, and only in the open folder: states in
other folders then name a look that is gone.

A look gets an id: written by us as a comment line,
`# greycard-id: <16 hex>`, in the `.cube` we make (imports, fits,
camera match); for a cube we did not make, the blake3 of its bytes,
which a rename does not change. `LookLut` gains `id`, written beside
the name. The look store resolves by id first, then by name, else
"(missing)" as today. A rename moves the files and changes no
sidecar, and look_rename.rs's sidecar pass goes (most of its 1,494
lines); presets, the clipboard, and the export queue still take the
new name, or resolve by id the same way. A look only on another
machine resolves there by id or name and is missing here, as now. A
look's variants per display curve (§235) follow its own file's stem.

Camera profiles name files the same way but have no rename; they can
take the same pattern if one is added.

### Snapshots, versions, and the original

- A snapshot gets a random id at its taking, as a version has. Its
  name carries a time; a removal leaves a tombstone.
- A version's name carries a time; a delete leaves a tombstone. Both
  replace §243's `seen` guess.
- Every line gets an id, version 0's derived from its root state so
  that two migrated copies agree, and the file says which line is the
  original, with a time. Swap with original changes that pointer and
  moves no line.
- A line's head carries the time and host it was last moved (record,
  undo, redo, go to): `Line::head_at`.
- Undone states are written (today's redo stack), so a copy can tell
  "I undid that" from "I never had that". The panel does not show them
  across sessions.

## Reconciliation once states never change

`sync::join(a, b)`, also the save path's merge, is then:

1. **States**, per line id: the union by id. Export records by
   (state, file, time). The sources map by union.
2. **The head.** The later `head_at` is current (then host, then id,
   for a total order). The other head is dropped when it is an
   ancestor of the current, or when the winning copy had it before the
   join (it moved away from it, as an undo does). Otherwise it is a
   conflict: it is kept as a virtual copy named for its host and time
   ("laptop, Nov 19 14:02"), its id derived from the head's state id,
   its line the states up to it.
3. **Snapshots and versions** by id, names by their times, tombstones
   winning.
4. **Meta and the turn** by field, as §243 has it.
5. **The cap**, applied to the joined graph: the root, each head's
   last 50 ancestors, and the last 50 undone states. A state one copy
   capped comes back in the union and goes again in the same pass.

Why a virtual copy and not today's interleaved history? An interleave
needs an order of two branches, a "Reconciled" state recorded again,
and a cap over a mixed list, which is where §243's four rules and
ten passes went. A copy is visible: the strip shows both pictures, the
user keeps one and deletes the other. The current line's history is
its own ancestry, nothing hidden behind an undo.

Every step is a set union, a total order, or a deterministic
derivation, so `join(a, b) == join(b, a)` and `join(j, j) == j`. Two
machines that save and sync in turn reach the same file in one
exchange (all but the revision each next save adds).

The save path keeps the checked save's stat and byte check. When the
bytes on disk are the ones this window read or wrote, it writes; when
not, it reads them, joins, writes the join, and shows it (a new
current, or a new copy in the strip). The archive sync calls the same
`join`.

**What can no longer diverge:** a state's content; a turn against the
states; a look's name; a seed; a source. **What still can:** two heads
(kept, as a copy); meta fields (by revision, then clock, as §243); a
look or profile present on one machine only; and which head is
current when clocks disagree (both are kept, so nothing is lost).

## Migration and what users see

- The schema goes to 10. A build before it refuses a version 10
  sidecar ("made by a newer greycard"), as every bump has.
- Nothing is written at upgrade. A sidecar migrates in memory at
  read and is written as version 10 at its next save. Untouched
  frames stay readable by older builds.
- Anyone with two machines, or a shared archive, upgrades both, since
  the first save on the new build locks the old one out of that file.
- Pictures do not move: every state shows as it showed. Frames whose
  copies differed by a turn or an older build's save before the
  upgrade get one conflict copy at their first join.

## Order of work

Each step lands on master with its tests. Steps 1 to 4 and 6 change
the file and ship in one release behind the one bump; master can carry
version 10 unreleased between them.

| Step | What | Size | Notes |
|---|---|---|---|
| 1 | `placed_under`, shown copies, turn as one field; the bump to 10 | ~400 lines in, ~100 out | Removes the most harm: the join's turn fault, the XMP turn, and the turn rewrite |
| 2 | States kept as written, explicit parents, the re-id migration | ~500 | Measure memory and read time first |
| 3 | Unrecorded untouched lines; seeds and the Lightroom start off the states | ~150 in, ~200 out | rows.rs, export_record.rs, preset_fit.rs, beside.rs |
| 4 | Sources map | ~200 | Retires `take_sources`' merge |
| 5 | Looks by id | ~250 in, ~800 out | Can follow in a later release: the id is a new field an older build ignores |
| 6 | Snapshot ids, tombstones, line ids and the original pointer, `head_at`, undone states written | ~400 | |
| 7 | The new `join` and the save path's merge | ~300 in, ~1,500 out of sync.rs | Last; needs 1 to 4 and 6 |

Tests:

- **Property:** random sequences of edits, undos, turns, renames,
  snapshots, versions, swaps, and saves on two machines with random
  exchanges converge (equal files after one exchange), and no state's
  content changes after it is recorded (every id checks against its
  content at every step).
- Canonical form round-trips through the file for random states.
- Migration: a version 9 copy turned in place and its unturned twin
  show the same picture and join with one conflict copy, no loss.
- Shown copies: a state placed under turn 1 shown at turn 3 equals
  today's `turn_by(2)` of it.
- An unknown field and a later build's shape survive a save untouched.

## Alternatives considered

- **Store the aspect in the sidecar and keep the in-place turn.** It
  fixes masks staying behind, not ids that lie: a turn still rewrites
  every state under its old id, and the join still meets two copies
  in different frames.
- **One fixed frame for all states** (the tag's, or the sensor's).
  Above: every tool needs a map, the models need turning back, and a
  record rounds untouched masks. The sensor's frame would also need
  every file's tag at migration.
- **Masks in fractions of each side, lengths in diagonals,** which
  turn without the aspect. A migration of every mask on every frame,
  needing the aspect for each.
- **An operation log** (a CRDT of field writes). Converges by
  construction, but the edit is a deep tree of floats where a slider
  drag is hundreds of writes, the history panel and snapshots are
  whole edits, and every develop would replay a log. A large rewrite
  for what a union of immutable states gives.
- **Never merge; keep the other copy aside.** What the save path
  should do until step 7: on a changed file, keep the disk copy as
  `<name>.gcd.conflict-<host>-<time>` and write ours. Safe, but two
  windows on one frame pile up copies the user must sort by hand.

## Decided

1. A state says which turn it was placed under (`placed_under`), not
   one fixed frame for all states: no aspect at migration, no map in
   the tools.
2. Diverged heads become a virtual copy, named for its host and time,
   not an interleaved history: visible, and simple to converge.
3. A line nobody developed holds no recorded state, not a blend that
   is an `Option`: it also removes the lens flip and the Lightroom
   start.
4. A look has an id, in the cube we write and a content hash for
   others: a new look reusing an old name cannot capture old states.
5. A new state does not carry its parent's unknown fields forward: a
   later build's setting may not fit what this build changed, and it
   stays in the parent state, one undo away.
6. Undone states are written, under the cap, and not shown across
   sessions.
7. Steps 1 to 7 ship in one release, behind the one bump to
   version 10: the upgrade's first joins are the ones that most need
   the new rule.
