# 81. A switch on every shape in a mask (2026-09-19)

An adjustment has had its own switch since §48; a mask's shapes had
not, so the only way to ask "what did this look like before I added
that subtraction" was to delete the shape and draw it again. Now
`Component` carries `enabled`, the shape list has a dot per row, and
a shape switched off contributes nothing.

**What "nothing" means.** Not "adds nothing" and not "subtracts
nothing": absent. An Add switched off is easy either way, but a
Subtract that stayed in the join and took nothing away would still be
a Subtract, and an Intersect that stayed in and intersected with
everything would still multiply the accumulator by one — which is
right for an Intersect and wrong for the question the user is asking,
which is "what did the mask read before this shape". So the switch
skips the component outright: `Mask::live` is the components that
count, `Mask::at_with` walks that instead of `components`, and the
mask reads exactly as it did before the shape was put in, whatever
its mode. The test is written that way round: an Add plus a Subtract,
the Subtract off, sampled across the frame against the Add alone.

**One join per path, and the two agree.** The CPU has a single join
(`Mask::at_with`), so the skip is one line there and the export, the
picks and the viewport's CPU reference all follow. The shader has its
own, and it cannot skip anything, because `mask_at` walks a flat
array of shapes by `shapes_start` and `shapes_count`: so the skip
happens where that array is packed. `locals_gpu` now builds a local's
shapes first, from `mask.live()`, and takes `shapes_count` from what
it pushed rather than from what the mask holds — the same absence,
one step earlier. A switched-off brush does not take a texture layer
either, and `finish::rasterize` hands back `None` for it rather than
painting a raster nothing reads. Taking the count from what was
pushed turned up an older bug in the same lines, and paid for itself:
a raster shape still *waiting* for its raster — a subject the model
has not found yet, a model not downloaded — was packed as a brush at
`rasters.len()`, which is the layer the *next* raster component will
take, so it sampled that one's paint; a pending Subject above a brush
at 0.8 read 0.96 on the GPU against the CPU's 0.8. It goes as a kind
of its own now, kind 3, nothing, holding no layer, which is exactly
what `Local::weight` reads a missing raster as — and a kind rather
than a skip because the CPU still applies that component's mode and
its invert to the nothing, so an Intersect against a subject not yet
found is nothing, not a shape passed over. A switched-off learned shape is not
asked of the model at all, on the viewport's path and the export's
both, but a raster it already has is kept, so the switch comes back
without the seven seconds (§58, §67).

**The empty mask.** Every shape off is an empty mask, and an empty
mask is no place, not every place. `Mask::is_empty` is now "nothing
live" rather than "no components", so the callers that already turned
a local off for an empty mask (`main.rs`, `export.rs`) turn it off for
an all-off one too. That alone would have left one hole: a mask with
`invert` set and nothing live read as one everywhere, and while the
local was off and the picture unmoved, "Show mask" paints
`weights[k]` without the local's switch, so it would have washed the
frame red for a mask doing nothing. `at_with` and `mask_at` both now
return nothing before they get to `invert` when nothing is live.
Nothing divides by the mask: the blend is `w *` throughout
(`finish_pixel_with`, and the shader's `look.exposure + w * ...`), so
a weight of zero is simply no change and there is no mean or
normalization to guard.

**The panel.** A dot per row in the shape list, the adjustments
list's dot copied (§48, §62): filled in the accent when on, an
outlined ring when off, the row's name dimmed to half. The click is
its own TouchArea inside the row, so it toggles without choosing the
row, as the adjustments list works. The handler goes through
`read_edit` and `show_edit` like every other panel change, then
`view-changed`, so the mask overlay and the hover preview follow at
once. A shape switched off still draws its outline and handles when
it is the chosen row: it is still the shape being edited.

**The schema, and presets.** `enabled: bool` with `serde(default)`
under `Component`'s `#[serde(default)]`, defaulting to true, so an
older sidecar reads unchanged and comes out on. No `VERSION` bump:
§16's rule is that a field with a default is added freely and only a
field that *changes meaning* bumps and gets a case in `migrate`; no
older sidecar's reading changes. Presets needed no change at all —
`Section::Adjustments` clones the whole `Adjustment`, so the flag
rides along with the mask it belongs to; a test says so rather than
leaving it to be discovered. History names the shape and the switch,
"Two shapes Radial off", from the same `switched` helper the
adjustment's own switch uses, and only when one component differs and
differs only in that flag; anything else is still "mask".

**Checked.** A copy of `5M0A5391.CR3` (the orchids) with a mask of
two radials, the second off, its look at -1.5 EV. The window snapshot
shows one red disc where two would be, the shape list shows a filled
dot on the live row and a hollow one on the dimmed row, and the
counts still say "Shapes (2)". Then the same sidecar against one with
the off shape deleted outright: the 900 px export differs by 0 pixels
of 810,000, and the viewport screenshot by 0 as well. With the second
shape a Subtract over the first instead: off against deleted, 0
pixels again; on, 271,100 pixels differ, so the comparison is not
vacuous. All shapes off, against the same file with no adjustment at
all: 0 pixels, export and viewport, and with `invert` set on the empty
mask and "Show mask" on, still 0 — nothing painted and nothing moved.
A sidecar with the field stripped out opens with both shapes on and
both discs painted. The layer fix has its own pair: a mask of a
Subject over a brush stroke, run with the model store pointed at an
empty directory so the subject never gets a raster, against the brush
alone — 9,985 pixels wrong before, 0 after. The sidecar written back after a session keeps the
flag. `cargo test`, `cargo clippy --all-targets`, `cargo fmt` clean;
new tests for the join, the empty mask inverted, the GPU packing
against the same mask with the shape deleted, a raster shape with no
raster taking no layer so the brush after it keeps its own,
`rasterize` skipping a switched-off brush, the history row and the
preset.

**Not.** No switch on the mask as a whole beyond the adjustment's own
— that is what the adjustment's switch is. No reordering of shapes,
which is the other thing a list like this wants and is a bigger
change: the join is ordered and a reorder changes the answer, so it
needs its own thinking about drag targets. No dimming of a
switched-off shape's outline in the viewport.
