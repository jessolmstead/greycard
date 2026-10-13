# §311. A word at the pointer over an icon-only button (2026-10-12)

The pool asked for tooltips on the buttons that show a picture and no
text, and asked them to be short: a word or two, the absolute minimum,
never a sentence or a key hint, which the manual has. We began with the
snapshot row's three icons ("Restore", "Rename", "Remove") on the hover
text layer the roots list already had for a cut-short name, and then
carried it to the rest. A control with a visible text label gets none:
it already says what it is.

We carried the word to every other control that shows a picture and no
text. The mechanism is one global and one element. `Tips`
(controls.slint) holds the text, its place and a serial that moves on
every show and hide. `TipArea` is a TouchArea with a `words` property:
after a 600 ms rest under the pointer it writes its words to `Tips`,
just under its own box; it clears them on leaving and on a pointer down.
The window watches the serial and hands the text to `tip-at`, the same
layer the roots, the recent list and the snapshot row use, so there is
still one hover text in the window. We chose a global over a `tip`
callback forwarded up through every section because these controls sit
four and five components deep, and the forwarding would have touched
every panel's interface for no gain. `PanelButton` and `EdgeArrow` take
an optional `words` and use `TipArea` inside, so a button with an icon
and no text gains a tip by setting one property. The window also clears
the text when the panel's scroll changes (`panel-scroll`), which it did
not before.

Tips given, and their words:

- Mask list (adjustments): the bin on the chosen mask, "Remove".
- Shape list: the cross on a shape row, "Remove".
- Retouch: the bin on the chosen patch, "Remove".
- Presets: the bin on a preset row, "Remove".
- Panel head: undo, "Undo"; redo, "Redo".
- Update notice: the dismiss cross, "Not now".
- Edge arrows: left and right panel, "Hide panel" or "Show panel" by which way a press goes; the strip, "Hide strip" or "Show strip".

The bins, the shape cross and the edge arrows had no accessible name
before; they have one now (the bins "Remove <name>", the cross "Remove
this shape", the arrows "Left panel", "Right panel", "Strip"), which is
also how the tests find them.

Skipped, and why:

- The Masks tab's shape pickers (Linear, Radial, Brush, Subject, Background, Sky, Object, People, Luminance, Color) and the grid header's Lightbox and Loupe: each already shows its name as text beside the picture, so a tip would say it twice.
- The shape row's invert: it already has its own plate above the icon, which also comes up on keyboard focus. Left as it is; folding it into the shared layer would lose the focus case.
- The lit dots that switch a mask or a shape on and off: a circle, not an icon, and the row's state is the whole message. Left for a decision about naming them.
- Thumbnail badges and marks: they report state and take no press.
- Section header chevrons: the section's title is beside them.

Tests: `panel::mask::tests::a_mask_s_bin_says_remove_after_a_rest_and_th
e_word_goes_with_a_press` (rest, leave, press) and `panel::viewport::tes
ts::the_edge_arrows_and_undo_say_their_words_after_a_rest` (undo, redo,
the right arrow and the strip arrow, the arrow's word turning to "Show
panel" after a press, and a panel scroll taking the word down). Both
find the control by accessible label and rest at the middle of its box
(`testing::rest_on`).

## After review

The Rotate section's four icon buttons got "Turn left", "Turn right",
"Flip" and "Flip vertical" (with accessible labels), and the snapshot
row's icons now ride on `TipArea` too, so the per-section `tip`
callbacks are gone. A hover word is taken down by any key the window
handles, and by the left panel, right panel or strip being put away,
since a control that is destroyed never hears the pointer leave. The
mask list and the preset list scroll inside their own views, so each
takes the word down when it scrolls. The bins, the shape cross, undo and
redo now answer the default accessibility action as a press does; the
edge arrows are named "Left panel", "Right panel" and "Strip".
