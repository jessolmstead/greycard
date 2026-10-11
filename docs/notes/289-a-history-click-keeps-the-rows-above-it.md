# §289. A history click keeps the rows above it (2026-10-10)

The user reported that clicking back in the History panel sometimes lost
the rows above the one clicked, without changing anything, and that a
click on a greyed row to go forward again sometimes did nothing. It came
and went.

## What happened

`take_current_settled` (panel/history.rs) puts the sidecar's current
state on the panel after an undo, a redo or a row click. It assigned
`st.edit` only when the new state needed the engine to develop again
(`!edit.same_develop(&st.edit)`). `same_develop` compares only the raw
develop's inputs: the white balance, the noise, the demosaic, the camera
section whole, the lens, the retouch, the detail and the sharpen. A jump
between states that differ only in what the finish draws (any look,
the masks, the curves, the grain, the geometry) left `st.edit` on the
state the user came from.

`st.edit` is not only what the worker develops. The viewport renders
`read_edit` over it too, so after such a jump the picture on screen
kept the newer state's masks: a mask stayed visible after a click back
past its making. And `read_edit` builds the panel's edit on top of it, taking from it everything the panel does not
show at that moment: every mask, and the global look while a mask is
chosen. So after such a jump, the panel's edit was the clicked state
with the mask list, and with a mask chosen the global look, of the newer
state. The next action that recorded the panel saw news and recorded a
step, which cleared the redo:

- the save timer, after any slider moved;
- the save made when leaving the frame;
- the next history click, which records the panel first. If the click
  was on a greyed row, the panel was recorded (cutting the redo) and
  then the clicked state recorded on top. When the stale panel already
  equaled the clicked state, that second record was a no-op and the
  click appeared to do nothing.

The intermittency follows from the condition: it took a jump that needed
no develop, and a panel read that differed in what `read_edit` takes
from `st.edit`. A jump across a noise or white balance step updated
`st.edit` and was safe. The tester's sidecar shows the pattern: its last
step is five global look changes recorded at once. That is the clicked
state with the newer state's global look on it, recorded while a mask
was chosen. The user's build had this from the first commit; none of
this week's changes brought it in.

## The change

`take_current_settled` now always makes `st.edit` the state it shows. It
still sends a develop only when `same_develop` says the engine's part
changed, so no develop is added and no generation moves. Undo and redo
go through the same function and are fixed with it, and so are the two
callers in the sync (an archive's join landing, and `show_settled`),
where a joined state that differed only in its finish was overwritten
by the next save with the window's old masks or look.

The test (`a_click_back_keeps_the_rows_above_it_through_what_follows`)
makes a mask and four exposure steps, chooses the mask, clicks back two
rows, and holds the two rows above through a develop landing, the save
timer, a panel save and a switch to another frame and back; then
forward one row and to the newest, two quick clicks back past the
mask's making, forward again, an undo and a redo. Without the change it
fails on the first panel check, and with that check skipped it loses a
row exactly as the user saw: five states at the fourth become four.

## What we ruled out

- The row-to-state mapping is right, with export rows and the
  newest-first order. Snapshots are a separate list and are not
  interleaved.
- A frame switch keeps the in-memory redo. The redo is never written
  (`#[serde(skip)]`), so a sidecar read again from disk, or a join of an
  archive's copy (`sync::join`), still drops it by design.
- Slider values read back as they were set for the fields we checked
  (f32 sliders, a custom white balance's f32-valued kelvin and tint).
