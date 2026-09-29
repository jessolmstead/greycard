# 179. The two bugs at the top of the roadmap (2026-09-26)

**rawler's panics, caught at the decoder.** §163's review found a
half-copied CR3 panicking the worker with "capacity overflow", filed
upstream as dnglab/dnglab#849: a full-length CR3 with zeros past the
copied part has a zeroed CTMD record, and rawler's `Ctmd::new`
subtracts 12 from its size. §172 could not reproduce it by cutting
files short, and neither could this pass: 21 sample CR3s cut at 106
lengths each fail cleanly in every entry point. Zeroing the tail at
the same length instead, as Windows' CopyFile, `rsync --preallocate`
and some card importers leave a file until they finish, panics every
CR3 in all five of `probe_path`, `stance_path`, `preview_path`,
`decode_path` and `decode_path_with_metadata`, and panics the full
decode of the NEF, the RAFs and the RW2 among the samples ("Can't
refill bitpump", "range end index"); their probes and camera previews,
at the head of the file, still read. The ARW never panics, and 22 of
its 25 cuts decode without complaint to a picture blank below the
copied part; that is a line on the roadmap now.

The roadmap asked for a guard at the job. The jobs already had one:
the worker, the thumbnail pool, the culling prefetch, the index and
the import each call inside `catch_unwind`. What had none was the UI
thread, where `stance_path` runs when a frame is turned or its XMP
written and `probe_path` when a sync or a startup preset seeds the
denoiser's blend; a panic there ends the editor. So the guard went
one level down, into `rawler_backend`, where CLAUDE.md keeps rawler:
every public entry point runs rawler under `catch_unwind`, and a panic
comes back as `Error::Decode`, the error every caller already handles.
The job-level catches stay, as a second net.

The error says what can be said. If the file's last 64 KiB are all
zeros it "ends in zeros where a finished one has data: it looks
half-copied, or still being copied", which is what the status line
now shows for such a frame; otherwise "the decoder gave up on this
file", with rawler's message either way. No finished raw ends in
64 KiB of zeros: its tail is compressed samples or a preview JPEG. The
test is only made after a panic, so a false call would cost a wrong
reason on a file that was failing anyway.

Rust's panic hook runs before the unwind is caught, so the editor's
hook (which logs a panic as an error with a backtrace) and the CLI's
default one printed each caught panic as though it were a crash. The
guard sets a thread-local while rawler runs, `decode::inside_decoder`,
and both hooks read it: the editor logs such a panic at debug and the
CLI prints nothing, since the error that follows says it. The
library's test of its own catch now sees the decoder's error instead
of a panic. The core's tests throw a panic through the guard with a
zero tail and without, and run the library's scrubbed RW2 fixture, on
which rawler divides by zero, through four entry points; no sample
raw is in the repo.

**Upstream.** The CR3 fix went to rawler as dnglab/dnglab#851: the
CTMD record loop stops at a size under its 12-byte header or past the
data left, as the block loop under it already does, with three unit
tests. With it, three CR3s zeroed past their first MB read their
metadata, preview and samples without error, so a half-copied CR3
will no longer panic but open as a picture blank below the copied
part, as the ARW does; that roadmap line names both. The NEF, RAF and
RW2 panics are dnglab/dnglab#852, an issue and not a patch: the bit
pump's `refill` panics by design when its data runs out, and whether
it should return an error is rawler's call. The guard here stays
whatever lands, since rawler has panics nobody has found yet.

**Undo while culling.** Ctrl+Z in culling used to step the current
frame's develop history and leave culling to show it, which is not
what a hand on the rating keys means by undo, and the ratings, flags
and labels themselves had no undo at all. They are not in a sidecar's
history on purpose: a rating is a fact about the frame, not a step in
developing it (§117). So they get a history of their own, `tags`, in
the session's state: `set_meta`, the one way the keys and the frame
menu change them, records each change as one step over every frame it
moved, with the three fields before and after. In culling Ctrl+Z pops
a step and puts the frames back, Ctrl+Shift+Z (and Ctrl+Y) makes it
again, and a new change drops what was left to redo. The browser's
badges, the reject count and the filter follow as they do for a key,
through the same code. Outside culling Ctrl+Z is still the develop's.

A step names its frames by path, since the file list changes under
it: a rejects move, a root added, a filter. A frame no longer listed
is passed over, and so is one whose tags are no longer what the step
left, so a rating that arrived since through another tool's XMP or a
paste is not undone by a step that did not make it. An undo moves the
selection to the frame it changed when that is not the one on screen
and the filter still shows it; a culler who rated, moved on and
pressed Ctrl+Z wants to see the frame come back. The stack keeps the
last thousand steps and lives for the session.
