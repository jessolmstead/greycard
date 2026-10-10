# 276. Marks for edited and exported frames (2026-10-10)

We put two small marks in the top right corner of a frame's picture, on the
grid and on the strip: a pencil for a frame whose edit is not the default,
and a picture going down for a frame that has been exported. A frame with
both shows both side by side; a frame with neither shows nothing. They sit
on a scrim a little lighter than the badges' and in the secondary text
color, opposite the badges (rating, flag, color label) at the bottom left,
so the two never cover one another. The mark is a component of its own,
`ThumbMarks` in `controls.slint`, beside `ThumbBadges`.

## Where the exported fact comes from

The sidecar already records every export on the state it was made from
(§198, "An export as a line in History"). That record is a fact about a
state, though, and an export is a fact about the file: it vanishes on an
undo past the exported state, for good once a new edit replaces the redo
stack (which is not written), and once the state falls off the history's
cap. A user who exports a proof and then undoes would lose the mark at
once. So the sidecar gains one field at its top level, outside the history:

- `last_export`, an object of the same shape as an export record (`file`,
  optional `preset`, `at`): the last export made of the frame, whatever
  state it was made from. `Sidecar::record_export` writes it beside the
  history line it already writes, and only then: an export no state could
  take (the command line's overrides, never the frame's) still leaves the
  sidecar as it was, as §198 says. Left out of a sidecar
  never exported, read loosely like the other optional fields. A join of
  two copies keeps the later of the two.

A sidecar from before the field has none, and the index and the window fall
back to scanning `exported` on the current state and the history, as before;
it gets the field on its next export. The index mirrors the fact the way it
mirrors `edited`: a schema 8 `exported` column read from the sidecar by the
same pass, so it follows the sidecar through a move or a rename. Schema 8
clears the sidecar hashes, so the next pass over each folder reads the
sidecars again and fills it, and nothing else of a file is read.

## What counts as edited

The mark and the `edited` column must say the same thing, and the column did
not quite say what a person means. One rule, shared by the index and the
window (`is_default_edit`, `blend_is_default`, `develop_marks` in
`greycard-library`):

- A history left behind a reset is no develop. Before, any step in the
  history made the frame edited, so an edit reset to the default kept its
  mark and its edited thumbnail (§270 says the camera's picture comes back
  when the edit is reset). Now a frame is edited when its current edit is
  not the default, or it keeps a snapshot, or its sidecar cannot be read.
- The learned denoiser's blend is judged by the ISO. The first open or export
  of a raw seeds it from the ISO (`Noise::blend_for_iso`) with nobody having
  touched it, so every opened or exported raw would have shown the pencil.
  A blend counts as default when it is 1.0 or the seed for the frame's ISO;
  any other value was set on purpose and is an edit. The index judges it
  when it writes the row, where `files.iso` is; the window gets the ISO from
  the frame's row (`RowMeta::iso`, kept in `FromRow` past the read). A frame
  read from disk rather than stood in from a row has no ISO to judge by for the rest of the session, and its
  blend is then left out of the comparison.

Two knock-on changes. The guard that refuses to write a default sidecar over
a frame whose sidecar was not found (`rows::take`) now asks for
`edited || exported` of the row: the ISO seed used to make an exported raw
count as edited, and under the narrower rule an exported but unedited raw
would otherwise have been given a fresh sidecar by a disk hiccup, and its
next save would have dropped its export records. And the seed for a frame
standing in from its row is only a first guess (`!row.edited`); the read
that lands decides it from the sidecar itself (`files::never_developed`).

## How the cell follows the fact

`Thumb` gains `edited` and `exported`. A cell is made from the sidecar once
the window holds it and from the row (`FromRow::edited`, `exported`) while
the row stands in, by `frame_marks`. `show_badges` writes the two with the
meta, and only when the row differs. It is called from `show_history`,
which every save, reset, undo, redo, snapshot and export landing already
ends in, from the read that brings a frame's sidecar in, from the export's
record for a frame the window has let go of, and from the sync paths that
replace a loaded sidecar (the copy taken whole, a join) so a frame off screen
is refreshed too.

## Tests

In the window: a saved edit shows the pencil and a reset hides it; an export
landing shows its mark, both together when the frame is edited too, and a
failed export shows none; the export mark stays through an undo past the
exported state and through 51 further steps (`panel::history`, counting the
elements the window draws by their accessible labels "Edited" and
"Exported"). In the library: a row mirrors an export on the current state, on
a step behind it, after an undo past it, after the cap, and in a sidecar from
before `last_export`; a reset is not edited; an ISO-seeded blend is not
edited and the same frame with the blend moved is; the sidecar in memory
agrees with the row. A schema 7 library with a sidecar that records an export
and one with a history-only reset comes out of one pass with `exported` set
and `edited` cleared. The older migration tests (schemas 2 to 6) gain the
column, and the schema 4 and 5 ones now expect their report to be
`(meta_refreshed, unchanged) = (2, 1)` where it was `unchanged = 3`, since
schema 8 reads the two sidecars again.
