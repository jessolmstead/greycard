# §397. Fine steps, a mat around the picture, one section open at a time

*2026-10-23*

Three small asks from testers, built together.

## Fine steps

A slider's wheel click and arrow were one small step, Shift on an
arrow the big one. Ctrl on a focused slider's arrow, Ctrl with the
wheel, and Ctrl+Shift with the wheel on an unfocused slider now move
a fine step: a tenth of the small step, or the smallest change the
slider's number can show, whichever is larger. That last clause
decides most of the panel. The sliders that show whole numbers (the
percent ones, degrees, blacks, and the rest) have a small step of one
shown unit already, so Ctrl changes nothing there; it bites on
exposure (0.01 EV), tone, kelvin (5 K), distortion, and the sliders
shown to two places. Contrast is logarithmic and shown as a
multiplier, where a finer step hid at two places, so it keeps its
small step. Widening it would mean one more decimal across some forty
value displays, a separate decision. The keys sheet's Sliders row
says it; on a Mac Ctrl is Cmd as everywhere in the window.

## A mat

F toggles a band of the canvas color around the picture at Fit, six
percent of the viewport's shorter side on every side, so the picture
floats with room to judge exposure and color against. The Fit room
shrinks by twice the band, an even number, so the view keeps its
parity and the 1:1 alignment of §18 is untouched; every overlay reads
the same effective zoom, so the crop, the masks' handles, and the
navigator follow. Other zooms ignore it, as does the compare view.
Off by default, kept in settings.json, rebindable.

## One section open at a time

A Settings switch, off by default: opening a section of the right
panel folds the others of the same tab. The logic is a Slint global
watching each marked section's `collapsed`, so an opening by click,
key, or the Rust side counts. The grid's left pane shares fold state
with the hidden panel's Metadata section, which would have folded the
whole develop panel from the grid; the marking is `!grid-open`, so
nothing folds while the grid is up. Light, Color, Curves, Mixer,
Grading, Tint, and Detail are one fold state on Develop and Masks, as
before. Flipping the switch on folds nothing until the next opening.
