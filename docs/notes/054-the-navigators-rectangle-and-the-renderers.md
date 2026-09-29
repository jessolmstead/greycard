# 54. The navigator's rectangle, and the renderer's layers (2026-09-17)

**The bug.** The navigator's rectangle, the part of the frame the view
has, did not appear after a zoom by space, Z or a click, and appeared
when the view went back to fit: it was always one change behind. The
arithmetic was right; `--zoom 1 --snapshot` showed it in place, and
`--zoom 3` a smaller one. It was the change that was lost, and only
in one direction: a first frame after the zoom out still showed the
1:1 rectangle, but frames after the zoom in never showed one at all.

**The cause.** The rectangle was an `if nav-partial:` element inside
the picture's box, which clips to a rounded corner. FemtoVG renders a
clip with a rounded corner through a layer, an offscreen picture of
the children, cached and redrawn only when a property it read while
drawing them changes. A conditional element that was absent when the
layer was drawn read `nav-partial` through its own tracker, not the
layer's, so the layer never learned it should exist; the rectangle
appeared only when something else the layer had read changed. That
was the asymmetry: an element going away changes the properties of
something the layer drew, an element arriving changes nothing it
knew. The same shape sits over the histogram, the clip marks that
come with the RGB scope, and works because the box's height changes
with the scope, which the layer did read.

**The fix.** The rectangle is always there; `nav-partial` drives its
border and fill instead of its existence, so the layer reads it on
every draw. The rule: under a `clip: true` with a `border-radius`,
show and hide with a property, not with `if`. Setting the rectangle
from inside the frame, in the rendering notifier, is not the problem;
the items are drawn after the notifier runs and see its values.
