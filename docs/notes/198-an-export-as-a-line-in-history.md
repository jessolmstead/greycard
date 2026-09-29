# 198. An export as a line in History (2026-09-28)

Roadmap line: "An export as a line in History: the file, its preset
and the time, at the state it was exported from, so a click goes back
to the look that was sent out."

Before this an export left no trace on the frame. The status line said
"exported IMG_0001.jpg", the log kept a line, and the sidecar was
untouched: a week later nothing said which of the frame's fifty states
the JPEG on the client's drive was made from.

### A record on the state, not a step of its own

Two shapes were on the table: a labeled step whose edit equals the
state before it, or a record carried beside the history. It is the
second, carried on the state itself: `Step` and the sidecar's current
state each have an `exports: Vec<Exported>` (`current_exports` for the
current one), and an `Exported` is the path written, the export
preset's name when the sheet was one, and the time.

The step was rejected for four reasons.

- An export changes nothing in the edit, so as a step it is one undo
  that does nothing. Ctrl+Z after an export would visibly do nothing
  and a second press would take the real change back; every undo
  counter and "can undo" state would be off by one per export. Making
  undo skip such steps means every walk of the history (`undo`, `redo`,
  `go_to`, `take_sources`' merge of equal neighbors, which would
  otherwise eat them outright) learning a second kind of state.
- `record_as` refuses an edit equal to the current one; a no-change
  step needs a way round the one guard that keeps the history free of
  duplicates.
- The export's edit is often not the current one by the time the file
  is written: a full-size develop takes seconds and the sliders keep
  moving. A step would have to be inserted mid-history, between the
  state that was rendered and the one the panel moved on to, which is
  a rewrite of the line, not an append.
- An older build reads a step as a state. A no-change state reads there
  as a "No change" row and an undo that does nothing, until that build
  saves and it is kept for good.

On the state, none of that arises. Undo, redo and `go_to` move the
state with its records (`record_as`, `undo`, `redo` carry the field the
way they carry the label), the cap is unchanged, and the row sits
exactly "at the state it was exported from". `take_sources`, which
merges equal neighbors, merges their records too, earlier first.

`Sidecar::record_export(edit, exported)` notes the record on the state
that *is* the rendered edit: the current one if it is, else the
nearest back through the history, else forward through the redo
stack. So the case in the brief, a slider moved while the export ran,
lands on the rendered state and not on the current one. To make sure
that state exists, the Export button now records the panel's edit as a
state first (`save_edit`, as undo, a history click and a snapshot
already do); otherwise a panel that moved on before the save timer
fired would have saved over the rendered edit before it was ever a
state.

The cost of the choice: a state that leaves the history takes its
records with it. Undo past an export and then change something, and
the exported state goes with the rest of the redo stack; 50 steps
later the cap drops it the same way. That is the history's own rule
for every state, and snapshots remain the way to keep a look for good.
If the state is already gone when the file finishes (undone and
replaced during the export), `record_export` returns false and nothing
is noted, with a line on the log: a row whose click cannot go back to
what went out would be the one thing it promises and does not do.

### The rows

`Sidecar::rows()` is the panel's list, newest first: each state under
its exports, newest of those first. `state_at_row` now goes through it,
so an export's row resolves to its state and the history click and
hover need no second path: a click on the row is a click on its state,
same `go_to`, same "whatever the panel holds is a state first".
`history-names: [string]` became `history-rows: [HistoryRow]` (name,
exported, time, undone), and `history-current` is the current state's
row.

A row is muted when its state is undone, not when it sits above the
current row: the current state's own exports sit above its row and
are as live as it is, and the panel used to mute everything above the
current row, which drew them as redo steps. `undone` is worked out in
Rust from the row's state against the history's position.

An export's row reads "IMG_0001.jpg · Web", in italics, after the
export icon (`image-down.svg`, text-muted), with the time on the
right. It began as "Exported IMG_0001.jpg (Web)", and on the panel's
width with the icon and the time beside it that elided the preset on
an ordinary camera name ("Exported 5M0A3976.jpg ..."): the word says
what the icon and the italics already say, so it went, and the file
keeps its extension, since a TIFF and a JPEG of one frame are two
different things to have sent. `history::EXPORT_ROW_CHARS` (22) is
what the row holds before it elides, measured on the snapshot; a
camera's name, its extension and a preset of up to six characters fit
whole, which the test holds it to. The time is HH:MM when it was
today, the date otherwise, local time. Hover
says the whole path and the local date and time ("…/IMG_0001.jpg,
exported 2026-09-28 22:10 with the export preset Web; click to go back
to the state it was exported from") and shows the state in the
viewport as any hover does. The rows' dates are local (chrono, already
a dependency); the snapshots' hover still says UTC through `date_of`,
left as it was.

The preset is named only when the sheet is that preset as saved
(`preset_in_use`: chosen and not "(edited)"); a sheet moved off it
names none, and a batch `--export FILE` whose extension changed the
preset's format names none either.

### Where it is written, and on which thread

Nothing new on the worker. `Job::Export` carries the frame and the
preset, `Outcome::Exported` hands them back with the edit rendered, and
`SetFrameDone` carries the frame's edit; `queue::Set` holds the set's
preset. The record is made in `deliver`, on the window's thread, where
every other save of a frame's sidecar is made, and written with
`write_sidecar` (the XMP when that is on, `save_in` with `saved`
incremented, then the indexer told), so it cannot race the save timer
or any other save. A failed, skipped or canceled export, single or a
set's frame, records nothing. A frame no longer in the window's list
(another folder opened mid-set) has its sidecar read from disk, noted,
and written back through `save_in` with the indexer told; a sidecar
that will not read is left alone. A batch `--export` records too;
`--no-sidecars` is the one way out, as it is for every other write.

### The command line's overrides are the panel's, not the frame's

`--develop-temperature` and `--exposure` used to be written into the
first file's sidecar state in memory at startup (`sidecars[i].current`
set in place). Nothing saved that as long as nothing saved the frame,
and a batch run saved nothing but the edit it closed with, which
equaled the changed current and so recorded nothing. The export record
changed that: `record_export` found the changed current, matched the
rendered edit to it, and `write_sidecar` put it on disk. A frame at
0.5 EV with a record came back at 1.0 EV with no step for it, and the
old record claimed a look that was never exported.

The overrides are now `startup::Overrides`, held on the state
(`overrides_at_start`) and laid over the edit `open_row` puts on the
panel and sends to the worker, not onto the sidecar. The edit that
makes is kept as `overridden`, with the frame's path, and the ISO seed
landing on the open updates it with the panel.

The overridden edit is never recorded as a state of its own, and the
rule for that is one-shot. Every place that records the panel before
it acts goes through one function, `edit::panel_state`: the save
timer, the save on a frame switch and at the run's close, the Export
button, undo and redo, a click in the history, taking and restoring a
snapshot, a preset laid over the frame (alone or with a set) and a
paste (`edit::record_panel` for the ones that record without writing).
While the panel still shows the overridden edit exactly, there is
nothing of the panel's to record, and the action goes on from the
sidecar's own current state: an undo as the first thing done steps
back from the sidecar's current, and a redo stack the sidecar carried
is still there to walk. The first time the panel's edit differs from
it, the flag goes and the new edit is recorded on top of the sidecar's
current state like any edit, carrying whatever of the override's
values are still on the panel, as anything the user did over what the
panel showed would. The flag goes too whenever one of the sidecar's
states is put on the panel instead (`take_current`, after an undo, a
redo, a click, a preset or a restore), on leaving the frame
(`open_row`) and on entering culling. After that the override's values
are values like any others: 1.0 from the command line, moved to 1.5,
moved back to 1.0 is on disk at 1.0.

The first version of this had the guard in `save_edit` alone, and
never cleared it: 1.0, then 1.5 (saved), then back to 1.0 was
swallowed as "the override, unmoved", and the next session opened at
1.5, the frame keyed by its index so the guard outlived a switch away
and back; and an undo, a click, a snapshot or a preset, which record
the panel without `save_edit`, recorded the override as a state and
dropped the redo stack the sidecar carried.

A batch export under an override renders an edit that is no state of
the history, so `record_export` notes nothing and the log says why
("exported under an edit that is no state of its history (undone and
replaced since, or the command line's); not recorded").

Two edges the rule settles one way on purpose. A snapshot taken while
the panel still shows the override is a snapshot of the sidecar's
current state, not of the panel: recording the panel would make the
override a state. Restoring a snapshot that is the sidecar's current
state already still puts it on the panel when the override is up, so
the click does what it says rather than answering "is the current
state" over a panel that shows something else.

### An export of a frame never opened creates its sidecar, seeded

A set can take frames the session never opened. Their learned-denoise
blend is still to be seeded from the ISO; the worker seeds it for the
render (`seeded`), as a first open would, but the window only knew the
edit it asked for. Recorded onto that, the sidecar went to disk with
the plain default blend of 1.0, and at the next load `load_sidecar`
saw a sidecar and did not seed again: 5M0A3976.CR3, ISO 200, would
have kept 1.0 in place of 0.35 for good.

`SetFrameDone` now carries the edit the worker rendered, seeded, and
`take_seed` gives the frame's current state that blend in place and
not as a step, as `Outcome::Opened` does, before the record goes on,
for a frame in the list whose `seed_blend` is still set and for one
off the list whose sidecar `never_developed` would call unseeded. The
sidecar written is what the first open and its save would have
written, plus the record.

So an export of a frame nobody has touched now creates its sidecar.
That is the right side of the line: the sidecar is the frame's truth,
the record is the reason for it, and a frame sent out is one whose
look somebody will want back. What it writes is the edit the frame
already had, seeded as its first open would seed it, so opening the
frame later shows exactly what the file was made from.

### Older builds

`exported` is a key on the sidecar and on each history entry, left out
when empty, so a sidecar with no exports writes byte for byte as
before. An older build ignores the key on both (neither `Sidecar` nor
`Edit` denies unknown fields) and drops the records at its next save,
the same as labels. This build reads the key loosely, a record at a
time: one that will not read is left out, a key that is not a list is
no records, and neither costs the edit.

### Checked

- One real single export, `5M0A3976.CR3 --export out/single.jpg
  --export-preset Web`, and one set of two, `shoot --export out/set/
  --also 2 --export-preset Web` (5M0A1023, on screen and never edited,
  and 5M0A3976, off screen with the single's sidecar), copies of the
  raws from ~/Pictures/Test under target/scratch, XDG redirected. Both
  sidecars read back with one record per export under `exported` on
  the current state, preset "Web", the path and the time; 5M0A3976's
  has both its records and `saved: 2`.
- A `--snapshot` of 5M0A3976 afterwards: two export rows above
  "Original", marked, italic, times on the right.
- After the review, on fresh copies: a set of two frames never opened
  (`shoot --export out/set/ --also 1 --export-preset Web`) wrote both
  sidecars with the ISO seed as current (0.35 on 5M0A3976, ISO 200), no
  step, one record each. Then `5M0A3976.CR3 --export out/bright.jpg
  --exposure 1.0` wrote the file and left the sidecar byte for byte as
  it was (sha256 before and after), with the not-recorded line in the
  log. Then a single export with the Web preset, and a snapshot: the
  rows read "single.jpg · Web" and "5M0A3976.jpg · Web" whole, in the
  live color, above "Original".
- After the second review, on the one-shot build: an export to create the
  sidecar, then `--exposure 1.0 --develop-temperature 4000` on a batch
  export of the same frame left the sidecar's sha256 unchanged, with
  the not-recorded line in the log.

Tests: `history.rs` `an_export_is_named_by_its_file_and_its_preset`,
`an_export_row_goes_back_to_the_state_it_was_sent_from` (rows, click,
undo and redo around the record, a lost state records nothing);
`lib.rs` `export_records_round_trip_and_an_older_build_reads_past_them`,
`states_that_merge_keep_both_their_exports`; UI
`an_export_is_a_row_on_the_state_it_was_written_from` (the exported
edit recorded while the panel moved on, failed/skipped/canceled record
nothing, hover and click restore the look, one undo is one change),
`a_set_records_a_row_on_each_frame_it_wrote`,
`a_frame_no_longer_shown_is_noted_on_the_disk`,
`an_export_s_moment_reads_in_local_time`,
`a_command_line_exposure_leaves_the_sidecar_as_it_was` (the sidecar's
bytes unchanged through the export and the closing save, which fails
without the guard; then 1.0, 1.5, 1.0 on the panel, and 1.0 on disk),
`a_frame_left_and_come_back_to_is_its_sidecar_s_again` (nothing of the
override's recorded on the way out, the sidecar's edit on the way
back, and the override's values saved like any others after),
`a_first_undo_under_an_override_records_nothing_and_keeps_the_redo`
(no state for the override, the carried redo walked back after),
`an_export_of_a_frame_never_opened_keeps_its_iso_seed` (in the list
and off it, read back through `load_sidecar`). The undone flag is
checked in the first UI test: the current state's export live, a redo
state's muted.

### Not done

- The row does not ask for the file itself (open it, show it in the
  file manager); the path is on hover only.
- Records go with their state when the history drops it; there is no
  "exports" list that outlives the history.
- A preset name longer than six characters, or a file name longer than
  a camera's (a renamed export, an "-1" the policy added), still elides
  the end of the row; the hover has it whole.
- `Job::Export` renders the worker's open input, not the job's source,
  and a frame switch is not blocked while the single export's chooser
  is up: switch frames with the chooser open and frame B's pixels can
  be written under A's edit, and now recorded on A. Older than this
  item; the job should carry its source and the worker refuse a
  mismatch.
- The time on a row says HH:MM for today and the date otherwise, as of
  the last time the history was drawn: across midnight a row keeps
  saying yesterday's time until something redraws the history.
- `--develop-temperature` and `--exposure` with `--cull` are not laid
  over anything: culling develops the sidecar's state on leaving, and
  the overrides go only on the panel's.
- Not clicked through by hand in a live window: the rows were checked
  by snapshot and the click, hover and muting by the headless tests.
