# 282. A mask's curve editor has its histogram when shown (2026-10-10)

The bug, in the user's words: "the histogram in the curves editor of a
mask doesn't populate until I touch a slider or pivot point, then it
shows up." Choose a mask, open its Curves, and the editor stood on its
bare grid.

**The cause.** The curve editor's picture is one image, `curve-image`,
drawn by `draw_curve` from the last histogram the analysis read back,
`State::bins`. Choosing a mask goes through `target_changed`, which
holds the state borrowed while it calls `show_edit` to put the mask's
look on the panel. `show_edit` drew the curve again for the new look,
but it fetched the bins through the thread's handle on the state
(`STATE`) with a `try_borrow`, a second borrow redundant with the
state it is handed. With the state already held, the second borrow failed
and the editor was drawn with no histogram. Nothing drew it again: the
analysis is keyed on the edit, the scope and the weighing, and a switch
of target changes none of them while the Selection toggle is off, so no
new bins arrived. A slider moved changed the edit, so a new analysis
came back and drew the histogram; a point moved redrew the curve from
`st.bins` directly. Either way it appeared at the first touch. Every
caller of `show_edit` holds the state, so the global editor lost its
histogram the same way when the Develop tab was chosen back from a mask.

**The fix.** `show_edit` draws the curve from the `st.bins` it is
handed. The read back of the analysis, which sets the editor's picture,
the clipping lamps and `st.bins`, moved out of the frame notifier into
`startup::bins_read` so a test can drive it. A learned mask still being
made needs nothing more: its raster's landing goes into `st.learned`,
the next frame's locals carry it, the analysis's key changes with them,
and the fresh bins come through `bins_read` with no touch of the panel.
The global editor, the scopes and the clipping lamps draw from the same
bins as before.

**Tests.** Two UI tests in `panel::curve`, on a window with one mask and
a histogram in hand:
- `a_masks_curve_editor_has_its_histogram_when_shown`: the mask chosen
  through `target_changed` and its Curves opened, the editor's picture
  is the histogram's, not the bare grid's; back on the Develop tab the
  global editor has it too. It failed before the fix.
- `a_masks_curve_editor_follows_its_raster_landing`: a Sky mask whose
  raster is still to come shows the picture's histogram at once; the
  worker's `Mask` outcome landing keeps it; the baked locals differ
  before and after the landing, so the analysis reads again; and that
  read, given to `bins_read`, is what the editor then draws. Its first
  assertion failed before the fix.

One thing the fix changes on the way: on a frame switch, `show_edit`
now draws the last frame's histogram for the moment before the new
frame's analysis lands, where it drew a bare grid; the renderer keeps
the old picture for that moment too, so nothing new is shown out of
step.
