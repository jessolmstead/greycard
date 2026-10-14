# §299. A two-finger scroll over the picture pans (2026-10-11)

A two-finger scroll on a touchpad over the picture zoomed it (issue
#17). The viewport took every wheel event as a click: any delta up was
a 1.25 zoom about the pointer, any other a step out, so a touchpad's
stream of small deltas zoomed in and out at its own rate and the
fingers could never move the picture.

## The rule

We tell a click from a touchpad much as the sliders do since the
wheel-click section: Slint hands a mouse wheel's click as a line, and a
line as 60 logical pixels, while a touchpad sends pixel deltas.

- With Ctrl held, the wheel zooms about the pointer whatever the delta,
  by as much of a step as the delta is of a click (1.25 to the power of
  the delta over 60 px), so a click is the step it always was and a
  stream of small deltas zooms smoothly rather than a step each. A
  browser and the other editors zoom on Ctrl and the wheel, and
  Windows hands a precision touchpad's pinch to an application as
  exactly that.
- A wheel's click zooms a step about the pointer, as before: a single
  vertical delta, nothing sideways, that is a whole number of clicks
  (60 px, 120 px), and that comes after a quarter second with no sizable pan.
- Anything else pans: a delta that is no whole number of clicks,
  deltas on both axes, a tilted wheel's sideways click, and a whole
  click within a quarter second of the last sizable pan. The picture moves
  with the delta, as a drag does, which is the direction the system
  already chose for the fingers (natural or not is the system's
  setting, not ours); the grid and the strip scroll the same way.

The quarter second is there because a size alone does not tell them
apart. A fast flick carries a click's size or more in one frame: libinput
on Wayland locks a two-finger scroll to one axis, Windows sends the two
axes as separate messages and a precision touchpad's flick at 120 px or
more a message, and a Mac's momentum deltas are whole points, often
above 60. The touchpad's own phase would settle it, but Slint 1.18
drops the phase on the way to the window, so we judge by the company a
delta keeps: one that lands on a whole click in the middle of a pan is
part of the pan.

Only a sizable pan, 20 px or more across both axes, starts that quarter
second. A flick ramps through deltas of 20 px and more on its way to a
click's size, while a high-resolution wheel (Wayland's partial steps on
a version-7 seat under KWin or mutter, a Windows smooth-scroll driver
under 120 a message) sends fragments of about 2 to 15 px, with its
whole detent milliseconds behind them. Such a wheel now pans a little
with each fragment and still zooms a step on each detent, where before
every fragment zoomed a full step.

A tilted wheel's sideways click is a sizable pan too, so a vertical
click within a quarter second of a tilt pans rather than zooms. We
accept that.

A zoom by a factor of exactly one, which Ctrl and a purely sideways
scroll ask for, and a gesture that only rotates, changes nothing: a
fitted picture stays at Fit rather than being pinned to the zoom the
fit happened to be.

The pan stops at the frame's edge rather than run the picture off the
view, and where the whole frame is in view at the current zoom it has
nowhere to go and does nothing: it never zooms instead. A center a drag
has already left past the edge is not pulled back in a jump; the wheel
only stops it going further out. In the compare view the room is a
tile's, not the whole view's.

Zoom keys, a click to zoom and the navigator are unchanged. We left the
brush and the retouch patch alone: with either in hand the wheel still
sizes it by a tenth a wheel event, so a touchpad's gesture, many events
long, sizes it far faster than a wheel does. That is its own item.

## A pinch

What reaches us depends on the platform, and we checked it in Slint
1.18 and winit 0.30 rather than assume it:

- Windows: a precision touchpad's pinch arrives as Ctrl and wheel
  deltas, which the Ctrl rule zooms.
- macOS: winit reports a trackpad pinch as its own gesture, which Slint
  never turns into a scroll; it reaches only a
  `ScaleRotateGestureHandler`. We put one over the viewport, on top of
  the TouchArea: it takes a pinch or a rotation, holds the pointer
  while one is active, and lets every other event through. A pinch
  zooms about the pointer by the change in the gesture's scale since
  its last update. It has not been tried on a Mac yet; the test
  backend has no public event for a pinch, so no test drives it.
- A touchscreen: the same element takes a two-finger pinch on every
  platform, but Slint first hands the TouchArea a release for the
  first finger, so the pinch starts with a click's zoom (to 1:1 or
  back to Fit) before it scales. Untried on hardware.
- Linux on Wayland: winit 0.30 has no pinch on Wayland at all (its
  gestures are macOS and iOS only), and a compositor does not turn a
  pinch into Ctrl and wheel for a client, so a touchpad pinch does
  nothing in the viewport there. A wheel click, or Ctrl and the
  touchpad's scroll, zooms. Should a later winit carry Wayland's
  pointer gestures, the gesture handler is already listening.

## Tests

Through the compiled window, wheel events at the viewport on a 6000 by
4000 picture: a click of 60 px zooms a step in and back out, and from
the fit a step above the fit; a delta of 10 px pans by 10 px and leaves
the zoom alone; five small deltas on both axes pan by their sum; a
60 px delta on both axes, and a sideways click, pan and do not zoom; a
72 px delta pans; a flick's ramp of 8, 24, 48 and 60 px pans all the
way, as does a 120 px delta 200 ms later, and a 60 px delta after a
300 ms quiet spell zooms; three 15 px fragments pan and the 60 px
detent right behind them zooms; on a fitted picture ten small deltas leave the center and the
fit where they were; a zoom by one, called directly and from Ctrl and a
sideways scroll, leaves the fit at Fit; with Ctrl held a 10 px delta
zooms by a sixth of a step, six of them make a whole step, and with
Ctrl let go the same delta pans again. The edge rule is checked on its
own: a pan stops at either edge, does nothing where the frame fits, and
never pulls a dragged-out center back in a jump.

## Checked on a Mac, 2026-10-13

On a MacBook trackpad: two-finger scroll pans, a pinch zooms about the
pointer, and Command with the scroll zooms, which is the modifier Slint
reports as control on macOS; Ctrl does not. All as designed.
