# 166. A name on a history step (2026-09-24)

The Editor backlog line: the history panel named every row by diffing
it against the one before, so a preset, a sync or a preset over the set
read as the list of sections that moved ("Light, Color, Tint, Grain")
and not as what the user did. A snapshot restored was the one named
case, and only when more than one section moved.

**Where the words live.** With the state they produced. The sidecar's
`history` and `redo` are `Vec<Step>` now, a `Step` being the edit and an
`Option<String>` label; the current state's label is
`Sidecar::current_label`, beside `current` (which stays a bare `Edit`,
so none of the some 130 `sidecar.current` reads moved). `record`
is `record_as(edit, None)`; `record_as` swaps the old current and its
label onto the history together; `undo` and `redo` move the pair; a
new step after an undo clears the redo stack and the labels with it;
the history cap drains pairs. A step that changes nothing records
nothing, its label included. `Sidecar::label(index)` and
`Sidecar::describe(index)` read a state's words and its row, and the
panel's `show_history` calls the latter.

**No schema bump, and why.** The roadmap line expected a version bump
and the brief asked for one, with a migration. It was not done, because
nothing here changes what a stored field means, which is the only thing
the rule in `edit/lib.rs` bumps for; §117 (the meta) and §122 turned
down bumps for new fields on the same ground. A labeled state is written as the edit's own JSON
object with one extra key, `step`, beside its fields (a serialize-only
`#[serde(flatten)]` wrapper, so the edit's key order and bytes are as
before), and the current state's as a top-level `step` beside
`current`. Both are left out when there is no label, so a sidecar with
no labels is byte for byte what this build wrote before (a test holds
that against a hand-built struct of the old shape). A build from before
labels reads each state as the edit it is, since an edit ignores a
field it does not know, and drops the words on its next save: the rows
fall back to the diff, which is what that build shows anyway. A bump of
`Edit`'s `VERSION` would have made every sidecar and preset this build
writes unreadable to 0.1.1 — the whole edit refused, not only the
words — to protect a label; the sidecar's `saved` counter was
accepted as droppable by an older build on the same reasoning. There is
no migration to write: a sidecar of any version reads with no labels
and its rows read exactly as before (tested with a hand-written sidecar
holding version 1 to 4 states). A `step` that is not a string reads as
no label rather than costing the state, as the meta is read loosely.

**The words.** In `greycard_edit::history`, next to `describe`:
`preset_label` "Preset: Faded film", `preset_over_set_label` "Preset
×3: Faded film" (the count is the set's size), `sync_label` "Sync from
5M0A3021.CR3",
`snapshot_label` "Snapshot: Snapshot 1" (which `describe`'s own
snapshot case now also uses). The sync names the frame by its file
name with the extension, since a RAW and its JPEG can share a stem in
one folder. The panel's row is about 200 px of `font-xs` in the left
column, which holds about 36 characters at a scale of one
(`history::ROW_CHARS`) and elides the rest, the hover's status line
having it whole. The first cut said "Preset over the set: " — 21
characters of prefix — and the review found "Preset over the set:
Kodachrome 64 Sunset" showing as "…Kodachrome 6…", the part that says
which preset lost. "Preset ×N: " is 11 even for a set in the hundreds,
so a preset name of sixteen characters survives whole; a test holds
every label shape to `ROW_CHARS` with such a name, and the snapshot
shows "Preset ×3: Kodachrome 64 Sunset" whole. The kind of step still
comes first, as in "Preset:", "Sync from" and "Snapshot:", so the rows
scan alike. `describe_step(before, after, label, snapshots)` returns the
label when there is one that is not blank, else `describe`; `describe`
itself and its tests are unchanged.

**The paths.** `panel::sync::apply_preset` records "Preset:" on the
frame alone, in and out of culling, and "Preset ×N:" on the
frame on screen and, through `lay_over_targets` (which takes a `label`
now) and `apply_preset_into` (likewise), on every target that moved;
`sync_selection`, which the sync sheet's apply and `--sheet synced`
run, passes "Sync from" the current frame; `sync_into` takes the label
too. `Sidecar::restore_snapshot` records "Snapshot:" and the name the
snapshot had then, so the row keeps saying where it came from after a
rename or a removal; in the editor that covers both the culling and
the panel branch. The `--preset` startup step and the CLI's `--apply`
record "Preset:" as well. The frame a sync comes from records its panel
state as a plain step, as it did.

`record_as` treats a blank label as none, so nothing can write an
empty `step`, and `lay_over_targets` takes an `Option` rather than a
string that could be empty. `Step` takes the `step` key off a state's
object before the edit reads the rest, so an edit field of that name
would be eaten; a test holds that no field of a populated edit is
called `step`. In the file the current state's `step` sits just after
`current`.

One path deliberately keeps no label: clicking an undone row after the
panel changed re-records that row's state as a new step after the
panel's. Its before is the panel's state now, not the one the preset
or sync was laid over, so it is named by what moved.

**Tests.** In `greycard-edit`: a labeled step round-trips through
save and load, with the `step` keys where they belong and none on an
unlabelled state, and an unlabelled sidecar writes byte for byte as
before; a labeled sidecar reads as plain edits into the old shape; an
old sidecar of every version loads with no labels, its rows equal to
`describe`'s, and a malformed `step` costs only the label; undo carries
a label to the redo stack and back, a new step drops it, `go_to` walks
it; a snapshot restored is named for it and keeps the name after a
rename; `apply_preset_into` and `sync_into` label every frame that
moved and none that did not; `describe_step` takes the words over the
diff and the words fit a row. In the UI: the sync sheet's target has
"Sync from IMG_0000.CR3" in memory and on disk, the source none; a
preset click over three selected frames labels the two that moved and
the history row reads it; one frame reads "Preset:"; the culling test
now labels both branches — and its set case, which had been opening
frame 0 after setting the set and so collapsing it to one frame, now
makes the set after the open and asserts it has two, so it runs the
set branch it was named for; and a new end-to-end test walks the
panel's rows through a preset, an undo, a redo, a snapshot restored
after a moved slider, and a new step after undoing past the preset.

**Snapshots.** Taken on a headless mutter with copies of three sample
frames: `history-preset.png` (`--preset "Warm Negative"`, the row
"Preset: Warm Negative"), `history-preset-over-set.png` (`--also 1,2
--sheet preset-onto-set` with a preset named "Kodachrome 64 Sunset"
first in the store, "Preset ×3: Kodachrome 64 Sunset" whole),
`history-sync-target.png` (a synced frame reopened, "Sync from
5M0A3021.CR3"), `history-sync-source.png` (the frame that sync came
from, its own "Preset: Warm Negative").

**The `--preset` step was never written.** Older than this item: the
startup recorded the step in memory, and the quit's `save_edit` found
the panel equal to it, so `record` returned false and nothing was
written; a `--preset` run followed by a normal close lost the step.
`startup::preset_at_start` now records it and writes the sidecar when
sidecars are on, where the setting puts it. A raw still waiting on its
ISO's learned-denoiser blend is given it first, as `lay_over_targets`
gives a target, since a written non-default edit tells the next launch
the blend was seeded already; a file that will not say its ISO keeps
waiting. A unit test covers the write, the placement, a preset already
on (no step, no write) and sidecars off; a real run with `--preset`
and `--snapshot` left a sidecar with the labeled step, `step` just
after `current`, and the ISO 100 blend in both states.

**Seen, not changed.** With `--preset` and `--exposure` or
`--develop-temperature`, the startup sets the exposure or the white
balance on the current state in place after the labeled step, so the
row says "Preset: X" of a state that also holds that value. Older than
this item, and those two flags exist for measuring; applying them
before the preset instead would let the preset's Light override the
exposure asked for, which is not obviously better.

**Left.** No way for the user to name a step by hand, which the
roadmap line does not ask for; a renamed snapshot does not rename rows
already recorded from it, by choice.

**The review.** The first pass held on the format: the reviewer built
two probes, one against master's `greycard-edit` and one against the
branch's, and read one labeled sidecar with both, six states, the
same position, snapshot and values, master ignoring `step`; a sidecar
of four steps, a snapshot and an undo wrote 23,132 bytes with the
same hash from either build; and master's `presets --apply` over a
labeled file kept its three states and dropped the words. What it
found was the row: a 21-character prefix on a 36-character row, the
blank label that could have written `"step": ""`, a doc that put
`step` beside `current` where the file put it after `saved`, and the
`--preset` step that no build had ever written to disk. All fixed in
two commits; the last as its own.
