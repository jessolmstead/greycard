# Keys follow-ups: a focused slider in view, text boxes, Escape out of the crop (2026-10-21)

Three loose ends of §383 and §385.

**A focused slider kept in view.** Slint drops the focus of an item
clipped wholly out of view at the next key, and gives it to nobody.
The focused slider now reports its place whenever it moves
(`held-y`), and the panel tells the two causes apart by its scroll:
unchanged, the layout moved the slider (a section opening above it),
and the panel follows; changed, the wheel moved it, and we do not
chase, since that would hold the wheel at the slider. Wheeled wholly
out, the slider lets go, so the next key is the window's. A section
header's click used to let go of any focused slider, so the one way
a user opens a section dropped the keys before the layout moved; it
now lets go only of a slider in its own section. The follow keeps to
the panel, not a sheet's sliders.

**Text boxes keep the keys.** `Typing.boxes` counts the panel's
focused text boxes, each in at its focus and out at its loss, and
the picture does not take the keys at the pointer's crossing while it
is above zero: the filter, a custom aspect, a mask's name, the title,
and the keywords. `close-all` zeroes it. The custom aspect's box goes
with "Custom", which a frame change, an undo, or a sync can take away
under its focus, so the crop section closes it when the aspect leaves
"Custom" while that box, and no other, has the focus; the filter's
words moving the frame to another aspect keep their keys.

**Escape leaves the crop** as Done does, once nothing is in hand: a
typed value, then the Draw, the level, or a guide, then the crop, one
a press; Develop view only. A held Escape is one press across the
exit: its repeats are eaten where they would go on to stop an export,
an import, or let go of a set.

**Review.** Two rounds: the header click dropping the holder (seen by
hand), held Escape's repeats falling through, the custom aspect's box
sticking the count when destroyed, the follow reaching from a sheet;
then the aspect's close reaching any focused box. Each fix has the
reviewer's probe as its test. Escape stays a fixed line of the keys
table (§390), so the crop's branch still sees "Esc".

**Not done.** A text box wheeled out of view loses the keys to
nobody, as before.
