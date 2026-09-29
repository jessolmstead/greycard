# 31. Quarter turns and mirrors (2026-09-07)

The orientation item of §24. Two more fields on the geometry, `turns`
(quarter turns counter-clockwise on the screen, 0 to 3) and `flip`
(the source mirrored left to right), and one matrix in place of the
sine and cosine: `Geometry::matrix` is the fine turn, then the
quarter turns from exact integer matrices, then the mirror as a
negated row, taking a point of the plane about its center to the
source about its. `to_source` and `to_plane` (the transpose, the
matrix being orthogonal) go through it, so the crop, the masks, the
handles, the level tool, the shader and the export all follow with
no other change of their own. The plane is the source's size or,
after an odd number of turns, that size on its side; crop fractions
are of the plane, and `plane_size` says which. A quarter turn or a
mirror lands every plane pixel center on a source pixel center,
since the matrix entries are exactly 0 and ±1 when the fine angle is
0 and the centers are halves, so both paths fetch texels rather
than resample (`resamples` is the fine angle alone); the test turns
and mirrors a ramp and demands equality, not tolerance. The shader
takes the matrix's rows and the plane's size; the uniform's vec4s
had to stay sixteen-aligned, which cost one padded vec4 and one
validation error to find.

The buttons do the bookkeeping through `turned` and `flipped`. A
turn carries the crop with the picture, (x, y) to (y, 1 − x) for a
quarter counter-clockwise, and swaps a held aspect's way. A mirror
on the screen is not just the flag: with the picture mirrored the
fine angle runs the other way, so it is negated, and the quarter
turns reverse (a mirror of a turn is the opposite turn of a mirror);
a vertical mirror is the horizontal one and a half turn. Tested:
the crop's center lands on the same source pixel before and after,
and each is its own inverse. Four icon buttons in the geometry
section (Lucide's rotate and flip pairs; Reset moved to undo-2);
the check of §18 on a turned, mirrored, four-degree frame agrees to
0.06%, and the brush of §30 sits where it was painted.
