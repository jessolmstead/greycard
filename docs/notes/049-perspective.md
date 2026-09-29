# 49. Perspective (2026-09-17)

The keystone, in the geometry stage beside the turn. Two fields on
`Geometry`, `vertical` and `horizontal`, each the tilt of the camera
in degrees for a lens whose focal length is the plane's half side
(a field of ninety degrees on that axis); positive is a camera that
looked up, or to the right, whose verticals or horizontals converge
that way. The unit is a choice: a real tilt needs the lens's focal
length to be a keystone, and the file's is not always there or true,
so the slider is the tilt at one fixed field and a longer lens wants
proportionally more of it. ±40 degrees, where the far edge is shown
at nearly six times the near one; a 24 mm lens tilted 40 degrees
wants about 22 on the slider.

**The model.** The plane-to-source map is a homography now: the
perspective first, about the plane's center, then the orthogonal
matrix of §31. `Geometry::perspective` is the homography's third
row, the tangents over the half sides; `to_source` divides by one
plus its dot with the plane point and then applies the matrix;
`to_plane` undoes the matrix by its transpose as before and then the
perspective by its inverse, which is the same row negated (the
inverse of `I + e₃kᵀ` is `I − e₃kᵀ`). The far edge of the plane
divides by more and so reaches less of the source, spreading it: a
building that leaned back stands up. Everything downstream follows
by construction, as §31 promised it would: the crop's fit (a
rectangle's corners still bound its image, since straight lines stay
straight), the bounds while cropping, the level tool, the masks'
handles, the export. The shader takes the row as one more vec4
(`persp`) after the matrix's, sixteen-aligned, and divides before
its two dots. The horizon needs care: a plane point past it would
come back mirrored inside the source, so the divisor is held above a
small positive and `fits` refuses a crop with a corner beyond it,
which keeps the crop's bisection honest; `bounds` stays finite for
the same reason. The keystone turns and mirrors with the picture in
`turned` and `flipped`, (h, v) to (−v, h) for a quarter
counter-clockwise, tested against the transform itself: a point of
the turned plane samples where the unturned plane's turned point did.

**Checked.** The plane's verticals map to converging source lines,
straight and narrower at the top for a positive tilt; `to_plane`
undoes `to_source` under a turn, a mirror and both tilts to a tenth
of a pixel at 6000 wide; the CPU resample of a keystoned ramp lands
where `to_source` says and spreads the top; the viewport at 1:1 and
the export of the same edit (three degrees of turn, twenty vertical,
minus eight horizontal) agree to 0.064% RMSE on the R6 II frame,
the figure of §31. Two sliders under Angle in the GEOMETRY section;
no guided tool yet, which is the item's natural next step: two
strokes along lines that should be vertical give the tilt and the
turn at once.
