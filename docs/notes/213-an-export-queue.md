# 213. An export queue (2026-09-30)

Roadmap line: "An export queue: Add to queue on the export sheet puts
the set with its preset and destination in a list that waits, shown
with its count, and Export the queue runs it all later, in order, each
entry a line in History as an export is; the queue kept across a
restart, so a day's picks are queued as they are made and sent out
when the machine is free".

Before this an export ran when Export was pressed, or not at all. A
set of forty frames from a day's picking held the worker for minutes
the moment it was asked for, and nothing remembered a set that had
been picked but not sent.

**What an entry is.** An entry is what Export would have sent at the moment Add to queue
was pressed, kept whole: each frame's file, the edit it is to be
written under, its turn and whether its learned blend is still to be
seeded (`set_frames` as it reads them, so the frame on screen comes
under the panel's edit and every other frame under its sidecar's); the
sheet itself, as `sheet::Sheet`; the export preset's name when the
sheet is that preset as saved (`preset_in_use`, the same rule §198
uses for the History row); and the folder chosen.

The sheet is kept, not `export::Settings`. The sheet is what the
settings file already serializes, it holds the on-exists choice as
well, and `Sheet::settings` and `Sheet::on_exists` turn it into exactly
what `read_export_settings` and `read_on_exists` would have given at
that moment. So a change to the sheet after queuing (another size,
another preset) does not change what was queued.

The edit is kept, not looked up again. An entry whose frame has been
edited since it was queued exports with the edit as queued: the queue
is a record of what was decided at the time, and a frame touched up
after it went into the queue would otherwise go out under a look nobody
had checked for sending. As for Export, the panel's edit is made a
state first (`save_edit`) when the frame on screen is queued, so the
History record has a state to sit on when the export lands much later.
If that state has left the history by then (undone and replaced, or
past the cap), `record_export` notes nothing and says so in the log, as
§198 already does for any export.

**Add to queue.** The sheet's footer is now Cancel, Add to queue..., and Export (or Choose
file...). Add to queue goes through the same sidecar request as Export
(`rows::request`) and the same gathering of frames, split out of
`export_frames` as `pressed_frames` so both buttons take exactly the
same set. A watermark with nothing to draw is refused at queue time,
with the sheet left up, rather than failing every frame later.

The destination is always a folder, from the desktop's folder chooser,
whether the set is one frame or many. A queued single frame is a set of
one: its file is named when it runs by `queue::names`, as a set's are,
because a file chooser's answer at queue time is a name that may well be
taken by the time the queue runs, and the set's naming already handles
that under the on-exists policy. With no chooser to ask (no portal), the
entry is queued to go beside each file under the `.greycard` name, as a
set with no chooser goes. A canceled chooser queues nothing. Nothing is
exported; the sheet closes and the status line says "2 frames queued;
1 set in the queue".

Sources are stored as absolute paths: a folder opened by a relative path
(`greycard shoot`) would otherwise name nothing from the next launch's
working directory. At run time each source is mapped back to the
window's own name for it when the frame is in the list, so the worker
still recognizes its open file and the History record finds its row.

**The count.** A row at the foot of the sheet, shown when anything is queued: "2 sets,
3 frames queued", Export the queue, and Clear. Clear asks first, in
place (the row turns into "Clear the queue of 2 sets?" with Cancel and
Clear), the way the presets' bin asks before it removes. Export the
queue is off while any export runs; Clear is off while the queue runs.
No new pane: the sheet is where exports are asked for, so it is where
the waiting ones are seen.

**Running it.** Export the queue makes one pass over the entries, in order. Each entry
is first looked for: its frames' files, its folder, and its watermark's
PNG when the mark is an image. The look runs on a thread of its own and
answers through the event loop, because a stat on a stale network mount
can hang for as long as it likes and the window must not hang with it.
A numbered probe keeps a late answer to a look the queue has since let
go of (a Clear meanwhile) from being taken for the current one.

Then the entry goes through `start_set` (which now hands back the set it
began) and the worker's `ExportSet` and `ExportFrame` jobs: the same
names, the same bar on the status plate, the same Stop and Escape. The
window keeps a `queue_run` beside the set: which entry it is, which of
the entry's frames each frame of the set is, and which are done with.
`SetFrameDone` marks a frame done when it was begun, however it went;
`SetDone` drops the done frames from the entry, the entry when it has
none left, and begins the next entry unless the set was stopped. Stop is
read from the set's own cancel flag, not from the tally, so a Stop
pressed during an entry's last frame still stops the queue after it. A
plain set's outcomes never touch the queue: the run is matched to its
set by pointer.

So Stop finishes the frame in hand, passes over the rest of that entry
(those frames stay queued in it), and leaves every later entry as it
was. A frame that fails is reported as any set's failure (the log line,
the finished line's "1 failed (NAME: why)") and is done with: it leaves
the queue and does not stop it. A failed frame makes no History line,
as §198 decided for a failed export.

**What is not there at run time.** Running later with a drive unplugged is the queue's main use, so nothing
that is only missing for now may drain it.

- A frame whose file is not there (a drive unplugged, a root offline, or
  the file deleted) is not sent and stays queued in its entry, with a
  warning naming it. The finished line says "2 frames not there, left
  queued". An entry with none of its files there is passed over and
  kept, and the next entry is begun. The look does not try to tell an
  offline root from a deleted file: both are kept, and a frame that is
  gone for good stays until Clear.
- An entry whose folder is not there, or whose watermark's PNG is not
  there, stops the run with the reason ("the queue stopped: /media/card/out
  is not there; 2 sets, 5 frames queued left in the queue"), and the
  entry is left whole. Sending its frames anyway would fail every one,
  and failures leave the queue.

**Kept frame by frame.** The file is written after every frame done with, not only when an entry
ends: a window closed mid-entry (the worker stopped with frames still
queued) or a crash would otherwise leave the frames already written in
the file, and the next run would export them again (as "X (2).jpg" under
Increment) and record a second History line. The write leaves out the
running entry's done frames from the file only. The entry in memory
keeps them, because the run still counts the entry's frames by place.

Each exported frame is a History line as §198 makes it, through the same
`SetFrameDone` handling, under the preset name the entry was queued with:
the preset as it was at queue time, even if it has since been changed,
renamed or deleted. A frame queued with its learned blend still to be
seeded comes back under the edit the worker rendered (seeded), not the
queued one, and is recorded and seeded as any set's frame is.

**The file.** `export-queue.json` beside `settings.json` (`settings::path()`'s
directory). Written on every change and after every frame of a running
entry, through a `.partial` file synced to disk and then renamed over
it, and removed when the queue is empty. It is compact JSON, not pretty:
it holds every frame's whole edit and is written often. Read at launch
and shown, never run at launch. A snapshot or batch run reads it (so a
`--sheet export` capture shows the count) and never writes it, as for
the settings. A file that will not read (an I/O error), will not parse,
or has a version this build does not know is moved aside to
`export-queue.json.unread` (`.unread.2` and on when one is there) with a
warning, so the next write does not lose it.

Each frame's edit is read through `greycard_edit::migrate`, as a
sidecar's is: an edit of an older schema is brought up to this build's,
and one of a newer schema fails the file, which is then set aside rather
than read with the fields this build does not know dropped. (The
frame-shape step of a migration, `migrate_with_frame`, is not run on a
queued edit. The edit was the window's current edit when it was queued,
so it has already had it.)

```json
{
  "version": 1,
  "entries": [
    {
      "queued": 1790826112,
      "folder": "/path/to/out",
      "preset": "Web",
      "sheet": { "export_format": "JPEG", "export_size": "1024", ... },
      "frames": [
        { "source": "/path/IMG_0001.CR3", "edit": { ... }, "turn": 0, "seed_blend": false }
      ]
    }
  ]
}
```

`folder` is null for each beside its own file. `sheet` uses the
settings file's own keys; `edit` is the edit as the sidecar writes one.

**Checked in the editor.** XDG redirected into the worktree's target/scratch, the session bus
pointed at nothing so the folder chooser fell back to beside-each-file
without a dialog, and the buttons pressed with `--keys click:X,Y` in
live (non-snapshot) sessions:

- Three sessions queued three sets: two synthetic JPEGs under Web (JPEG
  85, 1024), one under Print (PNG, full), and two raws hard-linked from
  the sample folder (5M0A1023, 5M0A3976) under Web with `--no-sidecars`.
  The file held three entries with absolute sources, the right sheet and
  preset each.
- A fresh launch (`--sheet export --snapshot`) showed "2 sets, 3 frames
  queued" at the sheet's foot (from an earlier pass) and ran nothing.
- Export the queue in a new session ran the three entries in order: two
  1024 px JPEGs, a 1000x1500 PNG, then the raws as 1024x683 JPEGs with
  their own EXIF (EOS R6 Mark II, DateTimeOriginal); the file was gone
  at the end.
- After the review, on the probe-thread build: two sets queued (A and C
  under Web, C under Print), C.jpg moved out of the folder, the queue
  run. A was written. C was said "not there; left queued" for both
  entries, and the file held one frame in each. With C back, a second
  run wrote C as JPEG and PNG and emptied the file. C's sidecar carried
  both records, Web and Print.
- With sidecars on (JPEGs only): the sidecars carried one `exported`
  record each with the entry's preset (Web for A and B, Print for C),
  and a snapshot of C showed the row "C.greycard (2).png · Print" above
  Original (the 2 from the on-exists Increment, the earlier PNG being
  there).

**Tests.** `export_queue.rs`: the file round-trips, an empty queue leaves no file,
the count's words; a file that will not parse and one of another version
are set aside, not lost. `panel/export_queue.rs`, through the window's
callbacks on a headless window:
`add_to_queue_keeps_what_export_would_take_and_writes_it` (the frame on
screen under the panel's edit, the other under its sidecar's with its
turn, the sheet, the preset, the folder, the file, the count and the
status line; a later sheet change and a later sidecar change leave the
entry and the file as queued; an edited sheet names no preset);
`the_queue_runs_in_order_and_each_entry_leaves_when_done` (each set
under its entry's settings and preset, the entry leaving the queue and
the file, a History line on each frame with its preset);
`a_stop_leaves_the_rest_queued`;
`a_failed_frame_does_not_stop_the_queue`;
`the_queue_is_kept_across_a_restart_and_not_run` (and a run that may not
write leaves the file alone); `a_missing_file_is_left_queued_and_said`;
`a_missing_folder_or_mark_stops_the_queue_and_keeps_the_entry`;
`a_window_closed_mid_entry_does_not_send_its_done_frames_again` (one
frame back and no set end: the file no longer holds it);
`a_relative_name_is_kept_whole_and_sent_as_the_list_has_it`;
`an_empty_mark_is_not_queued`; `a_plain_set_leaves_the_queue_alone`;
`a_seeded_frame_is_a_line_in_its_history`;
`clear_empties_the_queue_and_the_file` (refused while running). In
`export_queue.rs`, also: a second unreadable file goes to `.unread.2`
and does not overwrite the first; an entry's edit of schema version 1 is
migrated, and one newer than this build's sets the file aside. The tests
answer the look for files at once, on the window's thread; the thread
was exercised in the editor.

**Not done.** - No per-entry view: the queue is counted, not listed, and an entry
  cannot be removed or reordered on its own; Clear is all or nothing.
- Nothing runs on its own: no "when idle" or scheduled run.
- A look for files that hangs (a dead network mount) leaves the queue
  waiting on it until the window closes. The window stays responsive,
  and Clear lets the look go, but there is no Stop for it.
- A file deleted for good stays queued until Clear, since it cannot be
  told from one on a drive that is unplugged.
- Export the queue is not greyed while a folder chooser is up. Pressing
  it then is refused with a status line.
- A frame that failed leaves the queue. Keeping failures queued for a
  retry would need a way to tell a failure worth retrying from one that
  is not.
- The queue's frames are not marked in the strip or the grid.
- Not clicked by hand in a live window: the buttons were pressed with
  `--keys click:` in live sessions, and the folder chooser was answered
  only by its no-portal fallback, never by a real choice.
