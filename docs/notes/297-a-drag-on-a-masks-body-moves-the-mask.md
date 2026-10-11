# §297. A drag on a mask's body moves the mask (2026-10-11)

Issue #16. On a radial or a linear mask, only the center handle moved the
shape; a drag anywhere else on it panned the picture. A tester expected
"holding on any part of the mask to move the mask", and that is what the
painted shape invites: it is the thing on screen, the handle a dot on it.

We now treat the shape's body as a handle. Inside the body a press and drag
translates the shape; outside it the drag pans as before. The handles keep
their meanings and sit above the body, so an axis end still resizes and
turns, an end line still pivots, and the center still moves.

The body is the painted area, read in the masks' units:

- A radial's body is its ellipse at the radii, which is the feather's outer
  edge: the whole area the mask touches, turned by its angle.
- A linear's body is the band between its two lines, where it fades, across
  the whole width of the picture however far from its handles. Beyond the
  lines the mask is flat (all or nothing), and a drag there pans.

The hit test maps the pointer through `view_to_source`, the same mapping a
handle's drag uses, so the zoom, the pan, the crop and the straightening all
apply and the body matches the painted shape, not the outline's drawing of it.
The shape's own test (`Shape::on_body`) lives with the shapes in
greycard-edit; the viewport asks it through a pure callback, which also sets
a move cursor over the body.

We did not add a second drag path. A body drag grabs handle 0, the center,
and feeds it the pointer's travel since the press, so it is the center
handle's drag exactly: the shape as pressed, moved by the travel, recorded on
release. Only the press point differs, and since the travel is relative, the
shape keeps its offset under the pointer.

Rules that keep the picture's other gestures:

- Only a linear or radial shape chosen with no tool in hand has a body: a
  dropper, a placing tool, retouch, crop, level and guide modes all keep
  their presses.
- A press on the body that never clears the drag threshold is still a click,
  so click-to-zoom works on top of a shape.
- Only the left button takes hold.
- The body stops at the picture's edge. A linear's band and a radial
  hanging off the frame are drawn on into the grey around the picture, but
  the mask paints nothing there, so a press on the grey pans rather than
  taking hold. We clip in the hit test rather than in the drawing: the
  outline still shows where the shape runs, and the gesture follows what
  the mask paints.

The cursor shows a move arrow over the body and while a body drag is
moving the shape, and on release goes back to what is under it: the move
arrow on the body, the grab hand off it. Slint caches
the cursor's binding and re-reads it only when a Slint property it read
changes, and the hit test lives in Rust, so on its own the cursor would go
stale when an undo or a panel edit moved the shape under a still pointer.
The binding therefore also reads a key summed from the drawn handles
and passes it to the hit test. The value proves nothing (a sum has
collisions); it is there so the binding depends on the handle rows, and
a changed row re-runs it, so any change to the shape asks again. A hit test that finds the state borrowed answers
"off the body" and is asked again on the next pointer move or shape change.

A shape that covers the whole frame (a large radial, or a linear whose band
spans the picture) makes a pan from inside impossible, since there is no
outside to press. We accept that: the shape can be shrunk or moved aside,
and a click still zooms. An inverted mask keeps the same body; the hold is on the
shape, not on whichever side of it is painted.
