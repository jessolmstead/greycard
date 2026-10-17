# §324. The camera match's survey over a network share (2026-10-16)

A tester on macOS opened a folder of Canon EOS R6 Mark II CR3s on a NAS,
with no library roots, and opened "Fit this camera's look". The sheet
said "No camera with a fixed picture style here." and kept saying it.
Their sample frame is Canon Faithful with Auto Lighting Optimizer and
Highlight Tone Priority off: `CameraStyle::read` and `probe_path` both
read it, and `survey_files` on it gives one group, fixed. The line
under the groups in their screenshot read "reading the frames' picture
styles...": the survey had not finished, and the sheet shows its empty
text while it runs.

**Why it is slow.** A folder no index covers is surveyed from the
files (`camera_match::survey_files`), one raw after another, each with
two reads:

- `CameraStyle::read` walks the CR3's boxes up to the maker note and
  seeks past the rest: about a millisecond, a few hundred KB;
- `probe_path` (for the body, the date and the lens) opens the file
  with rawler's `RawSource::new`, which maps it with `populate()` and
  advises `WillNeed` and `Sequential`: the whole file is read. The
  sample is 27 MB.

At gigabit speed that is about a quarter of a second a frame, and over
Wi-Fi a second or more, so a folder of a few hundred frames reads for
minutes with nothing on the sheet to say how far it has got.

**What rawler reads of a CR3 for its metadata.** Not only the head.
`get_decoder` needs the `mdat` box's header to finish the file box, and
`raw_metadata` reads the CTMD record, which sits near the end of
`mdat`. On the sample:

| Bytes given to rawler | Result |
|---|---|
| up to the end of `moov` (62 KB), or up to `mdat` (496 KB) | no decoder found |
| through `mdat`'s header, or the first 1 MiB | CTMD offset behind EOF |
| the head through `mdat`'s header and the last 1 MiB, the gap zeroed | the same metadata as the whole file: make, model, lens, every EXIF field |

So about 1.5 MB of 27.

## What we build

1. **A probe from the file's two ends.** `decode::probe_ends_path`,
   beside `probe_path` in `rawler_backend.rs`, gives the same `Probe`.
   It reads the head and the tail of the file into a buffer of the
   file's length, the gap zeroed (`vec![0; len]`, which the allocator
   hands out as untouched zero pages), and hands it to rawler as
   `RawSource::new_from_slice(..).with_path(path)`. For a CR3 the head
   runs through the `mdat` box header, found by walking the top-level
   boxes, and the tail is the last 1 MiB. Any error, or a panic under
   `guarded`, falls back to `probe_path`, so a format or a file this
   does not suit costs a little more than it does now and never gives
   a different answer.

   The other formats (NEF, ARW, RW2, RAF, DNG) get their own ends, or
   none, after measuring on the sample set: for each, the smallest head
   and tail rawler gives the whole file's `Probe` for. A format with no
   pair that works keeps `probe_path`. The table of what each reads
   goes in the code's header.

   Two routes we did not take:
   - **The identity from our own reader.** The style reader already has
     CMT1 and CMT2 in hand. But rawler's names are cleaned through its
     camera table and its lens table: "Canon EOS R6m2" in the EXIF is
     "EOS R6 Mark II" from rawler, and "RF28-70mm" is "RF 28-70mm". The
     index, the groups and every table's `MadeFor` carry rawler's
     names, so a second spelling would split a body's groups.
   - **A lazy map.** A `RawSource` mapped without `populate()` or
     `WillNeed` would fault in only what rawler touches, but it needs a
     constructor rawler does not have, and a mapped file on a share
     that drops is a SIGBUS that `guarded` cannot catch.

2. **The survey uses it and says how far it has got.**
   - `survey_files` calls `probe_ends_path`.
   - It reports the count as it goes ("Reading picture styles: 120 of
     800"), in the line under the groups.
   - The empty text, "No camera with a fixed picture style here.",
     shows only once a survey has finished and found no group.
   - While the survey runs the list says it is reading; when it fails,
     the error takes the empty text's place.
   - The survey skips macOS's `._` companion files: a `._3U2A3625.CR3`
     on an exFAT or SMB volume has a raw's extension and is not one.

3. **A few files at once.** The survey reads four raws at a time on
   scoped threads (`std::thread::scope`) that end with the survey, and
   no pool that outlives it: tests run surveys by the hundred, and
   0.6.0's macOS test runs ran out of threads on pools kept per test.
   Over
   a network the latency, not the bandwidth, is the limit. The groups
   come out in the same order as now.

## What this leaves out

- The indexer's `probe_and_mosaic_path` reads whole files the same
  way, for every raw it adds. Moving it onto the two ends is the next
  step, under the library track's network line, once the probe has
  had a release on testers' files. It touches `greycard-library`'s
  `index.rs`, where the kind filter and the catalog import are
  working now.
- A library's survey (`survey_library`) reads the index, not the
  files, and is not slow.

## Sequencing

Files touched:
- `greycard-core`: `decode/rawler_backend.rs`, `decode/mod.rs`;
- `greycard-ui`: `camera_match.rs`, `panel/camera_match.rs`,
  `ui/panel/match-sheet.slint`, and new sheet properties in
  `ui/app.slint`.

Of 0.8.0's work in flight:
- X-Trans step 2 may touch `rawler_backend.rs`;
- the look preview adds lines to `app.slint`;
- the kind filter, the catalog import and the `.pp3` import are in
  `greycard-library` and the CLI, and do not meet this.

It can run beside any of them, and lands after X-Trans step 2 if that
one changed `rawler_backend.rs`.

## Tests

- Unit tests:
  - the box walk that finds the head's end, on a scrubbed CR3 head
    fixture (no raw in the repo);
  - the fallback, on a file whose ends do not parse;
  - the sheet's three states (reading with a count, failed, finished
    and empty);
  - the `._` skip;
  - the parallel survey's order.
- An ignored test over the sample raws (`GREYCARD_TEST_RAWS`) asserts
  that `probe_ends_path` and `probe_path` give the same `Probe` for
  every file, and reports the bytes each read.
