# §313. Virtual copies, part 4: the version's name in the export (2026-10-12)

§309 §6, built ahead of part 2 because it is pure and the window does
not need to know of versions for it: the naming and the headless flag
are testable on the queue alone, and the one place the window records
an export now takes a version id and records on that line.

**The name.** `queue::version_stem(stem, name, position)` makes
`<stem>-<name>`, the name through `naming::safe` (the import's rule:
Windows' refused characters and control characters to `_`, the ends
trimmed, reserved device names guarded), and a name with nothing left
after that (`""`, `"..."`) falls back to `<stem>-v<N>`, `N` the
frame's position in the set. A name of refused characters alone
(`"::"`) becomes `__` and stands: only an empty result falls back,
which is the rule as written and not a judgment of the name. `named`
became `named_of`, taking a version name beside each source; the stem
is joined before the `.greycard` infix and the numbering, so beside the
raw it is `IMG_0001-Warm.greycard.jpg`, under Increment
`IMG_0001-Warm-1.jpg`, and a version named "1" takes `IMG_0001-1.jpg`
before the original's Increment reaches it, which then goes to `-2`:
every name of the set is taken before any is numbered, as before.

**The frame.** `queue::Frame` carries `version: Option<queue::Version
{ id, name }>`, a small pair rather than the edit crate's `Version`
with its whole line. `sources_of(&frames)` hands `named_of` its list.
The status line names the file written, which carries the name, so it
needed nothing.

**The record.** `Outcome::SetFrameDone` carries the version id, and
both recorders (`history::record_export` for a frame the window holds,
`record_export_on_disk` for one it does not) record on
`sidecar.line_mut(version)`: the file's own line for none, the
version's otherwise. A version gone from the sidecar since the set was
queued (deleted, or taken out by a join) is said in the log and the
export is not recorded, as an edit that is no state of the history is.

**The flag.** `--version-name NAME` with `--export`: each frame goes out
under the version of that name in its sidecar, first by position when
two share it, said in the log; a frame with no version of that name
stops the run with the file named, exit 1. The design said
`--version`; clap owns `--version` for the build's number, so the flag
is `--version-name`.

Tests: `queue::tests::a_version_s_stem_is_the_file_s_stem_and_its_name_made_safe`,
`queue::tests::a_version_s_export_is_named_beside_the_original_s_as_the_file_s_own_would_be`,
`headless::tests::a_version_named_is_exported_under_its_edit_and_its_name`
(the edit, the name in the file, the record on the version's line and
not the file's own, the refusal).

Left for part 2: the export sheet's queue rows say "IMG_0001 (Warm)"
once the window can queue a version at all; the window's frames are
all version 0 until then, so the `version` the sheet sends is none.
