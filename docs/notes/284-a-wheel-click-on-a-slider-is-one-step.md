# §284. A wheel click on a slider is one step (2026-10-10)

Scrolling on the Exposure slider moved it by 0.05 one click and by 0.10
the next. The slider counted the wheel in notches of 40 pixels and let
the remainder carry: Slint hands the window 60 pixels for a mouse
wheel's click, so the first click made one step and left 20 over, the
second brought 80 and made two. Every slider had it, at every step
size; Exposure's is where it shows.

## The rule

A single wheel event of a notch or more is a mouse wheel's click, and a
click is one step, whatever a platform makes a click in pixels (60 on
one, 15 on another once it is past the notch): the remainder is
dropped, and nothing carried before it counts. Smaller deltas are a
touchpad's and still add up to steps, one step when they reach the
notch and the remainder dropped there too, so two touchpad flicks
cannot make a double step either.

We did not change the notch, the step sizes or the keyboard's nudge,
and the Shift-to-scroll rule stands: the wheel moves a focused slider
or any slider with Shift held, and passes through otherwise.

## Tests

A window with the Develop tab up, the Exposure slider clicked to focus
it, and wheel events dispatched at it: four clicks of 60 pixels land on
0.05, 0.10, 0.15 and 0.20 (the old code landed the second on 0.15), a
click back is one step back, two clicks of 45 are one step each, and
touchpad deltas of 15 are nothing twice, the step on the third, and the
remainder does not carry.
