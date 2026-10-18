# §362. The curve's ends slide along their edges (2026-10-18)

A tester asked for the tone curve's end points to move freely in
both directions (issue #28). Out beyond 0 and 1 has no meaning for a
curve whose output is 0 to 1, and Lightroom and Photoshop clamp it
as we do. What we lacked, and they have, is the ends sliding along
their edges: the black point dragged right along the bottom sets the
input black, the white point dragged left along the top the input
white, and every tone past an end takes that end's Out.

The panel pinned the first point's In at 0 and the last at 1. Now
the black point's In moves between 0 and the next point's In less
the gap, the white point's between the previous point's In plus the
gap and 1, with the bounds ordered so a sidecar whose points sit
closer than the gap cannot panic; drags and the typed In go through
the same rule, so an end's In can be typed, which before did
nothing. The spline already held each end's Out beyond its In, and
the tables bake from the spline, so the evaluation, the finish and
the shader did not change; a GPU-against-CPU test with the master's
ends, the red curve's and a mask's blue curve's slid shows each
clipping measurably more than its cornered twin, on lavapipe and a
Radeon, within a level. The flat run past a slid end is drawn, so a
click there, which adds nothing and lets the selection go, reads as
what it is; grabbing the nearest end instead would have jumped its
Out to the pointer. The dropper takes the end for a tone past it,
and otherwise the nearest point in reach rather than the first in
the list, which with a slid end would have picked an interior point
for a tone the end governs.

Two things the rule exposed. The panel assumed its points sorted,
with index zero the black point, and a hand-edited sidecar could
break that; the points are sorted on the way into the panel, and a
guard in the read-back hands the sidecar's own points back whenever
the panel's are only their sorted form, so showing an out-of-order
curve is no edit and no history step, per channel, and a sidecar
with fewer than two points shows the identity it renders as. And
the curves' identity check, which took any curve with every point
on the diagonal for the identity, had no caller left; it was
removed rather than corrected. Lightroom's importer already carried
ends off the corners; RawTherapee's does not map tone curves.

Tests: an end slid in holds its Out beyond it, on the tables too;
seven pinned values on three curves with their ends at the corners;
the move rule at both ends, two points, and a too-close sidecar; the
ends dragged and typed, double-clicked and not removed, a press past
either end adding nothing, and Reset; the dropper's choice; an
out-of-order curve shown in order and no edit; the slid ends on both
sides of the GPU.
