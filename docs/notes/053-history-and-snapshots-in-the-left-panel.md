# 53. History and snapshots in the left panel (2026-09-17)

§5 rule 9 asked for snapshots and versions per image, and §16 left
the sidecar's history walkable by undo and redo alone. The left
panel now shows it, and keeps states by name.

**The history.** A HISTORY section under the navigator lists every
state the sidecar holds, newest first: the history, the current one
lit, and what was undone muted above it. Each row is named for what
it changed from the row below, by `greycard_edit::describe`, which
diffs two edits section by section in the panel's order: one moved
control is named with its value ("Exposure +0.50", "White balance
3200 K", "Straighten +2.3°", "Linear 1: Exposure -0.50", "Heal 3"),
a switch by its state ("Curves off"), and several sections by their
names ("Light, Curves off, Grain"). A turn, a straighten or a
perspective refits the crop, so a crop that moved with one is that
one's. The bottom row is "Original" when it is the default edit (or
a picture's), which it now stays: the cap of fifty drains the second
entry onward, not the first, so the state the file was opened in is
always there to go back to. Labels are derived, not stored, so old
sidecars get them and the schema is unchanged; a stored label would
be more exact for a preset ("Preset: X" rather than its sections)
and can come later.

A click on a row makes it current through `Sidecar::go_to`, which
undoes or redoes up to it, so what lies past stays until the next
change, as Lightroom's history does. Whatever the panel holds is
recorded first, as undo does; if that was news to the history (a
slider moved within the 800 ms rest), the redo stack is gone, and a
clicked undone row's state is recorded after the panel's as a step
instead.

**Hover to compare.** A row under the pointer shows its state in the
viewport in place of the panel's (`State::peek`, read by the frame
where it reads the panel), and the status line says which step it
is. The panel itself is not touched, since writing to it fires the
callbacks that schedule a save. Everything after the develop
previews at once on the GPU, the white balance through §14's matrix
preview; what the engine does (denoise, demosaic, lens, retouch,
sharpen) shows the current develop under the peeked look, and the
status line names what is not shown until restored. A develop on
hover was considered and left: a denoise takes seconds and would
make hovering feel stuck. The filmstrip's thumbnail follows the
panel's turns, not the peeked state's.

**Snapshots.** A SNAPSHOTS section above the history: whole edits
kept by name in the sidecar (`snapshots`, each a name, when it was
taken as Unix seconds, and an edit), outside the history's cap. Old
sidecars read as none; older builds ignore the field; a snapshot's
edit is migrated as the current one is. Take records the panel, keeps
the current state as "Snapshot N" and opens the row's name for
typing, since Lightroom's date-time default is rarely what anyone
wants. Restoring is `record` of the snapshot's edit, a step forward
rather than a rewind, so undo goes back to before it and the history
stays a line; a step that lands on a snapshot's state, when more than
one section moved to get there, is named "Snapshot: Evening" in the
list. Hover shows the snapshot as a history row does, with its name
and date on the status line; double-click renames; the bin removes.
Both sections fold and are remembered with the others.

Versions (virtual copies) wait on this: they would be pictures of
their own in the strip, with thumbnails, export names and a sidecar
each. Snapshots give most of the value inside one sidecar.

Tests: `go_to` walks both ways and keeps what lies past; the cap
keeps the earliest; snapshots round-trip, restore as a step, and a
sidecar without any loads; `describe` for single controls, several
sections, snapshot steps, adjustments and patches. Checked with a
`--snapshot` of a real sidecar of twenty-eight steps: "Tone curve
on", "Tone curve off", "Curves", "Crop", "Straighten +2.3°" read as
what was done.
