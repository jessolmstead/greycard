# 114. The slider handle grabs before it moves (2026-09-20)

The roadmap's bug: clicking a slider's handle to focus it, so the
wheel would work over it, moved the value by a pixel's worth, because
a press anywhere on the row set the value from the pointer's x. Now a
press within the handle records where it was grabbed and changes
nothing; a move past a 4 px dead zone (the viewport's own threshold
for telling a click from a drag, with the same strict comparison)
latches a drag that tracks the pointer minus the grab offset, so the
handle does not jump under the finger, and returning to the press
point restores the exact value since the tracking is absolute. A
press on the track away from the handle still jumps there. The
review found the older, worse case beside it: the touch area is the
whole row, so a press on the label or the value text clamped the
value to an end of the range and wrote a history entry. Those
presses now only take focus. The one trade-off is that a single
gesture from the handle cannot make an adjustment smaller than the
dead zone's worth of the track, about 2.4 percent; the wheel and the
arrow keys remain the fine controls.
