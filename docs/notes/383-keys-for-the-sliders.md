# §383. Keys for the sliders (2026-10-21)

Two testers asked for Lightroom's slider keys (issue 31). A slider
focused by a click, by Tab, or by comma and period now holds the keys
until a click elsewhere, Tab, comma, period, or Escape moves them on.

## What a focused slider takes

The four arrows, with or without Shift; plus and minus, with = and _,
so the unshifted key, the shifted one, and the numeric pad all work;
comma and period; and Escape. Up, Right, +, and = nudge up; Down,
Left, -, and _ nudge down. Shift takes the big step on an arrow only:
plus and minus always take the small one, since Shift makes them other
keys on some boards (Shift+plus is `*` on a German board, and
Shift+minus is `6` on a French one, which then sets the red label as
any 6 does). With Ctrl, Alt, or Meta held, none of these is the
slider's. Comma and period focus the previous and the next slider in
the panel. Escape is always the slider's first: it hands the keys back
to the window and does nothing else, so a second Escape is the
window's. Tab and Shift+Tab are Slint's own move to the next control.
Every other key goes to the window's key handler as if the window had
the keys: the rating digits, the flags, the labels, the letters, undo,
and the rest work while a slider is focused.

In a slider's typed value, Up and Down nudge (Shift for the big step)
after setting whatever was typed, and Tab and Shift+Tab set the typed
value and open the next or the previous slider's value, all selected.
Tab past the last closes the field and hands the keys back, as Enter
does.

## The steps

One table for the keys, `Steps` in `controls.slint` beside
`EditSlider`; each slider names its line (`steps: Steps.exposure`),
and a test holds every slider to a line and every line to a slider.
The wheel keeps its own step, `step:` on each slider as before, and
that is also the step a screen reader is told of.

| Line | Small | Shift | Sliders |
|---|---|---|---|
| exposure | 0.1 EV | 1/3 EV | Exposure, vignette Amount |
| tone | 0.05 EV | 0.25 EV | Highlights, Shadows, and Whites |
| blacks | 1 % | 5 % | Blacks |
| contrast | 0.02 stop | 0.2 stop | Contrast |
| percent | 1 | 10 | every slider shown 0 to 100 or ±100 |
| hundredths | 0.01 | 0.1 | sharpen Radius, grain Size |
| twentieths | 0.05 | 0.25 | B&W Weight and Strength, denoise Strength |
| tenths | 0.1 | 1 | crop Angle, the keystones, defringe Radius, and mark Margin |
| halves | 0.5 | 5 | mark Size, border Width |
| whole | 1 | 10 | Quality, Opacity, Iterations, and defringe Threshold |
| degrees | 1° | 10° | the hues and hue widths |
| tenth-degrees | 0.1° | 1° | the tint's Hue |
| hue-shift | 1° | 5° | the mixer's Hue (±30°) |
| kelvin | 50 K | 500 K | Temperature |
| thousandths | 0.001 | 0.01 | Tint, Chroma, and brush and repair Size |
| distortion | 0.5 shown | 5 shown | lens Manual |
| fringe | 0.1 shown | 1 shown | lens Red / cyan, Blue / yellow |

## How

A nudge calls the slider's `changed`, the same path a drag's step
takes, so the develop runs reduced then full, and the save waits for
the panel to rest. One nudge is one history step; a run of nudges
inside the rest (800 ms) is one step, as a drag is, since the history
records at the rest and not at the change.

The next and the previous slider are found by place, not by a list:
the asking slider counts up a question in a global, every slider
below or above it in its column puts itself forward in the first round
of change handlers if it is nearer than the best so far, and in the
second round the nearest takes the focus (or opens its field). Both
rounds run inside the press. A disabled slider is passed over, and so
is one in a folded section, whose body is put far to the left once the
fold has closed, out of every column; that offset exists for this walk
alone. (A changed handler in `Section` to the same end broke the
testing backend's accessible lookups across the window, so the fold is
a pure binding.) The slider reached is scrolled into the panel's view.

A focused slider sets `SliderKeys.holder`; the picture does not take
the keys at the pointer's crossing while it is set, nor while a typed
value is open (`Typing.editing`), so the roadmap's trap no longer
catches a slider or its value (the filter, a custom aspect, and a
mask's name still have it). `Typing.close-all` lets go of a focused
slider as it closes a field. It is called on a change of the tab, the
target, the curve mode, culling, the grid, the lightbox, and the
panel's hiding (F8, the edge arrow), and on a click on a section's
header, a swatch, or a segment: G, L, or F8 with a slider focused hands
the keys to the window at once. A slider's keys that are not its own
reach the window through a counter in the same global, since a
global's callback cannot be answered in `.slint`.

The keys sheet has a Sliders group of three lines, without chords,
since the window's handler does not compare these.

## Tests

In `entry.rs`: Up, +, and Shift+Up nudge the exposure by 0.1, 0.1, and
1/3, Shift with + and _ takes the small step, and the other keys and
arrows go the same ways, one change each; period and comma move the
focus between exposure, contrast, and highlights; Tab and Shift+Tab
move between the typed values, with Up and Down in them; one nudge is
one history step, and three in a run are one; a rating and the arrow
reach the window with nothing focused, the rating does with a slider
focused while the arrow does not, the pointer crossing the picture
leaves the keys with the slider, and Escape gives the arrow back; G,
L, and F8 with a slider focused leave the keys to the grid, the
lightbox, and the window.

## Not done

The user guide's Keys chapter needs the Sliders group the sheet now
has. A slider destroyed while focused (inside an `if` that turns
false) by anything other than the changes listed above leaves the
holder stuck, so the picture does not take the keys at a crossing,
until another slider is focused or one of those changes comes; a
click on the picture still gives the window the keys. We reveal a
slider when a key brings the focus to it, not when the layout moves
it; Slint drops the focus at the next key from a control outside its
scroll view's visible part.
