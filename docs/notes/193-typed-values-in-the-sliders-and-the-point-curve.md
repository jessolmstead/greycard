# 193. Typed values in the sliders and the point curve (2026-09-27)

A click on a
slider's reading turns it into a field that starts on the reading's
number alone, selected: "+0.40 EV" becomes "0.40", "-12%" becomes
"-12". Enter reads the typed number back into the value, clamps it to
the slider's range and fires the slider's own `changed`, so the rest
of the window cannot tell it from a drag: one change, one save, one
history step. Escape, a text with no number in it, or a click away
leaves the value as it was. Enter on the field's text as it started,
the reading's own number, does nothing at all: the reading is rounded
("128" over a point at 0.5, "+0.44 EV" over 0.4437), and reading it
back would move the value and make a step. The comparison is of the
text, so typing the reading's own number over itself is the same no-op.
Typed values are clamped but not snapped to the slider's step (0.123
EV stays 0.123).

Where the reading lives decided where the parse lives. Every slider's
text is formatted in Slint, next to the slider, from the value
(`round(v * 100) + "%"` and so on); the Rust side never sees it. So
the field is seeded from that exact shown text, and the one new fact a
slider carries is its `scale`, the shown number over the value (100
for a percent or a -100 to +100 reading, 1 for a value shown as it
is, the black-and-white strength for the weight, whose reading is
weight times strength). The parse itself, pulling a number out of what
was typed (a sign or the typographic minus, a comma for a decimal
point, a trailing unit, an "x" before a multiplier) and refusing
anything else, is Rust (`entry.rs`) behind a `Typing` global with two
pure callbacks, because Slint has no string functions to strip a unit
with and because it wants unit tests. A logarithmic slider needs no
special case: it shows its multiplier, so its scale is 1 and a typed
number is the multiplier. A test reads every `EditSlider` in the panel
sources and checks its `scale` against the factor its text rounds the
value by, so a reading and its scale cannot drift apart unnoticed.
Three sliders read another property instead of their own value (the
black-and-white weight through `bw-applied`, its strength through
`bw-strength-cents`, the tint's hue through `tint-hue-text`); rather
than teach the test to follow a property, it lists those three with
the text and the scale each must have, and fails on any other slider
whose text is not its own value.

The keyboard goes back to the window through a counter on the global:
a field bumps `Typing.done` whenever it closes (Enter, Escape, or its
focus going elsewhere), and the window's root has a change callback on
it that focuses `keys`, which is what the sheets do when they close. A
component cannot handle a global's callback in Slint, and threading a
`focus-keys` callback through eighty sliders in a dozen sections was
not worth it. Each opening takes a ticket (`Typing.opened`), and only
the field last opened hands the keys back, so a click from one field
into another leaves the new one its focus.

Two ways out say nothing on their own. A field whose section is taken
away while open (a tab switched, a mask target changed, the curve's
mode changed, culling entered) is destroyed without a focus event, and
the window's focus is left pointing at nothing, so no key reached the
shortcuts until the pointer crossed the picture. And a click on a
control that takes no focus (a tab, a channel, a swatch, a section's
header, the curve itself) left the field open, so typing then went
into it, onto a point just clicked. `Typing.close-all()` covers both:
the window calls it when the tab, the target, the curve's mode or
culling changes, and `Segmented`, `Swatches`, a section's header and
the curve editor call it on a press. It closes any open field through
a cancel counter the fields watch, and hands the keys back itself,
since a destroyed field cannot. A click on anything that takes the
focus (a slider, a button, a field) closes the field through its own
focus loss, and the pointer over the picture hands the keys back as it
always has. One cost: a click on a slider's track while a field is
open ends with the keys on the window, not on that slider, so the
arrows then step frames rather than nudge it.

The point curve's selected point is `curve-selected` on the window,
an index into the current channel's points. A press selects the point
it takes or adds and it stays selected after the release; the
selected point is drawn a size up with an accent ring. In and Out
fields under the curve show it in 0 to 255 and take a typed number
through the same parse; the point moves through `moved_point`, the
clamp a drag already used (the ends keep their In, the rest keep a
gap to their neighbors' In, Out stays within 0 to 1), and the move is
one `view-changed`. A press that neither takes nor adds a point (too
close to another's In), a double-click that deletes, Reset, a change
of channel or mode, and a showing of the edit that changes the current
channel's points (undo, another frame) let the selection go; the
fields grey out with none.

The headless tests find controls through the testing backend's
element queries, which need the Slint compiler's debug info; the
build script turns it on for debug builds only.

Not done:
- Tab from one slider's field to the next.
- With no point selected, the In/Out row shows two empty dark boxes
  beside muted labels, left-aligned above the right-aligned Pick and
  Reset row; it may want greying differently, or hiding until a point
  is selected.
- Typed values are not snapped to the step.
