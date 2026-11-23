# §734. Schema 10 as built (2026-11-22)

What we built of §724's design (docs/notes/724-states-that-are-never-rewritten.md), and where it departs or adds. Steps 1 to 4 and 6 are in; step 5 (looks by id) and step 7 (the new join) are not. Sidecars are at schema 10.

### The turn a state was placed under

- Each state says the frame's turn its positions were placed under (`Edit::placed_under`, left out at 0). A turn changes `Sidecar::turn` and no state.
- Every state is held as written and as shown. The shown copy is made from the written one by the frame's shape, which the caller gives (a header read). The edit crate reads no frame.
- Where nothing can measure the frame and a state has placed work under another turn, the state waits. The frame is read-only until it can be measured, and the window says "Read-only: {file}'s masks cannot be placed until its raw is reachable".
- The version 4 crop step is applied to the shown copy. The written state keeps its bytes.
- An XMP's orientation that cannot be measured is owed (`turn_owed`), not applied. It is made once the shape is known, and it travels with the XMP mark it came with.
- The shape is written while placed work or an owed turn needs it.

### States kept as written, and their ids

- We did not keep each state's text, as §724 planned. Measured, the round-trip check that would decide when to keep it cost more than a quarter on both read and memory. Instead, nothing at schema 10 rewrites a recorded state, and the written form is pinned (below).
- **Ids at 10:** `blake3(parent ‖ "\n" ‖ S ‖ "\n" ‖ label)`, where S is the state's sparse text, written by our own canonical writer (keys in byte order).
- **Ids at 9 are kept, not made again** (a departure from §724). `filled_id` and the at-9 check use the whole form a 9 build used, the process left out. A state read at 9 gets `placed_under` from its file's turn.
- **Parents are written** on every state with an id. A state read at 9 gets its parent once at the read: the state before it, when its id checks against that state. That is the only place an id is checked against content. The join's chain check is `child.parent == parent.id`.
- A look rename makes again the renamed states and those made from them, down their own parents, with new ids. No other state is touched.

### The sparse form, and no default changes within a schema

- Every state is written with each field at its container's default left out. List elements are stripped against their own type's default.
- Tagged enums and structs without a default (a crop, a pick, a person) leave out each key they read the same without.
- `version` and `process` are always written.
- Rules are by path, never by key name. A shape scan fails when a list, an option, or a tagged enum is added to a state without a rule.
- No default may change within a schema version: a state that left a field out would change its picture.
  - `fixtures/defaults-at-10.json` and `no_default_changes_within_schema_10` guard the defaults.
  - `fixtures/ids-at-10.json` guards the ids' text.
  - `fixtures/states-at-10.gcd` must write back byte for byte.
  - The golden writers run only before the cut (`testing::SCHEMA_10_CUT`). A test fails once the crate is 0.13.0 and the cut is not set.

### The delta reader, with no writer yet

- A history entry `{"delta": …}` is made whole against an earlier entry: `$del`, `$+`, `$at`, and plain replacement.
- The reader is strict. Anything else loses that state and those made from it. The loss is counted, and the file is then read-only.
- What deltas make whole is held to 64 MiB a file, weighed before each copy.
- A delta in a current state or a snapshot refuses the file.
- No build writes deltas yet. A writer using only these operations keeps the read floor at 10. A writer must write whole any list its base leaves out.

### Seeds and untouched lines

- A line nobody developed holds no recorded state. Its current is the start the reader gives (`set_start`, every reader names one), and the file holds none of it.
- The ISO seed, the export's seed and lens flip, the preset's seed, and the Lightroom start change the start, not a state.
- The root is recorded at first need: a step, an export record, a snapshot, or a version made from it.
- A version 9 line with one state equal to the reader's start (its blend aside), and nothing else, stops holding a state at the read.
- A seeded start is kept per line through joins and the checked save.

### The sources map

- A source the engine chose lives in `Line::sources`, beside the states. Its key is the patch as written and the turn it was placed under; its offset is in that frame. Joins take the union, the lower offset on a tie.
- Nothing merges neighbor states or hands ids over any more.
- A spot moved or resized after its source was chosen writes that source into its state.
- A version 9 line that recorded its source as a step of its own keeps both states.

### Floors and read-only

- Every file starts with `"floors": {"read": 10, "write": 10}`.
  - A read floor past this build's schema is refused.
  - A write floor past it is read and never written. Its states past 10 are read as they are, an enum value this build does not know reads as its default, and every write path refuses. The window says "Read-only: made with a newer greycard".
  - A file with no floors is judged by its states' versions, as before.
- A file with states this build could not read is read-only the same way.
- The contract a later build keeps:
  - A new enum value raises the write floor. An older build reads it as its default, so a write from that build would lose the value.
  - A new field an older build would drop on write raises the write floor.
  - The read floor is raised only when an older build cannot show the picture at all.
  - `floors` is the first key of the file. An older build reads only the file's head (256 bytes) to tell a newer build's copy.
  - An older build never removes, moves over, or writes over a newer build's file, or one that will not parse. That covers a save's clean-up of the other placement, Move sidecars, and a reject move's orphans. Its own unsaved edit goes beside the file as `.other`. A copy that will not parse and names no later floors is set aside as `.bad`.
  - `undone` is a list of state ids. An entry that is not a string is passed over, so a later build may add other entries without a floor.
  - A state's `process` above the build's is kept as written and renders as the build renders. It raises no floor. A later build that keeps an old rendering raises no floor for it either.
  - A delta that uses only this build's operations and keys needs no floor. Any other needs the read floor raised.

### Lines, snapshots, versions, and the head

- Every recorded line has a `line_id`.
  - A version's line takes its version's id.
  - The original's is derived from its root state, so copies migrated apart agree.
- `original_at` is stamped by a swap with the original. A swap exchanges the two lines' places, and each keeps its id.
- A snapshot's id is made from the state it keeps and the time it was taken, not at random (a departure from §724). A file with none is given the same ids by every copy.
- A rename stamps `named`, on a version and on a snapshot.
- A delete leaves a tombstone: `removed` for versions, and `snapshots_removed` on a line.
  - A join takes the union at the later time, less what it keeps.
  - Each map keeps its last 500.
  - These times are each machine's own clock seconds, comparable only with an allowance for skew.
- `head_at` holds when a line's head last moved (a record, an undo, a redo, a new version) and an opaque install id. The install id is made once per install, beside the settings. The host name stays in `Revision.host` and in the "(on host)" snapshot names, by the owner's ruling.

### The join as built

- `join_lines` places every line of either copy once.
  - The copy that swapped with the original later (by `original_at`, the newer copy on a tie) wins the arrangement.
  - Lines pair by id wherever they sit. They pair by place only where one id is empty.
- A version only the losing copy has gets its line in three passes, every version's claim before any fallback:
  1. the line it holds there, where the winner did not place it;
  2. else the line it was made with, which a swap left elsewhere;
  3. else, unless a join made it, the loser's original or a line no listed version holds or was made with.
- A line still unplaced is kept as a version, under its own id, or one derived from it where a version already holds that id. It is named "Version N" by the version-name rule. The exception is a line the newer copy dropped having seen the other's last revision.
- A version a join made gives way to a real version holding the same line, so three machines joining in turn agree.
- Nothing in a join is random.
- States, snapshots by name, meta, and the turn still join by §243's rules. The heads, the conflict copy, and the tombstones-win rule are step 7's.
- **Clocks:** the swap time and the newer copy (by written time, then revision) are wall clocks. A machine with a fast clock wins swaps and ties.
- A join asks that each copy be as its last revision left it. The window debug-asserts it. A copy keeps, unwritten, the content key of its last read or save (`as_revised`) to check it by.

### Undone states, as ids

- A new step after an undo, and a join, move the redo stack's ids to `undone` (a departure from §724's decided item 6). A file that travels keeps nothing of what the user took back.
- Undone ids are capped at the history's 50, never redone, and never name a state the line holds.
- Nothing reads them yet. Step 7 will, and an undone id must never remove a live state.

### Process

- `PROCESS = 2` (§735; 1 is the clamp per channel), and every state has `Edit.process`: the rendering it was made under.
- It is stamped only on a state the user makes: a step (a slider, a reset, a paste, a preset), and a new version's first state. A restored snapshot keeps its own.
- It is never restamped by a read, a migration, a join, or a save. Absent reads as 1.
- Before 1.0 it only records. Nothing renders by it, and no old rendering is kept.
- It raises no floor, and it is out of the picture key and of the ids at 9. `key::PROCESS_KEYED` and its test make a bump re-key the pictures; a bump that changes the finish alone raises the engine's edited and reduced recipes (`RECIPE_FOR_PROCESS`), not `DEVELOP_RECIPE`, which also names the masks' rasters (§735). The render gate's process follows it.

### The revision hash

- A revision hashes the content without the revision list, the meta's times, and the XMP marks. A batch save learns its mark after the revision.
- Nothing hashes a stored revision again, so files written at 9 keep theirs.

### What we measured

Measured at the sparse commits, on synthetic files, and not again since.

| What | Before | After |
|---|---|---|
| Size of a turned 200-state sidecar | 4.44 MB | 1.21 MB |
| Read of that sidecar | 52.3 ms | 16.1 ms |
| Save with nothing new | — | 3.9 ms (21.5 ms uncached) |
| Size of a 2,000-sidecar folder (whole form) | 148 MB | 12.3 MB |
| Read of that folder | 1,555 ms | 164 ms |
| Written parents | — | +0.4% |

- **Simulations:** 32 modes over 6000..6200 and over 6200..6400, every one green.
- **Join matrices:**
  - every pair of up to two moves, at 10 and at 9: 24,642 pairs;
  - three moves against one: 44,928 pairs;
  - three against two, in release: 591,890 pairs, every count zero;
  - 600 three-machine rotations of eight rounds.

### Known and accepted

- A delete against a rename loses the rename: one copy renames V while the other swaps V into the original and deletes its slot. The states survive as the original.
- Under the newer-name rule, three machines joining in turn can undo a rename.
- In a three-copy join where two copies deleted W, which entry holds W's line (W back from the third copy, or one a join made) depends on the order of the joins. The lines are the same either way.
- Two instances starting at once on a share without hard links may each keep their own install id for that session, if one's write takes longer than 200 ms. Only `head_at` differs.
- Two machines whose starts differ (the lens setting) each record a different root for one untouched line. The join keeps both.
- `history` and `redo` are still public fields. Nothing outside the edit crate writes them.

---
