# §290. A new snapshot's name ends with a click away (2026-10-10)

The user could not restore a snapshot: every click on its row opened
the name for typing instead. Take puts the new row into naming mode so
the name can be typed at once, and the mode ended on Enter or Escape
alone. A user who took a snapshot and went back to the sliders without
either left the row in that mode for good; its click was disabled
while a name was being typed, and the only control left under the
pointer was the text box, which took the click as a return to typing.

## The rule

The naming ends when its text box loses focus, as it ends on Enter,
and keeps what was typed: a click anywhere else, a press on another
control, or a move to another frame. Escape still ends it and keeps the
old name. From then on the row is a button again: a click restores the
snapshot, a double-click renames it, and the hover's words say so.

The picture takes the keys whenever the pointer enters it, so that the
shortcuts work wherever the pointer last was; that would end the naming
the moment the pointer crossed the picture on its way anywhere. While a
snapshot's name is being typed the picture waits for a press instead.
The other text boxes in the panels (the filter, a custom aspect, a
mask's name) still lose the keys to a hover; a general rule for typing
is on the roadmap.

## Tests

A snapshot taken, "Keep" typed, the pointer moved over the picture (the
naming goes on), and a click on the picture: the naming is over and the snapshot is named Keep; a click on its row restores it
(the status says it is the current state) and does not open the name.
