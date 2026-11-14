# §667. A sidecar's lists of numbers on one line (2026-11-13)

A tester's sidecar for one frame, a few brush masks and two fills, was
28.3 MB and 809,346 lines, larger than its raw. `serde_json`'s pretty
writer gives every number of every stroke point a line of its own, deep
in indentation.

The rule: the file is the pretty form, except that a list holding only
numbers, or only such lists, is written on one line as the compact form
writes it (`[[0.05054598,0.1175683],[0.052230876,0.1175683]]`). We write
compact and lay the text out after (`layout.rs`), so every number is the
compact writer's own bytes, copied; nothing goes through
`serde_json::Value`, which would widen an f32 to f64 digits. Empty lists
stay `[]`; a list mixing numbers with anything else stays as it was.

Measured on that file, read with the real reader and written both ways
(release build, best of seven writes and five parses):

|            | bytes      | lines   | write   | parse   |
|------------|------------|---------|---------|---------|
| pretty     | 28,306,413 | 809,347 | 26.0 ms | 74.5 ms |
| this rule  |  6,631,746 |  85,234 | 11.7 ms | 66.0 ms |

4.3 times smaller, and quicker to write. Both read back to the same
sidecar. A small sidecar with no masks shrinks too, 22.8 kB to 17.7 kB,
from the curves' points and the mixer's bands.

Only whitespace moves, so every greycard version reads the new file and
the format's version stands. Only `Sidecar::to_json` changes: an edit's
own JSON (`Edit::to_json`), the develop key (compact, from the edit),
presets, settings, and carried state are as they were; the develop keys
and edit JSON of all 51 states in the tester's file hash the same before
and after. No sidecar is rewritten for its layout: every save advances
the save count and a revision, so the layout changes only at a save that
would have happened anyway. The index's hash of a rewritten file differs
once, as for any save.

Rejected: a `serde_json::Value` round, for the f32 widening; a buffering
`Formatter`, more code for the same result. The larger cause, whole
copies of the edit in every history step, is a format question left for
its own section.

Checked in review: 250,000 random documents (every number form, strings
holding brackets, quotes, and escapes) read back the same and, with the
added whitespace taken out, are the compact text to the byte; where no
list is all numbers the output is the pretty writer's to the byte. The
one other difference from pretty: a list of empty lists stays on one
line. The longest line of the tester's file is a stroke's points, 7,282
bytes. Peak allocation for the write fell from 67 MB to 16 MB.
