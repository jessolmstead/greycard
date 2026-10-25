# §424. The slider's drag: no jump past the dead zone, and Shift for a fine drag

*2026-10-24*

A tester found the sliders stick and then jump. A press on the handle
moves nothing, and a drag starts only once the pointer clears a 4px
dead zone, so a click to focus never nudges the value. But the drag
then set the value from the pointer less the offset it was grabbed at,
so the first step past the zone applied the whole 4px at once: on a
slider a few hundred pixels long, a visible lurch at the start of every
fine adjustment.

We keep the zone, since it tells a click from a drag, and change what
a drag measures. A drag now moves the track's fraction by the
pointer's travel from an anchor: a point and the fraction there. A
handle drag anchors at the zone's edge on the pointer's side with the
press value, so a step to 5px moves the value one pixel's worth, not
five. The handle then trails the grab point by the zone, which no one
sees; the jump everyone saw is gone. A press on the track still jumps
the value there and anchors at that point, so its drag follows the
pointer exactly as before.

Shift held on a drag moves the value a tenth as far, as RawTherapee and
ART do. Shift is read from each pointer event's modifiers, which Slint
sends just before the move itself. When it changes mid-drag the anchor
moves to the last point the drag was at, with the fraction it had there,
so the value carries on from where it is in both directions. The anchor
fraction is left unclamped on a plain drag, so a drag past an end and
back comes back where it left rather than moving the moment it turns;
when Shift toggles it is clamped to the track, since the value shown
is the clamped one and a fine drag back from past the end would
otherwise sit dead for hundreds of pixels. A logarithmic slider
works in its track fraction too, so a fine drag of Contrast is a tenth
as many stops.

Every slider in the panel is this one control (Light, Color, Detail,
the masks', the parametric curve's, and the crop's angle), so all of
them get both. The keys sheet's Sliders row and the guide say "Shift
on a drag for finer"; no other text. A UI test drives the Exposure
slider through the harness: nothing inside the zone, one pixel's worth
for the first step past it, a tenth with Shift, and full speed again
once Shift is let go, each from where the last left off.

## Review

A fresh review found the one case above, Shift pressed with the
pointer past an end, and judged the 4 px trail right: a catch-up
would bring back the burst the tester reported.
