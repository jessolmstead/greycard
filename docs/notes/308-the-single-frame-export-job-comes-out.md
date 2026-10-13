# §308. The single-frame export job comes out (2026-10-12)

We closed the follow-up §301 left open. The window already sent every
export as a set (the frame on screen as a set of one), so the
single-frame job lived on only for the headless parity test and the
history tests.

What moved: the parity test (`an_export_with_no_window_writes_the_window_s_pixels`,
through `window_export`) now sends a set of one and waits on the
frame's `SetFrameDone`; its edit already carries the blend the open
seeded, so the frame is sent with `seed_blend` off and is not seeded
twice. The window side still opens and develops on a lent device and
the headless side still runs the batched worker's own path, so pixel
and byte equality between them still proves what it did. The history
tests (`an_export_landing_shows_its_mark`,
`the_export_mark_outlives_the_state_it_was_made_from`,
`an_export_is_a_row_on_the_state_it_was_written_from`, and the two
sidecar tests) hand in a set-of-one `SetFrameDone` through one helper;
the skipped case joined the failed and canceled frames already checked
for recording nothing.

What came out: `Job::Export`, its worker arm, its queue arm and its
blame arm (`Blame::Export`), and with them the three outcomes only it
produced (`Exported`, `ExportSkipped`, `ExportFailed`) and their arms
in the window's delivery. 223 lines removed, 84 added, in four files.
Nothing the user sees changes: the set path already wrote the same
status line, log lines and history entry. The review confirmed the
headless `--export` contract of §236 (exit 0 on a write and on a skip,
1 on a missing input or an unwritable folder, the last line on stderr)
with real runs. Where §236 says the window's half was "`Job::Export`
of the open frame", read a set of one.

Left open, and older than this change: a windowed `--keys` capture
whose export frame fails does not set the failed exit code, because
the set path has no batch exit-code handling. §236 made `--export`
headless, so the case is likely moot.
