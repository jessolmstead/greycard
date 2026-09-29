# 200. Draw a crop by dragging outside today's rectangle (2026-09-28)

Crop mode had two gestures: a handle resized, and the rectangle's own
body moved it. A drag that started outside the crop did neither — it
fell through to the viewport's own touch area and panned or, on a
plain click, toggled the zoom. Now a press outside the current crop
(but on the picture) starts a fresh one from the press point: dragging
draws the rectangle out to the pointer, held to the aspect in force
when one is set, no smaller than a handle's own minimum, clamped to
the frame. A press inside the rectangle or on a handle is untouched —
resize and move still work exactly as they did.

`Geometry::drawn` (`greycard-edit/src/geometry.rs`), beside
`drag_within`, does the math: the free rectangle between an anchor and
a point, or, with a ratio, the shorter side grown to match (the same
"larger of the two moves wins" rule a corner's aspect drag already
used), floored to `MIN_CROP` on both axes before the aspect math runs
so a straight drag never exports a one-pixel sliver. Unlike a handle,
going past the anchor on either axis flips the rectangle to its far
side rather than clamping there — the anchor is always a corner of
whatever comes out, not a wall. Too big for the source, an aspect held
shrinks both axes together, straight back toward the anchor, so it
stays held all the way in; free, each axis is its own — the largest
share of the whole reach that still fits, then whatever is left of
each axis alone from there, the same two-step `drag_within` already
takes — so a free crop pushed past one edge slides along it instead of
shrinking on the other axis too.

The anchor itself can already sit on, or within `MIN_CROP` of, an
edge — a press pulled in from the letterbox lands exactly there. Going
the pointer's way can then have no room at all for even the floor,
however much the reach toward it shrinks, since shrinking a reach that
is already at the floor does nothing: the floor is what does not fit,
not the reach. Two passes missed this; the fix tries the reach at its
own size (shrinking if it has to) toward the pointer's way first, then
each of the other three ways in turn — one axis flipped, the other,
both — taking the first that fits, and refusing outright, `None`
rather than a crop that cannot stand on the source at all, if nothing
does. `drawn` returning something `fits` will not take was the actual
defect underneath two rounds of this — a crop hanging half off the
edge is exactly as wrong as the whole-frame one `effective_crop` used
to fall back to when nothing fit, just wrong in a way the reviewer's
grid of presses had to go looking for. A property test now runs
`drawn` over a grid of anchors and pointers, free and held to two
aspects, upright and turned, and checks every crop it hands back
against `fits` itself, rather than trust a handful of hand-picked
cases to say the same thing a proof would.

`Geometry::anchored` pulls a press that missed the source onto it —
the letterbox, or, turned, a corner of the plane's own unit square a
small angle had cut away. The first pass pulled toward the plane's
center, which finds a point on the source but not the nearest one: a
press held level with the picture's middle third can come back a third
of the way toward the middle instead of straight across. The fix
clamps in the source's own pixels instead — a plain rectangle there —
and maps the clamped point back through `to_plane`. Absent a keystone
this is exact, not approximate: a turn and the fine angle are rigid,
so the plane and the source are the same distances apart, and a
distance-preserving map carries a nearest point across it unchanged.
A keystone bends that a little, the source's edges no longer straight
in the plane, so the point this finds is not always the nearest one
under a keystone specifically — but it is still on the source, which
is the part a fresh crop actually depends on.

The nearest point can itself be a corner of the turned source, where
no upright rectangle of even the minimum fits any way round — every
one pokes out one side or another of a shape that has already turned
away from the axes there. A press just outside a corner of a
straightened picture drew nothing at all: `anchored` handed back the
corner, `drawn` tried it in all four directions and refused every one,
and the drag stayed dead through every later move too, since nothing
ever moved the anchor off that corner. `anchored` now asks the same
question `drawn` will — is a minimum crop drawable from here at all —
and, where the answer is no, steps the point in from the corner
straight toward the plane's own center until it is. Twenty-two of a
grid of eighty-one presses failed this way at a 3° straighten, and
seventeen of eighty-one under a keystone; both are now in the grid
`every_drawn_crop_fits_and_every_anchor_can_draw_toward_the_middle`
runs, which also now asserts, for every anchor `anchored` accepts,
that a drag toward the center finds something — the assertion its
first version was missing, which is why it did not catch this on its
own even though the grid already covered the corners that fail.

Accepted as it stands: with the pointer's own side blocked at an edge,
the flipped rectangle grows inward as the pointer reaches farther
out — `(0.995, 0.5)` to `(1.2, 0.7)` gives `x` 0.79, `w` 0.205, wider
the farther right the pointer goes even though the rectangle itself
sits to the left of the anchor throughout.

The gesture lives in the viewport's own touch area (`ta` in
`ui/panel/viewport.slint`) rather than a new one layered over it. A
first pass added a dedicated `TouchArea` under the crop overlay's
handles, which correctly left a press on the rectangle or a handle
alone, but caught every press outside it whatever the button: a
right-click no longer opened the frame menu, a Mac's Control+click the
same, a plain click under the drag threshold no longer toggled the
zoom, and a drag no longer panned. Folding the gesture into `ta`
instead means all of that keeps working unchanged — the menu and the
click were never touched, since they are `ta`'s own logic already —
and only a left drag past the four-pixel threshold, in crop mode and
not over the camera's JPEG standing in for a develop, now draws
instead of panning; over the placeholder, where there is nothing to
draw against yet and the overlay itself stays off, a drag still just
pans, the same gate the overlay uses. Crop mode draws on a drag
whether or not the picture is zoomed in enough that a drag would
otherwise have panned it: crop mode is a mode, not a modifier on the
pan, and a crop's edges are worth more there than scrolling around
zoomed in is.

A pointer cancel — the window losing focus mid-drag, or the platform
taking the grab back, rather than a release ending the gesture where
it was pressed — used to leave the anchor waiting for a release that
was never coming, on both the new gesture and a handle's own drag
(`CropHandle` in `controls.slint`, which read every event's button
before its kind and so never reached its `up` handling for a cancel,
whose button is always `other`). Both now treat a cancel as a release,
committing whatever was drawn so far rather than discarding it.

Tests: `Geometry::drawn` and `Geometry::anchored` have direct unit
tests next to `drag_within`'s — free, aspect either way, the flip, the
minimum, sliding along one edge free, clamped-with-aspect, the nearest
point against a line to the center on three presses, flooring inward
at a letterbox edge on four more (free, free with no free axis to
shrink, an aspect, turned), a press off the source both plainly and
inside a turn's cut corner, an empty source refusing both, stepping in
from a corner nothing can be drawn from at a 3° straighten and under a
keystone — and the grid property test, now over four geometries
(upright, turned a quarter, 3°, keystoned) and asserting drawability
toward the middle as well as fitting. `panel/crop.rs` adds headless
tests that dispatch
real pointer events through the compiled window: a drag outside draws
a crop with the right fractions, an aspect set holds it, a tiny drag
changes nothing, a drag inside still moves the crop, a drag that
crosses back over the anchor flips, a press off the picture (plainly,
and in a turned corner) draws from the pulled-in anchor rather than
wiping the crop, a plain click and a right-click outside the crop
still zoom and still open the frame menu, and a pointer cancel
mid-draw clears the anchor. They set up their own
`crop-left`/`-top`/`-width`/`-height` by hand, in the units the real
render step would have left them, since nothing in the harness drives
that step; missing that the first time round let a press "inside" the
crop land on the touch area meant for outside it, which the regression
test for a drag inside now covers.

Checked by hand too: `--tool crop --snapshot` over a sample raw shows
the Crop tab and its overlay rendering as before, cursor changes
included. A snapshot mid-drag was the plan, but
`i-slint-backend-testing`'s window does not implement `take_snapshot`
(confirmed, not assumed), and the real binary has no command-line way
to script a live pointer drag, so there is no way to catch the gesture
itself on screen outside a real mouse; the headless dispatch-event
tests exercise the same code path the real gesture does, including
the Slint `TouchArea`'s own hit testing and its `Down`/`Up`/`Cancel`
handling, so they stand in for it.

Not done:
- `Geometry::anchored`'s nearest-point clamp is exact only absent a
  keystone; under one it still lands on the source, and now always on
  a point a crop can be drawn from, just not always the nearest such
  point — a dense sweep over a keystoned, turned and mirrored geometry
  found no crop that failed to fit and none on the wrong side of the
  anchor, but did not check nearness itself.
