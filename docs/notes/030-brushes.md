# 30. Brushes (2026-09-07)

The raster mask §27 said a brush would bring. A brush is a third
kind of shape, `Shape::Brush { strokes }`, so it joins, subtracts and
intersects with gradients and other brushes like any shape, and the
sidecar keeps the strokes, not pixels: a stroke is its points in the
masks' units, its radius in the same units, a feather, a flow and
whether it lifts paint rather than laying it. The raster is made from
the strokes the same way on both paths (`brush::Raster` in
greycard-edit, pure maths) at a fixed 2048 texels across, its height
the picture's aspect, so a painted mask is one thing at any export
size and a 45 MP export samples it four to one, which the feather
hides. Both paths sample it bilinearly, clamped at the edge, the CPU
as the GPU's sampler does; the check of §18 with three strokes and a
gradient agrees to 0.06%, as before.

Painting: discs stamped along the stroke every sixth of the radius,
each disc one minus a smoothstep over the feathered outer part of the
radius. Within a stroke the discs take the most any of them gave
rather than piling up, so a stroke's edge is one disc's edge whatever
the pointer's pace; the stroke then goes over what is there by its
flow (a lift takes that much away), so two half-flow strokes over
each other give three quarters, as an airbrush would. Eight bits per
texel, as masks are. While a stroke is under way only its new points
are stamped: the raster keeps the strokes it has painted and, given
the strokes again, stamps from where it left off when they are those
strokes with points added, and paints from nothing otherwise (an
undo, another file). The test paints a stroke a point at a time and
compares the texels with painting it at once.

On the GPU the brushes are a texture array, one R8 layer each, a
layer written again only when its raster's allocation or version
changed; the shape carries its layer and the raster's aspect. The
window keeps the rasters by adjustment id and component index, drops
the ones no brush has, and makes them again when the picture's
aspect changes. In the panel a brush is the third "+" button beside
linear and radial, and in a mask's shapes "Brush" adds one by the
mode chosen, or reads "Paint" when the chosen shape is already a
brush and adds strokes to it. With the brush in hand the panel shows
Size, Feather, Flow and Erase for the next stroke, the wheel over the
picture sizes it (our ask), a circle under the pointer shows
the radius and where the feather begins, and the mask shows red as
it is painted; a click is a dab; each release is one history entry;
Escape or the button puts the brush down. A brush has no handles; its
strokes are undone, not moved.

### Subtract and erase

Our first go: the one "Erase" switch muddled two things, a
stroke that takes some paint away and one that removes it. A stroke
now has an op, Add, Subtract or Erase, chosen in a segmented control
where the switch was. Add and Subtract lay down and take away by the
flow with the feather; Erase clears its whole radius outright,
hard-edged, whatever the flow and feather say, so it reads as an
eraser rather than a gentler brush. The cursor's ring is white,
amber or red by the op, and shows no feather ring for an erase. The
day's earlier sidecars said `erase: true`; that reads as Subtract,
which is what it did, and is not written again.
