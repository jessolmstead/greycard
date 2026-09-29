# 167. Export over the selection (2026-09-24)

Export took the frame on screen and nothing else, though the browser has
held a set of frames since the multi-select (§156). With two or more
frames selected, Export now writes the set: each frame under its own
sidecar's current edit (the frame on screen under the panel's state, as
before), the sheet's settings for all of them, and the on-exists policy
applied per file. With one frame selected nothing changed: the same sheet,
the same file chooser, the same job.

**What the window shows.** The sheet's title and button say what it will
do ("Export 3 frames", "Export 3 frames...") and the line under the title
names the frame on screen and how many others. The button asks the
desktop for a folder rather than a file. While the set runs the panel's
Export button is "Stop export", the status plate's spinner runs, and the
status line reads "exporting 3 of 5: 5M0A3021.jpg" (the name the policy
chose, so "A (2).jpg" when that is what is written); at the end it reads
"exported 5 frames to /path in 12.4 s", with ", 1 skipped (there
already)" or ", 1 failed (NAME: why)" (or "(see the log)" for more than
one) after it. Stop, or Escape with no sheet up and no tool in hand,
finishes the frame in hand and passes over the rest: "export stopped: 3
of 5 exported to ...". Escape stops the set before it collapses the
selection, since the sheet that would otherwise take the key is gone by
then; once the set is stopping, Escape is the window's again (it
collapses the set or leaves the grid) while the frame in hand finishes.
One folder chooser is up at a time: Export does nothing between asking
for a folder and the answer, so two sets cannot start and leave the first
unstoppable. (The single export's file chooser has the same gap, and is
left as it was.)

**Names.** In the chosen folder a frame is written under its own stem
and the format's extension. Every source of the set is taken before any
name is given, so no frame's export can land on another frame's file: the
review found a raw `X.CR3` and its camera `X.jpg` exported into their own
folder under Overwrite, where the raw's export wrote over the camera JPEG
and the JPEG frame was then read back from that new file. Now the raw's
export is `X (2).jpg`, the policy's own spelling, and the JPEG's is
`X.greycard.jpg`, the name the editor already uses beside a file when
there is no chooser; with no folder chooser at all each frame goes beside
its own file under that name, with the same reservation (a source already
named `X.greycard.jpg` is not written over by `X.CR3`'s export). Two
frames of one stem that are not each other's file are told apart with
` (2)` the same way. The comparison is case-blind because two of the three
platforms' file systems are (`A.jpg` and `A.JPG` are one file on macOS and
Windows); on Linux that errs on the safe side, so a set that exports
`IMG.JPG` into its own folder as a JPEG writes `IMG.greycard.jpg` where
the single export's chooser suggests `IMG.jpg`. The on-exists policy is
then applied to each name at the moment its frame is written, so a file
that appears mid-set is seen.

**The queue.** The worker already ranked its jobs: the newest develop,
then masks, then exports, then thumbnails, one at a time. Thumbnails
ranking after exports means a long set holds back any thumbnail that
queues behind it (a folder opened mid-set fills its strip once the set
is done). A set is sent
as one job and the worker queues it as one export a frame, each holding
the shared `queue::Set`. So a develop asked for while a frame is being
written (a slider dragged mid-set) runs as soon as that frame is done and
before the next one; the set never holds the viewport for longer than one
frame's export. The set is an `Arc` the window keeps as well: Stop sets an
atomic flag on it, and the worker, taking each frame off the queue, passes
over any frame of a canceled set without beginning it. The frame in hand
is never interrupted, which is what "finish the file in hand" asks, and
needs no check inside the develop. The worker says the set is done after
its last frame whichever way that frame went, so the finished line comes
exactly once and always after the frame in hand; frames passed over are
free, so a canceled set ends as soon as the queue reaches them. The
tally (exported, skipped, failed with name and reason, canceled) lives
on the set and is recorded by the worker as each frame finishes, and the
pure half of it (the step, the names, the lines) is `queue.rs`, tested
without an engine.

**Which picture.** The frame that is the worker's open file is exported
as the single export always was: from the last develop when it was of the
same develop and turn, else developed afresh on the CPU reference path.
Any other frame is decoded and developed on the side with a base and a
learned-denoiser pair of its own, so the open file's cached base, its
learned masks and its fills are where they were for the next slider move;
a set run past the open frame costs it nothing. The learned models for
those frames live in a second `Ai` made on the set's first such frame and
dropped with the set: the open file's own `Ai` holds per-file state
(masks by adjustment id, fills by patch id, a preview by base stamp) that
would collide with another file's, and clearing it would re-run the open
frame's masks on its next develop. The cost is that a set whose edits use
the learned denoiser loads it a second time for the length of the set.
A raw with no sidecar yet gets its learned-denoiser blend seeded from its
ISO, as its first open would, whether or not it is the open frame. The
render, the geometry, the learned
masks, the watermark and the write are one function now (`write_export`)
shared by the single export and the set, so the two cannot drift. A panic
in one frame is that frame's failure and drops the develop state, as a
panic in any job does; the set goes on.

**Failures.** A frame that fails (no decoder, a develop error, a write
error, a watermark that cannot be drawn) is one error line in the log and
the terminal, "export 2 of 3 failed: path: why", and the rest of the set
is written. The finished line counts them and is a warning rather than an
info line when anything failed or was passed over, so a command-line run
says it without -v.

**The command line.** `--export DIR --also ROWS` writes the set (the
opened frame and the rows) into DIR: a folder that is there, or a path
ending in a separator, which is made. `--export FILE` is exactly as it
was, one frame, whatever `--also` says. `--export-preset`, `--long-edge`
and `--on-exists` fill the sheet the set is written under as they fill it
for one frame; the format is the sheet's, since a folder has no extension
to name one. The run's exit code is a failure when any frame failed.
`--also` is no longer hidden from `--help`, since it is now how a script
names a set, and with `--export` it is never quietly dropped: a target
that is not there and names no picture format is taken as a folder when
`--also` is given (without it, such a path is the one file it always
was); `--also` with a file target (`--export out.jpg`) is an error, and
so is a row the opened folder has not got (a single file opened is a
folder of one), each saying why. A row the filter hides is warned and
left out. A look an edit names and the look directory has not got is
warned for each frame of the set, as the single batch export warns for
its one. `--export DIR --snapshot` is a developer's pair: the snapshot
quits the run once the set has begun, with the frame in hand written and
the exit code 0.

**Checked.** The camera-JPEG case the review found: `X.CR3` beside a
JPEG `X.jpg`, `--export` into that folder `--also 1 --on-exists
overwrite` wrote `X (2).jpg` and `X.greycard.jpg` and left the camera
JPEG's bytes as they were (same SHA-1 before and after). `--also 0,1,2`
on a single file opened, and `--also 1` with `--export one.jpg`, each
exit 1 with their reason and write nothing; `--export newdir --also 1,2`
(no separator, not there) makes the folder and writes three frames into
it. On three R6 Mark II raws copied from the sample folder
(6000x4000 each), `--export out/ --also 1,2` wrote three JPEGs whose
`Exif.Photo.DateTimeOriginal` and `ExposureTime` are each their own
source's (exiv2) and whose `Xmp.greycard.Source` names each source; the
set's pixels are identical to three single exports of the same frames
(ImageMagick `compare -metric AE`: 0 differing pixels for each). A set
with an undecodable file in the middle wrote the other two, logged the
one, ended "exported 2 frames to out4/ in 2.2 s, 1 failed (5M0A2999.CR3:
unsupported: ...)", and exited 1.

**Measured.** Wall time of the whole process, a three-frame set against
three single `--export FILE` runs of the same frames, release build, RTX
5070 Ti, interleaved round by round. The machine was shared with other
builds (load average 15 to 37 during the runs), so the numbers move.

| long edge | set of three | three singles (sum) | per frame in the set |
|---|---|---|---|
| 2048 | 4.59 s | 6.71 s | 1.09, 1.19, 1.17 s |
| 2048 | 5.33 s | 6.98 s | 1.31, 1.49, 1.40 s |
| 2048 | 4.45 s | 6.46 s | 1.07, 1.17, 1.14 s |
| full | 6.04 s | 16.47 s (one single at 8.7 s) | 1.58, 1.69, 1.61 s |
| full | 5.96 s | 17.40 s (two singles at 6.6 and 7.8 s) | 1.51, 1.64, 1.75 s |
| full | 7.27 s | 10.80 s | 2.01, 1.98, 1.60 s |
| full | 10.28 s | 8.23 s | 2.19, 3.15, 2.16 s |

At 2048 the set saves about 2 s over three runs: what a single run pays
three times is the window, the device and the viewport's GPU develop of
the first frame (together about 1 s a run); what the set pays per frame
(1.1 to 1.2 s: decode, the CPU reference develop, resize, sharpen, encode)
is what a single run pays for its one export. The full-size rows are too
noisy under that load to put a figure on beyond "the set is not slower
per frame than a single run"; the last row is a round where the set ran
during a spike.

**Tests.** `queue.rs`: the cancel on a fake set of five (the third in
hand is finished, four and five are passed over, done said once at the
last), the failure count and the finished line's words, and the names (a
raw and its JPEG, a case-blind clash, a JPEG into its own folder, no
folder at all, a raw's export kept off the camera JPEG beside it in
either case, a `.greycard` source kept in beside mode). `worker.rs`,
through a real worker on files of junk: a
set runs in order and counts three failures; a cancel while the second of
four is in hand finishes it and passes over two; an open asked for while
the first frame is in hand runs before the second. `panel/deliver.rs`: a
set reads the panel's edit for the frame on screen and each other frame's
sidecar edit and turn; Escape stops a running set and does not collapse
the selection, and the next Escape does while the frame in hand runs on;
the set's end clears the Stop button and says the line.
`panel/startup.rs`: `--also` with a file target and with a row the folder
has not got are errors, and a missing path with no format is a folder
only with `--also`.
`tests/export_set.rs` (ignored, as the tests that want raws are; `cargo
test --release -p greycard-ui --test export_set -- --ignored` with
`GREYCARD_SAMPLES`, a display and exiv2): the command line over three
different raws of one kind from the sample folder writes three files,
each with its own source's DateTimeOriginal and ExposureTime (so a mix-up
between frames shows) and a 1024 long edge, and `--export FILE` still
writes one; its scratch folder is under Cargo's test temp directory and
is removed however the test ends. It passed on this machine: the set in
8.3 s, one frame alone in 2.9 s, release build, the machine shared.

**Left.** A set writes into one folder or beside each file; a naming
template (a suffix, a sequence number, the date) is not built, and the
sheet has no field for one. There is no progress bar on the sheet itself,
since the sheet closes when the set starts; the status plate carries it.
The frames of a set are written one at a time on the worker's thread; the
develop inside each uses every core already, so running two at once would
trade the viewport's answer for little. Frames other than the open one
develop on the CPU reference path. The open frame is exported as the
single export always was, from the viewport's last develop when that
came back to the CPU, and that develop's CA may have run on the GPU: the
review showed it on 5M0A1023 with the learned denoiser on and sharpen
off, where the single export took 0.24 s with no CPU develop, and the
same frame exported as a non-open frame of a set differed from it by 45
pixels (AE, at 2048) while matching a `--cpu-ops` single export exactly.
So a frame's pixels in a set can depend on whether it was on screen. The
gap is in the single export's reuse of `last` and predates the set; it
is a separate item (the reuse should require a CPU CA, as the base's
`ca_on_gpu` check already does).

**The review.** The reviewer exported one frame with master's binary
and the branch's (0 pixels apart, EXIF equal but for the dates, the
same names under all three policies), then a set of four against a
single export of each frame with that frame open: a hidden-folder
sidecar with a crop, a 3° angle, a turn, a Brush and a Subject; an
XMP-only turn; the user's own sidecar. All 0 pixels apart. What it
found was the names: a raw and its camera JPEG of one stem exported
into their own folder under Overwrite wrote the raw's export over the
JPEG, then read the JPEG frame back from the new file, which on macOS
and Windows is every raw-and-JPEG pair. And Escape swallowed after a
cancel, `--also` dropped without a word on a file target, a test that
could not tell three frames' EXIF apart, and the draft's claim about
the open frame's CA. Thirteen items, one round, and the medians it
took (5.80 s the set, 7.89 s three singles) held the headline.
