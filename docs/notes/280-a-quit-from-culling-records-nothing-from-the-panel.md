# 280. A quit from culling records nothing from the panel (2026-10-10)

## The bug

Quitting while culling wrote the panel's edit onto whichever frame the
selection was on. Two ways to see it:

- `--cull` over a frame edited to +1.5 EV, then quit: the frame's
  sidecar gained a new history state with exposure 0 and the learned
  blend at 1.0, which is the panel's default.
- Develop an edited frame A, press `c`, arrow on to an untouched frame
  B, quit: A's +1.5 EV was written into a new sidecar for B.

## The cause

The quit's own save (`startup::save_at_quit`) reads the panel and hands
it to `save_edit`, which records it on `st.current`. In culling the
panel is no frame's. `enter_cull` saves it on the way in and then lets
it go. After that it holds the last-developed frame's edit, or the
default for a run that began in culling, where nothing was ever
developed. `cull_select` moves `st.current` without loading anything
onto the panel. The only guard `save_edit` had, `panel_is_frames`,
covers a sidecar standing in from its index row, not culling. So the
quit recorded a stranger's edit, or the default, as the current
frame's next state and wrote it.

## The fix

`save_edit` records nothing while `st.cull` is set. `record_panel`, its
companion that records without writing, gets the same guard. One check
in the function every panel save goes through covers the quit, a
debounce timer still running, and any later caller. `enter_cull` calls
`save_edit` before it sets the mode, so the save on the way in still
happens. `leave_cull` takes the mode down before it schedules its save.

An export does not save through `save_edit`; it reads the panel
directly. It had the same hole and gets its own fix, described with
the other saves below.

The rest of the quit does not read the panel, and it runs as before
while culling. A frame followed onto its archive copy still has its
last save written there, or noted waiting when its job is still out
(§275), and the archive writes that never went out are still noted as
"saved as the window closed" (§261). These now carry only what culling
itself saved: ratings, flags, labels, turns. It goes to the sidecars
directly and never through the panel.

## The other saves we checked

Every caller of `save_edit` and `record_panel`, and every place that
reads the panel:

- The quit (`save_at_quit`): the bug. Fixed by the guard.
- The debounce timer (`schedule_save`): `enter_cull` stops it. Every
  control reached in culling leaves the mode first (`control_over_frame`
  then `leave_cull`), so nothing restarts it inside the mode. The guard
  now covers it too.
- A frame picked (`open_frame`, `pick_pending`): `open_frame` returns
  through `cull_select` before its save, and `pick_pending` already
  checked `st.cull`.
- A folder opened (`open_loaded`), a turn (`turn_frames`), a sync's
  source (`source_edit`), a paste, a preset over a set, copy settings,
  a snapshot taken or restored, a history click, undo and redo: each
  already checked `st.cull` itself and reads the sidecar in culling.
- A delete (`delete.rs`) saves only while the save timer runs, which it
  does not in culling. The guard now covers it anyway.
- An export, and Add to queue, had the same hole by another route.
  Both go through `rows::request`, which waits while a frame of the
  set still stands in from its index row, and `enter_cull` does not
  cancel a request that is waiting. An export asked for before the
  mode could therefore land inside it. When it did, `set_frames_of`
  gave whichever frame was now current the panel's edit, and
  `pressed_frames` counted the pressed frame as on screen whenever it
  was current. Choose y and z with y still standing in, ask for the
  export from z with its panel at +2 EV, press `c` and move to y: the
  set exported y under z's edit, and a queue entry written to the
  queue file kept it. Both now use the panel only outside culling.
  `set_frames_of` reads the current frame's edit from the panel only
  when `st.cull` is unset. `pressed_frames` counts the pressed frame
  as on screen only outside culling, so in culling the export takes
  the sidecar path and goes out as a set, the way an export does for
  a frame that is no longer on screen.

## Tests

- `panel::cull::tests::a_quit_from_culling_at_launch_leaves_the_edited_frame_alone`:
  a run opened in culling over a frame with a +1.5 EV sidecar, then
  quit. The sidecar's bytes on disk are unchanged.
- `panel::cull::tests::a_quit_from_culling_writes_no_frames_edit_onto_another`:
  develop A, press `c`, arrow right to B, quit, all through the
  window's keys. B still has no sidecar and A's bytes are unchanged.
- `sync::tests::a_quit_from_culling_still_writes_a_followed_frames_copy`:
  a frame followed onto its archive copy, a rating made in culling with
  its job still out at the quit. The quit's followed write runs and is
  noted waiting, then reaches the copy after the job. The panel's
  exposure appears nowhere in it.
- `sync::tests::a_quit_from_culling_still_notes_the_writes_not_sent`:
  two culling keys on a frame, one write out and one queued. The quit
  notes the queued one as "saved as the window closed", and the
  frame's sidecar on disk does not change.

- `roots::tests::an_export_landing_in_culling_takes_each_frames_own_edit`:
  the export case above, through Add to queue, whose entry records
  what each frame took. y keeps its own edit (its turn and its
  exposure), and z has the +2 EV that `enter_cull` saved on the way in.

The first four fail without `save_edit`'s guard. The last fails
without the `set_frames_of` change; it is a different code path, which
the `save_edit` guard does not cover. `record_panel`'s guard is belt
and braces: every caller already checks culling before calling it, so
no test reaches it.
