# 27. The mask infrastructure (2026-09-07)

The last of the three Next up items, and the one the masking items
wait on. Our word on the shape: curves, and ideally the mixer,
must work inside a mask, which Lightroom does not offer. That settled
the architecture: a local adjustment is not a subset of the controls
but a whole look, the same struct the global edit has, and the panel
edits whichever target is selected.

**The schema.** `Look { light, curves, mixer, grading }` is the part
of an edit that is a look; `Edit` keeps its fields as they were (the
sidecar reads the same) and gains `adjustments: Vec<Adjustment>`,
each `{ id, name, enabled, mask, look }`, the id stable so the panel
and the history have something to hold. `Edit::look()`,
`set_look`, `target_look(Option<usize>)` and `set_target_look` are
the panel's way in. A mask (`mask.rs`) is a list of components, each
a shape added to what is there or taken from it, either inverted,
the whole invertible: a linear gradient (one on the `from` side,
nothing beyond `to`, straight between) or a radial one (an ellipse
with an angle and a feather over its outer fraction). Positions are
in units of the developed picture's width, both axes, so a shape
keeps its proportions and survives a crop or a turn, which only
change what is looked at. Shapes are evaluated where they are asked,
on both paths, so there is no mask raster and no resolution to
choose; a brush will bring a raster, and a texture beside the shapes.

**The blend, in parameter space.** Rather than blending pictures,
which stacks badly where masks overlap, the look at a pixel is the
global look with each local's blended into its parameters by the
mask's value there: stops and shifts add, contrast adds its excess
over one, the mixer's bands add, a point curve adds its departure
from the line before the global curve is applied (so at full weight
it is composed under the global curve), and the color shifts add.
Two overlapping locals at half each are one local at full, and a
feathered edge is a smooth walk between two looks. Evaluate-then-
blend, never blend-the-tables: each pixel reads each local's table
at its own value, so the CPU does no per-pixel table work. The tone
curve's switch stays the picture's. `finish_pixel` takes the baked
global look and the locals with their weights; `finish_with` takes
a `position` closure that maps an output pixel through the resize
and the geometry to the masks' units, so the export places them
exactly where the viewport does.

**The shader.** Three storage buffers per draw: the locals'
parameters, their tables (the same 512-vec4 layout as the global
uniform, one run each) and their shapes. The fragment builds a
`Look` struct, walks the locals (at most eight, `MAX_LOCALS`)
evaluating each mask at the source position, and the functions that
read `p.*` now take the look. A mask on show is painted red over the
picture after the display table, in the viewport only: the
histogram's pass draws with it off, and the export never sees it.
Storage buffers cannot be empty, so no locals is one zeroed entry.

**The panel.** An ADJUSTMENTS section at the top: the list, Global
first, and the row the panel edits; Linear and Radial buttons that
arm a drag on the picture (a press starts the shape, the drag
stretches it, a press without a drag makes nothing); Delete; and
for a chosen adjustment On, Invert, Show and Feather. The panel's
sliders, curves, mixer and wheels then edit the chosen look; white
balance, noise, sharpening, demosaic and geometry stay global. A
switch of target reads the panel into the old target first, so
nothing is lost; the panel is the truth for the target's look and
the edit struct for everything else, and `read_edit` merges the two.
A screenshot can paint a mask with `--show-mask N`.

**Checked.** Two adjustments in a sidecar (a linear one from the
top with a stop and a fifth down, contrast, a highlight shift, a
lifted point curve and a blue-yellow curve; a tilted, feathered
radial one with a stop up, the mixer's saturation and a warm
shadows wheel) under a three degree turn and a crop: the viewport
at 1:1 and the export's crop differ by 0.05 percent RMSE. The mask
paintings looked at: the gradient fades from the top, the ellipse
sits tilted where it was put.

**Not yet.** Handles to move and reshape a shape after placing it,
and more than one shape in a mask from the panel (the schema and
both paths take any number; only the placing makes one). Brushes
want a raster mask and a texture. Sixteen bits of look per pixel
per local is the shader's cost: eight locals at 4K is fine on this
box; a budget will be measured when someone has eight.

**Modes, shapes and names (2026-09-07, later).** We asked for
renaming, and for adding, subtracting and intersecting. A component's
`subtract` flag became a `mode`: add (where either is), subtract
(taken away from what is there) and intersect (only where both are),
applied in order, so a mask reads as a sentence: this gradient,
intersected with that ellipse, minus this disc. The chosen
adjustment's block on the panel gained a name field (the list
follows as you type), the mask's shapes as a list with the mode's
sign before each, a mode picker for the next shape, Linear and
Radial buttons that draw a shape into this mask rather than a new
adjustment, Remove for the chosen shape, and Invert shape and
Feather now for the chosen shape rather than the first. A press
without a drag removes the shape it would have made. Checked with a
gradient intersected with a feathered ellipse minus a hard disc,
with a saturation drop in the mixer: viewport and export agree to
0.07 percent, and the painted mask shows the hole.

A lesson about the check: the window does not always open at the
same size, so the crop of the export must be computed from the
screenshot's actual size, and the export's row count must share the
viewport's parity (an odd crop for an odd viewport) or the rows
sample between texels. A little loop that reads the sizes and
re-exports when the parity is wrong does it.

**Sixteen, and a tidier section (2026-09-07, later).** We asked
whether the count was limited and for the section to be cleaned up.
The bound was the shader's per-pixel weights array, eight; it is
sixteen now, and nothing is paid for the ones not used, since the
loop runs to the count. Any number of shapes go in a mask. The
section: the list carries each adjustment's switch as a dot at the
left and a bin on the chosen row, so On and Delete are gone as
buttons; New linear and New radial under it start an adjustment;
the chosen adjustment shows its name field, Show mask and Invert,
and its shapes behind a fold ("Shapes (2)") that opens when a shape
is being added: the list with a cross on each row, the mode picker,
Linear and Radial into this mask, Invert shape, and Feather only for
a radial. The sections the target colors, LIGHT, CURVES, COLOR
MIXER and COLOR GRADING, carry its name in their titles while it
is chosen, so the panel says what it is editing. `--show-mask N`
also makes that adjustment the first file's target, for a look at
the block.
