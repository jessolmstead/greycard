# 62. Seven bugs from the list (2026-09-18)

The Bugs section of the roadmap, worked through in one pass. Each
was traced in the code first; the crop drag was handed to another
model with the cause and the shape of the fix, the rest done here.

**The crop frame stuck.** `on_crop_dragged` moved the crop by the
whole offset since the press and applied it only where the result
still fitted the source, so a crop pushed into an edge stopped dead
and stayed there until the pointer had come all the way back to a
place where the whole offset fitted. `Geometry::drag_within` now
takes as much of the drag as fits: a bisection on the offset's
share, then the rest of each axis alone, so a diagonal push into the
right edge still runs down it. Everything goes through `dragged` and
`fits`, so the turn, the keystone and a held aspect are handled as
before. Tests in `geometry.rs`.

**The Object tool drew nothing.** `shape_handles` has no handles for
an object, and the outline pass skipped the chosen component while a
tool was in hand, so a box being dragged and the boxes and picks
after it were never on the view. Two models now, `mask-boxes` and
`mask-picks`, filled each frame from the chosen component or, with
the tool in hand, the one it adds to: a box as a white rectangle
with a dark inner line, a pick as a dot, green on the object and red
off it. The rows are synced in place by `sync_rows`, which the patch
and mask handles now share.

**A long name pushed the panel out.** Not the rename box: the
section titles carry the target's name ("COLOR GRADING · Lighthouse
Top"), and a Text without `overflow: elide` has its text's width as
its minimum, so the section's row widened the panel's content past
its 320 pixels. The `Section` title and the adjustment list's names
elide now.

**"Show mask" stuck on.** The render forced the mask on while any
tool was in hand, and the brush and the object tool stay in hand
until Escape, so the toggle did nothing meanwhile. The toggle rules
now; a tool going in hand turns it on and keeps the old value, put
back once the tool is down, unless the toggle was thrown by hand
meanwhile, when the choice stands. The command line's `--show-mask`
went into the same forced path and never cleared; it sets the
toggle now.

**No gamut warning proofing sRGB.** The proof's marks were computed
over the output space's grid, and the viewport's table is indexed by
the output value, clamped into its space already. With the export
space sRGB (the default) and the proof sRGB, no grid point is out of
gamut, so nothing was ever marked, whatever the picture held. The
marks are now over the working space's grid (Rec.2020 with the
output transfer), and the shader looks them up at the color before
the output matrix. A saturated edit of `4Z4A3525.CR3` marks most of
the frame; the edit as it is marks a little.

**The picture snapped to the center after a develop.** The frame's
arrival compared the view's frame size with the developed image's
size. The image is the whole source and the frame is the crop, so
with any crop every develop looked like a new picture and re-centered
the view; and only develops do that, which is why the denoiser and
the sharpen were the ones seen. The arrival now leaves the centering
to the render, which knows the frame, and asks for it only when the
source's size changed or the view is at fit.

**Hovering "Original" bounced.** A peek renders the row's state, and
the render fed the navigator from it. The navigator's height follows
its picture's shape, and the history list sits below it, so a peek
of a state with another crop moved the row out from under the
pointer, the peek ended, the row came back under it, and so on. The
navigator holds during a peek, the frame size the export sheet shows
stays the panel's, and the view's frame size and center are kept
across a peek so a hover over a row no longer loses a zoomed place.
